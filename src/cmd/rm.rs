use std::{fs, path::Path, time::SystemTime};

use anyhow::{Context, Result, bail};

use crate::{
    cli::RmArgs,
    clock,
    forks::{self, Fork},
    git,
    ledger::{self, Event},
    occupants::{self, Conversation, Occupant, Process},
    paths,
    place::{self, Repo},
    proc,
    runs::Kind,
};

/// A run's command is cut to this many characters: a pi task can be pages long.
const CMD_WIDTH: usize = 60;

struct Removed {
    label: String,
    saved: Option<String>,
}

/// What rm looks at once, before the first removal.
struct Look {
    /// Every process's working folder, from one lsof for all the forks.
    everyone: Vec<Process>,
    /// rm itself and every process above it.
    own: Vec<i32>,
}

/// The checks `--force` skips. Who's working in the fork isn't one of
/// them: nothing skips that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Skipped {
    open_turns: bool,
    uncommitted: bool,
}

/// What stands between a fork and its removal.
struct Found {
    present: Vec<Occupant>,
    conversations: Vec<Conversation>,
    uncommitted: bool,
}

/// What to tell someone about one occupant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hint {
    /// A bg run leads its own process group.
    Kill(i32),
    /// A door is someone's shell: never a kill.
    ExitDoor,
    DoorYoureIn,
    /// rm runs under it, so rm can't finish before it does.
    YoureIn,
    /// A fg run: someone sits at it.
    Quit,
    /// A process that's no run's: what `pfs kill <n>` stops.
    Leftover(u32),
}

/// Each fork is checked and removed on its own, so one that's refused
/// keeps only itself: the rest still go, and the exit code says so after.
/// One lsof serves them all, before the first removal; without it nothing
/// is removed.
pub fn run(args: RmArgs) -> Result<i32> {
    let place = place::here()?;
    place.require_main("rm")?;
    let ids = args
        .forks
        .iter()
        .map(|raw| place.target(Some(raw)))
        .collect::<Result<Vec<_>>>()?;
    let ids = unique(ids);
    let look = Look {
        everyone: occupants::processes().context("nothing was removed")?,
        own: proc::lineage(std::process::id() as i32),
    };
    let skipped = skipped(args.force);
    let kept = try_each(&ids, |id| {
        remove(&place.repo, id, skipped, &look).map(|removed| report(&place.repo, &removed))
    });
    if let Some(note) = kept_note(ids.len(), &kept) {
        eprintln!("{note}");
    }
    Ok(match kept.is_empty() {
        true => 0,
        false => 1,
    })
}

/// `pfs rm 3 3-csv` means fork 3, once.
fn unique(ids: Vec<u32>) -> Vec<u32> {
    ids.iter()
        .enumerate()
        .filter(|(at, id)| !ids[..*at].contains(id))
        .map(|(_, id)| *id)
        .collect()
}

/// Runs `step` on every id in order, whatever happened to the ones before,
/// printing each error as it comes. Returns the ids it failed on.
fn try_each(ids: &[u32], mut step: impl FnMut(u32) -> Result<()>) -> Vec<u32> {
    ids.iter()
        .copied()
        .filter(|id| match step(*id) {
            Ok(()) => false,
            Err(error) => {
                eprintln!("pfs: {error:#}");
                true
            }
        })
        .collect()
}

/// Which forks are still there after several, since their reasons may have
/// scrolled by. A lone fork's error already says it all.
fn kept_note(asked: usize, kept: &[u32]) -> Option<String> {
    match (asked, kept) {
        (0 | 1, _) | (_, []) => None,
        (_, kept) => Some(format!(
            "pfs: {} of {asked} not removed ({}), see above for why",
            kept.len(),
            kept.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn skipped(force: bool) -> Skipped {
    Skipped {
        open_turns: force,
        uncommitted: force,
    }
}

/// Never loses work or deletes a fork from under anyone. Refuses while
/// anyone works in it (a process with its folder there, a pfs run still
/// going wherever it is now), while a conversation there is mid-turn, or
/// while it has uncommitted changes, all said at once; `--force` skips the
/// last two and never kills anything. Then copies the fork's branch into
/// the main repo before deleting the folder. Conversations harnesses saved
/// outside the fork (pi's sessions) are left alone.
fn remove(repo: &Repo, id: u32, skipped: Skipped, look: &Look) -> Result<Removed> {
    let ledger = ledger::load(repo)?;
    let fork = forks::require(repo, &ledger, id)?;
    let present = occupants::present(&fork.dir, &look.everyone)?;
    let conversations = match skipped.open_turns {
        true => Vec::new(),
        false => occupants::mid_turn(&fork.dir, &present)?,
    };
    let uncommitted = !skipped.uncommitted && forks::uncommitted(&fork)?;
    let found = Found {
        present,
        conversations,
        uncommitted,
    };
    if let Some(refusal) = refusal(&fork, &found, &look.own, SystemTime::now()) {
        bail!("{refusal}");
    }
    let saved = save_branch(repo, &fork)?;
    let trash = fork.trash();
    fs::rename(&fork.dir, &trash).with_context(|| format!("can't move {} aside", paths::tildify(&fork.dir)))?;
    {
        let _lock = ledger::lock(repo)?;
        ledger::append(
            repo,
            &Event::Rm(ledger::Rm {
                id,
                saved_branch: saved.clone(),
                ts: clock::now(),
            }),
        )?;
    }
    paths::remove_tree(&trash)?;
    forks::sweep_trash(repo);
    Ok(Removed {
        label: fork.label(),
        saved,
    })
}

fn report(repo: &Repo, removed: &Removed) {
    let label = &removed.label;
    match &removed.saved {
        Some(branch) => {
            println!(
                "removed {label}. its work is on branch {branch} in the main repo (git branch -D {branch} drops it)"
            );
            if let Some(note) = merge_note(repo, branch) {
                println!("{note}");
            }
        }
        None => println!("removed {label} (it had no commits, so there was no branch to save)"),
    }
}

/// One plain line on how the branch meets the main folder's branch right
/// now, for picking a merge order. Not a task: merging is when it's handled.
fn merge_note(repo: &Repo, branch: &str) -> Option<String> {
    let main = repo.root.as_path();
    git::head(main)?;
    let ours = git::current_branch(main).unwrap_or_else(|| "HEAD".to_string());
    if git::is_ancestor(main, branch, &ours) {
        return Some(format!("vs {ours} right now: already in it"));
    }
    let conflicts = git::merge_conflicts(main, &ours, branch)?;
    Some(merge_words(&ours, &conflicts, &in_the_way(main, &ours, branch)))
}

/// The main folder's uncommitted files the merge would write: git refuses the
/// merge until they're committed or stashed, and merge-tree only sees commits.
fn in_the_way(main: &Path, ours: &str, branch: &str) -> Vec<String> {
    let uncommitted = git::uncommitted_paths(main).unwrap_or_default();
    git::incoming_paths(main, ours, branch)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| uncommitted.contains(path))
        .collect()
}

fn merge_words(ours: &str, conflicts: &[String], in_the_way: &[String]) -> String {
    let merge = match conflicts {
        [] => "merges clean".to_string(),
        files => format!(
            "{} would conflict ({}) · resolve when merging",
            file_count(files.len()),
            paths::shown(files, 3)
        ),
    };
    let once = match in_the_way {
        [] => String::new(),
        [file] => format!(", once {file} (uncommitted here) is committed or stashed"),
        files => format!(
            ", once {} uncommitted here ({}) are committed or stashed",
            file_count(files.len()),
            paths::shown(files, 3)
        ),
    };
    format!("vs {ours} right now: {merge}{once}")
}

fn file_count(count: usize) -> String {
    match count {
        1 => "1 file".to_string(),
        count => format!("{count} files"),
    }
}

/// Everything that keeps the fork, in one go, so one look is enough.
fn refusal(fork: &Fork, found: &Found, own: &[i32], now: SystemTime) -> Option<String> {
    let parts: Vec<String> = [
        someone_there(fork, &found.present, own),
        open_turns(fork, &found.conversations, now),
        found.uncommitted.then(|| unsaved(fork)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn someone_there(fork: &Fork, present: &[Occupant], own: &[i32]) -> Option<String> {
    let lines: Vec<String> = present
        .iter()
        .map(|occupant| line(fork.id, occupant, hint(fork.id, occupant, own)))
        .collect();
    (!lines.is_empty()).then(|| {
        format!(
            "fork {} has someone working in it:\n{}\nrm never stops anyone, and --force doesn't skip this: rm again once they're done",
            fork.label(),
            lines.join("\n")
        )
    })
}

pub(super) fn hint(id: u32, occupant: &Occupant, own: &[i32]) -> Hint {
    let kind = match occupant {
        Occupant::Run(run) => Some(run.start.kind),
        Occupant::Process(_) => None,
    };
    match (kind, own.contains(&occupant.pid())) {
        (Some(Kind::Open), true) => Hint::DoorYoureIn,
        (_, true) => Hint::YoureIn,
        (Some(Kind::Bg), false) => Hint::Kill(occupant.pid()),
        (Some(Kind::Open), false) => Hint::ExitDoor,
        (Some(Kind::Fg), false) => Hint::Quit,
        (None, false) => Hint::Leftover(id),
    }
}

fn say(hint: Hint) -> String {
    match hint {
        Hint::Kill(pid) => format!("stop: kill -- -{pid}"),
        Hint::ExitDoor => "exit that shell".to_string(),
        Hint::DoorYoureIn => "that's the door you're in: exit it, then rm from a normal shell".to_string(),
        Hint::YoureIn => "this rm runs inside it: leave it, then rm from outside the fork".to_string(),
        Hint::Quit => "finish or quit it".to_string(),
        Hint::Leftover(id) => format!("stop: pfs kill {id}"),
    }
}

/// `5.1 bg pid 4242  pi -p fix the csv…  (stop: kill -- -4242)`, or for a
/// process that's no run's, `pid 70897  sleep  (stop: pfs kill 5)`.
pub(super) fn line(id: u32, occupant: &Occupant, hint: Hint) -> String {
    match occupant {
        Occupant::Run(run) => format!(
            "  {id}.{} {} pid {}  {}  ({})",
            run.start.run,
            run.start.kind.as_str(),
            run.start.pid,
            clip(&run.start.cmd),
            say(hint)
        ),
        Occupant::Process(process) => format!("  pid {}  {}  ({})", process.pid, process.command, say(hint)),
    }
}

/// The command's first line, cut to `CMD_WIDTH` characters.
fn clip(cmd: &str) -> String {
    let cut: String = cmd.lines().next().unwrap_or_default().chars().take(CMD_WIDTH).collect();
    match cut.len() < cmd.len() {
        true => format!("{}…", cut.trim_end()),
        false => cut,
    }
}

fn open_turns(fork: &Fork, conversations: &[Conversation], now: SystemTime) -> Option<String> {
    let force = format!("pfs rm {} --force", fork.id);
    match conversations {
        [] => None,
        [one] => Some(format!(
            "fork {}: {}. wait for it, or if it crashed: {force}",
            fork.label(),
            occupants::describe(one, now)
        )),
        several => Some(format!(
            "fork {} has conversations mid-turn:\n{}\nwait for them, or if they crashed: {force}",
            fork.label(),
            several
                .iter()
                .map(|conversation| format!("  {}", occupants::describe(conversation, now)))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

fn unsaved(fork: &Fork) -> String {
    format!(
        "fork {} has uncommitted changes. commit them on {} (see them with pfs diff {}), or pass --force to drop them",
        fork.label(),
        fork.branch(),
        fork.id
    )
}

/// Copies the fork's branch into the main repo (a plain `git fetch` from the
/// fork) when it has commits of its own.
fn save_branch(repo: &Repo, fork: &Fork) -> Result<Option<String>> {
    let branch = fork.branch();
    let Some(tip) = git::branch_tip(&fork.dir, &branch) else {
        eprintln!("pfs: fork {} has no branch {branch}, so nothing is saved", fork.id);
        return Ok(None);
    };
    if fork.base() == Some(tip.as_str()) {
        return Ok(None);
    }
    if let Some(current) = git::current_branch(&fork.dir).filter(|current| current != &branch) {
        eprintln!("pfs: fork {} is on {current}; only {branch} is saved", fork.id);
    }
    git::fetch_branch(&repo.root, &fork.dir, &branch)
        .with_context(|| format!("couldn't save {branch} to the main repo, so fork {} was kept", fork.id))?;
    Ok(Some(branch))
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use super::*;
    use crate::{
        harness::Turn,
        ledger::Entry,
        runs::{Run, Start},
    };

    fn fork() -> Fork {
        Fork {
            id: 5,
            dir: PathBuf::from("/w/repo/.forks/5"),
            entry: Some(Entry {
                fork: ledger::Fork {
                    id: 5,
                    name: Some("x".into()),
                    branch: "pfs/5-x".into(),
                    base: None,
                    from: None,
                    of: None,
                    parent_session: None,
                    ts: "2026-10-15T05:00:00.000Z".into(),
                },
                removed: None,
            }),
        }
    }

    fn run(run: u32, kind: Kind, pid: i32, cmd: &str) -> Occupant {
        Occupant::Run(Run {
            start: Start {
                run,
                kind,
                cmd: cmd.into(),
                pid,
                proc_start: None,
                ts: "2026-10-15T05:00:00.000Z".into(),
            },
            exit: None,
        })
    }

    fn process(pid: i32, command: &str) -> Occupant {
        Occupant::Process(Process {
            pid,
            command: command.into(),
            cwd: PathBuf::from("/w/repo/.forks/5/src"),
        })
    }

    fn conversation(id: &str, turn: Turn, age: u64, now: SystemTime) -> Conversation {
        Conversation {
            harness: "pi",
            id: id.into(),
            last_write: now - Duration::from_secs(age),
            turn,
        }
    }

    fn nothing() -> Found {
        Found {
            present: Vec::new(),
            conversations: Vec::new(),
            uncommitted: false,
        }
    }

    const ELSEWHERE: &[i32] = &[9000, 8000, 7000];

    #[test]
    fn each_kind_gets_its_own_hint() {
        assert_eq!(hint(5, &run(1, Kind::Bg, 4242, "pi -p x"), ELSEWHERE), Hint::Kill(4242));
        assert_eq!(
            hint(5, &run(2, Kind::Open, 4300, "/bin/zsh (open)"), ELSEWHERE),
            Hint::ExitDoor
        );
        assert_eq!(hint(5, &run(3, Kind::Fg, 4400, "bun dev"), ELSEWHERE), Hint::Quit);
        assert_eq!(hint(5, &process(4500, "sleep"), ELSEWHERE), Hint::Leftover(5));
        assert_eq!(say(Hint::Kill(4242)), "stop: kill -- -4242");
        assert_eq!(say(Hint::ExitDoor), "exit that shell");
        assert_eq!(say(Hint::Quit), "finish or quit it");
        assert_eq!(say(Hint::Leftover(5)), "stop: pfs kill 5");
    }

    #[test]
    fn the_door_youre_in_is_found_in_rms_own_lineage() {
        let door = run(1, Kind::Open, 52014, "/bin/zsh (open)");
        let own = [61000, 60990, 52014, 52010, 400];
        assert_eq!(hint(5, &door, &own), Hint::DoorYoureIn);
        assert_eq!(
            say(hint(5, &door, &own)),
            "that's the door you're in: exit it, then rm from a normal shell"
        );
        assert_eq!(hint(5, &door, &[61000, 60990, 400]), Hint::ExitDoor);
    }

    #[test]
    fn nothing_rm_runs_inside_gets_a_kill() {
        let own = [61000, 4242, 4400];
        assert_eq!(hint(5, &run(1, Kind::Bg, 4242, "pi -p x"), &own), Hint::YoureIn);
        assert_eq!(hint(5, &process(4400, "pi"), &own), Hint::YoureIn);
    }

    #[test]
    fn long_commands_are_cut_to_one_short_line() {
        assert_eq!(clip("bun dev"), "bun dev");
        assert_eq!(clip("pi -p fix the csv\nthen the tests"), "pi -p fix the csv\u{2026}");
        let long = format!("pi -p {}", "\u{e9}".repeat(100));
        let cut = clip(&long);
        assert_eq!(cut.chars().count(), CMD_WIDTH + 1);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(clip(&"x".repeat(CMD_WIDTH)), "x".repeat(CMD_WIDTH));
        assert_eq!(clip(""), "");
    }

    #[test]
    fn nothing_found_is_no_refusal() {
        assert_eq!(refusal(&fork(), &nothing(), ELSEWHERE, SystemTime::now()), None);
    }

    #[test]
    fn someone_there_lists_each_occupant_with_its_hint() {
        let found = Found {
            present: vec![
                run(1, Kind::Bg, 4242, "pi -p fix the csv\nand more"),
                run(2, Kind::Open, 4300, "/bin/zsh (open)"),
                process(70897, "sleep"),
            ],
            ..nothing()
        };
        assert_eq!(
            refusal(&fork(), &found, ELSEWHERE, SystemTime::now()).unwrap(),
            "fork 5-x has someone working in it:\n\
             \x20 5.1 bg pid 4242  pi -p fix the csv\u{2026}  (stop: kill -- -4242)\n\
             \x20 5.2 open pid 4300  /bin/zsh (open)  (exit that shell)\n\
             \x20 pid 70897  sleep  (stop: pfs kill 5)\n\
             rm never stops anyone, and --force doesn't skip this: rm again once they're done"
        );
    }

    #[test]
    fn one_open_conversation_is_one_line() {
        let now = SystemTime::now();
        let found = Found {
            conversations: vec![conversation("01a13e1f-a572", Turn::RunningTools, 40, now)],
            ..nothing()
        };
        assert_eq!(
            refusal(&fork(), &found, ELSEWHERE, now).unwrap(),
            "fork 5-x: pi conversation 01a13e1f is mid-turn (running tools, last write 40s ago). \
             wait for it, or if it crashed: pfs rm 5 --force"
        );
    }

    #[test]
    fn everything_that_applies_is_said_at_once() {
        let now = SystemTime::now();
        let found = Found {
            present: vec![process(70897, "sleep")],
            conversations: vec![
                conversation("01a1ffff", Turn::Thinking, 5, now),
                conversation("01a13e1f", Turn::RunningTools, 120, now),
            ],
            uncommitted: true,
        };
        assert_eq!(
            refusal(&fork(), &found, ELSEWHERE, now).unwrap(),
            "fork 5-x has someone working in it:\n\
             \x20 pid 70897  sleep  (stop: pfs kill 5)\n\
             rm never stops anyone, and --force doesn't skip this: rm again once they're done\n\
             fork 5-x has conversations mid-turn:\n\
             \x20 pi conversation 01a1ffff is mid-turn (thinking, last write 5s ago)\n\
             \x20 pi conversation 01a13e1f is mid-turn (running tools, last write 2m ago)\n\
             wait for them, or if they crashed: pfs rm 5 --force\n\
             fork 5-x has uncommitted changes. commit them on pfs/5-x (see them with pfs diff 5), \
             or pass --force to drop them"
        );
    }

    #[test]
    fn force_skips_open_turns_and_uncommitted_work_only() {
        assert_eq!(
            skipped(true),
            Skipped {
                open_turns: true,
                uncommitted: true
            }
        );
        assert_eq!(
            skipped(false),
            Skipped {
                open_turns: false,
                uncommitted: false
            }
        );
        let forced = Found {
            present: vec![process(70897, "sleep")],
            ..nothing()
        };
        let refused = refusal(&fork(), &forced, ELSEWHERE, SystemTime::now()).unwrap();
        assert!(refused.starts_with("fork 5-x has someone working in it:"));
        assert!(!refused.contains("pfs rm 5 --force"));
    }

    #[test]
    fn repeats_count_once_in_first_order() {
        assert_eq!(unique(vec![7, 3, 7, 5, 3]), vec![7, 3, 5]);
        assert_eq!(unique(Vec::new()), Vec::<u32>::new());
    }

    #[test]
    fn a_refusal_keeps_only_itself() {
        let mut tried = Vec::new();
        let kept = try_each(&[3, 5, 7], |id| {
            tried.push(id);
            match id {
                5 => bail!("fork 5 has uncommitted changes"),
                _ => Ok(()),
            }
        });
        assert_eq!(tried, vec![3, 5, 7]);
        assert_eq!(kept, vec![5]);
    }

    #[test]
    fn every_fork_is_tried_even_when_all_are_refused() {
        let mut tried = Vec::new();
        let kept = try_each(&[2, 9], |id| {
            tried.push(id);
            bail!("no fork {id}")
        });
        assert_eq!(tried, vec![2, 9]);
        assert_eq!(kept, vec![2, 9]);
    }

    #[test]
    fn kept_note_only_after_several() {
        assert_eq!(kept_note(1, &[3]), None);
        assert_eq!(kept_note(3, &[]), None);
        assert_eq!(
            kept_note(3, &[5, 7]),
            Some("pfs: 2 of 3 not removed (5, 7), see above for why".to_string())
        );
    }

    #[test]
    fn the_note_names_main_s_uncommitted_files_in_the_way() {
        let files = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(merge_words("main", &[], &[]), "vs main right now: merges clean");
        assert_eq!(
            merge_words("main", &[], &files(&[".marbles/marbles.csv"])),
            "vs main right now: merges clean, once .marbles/marbles.csv (uncommitted here) is committed or stashed"
        );
        assert_eq!(
            merge_words("main", &files(&["README.md"]), &files(&["a.rs", "b.rs"])),
            "vs main right now: 1 file would conflict (README.md) · resolve when merging, \
             once 2 files uncommitted here (a.rs, b.rs) are committed or stashed"
        );
    }
}
