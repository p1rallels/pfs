//! Who's working in a fork: pfs's runs still going, the processes whose
//! working folder is in it, and harness conversations whose turn is still
//! open (pi-web hosts a whole conversation in one process whose working
//! folder is elsewhere, so lsof alone can't see it thinking).
//!
//! A conversation can read mid-turn after its harness is gone: a `pi -p`
//! killed mid-tool writes no closing message. So an open conversation
//! doesn't count when nobody is present (no pfs run going, no process
//! working there) and its last write came at or before the fork's latest
//! recorded run end.
//!
//! Harnesses come from the harness table; nothing here knows one.

use std::{
    ffi::OsStr,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    clock,
    harness::{self, Harness, Turn},
    paths, proc,
    runs::{self, Run, Status},
};

/// lsof lives here on every mac; a full path still works when pfs is
/// started with a bare PATH.
const LSOF: &str = "/usr/sbin/lsof";
const LSOF_ARGS: [&str; 6] = ["-a", "-d", "cwd", "-Fpcn", "+c", "0"];

/// A run's exit line is written just after its process ended, with ms
/// precision, so a last write a hair after it is still that run's.
const EXIT_SLACK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    pub command: String,
    pub cwd: PathBuf,
}

/// Someone in a fork, once: what runs under it is part of it.
#[derive(Debug, Clone)]
pub enum Occupant {
    /// A pfs run still going, wherever its folder is now. Its waiter (or the
    /// `pfs run`/`pfs open` waiting on it) and everything under it are part of it.
    Run(Run),
    /// A process working in the fork that's no run's.
    Process(Process),
}

impl Occupant {
    pub fn pid(&self) -> i32 {
        match self {
            Occupant::Run(run) => run.start.pid,
            Occupant::Process(process) => process.pid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub harness: &'static str,
    pub id: String,
    pub last_write: SystemTime,
    pub turn: Turn,
}

/// Every process's working folder, from one lsof (no tree walk, so it costs
/// the same whatever is in the folders). lsof exits 1 when it can't read
/// some processes; the ones it did list still count. pfs itself and its
/// lsof aren't anyone working.
pub fn processes() -> Result<Vec<Process>> {
    let child = Command::new(LSOF)
        .args(LSOF_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("can't run lsof, so pfs can't tell who's working in the forks")?;
    let lsof = i32::try_from(child.id()).unwrap_or(-1);
    let output = child
        .wait_with_output()
        .context("lsof didn't finish, so pfs can't tell who's working in the forks")?;
    let listed = parse_lsof(&output.stdout);
    let own = i32::try_from(std::process::id()).unwrap_or(-1);
    match listed.is_empty() {
        true => bail!(
            "lsof listed no processes ({}: {}), so pfs can't tell who's working in the forks",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        false => Ok(listed
            .into_iter()
            .filter(|process| process.pid != own && process.pid != lsof)
            .collect()),
    }
}

/// Who's in the fork, each once: its pfs runs still going, in order, then
/// the processes working in it that are no run's, by pid. `everyone` is
/// what `processes` listed.
pub fn present(fork: &Path, everyone: &[Process]) -> Result<Vec<Occupant>> {
    let going: Vec<(Run, Option<i32>)> = runs::load(fork)?
        .into_iter()
        .filter(|run| run.status() == Status::Running)
        .map(|run| {
            let parent = proc::parent(run.start.pid);
            (run, parent)
        })
        .collect();
    let working: Vec<(Process, Vec<i32>)> = working_in(everyone, &roots(fork))
        .into_iter()
        .map(|process| {
            let lineage = proc::lineage(process.pid);
            (process, lineage)
        })
        .collect();
    Ok(fold(going, working))
}

/// The fork's conversations still mid-turn, newest write first, minus the
/// ones dead runs left open. `present` is who `present` found: only with
/// nobody there can an open conversation belong to a dead run.
pub fn mid_turn(fork: &Path, present: &[Occupant]) -> Result<Vec<Conversation>> {
    mid_turn_by(fork, present.is_empty(), harness::HARNESSES)
}

/// `pi conversation 01a13e1f is mid-turn (running tools, last write 40s ago)`.
/// The id is cut to 8 characters, what `lookup` takes.
pub fn describe(conversation: &Conversation, now: SystemTime) -> String {
    let age = clock::human(now.duration_since(conversation.last_write).unwrap_or_default());
    let who = format!("{} conversation {}", conversation.harness, short(&conversation.id));
    match doing(conversation.turn) {
        Some(doing) => format!("{who} is mid-turn ({doing}, last write {age} ago)"),
        None => format!("{who} is done (last write {age} ago)"),
    }
}

fn doing(turn: Turn) -> Option<&'static str> {
    match turn {
        Turn::Thinking => Some("thinking"),
        Turn::RunningTools => Some("running tools"),
        Turn::Done => None,
    }
}

fn short(id: &str) -> &str {
    id.char_indices().nth(8).map_or(id, |(at, _)| &id[..at])
}

/// Each going run once, then each process that's no run's and has no other
/// such process above it. A process is a run's when the run is in its
/// lineage (the run itself or under it) or it's the run's parent: the
/// waiter of a bg run, the `pfs run` or `pfs open` holding a fg run or a
/// door. A lineage is the process first, then each one above it.
fn fold(going: Vec<(Run, Option<i32>)>, working: Vec<(Process, Vec<i32>)>) -> Vec<Occupant> {
    let of_a_run = |process: &Process, lineage: &[i32]| {
        going
            .iter()
            .any(|(run, parent)| lineage.contains(&run.start.pid) || *parent == Some(process.pid))
    };
    let loose: Vec<(Process, Vec<i32>)> = working
        .into_iter()
        .filter(|(process, lineage)| !of_a_run(process, lineage))
        .collect();
    let under_another = |lineage: &[i32]| {
        lineage
            .iter()
            .skip(1)
            .any(|above| loose.iter().any(|(other, _)| other.pid == *above))
    };
    let tops: Vec<Occupant> = loose
        .iter()
        .filter(|(_, lineage)| !under_another(lineage))
        .map(|(process, _)| Occupant::Process(process.clone()))
        .collect();
    going
        .into_iter()
        .map(|(run, _)| Occupant::Run(run))
        .chain(tops)
        .collect()
}

fn mid_turn_by(fork: &Path, nobody: bool, harnesses: &[Harness]) -> Result<Vec<Conversation>> {
    let roots = roots(fork);
    let until = dead_until(&runs::load(fork)?, nobody);
    let mut conversations: Vec<Conversation> = harnesses
        .iter()
        .map(|harness| filed_by(harness, &roots))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|conversation| counts(conversation, until))
        .collect();
    conversations.sort_by_key(|conversation| std::cmp::Reverse(conversation.last_write));
    Ok(conversations)
}

fn filed_by(harness: &Harness, roots: &[PathBuf]) -> Result<Vec<Conversation>> {
    Ok((harness.conversations)(roots)?
        .into_iter()
        .map(|filed| Conversation {
            harness: harness.name,
            id: filed.id,
            last_write: filed.last_write,
            turn: filed.turn,
        })
        .collect())
}

/// `lsof -F` output: a `p<pid>` line opens each process, `c<command>` names
/// it, then `f<fd>` and `n<path>` per file. Anything else is skipped.
fn parse_lsof(listing: &[u8]) -> Vec<Process> {
    let start: (Option<i32>, String, Vec<Process>) = (None, String::new(), Vec::new());
    let (_, _, found) = listing
        .split(|byte| *byte == b'\n')
        .filter_map(<[u8]>::split_first)
        .fold(start, |(pid, command, mut found), (field, value)| match *field {
            b'p' => (parse_pid(value), String::new(), found),
            b'c' => (pid, String::from_utf8_lossy(value).into_owned(), found),
            b'n' => {
                found.extend(pid.map(|pid| Process {
                    pid,
                    command: command.clone(),
                    cwd: PathBuf::from(OsStr::from_bytes(value)),
                }));
                (pid, command, found)
            }
            _ => (pid, command, found),
        });
    found
}

fn parse_pid(raw: &[u8]) -> Option<i32> {
    std::str::from_utf8(raw).ok()?.parse().ok()
}

fn working_in(everyone: &[Process], roots: &[PathBuf]) -> Vec<Process> {
    let mut kept: Vec<Process> = everyone
        .iter()
        .filter(|process| paths::inside(&process.cwd, roots))
        .cloned()
        .collect();
    kept.sort_by_key(|process| process.pid);
    kept.dedup_by_key(|process| process.pid);
    kept
}

/// The fork as given and as the filesystem resolves it: lsof and a terminal
/// pi report resolved paths (/private/tmp/…), pi-web can file a
/// conversation under the path it was handed (/tmp/…).
fn roots(fork: &Path) -> Vec<PathBuf> {
    let given = lexical(&std::path::absolute(fork).unwrap_or_else(|_| fork.to_path_buf()));
    let mut roots: Vec<PathBuf> = std::iter::once(given).chain(fs::canonicalize(fork).ok()).collect();
    roots.dedup();
    roots
}

/// Drops `.` and folds `..` without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    path.components().fold(PathBuf::new(), |mut acc, component| {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                acc.pop();
            }
            other => acc.push(other.as_os_str()),
        }
        acc
    })
}

/// Up to when an open conversation's last write belongs to a run that's
/// over. None while anyone is present, or when no run recorded its end (a
/// died run has none, so it clears nothing).
fn dead_until(runs: &[Run], nobody: bool) -> Option<SystemTime> {
    match nobody {
        false => None,
        true => latest_end(runs).map(|end| end + EXIT_SLACK),
    }
}

fn latest_end(runs: &[Run]) -> Option<SystemTime> {
    runs.iter()
        .filter_map(|run| run.exit.as_ref())
        .filter_map(|exit| instant(&exit.ts))
        .max()
}

fn instant(ts: &str) -> Option<SystemTime> {
    OffsetDateTime::parse(ts, &Rfc3339).ok().map(SystemTime::from)
}

fn counts(conversation: &Conversation, dead_until: Option<SystemTime>) -> bool {
    match (conversation.turn, dead_until) {
        (Turn::Done, _) => false,
        (_, Some(until)) => conversation.last_write > until,
        (_, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::process::CommandExt, thread};

    use super::*;
    use crate::{
        harness::Filed,
        runs::{Event, Exit, Kind, Start},
    };

    fn at(ts: &str) -> SystemTime {
        instant(ts).unwrap()
    }

    fn process(pid: i32, cwd: &str) -> Process {
        Process {
            pid,
            command: "pi".into(),
            cwd: PathBuf::from(cwd),
        }
    }

    #[test]
    fn parses_lsof_fields() {
        let listing = b"p498\ncloginwindow\nfcwd\nn/\np70897\ncsleep\nfcwd\nn/private/tmp/occ test/caf\xc3\xa9 \xc3\xbc/sub\np12\nfcwd\nn/a\npbad\ncx\nfcwd\nn/b\np13\ncnode\ntDIR\nfcwd\nn/c\n";
        assert_eq!(
            parse_lsof(listing),
            vec![
                Process {
                    pid: 498,
                    command: "loginwindow".into(),
                    cwd: PathBuf::from("/"),
                },
                Process {
                    pid: 70897,
                    command: "sleep".into(),
                    cwd: PathBuf::from("/private/tmp/occ test/café ü/sub"),
                },
                Process {
                    pid: 12,
                    command: String::new(),
                    cwd: PathBuf::from("/a"),
                },
                Process {
                    pid: 13,
                    command: "node".into(),
                    cwd: PathBuf::from("/c"),
                },
            ]
        );
        assert!(parse_lsof(b"").is_empty());
        assert!(parse_lsof(b"\n\nn/orphan\n").is_empty());
    }

    #[test]
    fn keeps_processes_inside_the_fork_component_wise() {
        let everyone = [
            process(1, "/w/repo/.forks/5"),
            process(2, "/w/repo/.forks/5/src/deep"),
            process(3, "/w/repo/.forks/50"),
            process(4, "/w/repo"),
            process(5, "/w/repo/.forks"),
            process(6, "/w/repo/.forks/5-x"),
            process(7, "relative/w/repo/.forks/5"),
            process(2, "/w/repo/.forks/5/src/deep"),
        ];
        let pids = |fork: &str| -> Vec<i32> {
            working_in(&everyone, &[PathBuf::from(fork)])
                .iter()
                .map(|process| process.pid)
                .collect()
        };
        assert_eq!(pids("/w/repo/.forks/5"), vec![1, 2]);
        assert_eq!(pids("/w/repo/.forks/5/"), vec![1, 2]);
        assert_eq!(pids("/w/repo/.forks/50"), vec![3]);
        assert_eq!(pids("/w/repo/.forks/7"), Vec::<i32>::new());
    }

    #[test]
    fn roots_are_the_fork_as_given_and_resolved() {
        assert_eq!(
            roots(Path::new("/tmp")),
            vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")]
        );
        assert_eq!(
            roots(Path::new("/w/./repo/x/../.forks/5")),
            vec![PathBuf::from("/w/repo/.forks/5")]
        );
    }

    fn start(run: u32, kind: Kind, pid: i32, proc_start: Option<u64>) -> Start {
        Start {
            run,
            kind,
            cmd: "pi -p x".into(),
            pid,
            proc_start,
            ts: "2026-10-15T05:00:00.000Z".into(),
        }
    }

    fn ended(run: u32, ts: &str) -> Run {
        Run {
            start: start(run, Kind::Bg, 999_999, None),
            exit: Some(Exit {
                run,
                code: 143,
                signal: None,
                ts: ts.into(),
            }),
        }
    }

    fn died(run: u32) -> Run {
        Run {
            start: start(run, Kind::Bg, 999_999, None),
            exit: None,
        }
    }

    fn running(run: u32, kind: Kind, pid: i32) -> Run {
        Run {
            start: start(run, kind, pid, None),
            exit: None,
        }
    }

    fn pids(occupants: &[Occupant]) -> Vec<(char, i32)> {
        occupants
            .iter()
            .map(|occupant| match occupant {
                Occupant::Run(_) => ('r', occupant.pid()),
                Occupant::Process(_) => ('p', occupant.pid()),
            })
            .collect()
    }

    fn working(pid: i32, lineage: &[i32]) -> (Process, Vec<i32>) {
        (process(pid, "/w/repo/.forks/5"), [&[pid], lineage].concat())
    }

    #[test]
    fn a_runs_waiter_and_everything_under_it_fold_into_its_line() {
        let going = vec![(running(1, Kind::Bg, 4242), Some(4241))];
        let tree = vec![
            working(4241, &[]),
            working(4242, &[4241]),
            working(4250, &[4242, 4241]),
            working(4260, &[4255, 4250, 4242, 4241]),
        ];
        assert_eq!(pids(&fold(going, tree)), vec![('r', 4242)]);
    }

    #[test]
    fn a_run_shows_even_with_nobody_of_it_in_the_fork() {
        let going = vec![(running(1, Kind::Open, 52014), Some(52013))];
        assert_eq!(pids(&fold(going, Vec::new())), vec![('r', 52014)]);
    }

    #[test]
    fn other_processes_fold_into_the_topmost_one_in_the_fork() {
        let tree = vec![
            working(100, &[90, 1]),
            working(101, &[100, 90]),
            working(102, &[95, 101, 100]),
            working(200, &[90]),
        ];
        assert_eq!(pids(&fold(Vec::new(), tree)), vec![('p', 100), ('p', 200)]);
    }

    #[test]
    fn a_shell_above_a_run_stays_its_own_line() {
        let going = vec![(running(2, Kind::Fg, 302), Some(301))];
        let tree = vec![
            working(300, &[1]),
            working(301, &[300]),
            working(302, &[301, 300]),
            working(303, &[302, 301, 300]),
        ];
        assert_eq!(pids(&fold(going, tree)), vec![('r', 302), ('p', 300)]);
    }

    #[test]
    fn an_orphaned_run_folds_only_what_is_under_it() {
        let going = vec![
            (running(1, Kind::Bg, 500), None),
            (running(2, Kind::Bg, 600), Some(599)),
        ];
        let tree = vec![working(500, &[]), working(501, &[500]), working(599, &[])];
        assert_eq!(pids(&fold(going, tree)), vec![('r', 500), ('r', 600)]);
    }

    #[test]
    fn a_write_at_or_before_the_last_exit_belongs_to_a_dead_run() {
        let runs = [
            ended(1, "2026-10-15T06:00:00.000Z"),
            ended(2, "2026-10-15T06:10:00.000Z"),
            died(3),
        ];
        let until = dead_until(&runs, true);
        assert_eq!(until, Some(at("2026-10-15T06:10:01.000Z")));
        assert!(!counts(&open("2026-10-15T06:09:59.000Z"), until));
        assert!(!counts(&open("2026-10-15T06:10:00.000Z"), until));
        assert!(!counts(&open("2026-10-15T06:10:00.900Z"), until));
        assert!(counts(&open("2026-10-15T06:10:01.500Z"), until));
    }

    #[test]
    fn nothing_is_cleared_while_anyone_is_present() {
        let runs = [ended(1, "2026-10-15T06:10:00.000Z")];
        assert_eq!(dead_until(&runs, false), None);
        assert!(counts(&open("2026-10-15T06:00:00.000Z"), None));
    }

    #[test]
    fn died_runs_clear_nothing() {
        assert_eq!(dead_until(&[died(1), died(2)], true), None);
        assert_eq!(dead_until(&[], true), None);
    }

    #[test]
    fn done_conversations_never_count() {
        let done = Conversation {
            turn: Turn::Done,
            ..open("2026-10-15T06:00:00.000Z")
        };
        assert!(!counts(&done, None));
    }

    fn open(last_write: &str) -> Conversation {
        Conversation {
            harness: "pi",
            id: "01a13e1f-a572-75c1-9fac-cfd8cb035217".into(),
            last_write: at(last_write),
            turn: Turn::RunningTools,
        }
    }

    #[test]
    fn describes_an_open_conversation_in_one_line() {
        let conversation = open("2026-10-15T06:00:00.000Z");
        let now = at("2026-10-15T06:00:40.000Z");
        assert_eq!(
            describe(&conversation, now),
            "pi conversation 01a13e1f is mid-turn (running tools, last write 40s ago)"
        );
        let thinking = Conversation {
            turn: Turn::Thinking,
            id: "01a1".into(),
            ..conversation.clone()
        };
        assert_eq!(
            describe(&thinking, at("2026-10-15T06:07:00.000Z")),
            "pi conversation 01a1 is mid-turn (thinking, last write 7m ago)"
        );
        assert_eq!(
            describe(&conversation, at("2026-10-15T05:00:00.000Z")),
            "pi conversation 01a13e1f is mid-turn (running tools, last write 0s ago)"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("pfs-occupants-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn filed(id: &str, last_write: &str, turn: Turn) -> Filed {
        Filed {
            id: id.into(),
            last_write: at(last_write),
            turn,
        }
    }

    fn three_conversations(_roots: &[PathBuf]) -> Result<Vec<Filed>> {
        Ok(vec![
            filed(
                "01a13e1f-a572-75c1-9fac-cfd8cb035217",
                "2026-10-15T06:09:00.000Z",
                Turn::RunningTools,
            ),
            filed("01a1ffff", "2026-10-15T06:20:00.000Z", Turn::Thinking),
            filed("01a1dddd", "2026-10-15T06:30:00.000Z", Turn::Done),
        ])
    }

    const FAKE: &[Harness] = &[Harness {
        name: "fake",
        session_env: "PFS_FAKE_SESSION",
        conversations: three_conversations,
    }];

    #[test]
    fn mid_turn_names_the_harness_and_sets_aside_what_dead_runs_left() {
        let fork = scratch("fork").join("5");
        fs::create_dir_all(&fork).unwrap();
        runs::ensure_dir(&fork).unwrap();
        let started = Event::Start(start(1, Kind::Bg, 999_999, None));
        let exited = Event::Exit(Exit {
            run: 1,
            code: 143,
            signal: None,
            ts: "2026-10-15T06:10:00.000Z".into(),
        });
        runs::record(&fork, &started).unwrap();
        runs::record(&fork, &exited).unwrap();
        let alone = mid_turn_by(&fork, true, FAKE);
        let busy = mid_turn_by(&fork, false, FAKE);
        let _ = fs::remove_dir_all(fork.parent().unwrap());
        let ids = |conversations: Vec<Conversation>| -> Vec<(&'static str, String)> {
            conversations
                .iter()
                .map(|conversation| (conversation.harness, short(&conversation.id).to_string()))
                .collect()
        };
        assert_eq!(ids(alone.unwrap()), vec![("fake", "01a1ffff".to_string())]);
        assert_eq!(
            ids(busy.unwrap()),
            vec![("fake", "01a1ffff".to_string()), ("fake", "01a13e1f".to_string())]
        );
    }

    fn spawn_group(dir: &Path, script: &str) -> std::process::Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .current_dir(dir)
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn pid_of(child: &std::process::Child) -> i32 {
        i32::try_from(child.id()).unwrap()
    }

    fn stop(mut child: std::process::Child) {
        unsafe { libc::kill(-pid_of(&child), libc::SIGKILL) };
        let _ = child.wait();
    }

    /// lsof until `want` processes work in `roots`, for up to 3s.
    fn settled(roots: &[PathBuf], want: usize) -> Vec<Process> {
        (0..30)
            .find_map(|_| {
                let everyone = processes().unwrap();
                match working_in(&everyone, roots).len() >= want {
                    true => Some(everyone),
                    false => {
                        thread::sleep(Duration::from_millis(100));
                        None
                    }
                }
            })
            .unwrap_or_default()
    }

    #[test]
    fn finds_real_processes_by_their_working_folder_and_folds_them() {
        let base = scratch("lsof");
        let fork = base.join("5");
        let sibling = base.join("50");
        fs::create_dir_all(fork.join("src")).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let shell = spawn_group(&fork, "sleep 30; true");
        let loose = spawn_group(&fork.join("src"), "exec sleep 30");
        let next_door = spawn_group(&sibling, "exec sleep 30");
        let run = spawn_group(&fork, "sleep 30; true");
        let (shell_pid, loose_pid, next_door_pid, run_pid) =
            (pid_of(&shell), pid_of(&loose), pid_of(&next_door), pid_of(&run));
        runs::ensure_dir(&fork).unwrap();
        let started = start(1, Kind::Bg, run_pid, proc::start_time(run_pid));
        runs::record(&fork, &Event::Start(started)).unwrap();
        let everyone = settled(&roots(&fork), 5);
        let found = present(&fork, &everyone);
        let beside = present(&sibling, &everyone);
        [shell, loose, next_door, run].into_iter().for_each(stop);
        let _ = fs::remove_dir_all(&base);
        let mut loose_pids = vec![('p', shell_pid), ('p', loose_pid)];
        loose_pids.sort_unstable();
        assert_eq!(pids(&found.unwrap()), [vec![('r', run_pid)], loose_pids].concat());
        assert_eq!(pids(&beside.unwrap()), vec![('p', next_door_pid)]);
        let own = i32::try_from(std::process::id()).unwrap();
        assert!(everyone.iter().all(|process| process.pid != own));
    }
}
