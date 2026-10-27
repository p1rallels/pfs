//! `pfs kill [n]`: stops what's left working in a fork, the processes rm
//! lists as no pfs run's, so stopping a dev server never takes a pattern: a
//! `pkill -f` by command line hits every fork and the main folder running it.

use std::{
    io, thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use super::rm;
use crate::{
    cli::KillArgs,
    clock, forks, ledger,
    occupants::{self, Occupant, Process},
    place, proc,
};

/// How long what was signalled gets to exit.
const GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);
/// Deeper than any real tree of leftovers; only there so peeling can't go on forever.
const MAX_DEPTH: usize = 64;

/// Who's in the fork, by what pfs kill does with them.
#[derive(Debug)]
struct Found {
    /// Processes that are no run's, each before what it started in the fork.
    leftovers: Vec<Process>,
    /// The fork's runs still going, other than the one pfs kill runs in.
    runs: Vec<Occupant>,
}

/// A leftover as it was listed: its start time tells it from a process that
/// got its pid since.
#[derive(Debug, Clone)]
struct Target {
    process: Process,
    start: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Term,
    /// Gone already, or its pid is another process's now: nothing was sent.
    Gone,
    /// Still there, not signalled: kill(2) refused, or pfs can't read it to
    /// be sure it's the one listed.
    Unsent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Stopped,
    /// Still running when the grace ran out.
    Survived,
    Unsent(String),
}

pub fn run(args: KillArgs) -> Result<i32> {
    let place = place::here()?;
    let id = place.target(args.fork.as_deref())?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    let own = proc::lineage(std::process::id() as i32);
    let everyone = occupants::processes().context("nothing was stopped")?;
    let found =
        leftovers(&everyone, &own, |listed| occupants::present(&fork.dir, listed)).context("nothing was stopped")?;
    let stopped = stop(&targets(found.leftovers), GRACE);
    println!(
        "{}",
        report(id, &fork.label(), &stopped, &found.runs, &own, GRACE).join("\n")
    );
    Ok(code(&stopped))
}

/// What rm lists as no run's, then what it would list once those were gone,
/// and so on: what a leftover started in the fork goes with it (a TERM to a
/// shell leaves the server it waits on running). Never the caller's lineage,
/// nor anything under it. `present` is `occupants::present` for the fork.
fn leftovers(
    everyone: &[Process],
    own: &[i32],
    present: impl Fn(&[Process]) -> Result<Vec<Occupant>>,
) -> Result<Found> {
    let first = present(everyone)?;
    let runs = first
        .iter()
        .filter(|occupant| matches!(occupant, Occupant::Run(_)) && !own.contains(&occupant.pid()))
        .cloned()
        .collect();
    let mut found = loose(&first, own);
    let mut rest = without(everyone, &found);
    for _ in 0..MAX_DEPTH {
        let next = loose(&present(&rest)?, own);
        if next.is_empty() {
            break;
        }
        rest = without(&rest, &next);
        found.extend(next);
    }
    Ok(Found { leftovers: found, runs })
}

/// The processes that are no run's, minus the caller and what it runs under.
fn loose(present: &[Occupant], own: &[i32]) -> Vec<Process> {
    present
        .iter()
        .filter_map(|occupant| match occupant {
            Occupant::Process(process) if !own.contains(&process.pid) => Some(process.clone()),
            _ => None,
        })
        .collect()
}

fn without(everyone: &[Process], gone: &[Process]) -> Vec<Process> {
    everyone
        .iter()
        .filter(|process| gone.iter().all(|other| other.pid != process.pid))
        .cloned()
        .collect()
}

fn targets(leftovers: Vec<Process>) -> Vec<Target> {
    leftovers
        .into_iter()
        .map(|process| Target {
            start: proc::start_time(process.pid),
            process,
        })
        .collect()
}

/// A TERM to each at once, then up to `grace` for them to go.
fn stop(targets: &[Target], grace: Duration) -> Vec<(Process, Outcome)> {
    let sent: Vec<Sent> = targets.iter().map(signal).collect();
    let waiting = || {
        targets
            .iter()
            .zip(&sent)
            .any(|(target, sent)| *sent == Sent::Term && there(target))
    };
    let deadline = Instant::now() + grace;
    while waiting() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    targets
        .iter()
        .zip(sent)
        .map(|(target, sent)| (target.process.clone(), outcome(sent, there(target))))
        .collect()
}

/// The process listed, still running: not one that got its pid since, and
/// not a zombie (exited, its parent yet to reap it), which `proc::alive`
/// can't tell from a live one because a zombie's start time can't be read.
fn there(target: &Target) -> bool {
    let pid = target.process.pid;
    match target.start {
        Some(start) => proc::alive(pid, Some(start)) && proc::start_time(pid) == Some(start),
        None => proc::alive(pid, None),
    }
}

fn signal(target: &Target) -> Sent {
    match (target.start, there(target)) {
        (_, false) => Sent::Gone,
        (None, true) => Sent::Unsent("pfs can't read its start time".to_string()),
        (Some(_), true) => match unsafe { libc::kill(target.process.pid, libc::SIGTERM) } {
            0 => Sent::Term,
            _ => refused(&io::Error::last_os_error()),
        },
    }
}

fn refused(error: &io::Error) -> Sent {
    match error.raw_os_error() {
        Some(libc::ESRCH) => Sent::Gone,
        _ => Sent::Unsent(error.to_string().to_lowercase()),
    }
}

fn outcome(sent: Sent, there: bool) -> Outcome {
    match (sent, there) {
        (Sent::Gone, _) | (_, false) => Outcome::Stopped,
        (Sent::Term, true) => Outcome::Survived,
        (Sent::Unsent(why), true) => Outcome::Unsent(why),
    }
}

/// What's gone, what's still running and what to do about it, then the runs
/// left alone with rm's hints (`own` is the caller's lineage, as rm's are).
fn report(
    id: u32,
    label: &str,
    stopped: &[(Process, Outcome)],
    runs: &[Occupant],
    own: &[i32],
    grace: Duration,
) -> Vec<String> {
    let gone: Vec<String> = stopped
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Stopped)
        .map(|(process, _)| format!("  pid {}  {}", process.pid, process.command))
        .collect();
    let left: Vec<String> = stopped
        .iter()
        .filter_map(|(process, outcome)| still(process, outcome))
        .collect();
    let alone: Vec<String> = runs
        .iter()
        .map(|run| rm::line(id, run, rm::hint(id, run, own)))
        .collect();
    let head = match stopped {
        [] => vec![format!("nothing to stop in fork {label}")],
        _ => [
            block(format!("stopped in fork {label}:"), gone),
            block(
                format!("still running in fork {label} after {}:", clock::human(grace)),
                left,
            ),
        ]
        .concat(),
    };
    [head, block(format!("pfs runs left alone in fork {label}:"), alone)].concat()
}

fn still(process: &Process, outcome: &Outcome) -> Option<String> {
    let next = match outcome {
        Outcome::Stopped => None,
        Outcome::Survived => Some(format!("kill -9 {}", process.pid)),
        Outcome::Unsent(why) => Some(format!("not signalled: {why}")),
    };
    next.map(|next| format!("  pid {}  {}  ({next})", process.pid, process.command))
}

fn block(header: String, lines: Vec<String>) -> Vec<String> {
    match lines.is_empty() {
        true => Vec::new(),
        false => [vec![header], lines].concat(),
    }
}

/// 0 once nothing it went after is left, 1 while anything is.
fn code(stopped: &[(Process, Outcome)]) -> i32 {
    match stopped.iter().all(|(_, outcome)| *outcome == Outcome::Stopped) {
        true => 0,
        false => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::process::CommandExt,
        path::{Path, PathBuf},
        process::{Child, Command},
    };

    use super::*;
    use crate::runs::{self, Event, Kind, Run, Start};

    fn process(pid: i32, command: &str) -> Process {
        Process {
            pid,
            command: command.into(),
            cwd: PathBuf::from("/w/forks/repo/5"),
        }
    }

    fn start(run: u32, kind: Kind, pid: i32, cmd: &str, proc_start: Option<u64>) -> Start {
        Start {
            run,
            kind,
            cmd: cmd.into(),
            pid,
            proc_start,
            ts: "2026-10-20T05:00:00.000Z".into(),
        }
    }

    fn run(run: u32, kind: Kind, pid: i32, cmd: &str) -> Occupant {
        Occupant::Run(Run {
            start: start(run, kind, pid, cmd, None),
            exit: None,
        })
    }

    /// `present` over a made-up tree of (pid, parent), everyone in the fork:
    /// the runs, then each listed process that's no run's and has no listed
    /// process above it.
    fn present_in(tree: Vec<(i32, i32)>, runs: Vec<Occupant>) -> impl Fn(&[Process]) -> Result<Vec<Occupant>> {
        move |listed| {
            let parent = |pid: i32| tree.iter().find(|(child, _)| *child == pid).map(|(_, up)| *up);
            let lineage = |pid: i32| std::iter::successors(Some(pid), |pid| parent(*pid)).take(MAX_DEPTH);
            let of_a_run = |pid: i32| lineage(pid).any(|up| runs.iter().any(|run| run.pid() == up));
            let under_another = |pid: i32| lineage(pid).skip(1).any(|up| listed.iter().any(|p| p.pid == up));
            let tops = listed
                .iter()
                .filter(|process| !of_a_run(process.pid) && !under_another(process.pid))
                .map(|process| Occupant::Process(process.clone()));
            Ok(runs.iter().cloned().chain(tops).collect())
        }
    }

    fn pids(processes: &[Process]) -> Vec<i32> {
        processes.iter().map(|process| process.pid).collect()
    }

    #[test]
    fn leftovers_are_peeled_top_down_and_runs_are_never_among_them() {
        let tree = vec![(101, 100), (102, 101), (201, 200), (202, 201), (301, 300), (501, 500)];
        let runs = vec![
            run(1, Kind::Bg, 100, "pi -p fix the csv"),
            run(2, Kind::Open, 500, "/bin/zsh (open)"),
        ];
        let everyone: Vec<Process> = [100, 101, 102, 200, 201, 202, 300, 301, 500, 501]
            .into_iter()
            .map(|pid| process(pid, "x"))
            .collect();
        let own = [101, 100, 50, 40];
        let found = leftovers(&everyone, &own, present_in(tree, runs)).unwrap();
        assert_eq!(pids(&found.leftovers), vec![200, 300, 201, 301, 202]);
        assert_eq!(
            found.runs.iter().map(Occupant::pid).collect::<Vec<_>>(),
            vec![500],
            "the caller's own run isn't listed, the door is"
        );
    }

    #[test]
    fn the_caller_and_everything_under_its_lineage_are_left_alone() {
        let tree = vec![(401, 400), (402, 400), (403, 402)];
        let everyone: Vec<Process> = [400, 401, 402, 403, 600]
            .into_iter()
            .map(|pid| process(pid, "x"))
            .collect();
        let own = [999, 401, 400, 1];
        let found = leftovers(&everyone, &own, present_in(tree, Vec::new())).unwrap();
        assert_eq!(pids(&found.leftovers), vec![600]);
        assert!(found.runs.is_empty());
        let nobody = leftovers(&[], &own, present_in(Vec::new(), Vec::new())).unwrap();
        assert!(nobody.leftovers.is_empty() && nobody.runs.is_empty());
    }

    #[test]
    fn a_present_that_fails_stops_nothing() {
        let failing = |_: &[Process]| -> Result<Vec<Occupant>> { anyhow::bail!("can't read runs.jsonl") };
        assert!(leftovers(&[process(1, "x")], &[], failing).is_err());
    }

    const ELSEWHERE: &[i32] = &[9000, 8000, 7000];

    #[test]
    fn reports_what_stopped_what_survived_and_the_runs_left_alone() {
        let stopped = vec![
            (process(70897, "sleep"), Outcome::Stopped),
            (process(70898, "bun"), Outcome::Stopped),
            (process(70899, "node"), Outcome::Survived),
            (
                process(70900, "zsh"),
                Outcome::Unsent("operation not permitted (os error 1)".into()),
            ),
        ];
        let runs = vec![
            run(1, Kind::Bg, 4242, "pi -p fix the csv\nand more"),
            run(2, Kind::Open, 4300, "/bin/zsh (open)"),
            run(3, Kind::Fg, 4400, "cargo test"),
        ];
        assert_eq!(
            report(5, "5-x", &stopped, &runs, ELSEWHERE, GRACE),
            vec![
                "stopped in fork 5-x:",
                "  pid 70897  sleep",
                "  pid 70898  bun",
                "still running in fork 5-x after 3s:",
                "  pid 70899  node  (kill -9 70899)",
                "  pid 70900  zsh  (not signalled: operation not permitted (os error 1))",
                "pfs runs left alone in fork 5-x:",
                "  5.1 bg pid 4242  pi -p fix the csv\u{2026}  (stop: kill -- -4242)",
                "  5.2 open pid 4300  /bin/zsh (open)  (exit that shell)",
                "  5.3 fg pid 4400  cargo test  (finish or quit it)",
            ]
        );
    }

    #[test]
    fn nothing_to_stop_is_one_line_and_runs_still_get_their_hints() {
        assert_eq!(
            report(5, "5-x", &[], &[], ELSEWHERE, GRACE),
            vec!["nothing to stop in fork 5-x"]
        );
        assert_eq!(
            report(5, "5-x", &[], &[run(1, Kind::Bg, 4242, "bun dev")], ELSEWHERE, GRACE),
            vec![
                "nothing to stop in fork 5-x",
                "pfs runs left alone in fork 5-x:",
                "  5.1 bg pid 4242  bun dev  (stop: kill -- -4242)",
            ]
        );
        let all_gone = [(process(70897, "sleep"), Outcome::Stopped)];
        assert_eq!(
            report(5, "5-x", &all_gone, &[], ELSEWHERE, GRACE),
            vec!["stopped in fork 5-x:", "  pid 70897  sleep"]
        );
    }

    #[test]
    fn exits_1_only_while_something_it_went_after_is_left() {
        let stopped = |outcome: Outcome| [(process(1, "sleep"), Outcome::Stopped), (process(2, "node"), outcome)];
        assert_eq!(code(&[]), 0);
        assert_eq!(code(&stopped(Outcome::Stopped)), 0);
        assert_eq!(code(&stopped(Outcome::Survived)), 1);
        assert_eq!(code(&stopped(Outcome::Unsent("no".into()))), 1);
    }

    #[test]
    fn whatever_is_gone_counts_as_stopped() {
        assert_eq!(outcome(Sent::Term, false), Outcome::Stopped);
        assert_eq!(outcome(Sent::Gone, false), Outcome::Stopped);
        assert_eq!(outcome(Sent::Gone, true), Outcome::Stopped);
        assert_eq!(outcome(Sent::Term, true), Outcome::Survived);
        assert_eq!(outcome(Sent::Unsent("x".into()), true), Outcome::Unsent("x".into()));
        assert_eq!(outcome(Sent::Unsent("x".into()), false), Outcome::Stopped);
    }

    #[test]
    fn a_refused_signal_says_why_unless_the_process_is_gone() {
        assert_eq!(refused(&io::Error::from_raw_os_error(libc::ESRCH)), Sent::Gone);
        assert_eq!(
            refused(&io::Error::from_raw_os_error(libc::EPERM)),
            Sent::Unsent("operation not permitted (os error 1)".into())
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("pfs-kill-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn spawn_group(dir: &Path, script: &str) -> Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .current_dir(dir)
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn pid_of(child: &Child) -> i32 {
        i32::try_from(child.id()).unwrap()
    }

    /// What a sub leaves behind: `cmd &` from a shell that has since ended,
    /// so launchd is its parent. Returns the shell (its group, for the
    /// cleanup) and the orphan's pid.
    fn orphan(dir: &Path, cmd: &str) -> (Child, i32) {
        let mut shell = Command::new("/bin/sh")
            .args(["-c", &format!("{cmd} >/dev/null 2>&1 & echo $!")])
            .current_dir(dir)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut said = String::new();
        std::io::Read::read_to_string(shell.stdout.as_mut().unwrap(), &mut said).unwrap();
        shell.wait().unwrap();
        (shell, said.trim().parse().unwrap())
    }

    fn reap(mut child: Child) {
        unsafe { libc::kill(-pid_of(&child), libc::SIGKILL) };
        let _ = child.wait();
    }

    /// lsof until `want` processes work in `dir`, for up to 3s.
    fn settled(dir: &Path, want: usize) -> Vec<Process> {
        (0..30)
            .find_map(|_| {
                let everyone = occupants::processes().unwrap();
                match everyone.iter().filter(|process| process.cwd.starts_with(dir)).count() >= want {
                    true => Some(everyone),
                    false => {
                        thread::sleep(Duration::from_millis(100));
                        None
                    }
                }
            })
            .expect("the test's processes never showed up in lsof")
    }

    #[test]
    fn a_pid_that_isnt_the_process_listed_is_never_signalled() {
        let dir = scratch("recycled");
        let child = spawn_group(&dir, "exec sleep 30");
        let pid = pid_of(&child);
        let start = proc::start_time(pid).unwrap();
        let recycled = Target {
            process: process(pid, "sleep"),
            start: Some(start + 100),
        };
        let sent = signal(&recycled);
        let still_alive = proc::alive(pid, Some(start));
        reap(child);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(sent, Sent::Gone);
        assert!(still_alive);
    }

    #[test]
    fn stops_what_works_in_the_fork_and_nothing_next_door() {
        let base = scratch("e2e");
        let fork = base.join("5");
        let next_door = base.join("50");
        fs::create_dir_all(fork.join("src")).unwrap();
        fs::create_dir_all(&next_door).unwrap();
        let shell = spawn_group(&fork, "sleep 30; true");
        let loose = spawn_group(&fork.join("src"), "exec sleep 30");
        let stubborn = spawn_group(&fork, "trap '' TERM; exec sleep 30");
        let same_command = spawn_group(&next_door, "exec sleep 30");
        let pfs_run = spawn_group(&fork, "sleep 30; true");
        let (left_by, orphan_pid) = orphan(&fork, "nohup sleep 30");
        let [shell_pid, loose_pid, stubborn_pid, same_command_pid, run_pid] =
            [&shell, &loose, &stubborn, &same_command, &pfs_run].map(pid_of);
        runs::ensure_dir(&fork).unwrap();
        let started = start(1, Kind::Bg, run_pid, "sleep 30; true", proc::start_time(run_pid));
        runs::record(&fork, &Event::Start(started)).unwrap();
        let everyone = settled(&fork, 7);
        let own = proc::lineage(std::process::id() as i32);
        let found = leftovers(&everyone, &own, |listed| occupants::present(&fork, listed)).unwrap();
        let stopped = stop(&targets(found.leftovers), Duration::from_millis(500));
        let next_door_alive = proc::alive(same_command_pid, None);
        let run_alive = proc::alive(run_pid, None);
        let lines = report(5, "5-x", &stopped, &found.runs, &own, GRACE);
        [shell, loose, stubborn, same_command, pfs_run, left_by]
            .into_iter()
            .for_each(reap);
        let _ = fs::remove_dir_all(&base);
        let gone: Vec<i32> = stopped
            .iter()
            .filter(|(_, outcome)| *outcome == Outcome::Stopped)
            .map(|(process, _)| process.pid)
            .collect();
        let outcome_of = |pid: i32| {
            stopped
                .iter()
                .find(|(process, _)| process.pid == pid)
                .map(|(_, outcome)| outcome.clone())
        };
        assert_eq!(
            gone.len(),
            4,
            "the shell, the sleep it waits on, the loose sleep, the orphan: {stopped:?}"
        );
        assert!(gone.contains(&shell_pid) && gone.contains(&loose_pid) && gone.contains(&orphan_pid));
        assert!(
            stopped
                .iter()
                .all(|(process, _)| process.cwd.starts_with(&fork) && process.pid != run_pid)
        );
        assert_eq!(outcome_of(stubborn_pid), Some(Outcome::Survived));
        assert_eq!(code(&stopped), 1);
        assert!(
            next_door_alive,
            "the same command line in another folder is never touched"
        );
        assert!(run_alive, "a pfs run is never signalled");
        assert_eq!(found.runs.iter().map(Occupant::pid).collect::<Vec<_>>(), vec![run_pid]);
        assert!(lines.contains(&format!("  pid {stubborn_pid}  sleep  (kill -9 {stubborn_pid})")));
        assert!(lines.contains(&format!("  pid {loose_pid}  sleep")));
    }
}
