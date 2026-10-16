//! `pfs wait`: block until forks have no bg run going, or until the bg runs
//! named have ended, reading only pfs's own run records. fg runs and doors
//! don't count: someone sits at those.

use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};

use crate::{
    cli::SettleArgs,
    forks::{self, Fork},
    ledger, place, proc,
    runs::{self, Kind, Run, Status},
};

const POLL: Duration = Duration::from_millis(500);

/// One argument: a fork (`5`) or one of its runs (`5.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Target {
    fork: u32,
    run: Option<u32>,
}

/// A fork waits on every bg run going in it; a run, on itself only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Fork,
    Run,
}

/// One bg run, as one look at its fork's run records saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    run: u32,
    kind: Kind,
    status: Status,
}

/// One look at a fork: its bg runs, and how the latest of them ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Look {
    seen: Vec<Seen>,
    ending: Option<String>,
}

/// What wait carries from one look at a target to the next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Memory {
    watched: Vec<Seen>,
    /// The fork's latest bg run, so a death told after "nothing was running"
    /// gets its second look too.
    latest: Option<Seen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Exited(i32),
    Died,
    /// The fork was removed before wait saw the run end.
    Unseen,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Item {
    run: u32,
    kind: Kind,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    removed: bool,
    items: Vec<Item>,
    /// How the latest bg run ended, for a fork where nothing was running.
    ending: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Settled(Report),
    /// What to remember for the next look, and what to say if wait gives up here.
    Busy(Memory, Report),
}

/// A target being waited on, with the runs it watches.
struct Watch {
    fork: Fork,
    scope: Scope,
    memory: Memory,
}

pub fn run(args: SettleArgs) -> Result<i32> {
    let place = place::here()?;
    let ledger = ledger::load(&place.repo)?;
    let given = args
        .targets
        .iter()
        .map(|raw| target(raw))
        .collect::<Result<Vec<Target>>>()?;
    let targets = chosen(&given, place.fork, forks::active_ids(&place.repo));
    if targets.is_empty() {
        println!("no forks, so nothing to wait for");
        return Ok(0);
    }
    let forks = targets
        .iter()
        .map(|target| forks::require(&place.repo, &ledger, target.fork))
        .collect::<Result<Vec<Fork>>>()?;
    let own = proc::lineage(std::process::id() as i32);
    let width = forks.iter().map(|fork| fork.label().chars().count()).max().unwrap_or(0);
    let watches = targets
        .into_iter()
        .zip(forks)
        .map(|(target, fork)| begin(target, fork, &own))
        .collect::<Result<Vec<Watch>>>()?;
    let deadline = args
        .timeout
        .and_then(|secs| Instant::now().checked_add(Duration::from_secs(secs)));
    poll(watches, &own, width, deadline, args.timeout.unwrap_or(0))
}

fn target(raw: &str) -> Result<Target> {
    place::parse_run_ref(raw)
        .map(|(fork, run)| Target { fork, run })
        .ok_or_else(|| anyhow!("expected <n> or <n>.<run>, got {raw}"))
}

/// What to wait on: the targets named, else the fork you're in, else every
/// active fork. A fork named whole covers its runs named alongside it.
fn chosen(given: &[Target], current: Option<u32>, active: Vec<u32>) -> Vec<Target> {
    let whole = |fork| Target { fork, run: None };
    let mut targets = match (given, current) {
        ([], Some(id)) => vec![whole(id)],
        ([], None) => active.into_iter().map(whole).collect(),
        (given, _) => given.to_vec(),
    };
    targets.sort_unstable();
    targets.dedup();
    let forks: Vec<u32> = targets
        .iter()
        .filter(|target| target.run.is_none())
        .map(|target| target.fork)
        .collect();
    targets
        .into_iter()
        .filter(|target| target.run.is_none() || !forks.contains(&target.fork))
        .collect()
}

fn scope(target: Target) -> Scope {
    match target.run {
        None => Scope::Fork,
        Some(_) => Scope::Run,
    }
}

fn begin(target: Target, fork: Fork, own: &[i32]) -> Result<Watch> {
    let memory = match target.run {
        None => watching(&look(&fork, own)?.unwrap_or_default().seen),
        Some(number) => {
            let runs = runs::load(&fork.dir)?;
            let run = named(fork.id, number, &runs, own)?;
            Memory {
                watched: vec![doubted(seen(run))],
                latest: None,
            }
        }
    };
    Ok(Watch {
        fork,
        scope: scope(target),
        memory,
    })
}

/// A fork watches the bg runs going now, and keeps its latest one in view.
fn watching(seen: &[Seen]) -> Memory {
    Memory {
        watched: seen
            .iter()
            .filter(|seen| seen.status == Status::Running)
            .copied()
            .collect(),
        latest: seen.last().copied().map(doubted),
    }
}

/// The bg run a `<n>.<run>` target names, unless it's the one wait runs in.
fn named<'a>(id: u32, number: u32, runs: &'a [Run], own: &[i32]) -> Result<&'a Run> {
    let run = runs
        .iter()
        .rev()
        .find(|run| run.start.run == number && counts(run.start.kind))
        .ok_or_else(|| anyhow!("fork {id} has no background run {id}.{number}"))?;
    match own.contains(&run.start.pid) {
        true => bail!("this wait runs inside {id}.{number}, so that run can't end first. wait on it from outside"),
        false => Ok(run),
    }
}

/// The look at the start doesn't count toward a death's two: the poll's
/// first look follows it at once, before a waiter could write its exit line.
fn doubted(seen: Seen) -> Seen {
    match seen.status {
        Status::Died => Seen {
            status: Status::Running,
            ..seen
        },
        _ => seen,
    }
}

fn poll(mut pending: Vec<Watch>, own: &[i32], width: usize, deadline: Option<Instant>, secs: u64) -> Result<i32> {
    let mut failed = false;
    loop {
        let last_look = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let steps = pending
            .into_iter()
            .map(|watch| {
                let now = look(&watch.fork, own)?;
                let step = step(watch.scope, &watch.memory, now.as_ref(), last_look);
                Ok((watch, step))
            })
            .collect::<Result<Vec<(Watch, Step)>>>()?;
        let mut busy = Vec::new();
        steps.into_iter().for_each(|(watch, step)| match step {
            Step::Settled(report) => {
                println!("{}", line(&watch.fork, width, &report));
                failed |= has_failure(&report);
            }
            Step::Busy(memory, report) => busy.push((Watch { memory, ..watch }, report)),
        });
        match (busy.is_empty(), last_look) {
            (true, _) => return Ok(i32::from(failed)),
            (false, true) => return Ok(give_up(&busy, width, secs)),
            (false, false) => {}
        }
        pending = busy.into_iter().map(|(watch, _)| watch).collect();
        thread::sleep(nap(deadline));
    }
}

fn nap(deadline: Option<Instant>) -> Duration {
    deadline
        .map(|deadline| deadline.saturating_duration_since(Instant::now()).min(POLL))
        .unwrap_or(POLL)
}

fn give_up(busy: &[(Watch, Report)], width: usize, secs: u64) -> i32 {
    busy.iter()
        .for_each(|(watch, report)| println!("{}", line(&watch.fork, width, report)));
    let forks: BTreeSet<u32> = busy.iter().map(|(watch, _)| watch.fork.id).collect();
    eprintln!("{}", timed_out(secs, forks.len()));
    124
}

fn timed_out(secs: u64, forks: usize) -> String {
    let forks = match forks {
        1 => "1 fork still has".to_string(),
        count => format!("{count} forks still have"),
    };
    format!("pfs: timed out after {secs}s and {forks} things running. `pfs wait` again to keep waiting")
}

/// The fork's bg runs right now, minus any wait itself runs inside (an agent
/// waiting in its own fork). None once the fork is gone.
fn look(fork: &Fork, own: &[i32]) -> Result<Option<Look>> {
    if !fork.dir.is_dir() {
        return Ok(None);
    }
    let bg: Vec<(Run, Seen)> = runs::load(&fork.dir)?
        .into_iter()
        .filter(|run| counts(run.start.kind) && !own.contains(&run.start.pid))
        .map(|run| {
            let seen = seen(&run);
            (run, seen)
        })
        .collect();
    Ok(Some(Look {
        ending: bg
            .last()
            .map(|(run, seen)| runs::ending_note(fork.id, run, seen.status)),
        seen: bg.into_iter().map(|(_, seen)| seen).collect(),
    }))
}

fn seen(run: &Run) -> Seen {
    Seen {
        run: run.start.run,
        kind: run.start.kind,
        status: run.status(),
    }
}

fn counts(kind: Kind) -> bool {
    matches!(kind, Kind::Bg)
}

/// One look at a target: settled once every watched run has ended and, for a
/// fork, nothing else in the background runs and a latest run that died got
/// its second look. Or once the fork is gone.
fn step(scope: Scope, memory: &Memory, now: Option<&Look>, last_look: bool) -> Step {
    let Some(now) = now else {
        return Step::Settled(removed(&memory.watched));
    };
    let watched: Vec<Seen> = memory.watched.iter().map(|old| latest(old, &now.seen)).collect();
    let ended = memory
        .watched
        .iter()
        .zip(&watched)
        .all(|(old, new)| has_ended(old.status, new.status, last_look));
    let others = match scope {
        Scope::Fork => others(&memory.watched, &now.seen),
        Scope::Run => Vec::new(),
    };
    let idle = scope == Scope::Fork && watched.is_empty() && others.is_empty();
    let newest = now.seen.last().copied();
    let told = !idle || newest.is_none_or(|new| has_ended(before(memory.latest, &new), new.status, last_look));
    let report = Report {
        removed: false,
        items: watched
            .iter()
            .chain(&others)
            .map(|seen| item(seen, outcome(seen.status)))
            .collect(),
        ending: now.ending.clone().filter(|_| idle),
    };
    match ended && others.is_empty() && told {
        true => Step::Settled(report),
        false => Step::Busy(
            Memory {
                watched,
                latest: newest,
            },
            report,
        ),
    }
}

fn removed(watched: &[Seen]) -> Report {
    Report {
        removed: true,
        items: watched
            .iter()
            .map(|seen| item(seen, removed_outcome(seen.status)))
            .collect(),
        ending: None,
    }
}

/// Bg runs going that wait didn't watch: started since, they hold the fork too.
fn others(watched: &[Seen], now: &[Seen]) -> Vec<Seen> {
    now.iter()
        .filter(|seen| seen.status == Status::Running && !watched.iter().any(|old| old.run == seen.run))
        .copied()
        .collect()
}

fn latest(old: &Seen, now: &[Seen]) -> Seen {
    now.iter()
        .rev()
        .find(|seen| seen.run == old.run)
        .copied()
        .unwrap_or(*old)
}

/// How the last look saw a run. One it didn't see hadn't started yet, so it
/// was as good as running.
fn before(latest: Option<Seen>, new: &Seen) -> Status {
    latest
        .filter(|old| old.run == new.run)
        .map_or(Status::Running, |old| old.status)
}

/// A run with no exit line whose process is gone counts as died only once a
/// second look agrees (or there's no second look coming): the waiter writes
/// the exit line just after the process ends, and a look can land in between.
fn has_ended(before: Status, now: Status, last_look: bool) -> bool {
    match (before, now) {
        (_, Status::Exited(_)) => true,
        (_, Status::Running) => false,
        (Status::Died, Status::Died) => true,
        (_, Status::Died) => last_look,
    }
}

fn outcome(status: Status) -> Outcome {
    match status {
        Status::Exited(code) => Outcome::Exited(code),
        Status::Died => Outcome::Died,
        Status::Running => Outcome::Running,
    }
}

fn removed_outcome(status: Status) -> Outcome {
    match status {
        Status::Exited(code) => Outcome::Exited(code),
        Status::Died | Status::Running => Outcome::Unseen,
    }
}

fn item(seen: &Seen, outcome: Outcome) -> Item {
    Item {
        run: seen.run,
        kind: seen.kind,
        outcome,
    }
}

fn has_failure(report: &Report) -> bool {
    report.items.iter().any(|item| match item.outcome {
        Outcome::Exited(code) => code != 0,
        Outcome::Died => true,
        Outcome::Unseen | Outcome::Running => false,
    })
}

fn describe(outcome: Outcome) -> String {
    match outcome {
        Outcome::Exited(code) => runs::describe(Status::Exited(code)),
        Outcome::Died => runs::describe(Status::Died),
        Outcome::Unseen => "outcome unknown".to_string(),
        Outcome::Running => "still running".to_string(),
    }
}

fn line(fork: &Fork, width: usize, report: &Report) -> String {
    format!("{:<width$}   {}", fork.label(), body(fork.id, report))
}

fn body(id: u32, report: &Report) -> String {
    let removed = report.removed.then(|| "removed".to_string());
    let items = report
        .items
        .iter()
        .map(|item| format!("{} {id}.{} {}", item.kind.as_str(), item.run, describe(item.outcome)));
    let parts: Vec<String> = removed.into_iter().chain(items).collect();
    match (parts.is_empty(), &report.ending) {
        (false, _) => parts.join(" · "),
        (true, None) => "nothing was running".to_string(),
        (true, Some(ending)) => format!("nothing was running · {ending}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(run: u32, kind: Kind, status: Status) -> Seen {
        Seen { run, kind, status }
    }

    fn sub(status: Status) -> Seen {
        seen(1, Kind::Bg, status)
    }

    fn watching_only(watched: &[Seen]) -> Memory {
        Memory {
            watched: watched.to_vec(),
            latest: None,
        }
    }

    fn now(seen: &[Seen]) -> Look {
        Look {
            seen: seen.to_vec(),
            ending: None,
        }
    }

    fn told(seen: &[Seen], ending: &str) -> Look {
        Look {
            seen: seen.to_vec(),
            ending: Some(ending.to_string()),
        }
    }

    fn report(items: Vec<Item>) -> Report {
        Report {
            removed: false,
            items,
            ending: None,
        }
    }

    fn settled(step: &Step) -> Option<&Report> {
        match step {
            Step::Settled(report) => Some(report),
            Step::Busy(..) => None,
        }
    }

    fn fork(fork: u32) -> Target {
        Target { fork, run: None }
    }

    fn run(fork: u32, run: u32) -> Target {
        Target { fork, run: Some(run) }
    }

    fn record(run: u32, kind: Kind, pid: i32) -> Run {
        Run {
            start: runs::Start {
                run,
                kind,
                cmd: "x".into(),
                pid,
                proc_start: None,
                ts: "t".into(),
            },
            exit: None,
        }
    }

    #[test]
    fn targets_are_forks_or_runs() {
        assert_eq!(target("5").unwrap(), fork(5));
        assert_eq!(target("5.2").unwrap(), run(5, 2));
        assert_eq!(target("5-kebab").unwrap(), fork(5));
        assert_eq!(
            target("5.x").unwrap_err().to_string(),
            "expected <n> or <n>.<run>, got 5.x"
        );
        assert!(target("kebab").is_err());
        assert!(target("5.0").is_err());
    }

    #[test]
    fn picks_named_then_current_then_all() {
        assert_eq!(
            chosen(&[fork(5), fork(3), fork(5)], Some(2), vec![1, 2]),
            vec![fork(3), fork(5)]
        );
        assert_eq!(chosen(&[], Some(2), vec![1, 2, 3]), vec![fork(2)]);
        assert_eq!(chosen(&[], None, vec![1, 4]), vec![fork(1), fork(4)]);
        assert!(chosen(&[], None, Vec::new()).is_empty());
    }

    #[test]
    fn a_whole_fork_covers_its_runs() {
        assert_eq!(chosen(&[run(5, 1), fork(5)], None, Vec::new()), vec![fork(5)]);
        assert_eq!(
            chosen(&[run(5, 2), run(5, 1), run(6, 1), run(5, 1)], Some(9), Vec::new()),
            vec![run(5, 1), run(5, 2), run(6, 1)]
        );
        assert_eq!(
            chosen(&[run(6, 1), fork(5)], None, Vec::new()),
            vec![fork(5), run(6, 1)]
        );
    }

    #[test]
    fn only_background_work_counts() {
        assert!(counts(Kind::Bg));
        assert!(!counts(Kind::Fg));
        assert!(!counts(Kind::Open));
    }

    #[test]
    fn a_run_target_must_be_a_bg_run_outside_wait() {
        let records = [
            record(1, Kind::Bg, 40),
            record(2, Kind::Fg, 41),
            record(3, Kind::Bg, 42),
        ];
        assert_eq!(named(19, 1, &records, &[7]).unwrap().start.run, 1);
        assert_eq!(
            named(19, 2, &records, &[7]).unwrap_err().to_string(),
            "fork 19 has no background run 19.2"
        );
        assert_eq!(
            named(19, 4, &records, &[7]).unwrap_err().to_string(),
            "fork 19 has no background run 19.4"
        );
        assert!(named(19, 3, &records, &[7, 42]).is_err());
    }

    #[test]
    fn the_first_look_never_confirms_a_death() {
        assert_eq!(doubted(sub(Status::Died)), sub(Status::Running));
        assert_eq!(doubted(sub(Status::Exited(1))), sub(Status::Exited(1)));
        assert_eq!(doubted(sub(Status::Running)), sub(Status::Running));
    }

    #[test]
    fn a_fork_watches_whats_going_and_keeps_the_latest_in_view() {
        let memory = watching(&[sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Running)]);
        assert_eq!(memory.watched, vec![seen(2, Kind::Bg, Status::Running)]);
        assert_eq!(memory.latest, Some(seen(2, Kind::Bg, Status::Running)));
        let memory = watching(&[seen(2, Kind::Bg, Status::Died)]);
        assert!(memory.watched.is_empty());
        assert_eq!(memory.latest, Some(seen(2, Kind::Bg, Status::Running)));
        assert_eq!(watching(&[]), Memory::default());
    }

    #[test]
    fn busy_while_a_watched_run_runs() {
        let watched = [sub(Status::Running)];
        let step = step(Scope::Fork, &watching_only(&watched), Some(&now(&watched)), false);
        assert_eq!(
            step,
            Step::Busy(
                Memory {
                    watched: watched.to_vec(),
                    latest: Some(watched[0]),
                },
                report(vec![item(&watched[0], Outcome::Running)])
            )
        );
    }

    #[test]
    fn settles_on_exit_lines() {
        let watched = [sub(Status::Running), seen(2, Kind::Bg, Status::Running)];
        let later = [sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Exited(3))];
        let step = step(Scope::Fork, &watching_only(&watched), Some(&now(&later)), false);
        let report = settled(&step).unwrap();
        assert_eq!(
            report.items.iter().map(|item| item.outcome).collect::<Vec<_>>(),
            vec![Outcome::Exited(0), Outcome::Exited(3)]
        );
        assert!(has_failure(report));
    }

    #[test]
    fn died_needs_a_second_look() {
        let first = step(
            Scope::Fork,
            &watching_only(&[sub(Status::Running)]),
            Some(&now(&[sub(Status::Died)])),
            false,
        );
        let Step::Busy(memory, _) = &first else {
            panic!("one look at a vanished process isn't enough");
        };
        let second = step(Scope::Fork, memory, Some(&now(&[sub(Status::Died)])), false);
        assert_eq!(settled(&second).unwrap().items[0].outcome, Outcome::Died);
        let exit_landed = step(Scope::Fork, memory, Some(&now(&[sub(Status::Exited(0))])), false);
        assert_eq!(settled(&exit_landed).unwrap().items[0].outcome, Outcome::Exited(0));
    }

    #[test]
    fn the_last_look_takes_died_at_its_word() {
        let step = step(
            Scope::Fork,
            &watching_only(&[sub(Status::Running)]),
            Some(&now(&[sub(Status::Died)])),
            true,
        );
        assert_eq!(settled(&step).unwrap().items[0].outcome, Outcome::Died);
    }

    #[test]
    fn runs_started_meanwhile_hold_the_fork() {
        let memory = watching_only(&[sub(Status::Running)]);
        let going = [sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Running)];
        let Step::Busy(_, report) = step(Scope::Fork, &memory, Some(&now(&going)), true) else {
            panic!("a bg run still going keeps the fork busy");
        };
        assert_eq!(body(4, &report), "bg 4.1 done · bg 4.2 still running");
        let later = [sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Exited(1))];
        let step = step(Scope::Fork, &memory, Some(&now(&later)), false);
        let report = settled(&step).unwrap();
        assert!(!has_failure(report));
        assert_eq!(body(4, report), "bg 4.1 done");
    }

    #[test]
    fn old_runs_dont_matter() {
        let old = [sub(Status::Died), seen(2, Kind::Bg, Status::Exited(1))];
        let memory = watching(&old);
        let step = step(Scope::Fork, &memory, Some(&now(&old)), false);
        let report = settled(&step).unwrap();
        assert!(!has_failure(report));
        assert_eq!(body(3, report), "nothing was running");
    }

    #[test]
    fn nothing_running_says_how_the_latest_ended() {
        let old = [sub(Status::Exited(1))];
        let step = step(
            Scope::Fork,
            &watching(&old),
            Some(&told(&old, "bg 19.1 failed (exit 1) 44s ago")),
            false,
        );
        let report = settled(&step).unwrap();
        assert!(!has_failure(report));
        assert_eq!(
            body(19, report),
            "nothing was running · bg 19.1 failed (exit 1) 44s ago"
        );
    }

    #[test]
    fn a_fork_that_never_ran_anything_keeps_the_plain_line() {
        let step = step(Scope::Fork, &watching(&[]), Some(&now(&[])), false);
        assert_eq!(body(19, settled(&step).unwrap()), "nothing was running");
    }

    #[test]
    fn a_latest_death_behind_nothing_running_gets_a_second_look() {
        let died = [sub(Status::Died)];
        let first = step(Scope::Fork, &watching(&died), Some(&told(&died, "bg 19.1 died")), false);
        let Step::Busy(memory, _) = &first else {
            panic!("the exit line may still be on its way");
        };
        let exit_landed = [sub(Status::Exited(0))];
        let landed = step(
            Scope::Fork,
            memory,
            Some(&told(&exit_landed, "bg 19.1 done 0s ago")),
            false,
        );
        assert_eq!(
            body(19, settled(&landed).unwrap()),
            "nothing was running · bg 19.1 done 0s ago"
        );
        let agreed = step(Scope::Fork, memory, Some(&told(&died, "bg 19.1 died")), false);
        assert_eq!(
            body(19, settled(&agreed).unwrap()),
            "nothing was running · bg 19.1 died"
        );
        let last = step(Scope::Fork, &watching(&died), Some(&told(&died, "bg 19.1 died")), true);
        assert!(settled(&last).is_some());
    }

    #[test]
    fn a_run_target_waits_on_that_run_only() {
        let memory = watching_only(&[sub(Status::Running)]);
        let going = [sub(Status::Running), seen(2, Kind::Bg, Status::Running)];
        assert!(settled(&step(Scope::Run, &memory, Some(&now(&going)), false)).is_none());
        let ended = [sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Running)];
        let step = step(Scope::Run, &memory, Some(&told(&ended, "bg 4.2 running")), false);
        let report = settled(&step).unwrap();
        assert!(!has_failure(report));
        assert_eq!(body(4, report), "bg 4.1 done");
    }

    #[test]
    fn a_run_that_already_ended_says_how() {
        let failed = [sub(Status::Exited(1))];
        let step = step(Scope::Run, &watching_only(&failed), Some(&now(&failed)), false);
        let report = settled(&step).unwrap();
        assert!(has_failure(report));
        assert_eq!(body(19, report), "bg 19.1 failed (exit 1)");
    }

    #[test]
    fn a_run_that_already_died_gets_a_second_look() {
        let memory = watching_only(&[doubted(sub(Status::Died))]);
        let died = [sub(Status::Died)];
        let first = step(Scope::Run, &memory, Some(&now(&died)), false);
        let Step::Busy(memory, _) = &first else {
            panic!("wait's first look can't confirm a death");
        };
        let second = step(Scope::Run, memory, Some(&now(&died)), false);
        let report = settled(&second).unwrap();
        assert!(has_failure(report));
        assert_eq!(body(19, report), "bg 19.1 died");
    }

    #[test]
    fn a_removed_fork_settles() {
        let watched = [sub(Status::Exited(0)), seen(2, Kind::Bg, Status::Running)];
        let step = step(Scope::Fork, &watching_only(&watched), None, false);
        let report = settled(&step).unwrap();
        assert!(!has_failure(report));
        assert_eq!(body(6, report), "removed · bg 6.1 done · bg 6.2 outcome unknown");
        assert_eq!(body(6, &removed(&[])), "removed");
    }

    #[test]
    fn failures_are_nonzero_exits_and_deaths() {
        let one = |outcome| report(vec![item(&sub(Status::Running), outcome)]);
        assert!(!has_failure(&one(Outcome::Exited(0))));
        assert!(has_failure(&one(Outcome::Exited(2))));
        assert!(has_failure(&one(Outcome::Died)));
        assert!(!has_failure(&one(Outcome::Unseen)));
    }

    #[test]
    fn lines_say_how_each_run_ended() {
        let report = report(vec![
            item(&sub(Status::Running), Outcome::Exited(0)),
            item(&seen(2, Kind::Bg, Status::Running), Outcome::Exited(101)),
            item(&seen(3, Kind::Bg, Status::Running), Outcome::Died),
        ]);
        assert_eq!(body(5, &report), "bg 5.1 done · bg 5.2 failed (exit 101) · bg 5.3 died");
    }

    #[test]
    fn timeouts_count_forks_not_runs() {
        assert_eq!(
            timed_out(3, 1),
            "pfs: timed out after 3s and 1 fork still has things running. `pfs wait` again to keep waiting"
        );
        assert!(timed_out(3, 2).contains("2 forks still have"));
    }

    #[test]
    fn naps_stop_at_the_deadline() {
        assert_eq!(nap(None), POLL);
        assert_eq!(nap(Some(Instant::now())), Duration::ZERO);
        assert_eq!(nap(Some(Instant::now() + Duration::from_secs(60))), POLL);
    }
}
