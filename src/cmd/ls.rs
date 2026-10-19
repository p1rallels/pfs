use std::{collections::BTreeMap, path::Path, thread};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    cli::LsArgs,
    clock, config, everywhere,
    forks::{self, Fork},
    git,
    ledger::{self, Entry},
    paths, place, ports,
    runs::{self, Kind, Run, Status},
};

/// Logs this big get flagged so a chatty `bun dev` can't quietly eat the disk.
const BIG_LOG: u64 = 1024 * 1024 * 1024;

pub fn run(args: LsArgs) -> Result<i32> {
    match place::try_here()? {
        Some(place) => in_repo(args, place),
        None => every_repo(args),
    }
}

/// Outside any repo: every main's forks, from the forks dir, and the forks
/// whose main isn't there anymore.
fn every_repo(args: LsArgs) -> Result<i32> {
    if args.all || args.json || args.overlap {
        bail!("--all, --json and --overlap work inside a repo. outside one, `pfs ls` lists every repo's forks");
    }
    let config = config::load_global()?;
    let repos = everywhere::scan(&config.forks_dir)?;
    everywhere::render(&config.forks_dir, &repos, paths::tildify)
        .iter()
        .for_each(|line| println!("{line}"));
    Ok(0)
}

fn in_repo(args: LsArgs, place: place::Place) -> Result<i32> {
    let config = config::load_global()?;
    let ledger = ledger::load(&place.repo)?;
    let active: Vec<Fork> = forks::active_ids(&place.repo)
        .into_iter()
        .filter_map(|id| forks::require(&place.repo, &ledger, id).ok())
        .collect();
    let removed: Vec<&Entry> = match args.all {
        true => ledger
            .values()
            .filter(|entry| entry.removed.is_some())
            .filter(|entry| !active.iter().any(|fork| fork.id == entry.fork.id))
            .collect(),
        false => Vec::new(),
    };
    let kept = match args.all {
        true => git::branches(&place.repo.root, "pfs/"),
        false => Vec::new(),
    };
    let views: Vec<View> = active
        .iter()
        .zip(works(&active, args.overlap))
        .map(|(fork, work)| view(fork, place.fork, config.ports.block(fork.id), work))
        .collect();
    let shared = args.overlap.then(|| shared(changes(&views)));
    match (args.json, views.is_empty() && removed.is_empty()) {
        (true, _) => print_json(&views, &removed, &kept, shared.as_deref()),
        (false, true) => println!(
            "{}",
            empty_note(ledger.values().filter(|entry| entry.removed.is_some()).count())
        ),
        (false, false) => {
            print_table(&views, &removed, &kept);
            if let Some(shared) = &shared {
                print_overlap(&views, shared);
            }
        }
    }
    Ok(0)
}

struct View<'a> {
    fork: &'a Fork,
    current: bool,
    ports: (u32, u32),
    runs: Vec<(Run, Status)>,
    work: Work,
}

/// What `pfs rm` would land from a fork, whether it would refuse, and with
/// `--overlap` the files it changed since it started. `None` where git
/// couldn't tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Work {
    commits: Option<u32>,
    uncommitted: Option<bool>,
    changed: Option<Vec<String>>,
}

/// A few git calls per fork, so the forks are checked at once.
fn works(active: &[Fork], overlap: bool) -> Vec<Work> {
    thread::scope(|scope| {
        let checks: Vec<_> = active
            .iter()
            .map(|fork| scope.spawn(move || work(fork, overlap)))
            .collect();
        checks
            .into_iter()
            .map(|check| check.join().unwrap_or_default())
            .collect()
    })
}

/// Forks without a ledger entry are skipped: mid-creation, or with no base
/// to count from. The folder's tree is built once, for both checks on it.
fn work(fork: &Fork, overlap: bool) -> Work {
    match &fork.entry {
        Some(_) => {
            let now = forks::now_tree(fork, "ls").ok();
            Work {
                commits: forks::commits(fork).ok(),
                uncommitted: now.as_deref().and_then(|now| forks::uncommitted_at(fork, now).ok()),
                changed: now
                    .as_deref()
                    .filter(|_| overlap)
                    .and_then(|now| forks::changed(fork, now).ok()),
            }
        }
        None => Work::default(),
    }
}

/// Blank would read as nothing to land, so what pfs couldn't tell is a `?`.
fn work_notes(work: &Work) -> Vec<String> {
    let commits = match work.commits {
        None | Some(0) => None,
        Some(1) => Some("1 commit".to_string()),
        Some(count) => Some(format!("{count} commits")),
    };
    let uncommitted = (work.uncommitted == Some(true)).then(|| "uncommitted".to_string());
    let unknown = (work.commits.is_none() || work.uncommitted.is_none()).then(|| "?".to_string());
    commits.into_iter().chain(uncommitted).chain(unknown).collect()
}

/// Each fork pfs could check, with the files it changed since it started.
fn changes<'a>(views: &'a [View]) -> impl Iterator<Item = (u32, &'a [String])> {
    views
        .iter()
        .filter_map(|view| view.work.changed.as_deref().map(|paths| (view.fork.id, paths)))
}

/// Files two or more forks changed, by path, each with those forks in the
/// order they came.
fn shared<'a, K: Copy + PartialEq>(changes: impl IntoIterator<Item = (K, &'a [String])>) -> Vec<(&'a str, Vec<K>)> {
    changes
        .into_iter()
        .flat_map(|(fork, paths)| paths.iter().map(move |path| (path.as_str(), fork)))
        .fold(BTreeMap::<&str, Vec<K>>::new(), |mut by_path, (path, fork)| {
            let forks = by_path.entry(path).or_default();
            if !forks.contains(&fork) {
                forks.push(fork);
            }
            by_path
        })
        .into_iter()
        .filter(|(_, forks)| forks.len() > 1)
        .collect()
}

/// One line per shared file, its forks lined up after the longest path.
fn overlap_lines(rows: &[(&str, String)]) -> Vec<String> {
    let width = rows.iter().map(|(path, _)| path.chars().count()).max().unwrap_or(0);
    rows.iter()
        .map(|(path, forks)| format!("{path:<width$}   {forks}"))
        .collect()
}

/// "yet" only when there never was a fork: history is one flag away.
fn empty_note(removed: usize) -> String {
    match removed {
        0 => "no forks yet. `pfs new <name>` makes one".to_string(),
        count => format!("no active forks ({count} removed: `pfs ls --all`). `pfs new <name>` makes one"),
    }
}

/// The shared files, or a line saying there are none (so an empty answer
/// can't pass for a flag that was ignored), then what was skipped.
fn overlap_report(rows: &[(&str, String)], skipped: &[String]) -> Vec<String> {
    let shared = match (rows.is_empty(), skipped.is_empty()) {
        (false, _) => overlap_lines(rows),
        (true, true) => vec!["no file changed in more than one fork".to_string()],
        (true, false) => vec!["no file changed in more than one of the forks pfs could check".to_string()],
    };
    shared.into_iter().chain(skipped_note(skipped)).collect()
}

/// Forks left out of the overlap, in one line.
fn skipped_note(labels: &[String]) -> Option<String> {
    (!labels.is_empty()).then(|| format!("skipped {}: couldn't tell what changed", labels.join(", ")))
}

/// The files one fork shares, each with the other forks that changed it.
fn overlap_of<'a>(shared: &[(&'a str, Vec<u32>)], id: u32) -> BTreeMap<&'a str, Vec<u32>> {
    shared
        .iter()
        .filter(|(_, ids)| ids.contains(&id))
        .map(|(path, ids)| (*path, ids.iter().copied().filter(|other| *other != id).collect()))
        .collect()
}

fn view(fork: &Fork, current: Option<u32>, ports: (u32, u32), work: Work) -> View<'_> {
    let runs = runs::load(&fork.dir)
        .unwrap_or_default()
        .into_iter()
        .map(|run| {
            let status = run.status();
            (run, status)
        })
        .collect();
    View {
        fork,
        current: current == Some(fork.id),
        ports,
        runs,
        work,
    }
}

/// Where a fork came from, when it isn't plainly the main folder.
/// `from` keeps the rev as typed, so a moving one (`HEAD~1`, a branch) gets
/// the commit it meant at the time.
fn lineage(fork: &ledger::Fork) -> Option<String> {
    match fork.of {
        Some(of) => Some(format!("of {of}")),
        None => fork.started_from().map(|from| format!("from {from}")),
    }
}

/// What's going on in a fork, in a few words: where it came from, work it
/// hasn't landed, what's running, how its last bg run ended, and logs worth
/// deleting.
fn notes(view: &View) -> Vec<String> {
    let id = view.fork.id;
    let origin = view.fork.entry.as_ref().and_then(|entry| lineage(&entry.fork));
    let running = view
        .runs
        .iter()
        .filter(|(_, status)| *status == Status::Running)
        .map(|(run, _)| running_note(id, run));
    let last_bg = last_bg_note(id, &view.runs);
    let big_logs = view.runs.iter().filter_map(|(run, _)| {
        runs::log_size(&view.fork.dir, run)
            .filter(|size| *size >= BIG_LOG)
            .map(|size| format!("log {id}.{} {}", run.start.run, paths::human_bytes(size)))
    });
    let orphan = view
        .fork
        .entry
        .is_none()
        .then(|| "not in the ledger (being created, or left by a crash)".to_string());
    origin
        .into_iter()
        .chain(work_notes(&view.work))
        .chain(running)
        .chain(last_bg)
        .chain(big_logs)
        .chain(orphan)
        .collect()
}

/// Once no bg run is going, the latest one says how it ended, so a finished
/// sub doesn't vanish from the board.
fn last_bg_note(id: u32, runs: &[(Run, Status)]) -> Option<String> {
    let bg = || runs.iter().filter(|(run, _)| run.start.kind == Kind::Bg);
    match bg().any(|(_, status)| *status == Status::Running) {
        true => None,
        false => bg()
            .next_back()
            .map(|(run, status)| runs::ending_note(id, run, *status)),
    }
}

fn running_note(id: u32, run: &Run) -> String {
    let age = clock::ago(&run.start.ts);
    match run.start.kind {
        Kind::Open => format!("open {age}"),
        Kind::Bg => format!("bg {id}.{} running {age}", run.start.run),
        Kind::Fg => format!("run {id}.{} running {age}", run.start.run),
    }
}

fn print_table(views: &[View], removed: &[&Entry], kept: &[String]) {
    let labels: Vec<String> = views.iter().map(|view| view.fork.label()).collect();
    let removed_labels: Vec<String> = removed.iter().map(|entry| entry.label()).collect();
    let width = labels
        .iter()
        .chain(&removed_labels)
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
    views.iter().zip(&labels).for_each(|(view, label)| {
        let marker = if view.current { '*' } else { ' ' };
        let notes = notes(view).join(" · ");
        let line = format!("{marker} {label:<width$}   {}   {notes}", ports::show(view.ports));
        println!("{}", line.trim_end());
    });
    removed.iter().zip(&removed_labels).for_each(|(entry, label)| {
        let rm = entry.removed.as_ref();
        let ago = rm.map(|rm| clock::ago(&rm.ts)).unwrap_or_default();
        let saved = saved_note(rm.and_then(|rm| rm.saved_branch.as_deref()), kept);
        println!("  {label:<width$}   removed {ago} ago{saved}");
    });
}

/// A removed fork's saved branch, which a merge and `git branch -d` may have
/// deleted since: the ledger only knows it was saved.
fn saved_note(saved: Option<&str>, kept: &[String]) -> String {
    match saved {
        None => String::new(),
        Some(branch) if kept.iter().any(|kept| kept == branch) => format!(" · saved {branch}"),
        Some(branch) => format!(" · saved {branch}, deleted since"),
    }
}

/// After the table, apart from it: the shared files, then what was skipped.
fn print_overlap(views: &[View], shared: &[(&str, Vec<u32>)]) {
    let label = |id: &u32| {
        views
            .iter()
            .find(|view| view.fork.id == *id)
            .map(|view| view.fork.label())
            .unwrap_or_else(|| id.to_string())
    };
    let rows: Vec<(&str, String)> = shared
        .iter()
        .map(|(path, ids)| (*path, ids.iter().map(label).collect::<Vec<_>>().join(", ")))
        .collect();
    let skipped: Vec<String> = views
        .iter()
        .filter(|view| view.work.changed.is_none())
        .map(|view| view.fork.label())
        .collect();
    println!();
    overlap_report(&rows, &skipped)
        .iter()
        .for_each(|line| println!("  {line}"));
}

/// With `--overlap`, each active fork gets `overlap`: the files it shares,
/// each with the other forks' ids, `{}` for none, `null` if skipped.
fn print_json(views: &[View], removed: &[&Entry], kept: &[String], shared: Option<&[(&str, Vec<u32>)]>) {
    views.iter().for_each(|view| {
        let runs: Vec<Value> = view
            .runs
            .iter()
            .map(|(run, status)| run_json(&view.fork.dir, run, *status))
            .collect();
        let entry = view.fork.entry.as_ref().map(|entry| &entry.fork);
        let mut object = json!({
                "id": view.fork.id,
                "label": view.fork.label(),
                "branch": view.fork.branch(),
                "path": view.fork.dir,
                "ports": [view.ports.0, view.ports.1],
                "current": view.current,
                "removed": false,
                "from": entry.and_then(|fork| fork.from.clone()),
                "of": entry.and_then(|fork| fork.of),
                "parent_session": entry.and_then(|fork| fork.parent_session.clone()),
                "created": entry.map(|fork| fork.ts.clone()),
                "commits": view.work.commits,
                "uncommitted": view.work.uncommitted,
                "notes": notes(view),
                "runs": runs,
        });
        if let Some(shared) = shared {
            object["overlap"] = json!(view.work.changed.as_ref().map(|_| overlap_of(shared, view.fork.id)));
        }
        println!("{object}");
    });
    removed.iter().for_each(|entry| {
        let saved = entry.removed.as_ref().and_then(|rm| rm.saved_branch.clone());
        println!(
            "{}",
            json!({
                "id": entry.fork.id,
                "label": entry.label(),
                "branch": entry.fork.branch,
                "removed": true,
                "saved_branch_exists": saved.as_ref().map(|branch| kept.contains(branch)),
                "saved_branch": saved,
                "from": entry.fork.from,
                "of": entry.fork.of,
                "parent_session": entry.fork.parent_session,
            })
        );
    });
}

fn run_json(fork_dir: &Path, run: &Run, status: Status) -> Value {
    json!({
        "run": run.start.run,
        "kind": run.start.kind,
        "cmd": run.start.cmd,
        "pid": run.start.pid,
        "status": runs::describe(status),
        "code": run.exit.as_ref().map(|exit| exit.code),
        "started": run.start.ts,
        "ended": run.exit.as_ref().map(|exit| exit.ts.clone()),
        "log": run.log(fork_dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fork(from: Option<&str>, of: Option<u32>, base: Option<&str>) -> ledger::Fork {
        ledger::Fork {
            id: 9,
            name: None,
            branch: "pfs/9".into(),
            base: base.map(str::to_string),
            from: from.map(str::to_string),
            of,
            parent_session: None,
            ts: "t".into(),
        }
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn work_shows_only_what_would_land_or_block() {
        let work = |commits, uncommitted| {
            work_notes(&Work {
                commits,
                uncommitted,
                changed: None,
            })
        };
        assert_eq!(work(Some(2), Some(true)), vec!["2 commits", "uncommitted"]);
        assert_eq!(work(Some(1), Some(false)), vec!["1 commit"]);
        assert_eq!(work(Some(0), Some(true)), vec!["uncommitted"]);
        assert!(work(Some(0), Some(false)).is_empty());
    }

    #[test]
    fn work_pfs_couldnt_check_is_a_question_mark_not_blank() {
        let work = |commits, uncommitted| {
            work_notes(&Work {
                commits,
                uncommitted,
                changed: None,
            })
        };
        assert_eq!(work(None, None), vec!["?"]);
        assert_eq!(work(Some(2), None), vec!["2 commits", "?"]);
        assert_eq!(work(None, Some(false)), vec!["?"]);
    }

    #[test]
    fn shared_files_are_the_ones_two_or_more_forks_changed() {
        let (two, three, four) = (
            paths(&["src/cli.rs", "src/cmd/wait.rs"]),
            paths(&["src/cli.rs", "src/cmd/rm.rs", "README.md"]),
            paths(&["src/cmd/rm.rs", "src/cli.rs"]),
        );
        let changes = [(2, two.as_slice()), (3, three.as_slice()), (4, four.as_slice())];
        assert_eq!(
            shared(changes),
            vec![("src/cli.rs", vec![2, 3, 4]), ("src/cmd/rm.rs", vec![3, 4])]
        );
    }

    #[test]
    fn nothing_is_shared_alone_or_twice_by_one_fork() {
        let (alone, twice) = (paths(&["a", "b"]), paths(&["c", "c"]));
        assert!(shared([(2, alone.as_slice())]).is_empty());
        assert!(shared([(2, alone.as_slice()), (3, twice.as_slice())]).is_empty());
        assert!(shared(Vec::<(u32, &[String])>::new()).is_empty());
    }

    #[test]
    fn overlap_lines_up_the_forks() {
        let rows = [
            ("src/cli.rs", "2-wait, 3-rm-many".to_string()),
            ("src/cmd/rm.rs", "3-rm-many, 4-ls-work".to_string()),
        ];
        assert_eq!(
            overlap_lines(&rows),
            vec![
                "src/cli.rs      2-wait, 3-rm-many",
                "src/cmd/rm.rs   3-rm-many, 4-ls-work"
            ]
        );
        assert!(overlap_lines(&[]).is_empty());
    }

    #[test]
    fn skipped_forks_get_one_line_and_none_get_nothing() {
        let labels = paths(&["2-wait", "3-rm-many"]);
        assert_eq!(
            skipped_note(&labels).unwrap(),
            "skipped 2-wait, 3-rm-many: couldn't tell what changed"
        );
        assert_eq!(
            skipped_note(&labels[..1]).unwrap(),
            "skipped 2-wait: couldn't tell what changed"
        );
        assert_eq!(skipped_note(&[]), None);
    }

    #[test]
    fn overlap_says_so_when_nothing_is_shared() {
        let rows = [("src/cli.rs", "2-wait, 3-rm-many".to_string())];
        let skipped = paths(&["4-ls-work"]);
        assert_eq!(overlap_report(&rows, &[]), vec!["src/cli.rs   2-wait, 3-rm-many"]);
        assert_eq!(overlap_report(&[], &[]), vec!["no file changed in more than one fork"]);
        assert_eq!(
            overlap_report(&[], &skipped),
            vec![
                "no file changed in more than one of the forks pfs could check",
                "skipped 4-ls-work: couldn't tell what changed"
            ]
        );
    }

    #[test]
    fn json_overlap_names_the_other_forks() {
        let shared = [("src/cli.rs", vec![2, 3, 4]), ("src/cmd/rm.rs", vec![3, 4])];
        assert_eq!(
            overlap_of(&shared, 3),
            BTreeMap::from([("src/cli.rs", vec![2, 4]), ("src/cmd/rm.rs", vec![4])])
        );
        assert_eq!(overlap_of(&shared, 2), BTreeMap::from([("src/cli.rs", vec![3, 4])]));
        assert!(overlap_of(&shared, 5).is_empty());
    }

    fn bg(run: u32, kind: Kind, code: Option<i32>, alive: bool) -> (Run, Status) {
        let run = Run {
            start: runs::Start {
                run,
                kind,
                cmd: "x".into(),
                pid: 0,
                proc_start: None,
                ts: clock::now(),
            },
            exit: code.map(|code| runs::Exit {
                run,
                code,
                signal: None,
                ts: clock::now(),
            }),
        };
        let status = match (code, alive) {
            (Some(code), _) => Status::Exited(code),
            (None, true) => Status::Running,
            (None, false) => Status::Died,
        };
        (run, status)
    }

    #[test]
    fn the_last_bg_run_says_how_it_ended() {
        let done = [bg(1, Kind::Bg, Some(1), false), bg(2, Kind::Bg, Some(0), false)];
        assert_eq!(last_bg_note(5, &done).unwrap(), "bg 5.2 done 0s ago");
        let failed = [bg(1, Kind::Bg, Some(1), false), bg(2, Kind::Fg, Some(0), false)];
        assert_eq!(last_bg_note(5, &failed).unwrap(), "bg 5.1 failed (exit 1) 0s ago");
        let died = [bg(3, Kind::Bg, None, false), bg(4, Kind::Open, None, true)];
        assert_eq!(last_bg_note(5, &died).unwrap(), "bg 5.3 died, started 0s ago");
    }

    #[test]
    fn no_ending_while_a_bg_run_goes_or_before_any() {
        let going = [bg(1, Kind::Bg, Some(0), false), bg(2, Kind::Bg, None, true)];
        assert_eq!(last_bg_note(5, &going), None);
        assert_eq!(last_bg_note(5, &[bg(1, Kind::Fg, Some(0), false)]), None);
        assert_eq!(last_bg_note(5, &[]), None);
    }

    #[test]
    fn lineage_names_the_commit_a_moving_rev_meant() {
        assert_eq!(lineage(&fork(None, Some(3), Some("abc"))), Some("of 3".to_string()));
        assert_eq!(
            lineage(&fork(Some("HEAD~1"), None, Some("4e7547fdeadbeef"))),
            Some("from HEAD~1 (4e7547f)".to_string())
        );
        assert_eq!(
            lineage(&fork(Some("4e7547f"), None, Some("4e7547fdeadbeef"))),
            Some("from 4e7547f".to_string())
        );
        assert_eq!(lineage(&fork(None, None, Some("abc"))), None);
    }

    #[test]
    fn a_saved_branch_deleted_since_says_so() {
        let kept = paths(&["pfs/2-truncate"]);
        assert_eq!(saved_note(Some("pfs/2-truncate"), &kept), " · saved pfs/2-truncate");
        assert_eq!(
            saved_note(Some("pfs/1-slugify"), &kept),
            " · saved pfs/1-slugify, deleted since"
        );
        assert_eq!(saved_note(None, &kept), "");
    }

    #[test]
    fn an_empty_board_says_yet_only_without_history() {
        assert_eq!(empty_note(0), "no forks yet. `pfs new <name>` makes one");
        assert_eq!(
            empty_note(16),
            "no active forks (16 removed: `pfs ls --all`). `pfs new <name>` makes one"
        );
    }
}
