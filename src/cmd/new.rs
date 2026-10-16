use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};

use super::open;
use crate::{
    cli::NewArgs,
    clock, clone,
    config::{self, Global},
    forks::{self, START_REF},
    git, harness,
    ledger::{self, Event, Ledger},
    names, paths,
    place::{self, FORKS_DIR, Place, Repo, parse_fork_ref},
    ports, runs, store, venv,
};

/// What populate made: the venvs it re-pointed, the base commit, and main's
/// uncommitted files the fork starts with.
struct Made {
    venvs: Vec<PathBuf>,
    base: Option<String>,
    carried: Vec<String>,
}

/// What a new fork starts as.
enum Source {
    /// The main folder as it is, uncommitted work included.
    Main { base: Option<String>, dirty: bool },
    /// A clean checkout of a commit the main repo has.
    Rev { base: String },
    /// A clean checkout of a live fork's branch tip (`--from pfs/5-x`).
    ForkBranch { dir: PathBuf, branch: String },
    /// Another fork's folder as it is, uncommitted work included (`--of 5`).
    Fork { id: u32, dir: PathBuf },
}

impl Source {
    fn dir<'a>(&'a self, repo: &'a Repo) -> &'a Path {
        match self {
            Source::Fork { dir, .. } => dir,
            _ => &repo.root,
        }
    }
}

pub fn run(args: NewArgs) -> Result<i32> {
    let place = place::here()?;
    if let Some(id) = place.fork {
        bail!(
            "forks are flat: you're in fork {id}. create forks from the main folder ({})",
            place.repo.root.display()
        );
    }
    let repo = place.repo.clone();
    require_plain_repo(&repo)?;
    if let Some(raw) = args.name.as_deref() {
        refuse_bare_number(raw)?;
    }
    let config = config::load_global()?;
    let name = args.name.as_deref().and_then(names::clean);
    let ledger = ledger::load(&repo)?;
    let source = source(&repo, &ledger, &args)?;

    prepare(&repo)?;
    let id = claim(&repo)?;
    let branch = names::branch(id, name.as_deref());
    let fork_dir = repo.fork_dir(id);
    let Made { venvs, base, carried } = match populate(&repo, &fork_dir, &branch, &source) {
        Ok(made) => made,
        Err(error) => {
            let _ = paths::remove_tree(&fork_dir);
            return Err(error.context(format!("couldn't create fork {id}")));
        }
    };

    let fork = ledger::Fork {
        id,
        name: name.clone(),
        branch: branch.clone(),
        base,
        from: args.from.clone(),
        of: match &source {
            Source::Fork { id, .. } => Some(*id),
            _ => None,
        },
        parent_session: harness::parent_session(),
        ts: clock::now(),
    };
    record(&repo, fork)?;
    let created = created_line(&place, &config, id, name.as_deref(), &branch);
    let notes: Vec<String> = name_note(id, args.name.as_deref(), name.as_deref())
        .into_iter()
        .chain(source_notes(&ledger, &source))
        .chain(venv_note(&venvs, &fork_dir))
        .chain(carried_note(&branch, &carried))
        .collect();
    let (out, err) = split_output(args.quiet, id, created, notes);
    out.iter().for_each(|line| println!("{line}"));
    err.iter().for_each(|line| eprintln!("{line}"));

    match args.open {
        true => open::launch(&place, id),
        false => Ok(0),
    }
}

fn require_plain_repo(repo: &Repo) -> Result<()> {
    match repo.root.join(".git").is_dir() {
        true => Ok(()),
        false => bail!(
            "{} is a git worktree or submodule (.git is a file). pfs needs a plain repo",
            repo.root.display()
        ),
    }
}

/// `pfs new 5` reads like "a fork of 5" or a slip for `pfs open 5`, but it
/// would make a fork labelled "5". Numbers are pfs's, so a bare one is an error.
fn refuse_bare_number(raw: &str) -> Result<()> {
    let trimmed = raw.trim();
    match !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        true => bail!(
            "\"{trimmed}\" is only a number, and pfs picks the numbers. a copy of fork {trimmed}: `pfs new --of {trimmed} [name]`. into it: `pfs open {trimmed}`"
        ),
        false => Ok(()),
    }
}

fn source(repo: &Repo, ledger: &Ledger, args: &NewArgs) -> Result<Source> {
    let main = repo.root.as_path();
    match (args.from.as_deref(), args.of.as_deref()) {
        (Some(rev), _) => match git::commit(main, rev) {
            Ok(base) => Ok(Source::Rev { base }),
            Err(error) => {
                live_fork_branch(repo, ledger, rev).ok_or_else(|| removed_fork_branch(ledger, rev).unwrap_or(error))
            }
        },
        (None, Some(raw)) => {
            let id = parse_fork_ref(raw).ok_or_else(|| anyhow!("not a fork number: {raw}"))?;
            let fork =
                forks::require(repo, ledger, id).map_err(|error| with_saved_branch_hint(repo, ledger, id, error))?;
            Ok(Source::Fork { id, dir: fork.dir })
        }
        (None, None) => Ok(Source::Main {
            base: git::head(main),
            dirty: git::is_dirty(main)?,
        }),
    }
}

/// `pfs/5-x` while fork 5 is still open: the branch lives in fork 5 only.
fn live_fork_branch(repo: &Repo, ledger: &Ledger, rev: &str) -> Option<Source> {
    let id = names::id_from_branch(rev)?;
    let fork = forks::require(repo, ledger, id).ok()?;
    (fork.branch() == rev).then(|| Source::ForkBranch {
        dir: fork.dir,
        branch: rev.to_string(),
    })
}

/// `--of 3` after fork 3 was removed: its folder is gone, its branch may not be.
fn with_saved_branch_hint(repo: &Repo, ledger: &Ledger, id: u32, error: anyhow::Error) -> anyhow::Error {
    let saved = ledger
        .get(&id)
        .and_then(|entry| entry.removed.as_ref())
        .and_then(|rm| rm.saved_branch.clone())
        .filter(|branch| git::branch_tip(&repo.root, branch).is_some());
    match saved {
        Some(branch) => anyhow!("{error:#}. to start from its work: pfs new --from {branch}"),
        None => error,
    }
}

/// `pfs/3-x` after fork 3 was removed and its saved branch deleted.
fn removed_fork_branch(ledger: &Ledger, rev: &str) -> Option<anyhow::Error> {
    let id = names::id_from_branch(rev)?;
    let entry = ledger.get(&id).filter(|entry| entry.fork.branch == rev)?;
    let rm = entry.removed.as_ref()?;
    Some(anyhow!(
        "not a commit: {rev}. fork {id} was removed {} ago and that branch is no longer in the main repo (deleted after merging?)",
        clock::ago(&rm.ts)
    ))
}

fn prepare(repo: &Repo) -> Result<()> {
    let forks = repo.forks();
    if forks.exists() && !forks.is_dir() {
        bail!("{} exists and isn't a folder", forks.display());
    }
    fs::create_dir_all(&forks).with_context(|| format!("can't create {}", forks.display()))?;
    git::exclude(&repo.root, &format!("/{FORKS_DIR}/"))?;
    forks::sweep_trash(repo);
    Ok(())
}

/// Picks the next number and claims it by creating the folder, under the
/// ledger lock so two `pfs new` at once never share a number. Numbers are
/// never reused: removed forks stay in the ledger and their saved branches
/// stay in git.
fn claim(repo: &Repo) -> Result<u32> {
    let _lock = store::lock(&repo.lock())?;
    let recorded = ledger::load(repo)?.keys().max().copied().unwrap_or(0);
    let on_disk = ledger::ids_on_disk(repo).into_iter().max().unwrap_or(0);
    let saved = git::branches(&repo.root, "pfs/")
        .iter()
        .filter_map(|branch| names::id_from_branch(branch))
        .max()
        .unwrap_or(0);
    let id = recorded.max(on_disk).max(saved) + 1;
    let dir = repo.fork_dir(id);
    fs::create_dir(&dir).with_context(|| format!("can't create {}", dir.display()))?;
    Ok(id)
}

/// Clone, then make the clone a fork: its own branch, its own venvs, and a
/// record of where it started. Every git command here runs in the new fork.
fn populate(repo: &Repo, fork_dir: &Path, branch: &str, source: &Source) -> Result<Made> {
    let from = source.dir(repo);
    clone::tree(from, fork_dir, &[FORKS_DIR])?;
    reset_pfs_state(fork_dir)?;
    let venvs = venv::relocate(from, fork_dir)?;
    let base = start_branch(fork_dir, branch, source)?;
    let scratch = runs::dir(fork_dir).join("start.index");
    let start = match source {
        Source::Main { dirty: true, .. } | Source::Fork { .. } => Some(git::worktree_tree(fork_dir, &scratch)?),
        _ => base.as_deref().and_then(|base| git::tree_of(fork_dir, base)),
    };
    start
        .as_deref()
        .map(|tree| git::set_ref(fork_dir, START_REF, tree))
        .transpose()?;
    let carried = match source {
        Source::Main { dirty: true, .. } => carried(fork_dir, base.as_deref(), start.as_deref()),
        _ => Vec::new(),
    };
    Ok(Made { venvs, base, carried })
}

/// Main's uncommitted files in the fork: its start against the commit it's on.
fn carried(fork_dir: &Path, base: Option<&str>, start: Option<&str>) -> Vec<String> {
    let committed = base.and_then(|base| git::tree_of(fork_dir, base));
    start
        .and_then(|start| git::changed_paths(fork_dir, committed.as_deref(), start).ok())
        .unwrap_or_default()
}

fn start_branch(fork_dir: &Path, branch: &str, source: &Source) -> Result<Option<String>> {
    match source {
        Source::Main { base, .. } => {
            git::switch_new_branch(fork_dir, branch)?;
            Ok(base.clone())
        }
        Source::Fork { .. } => {
            let base = git::head(fork_dir);
            git::switch_new_branch(fork_dir, branch)?;
            Ok(base)
        }
        Source::Rev { base } => {
            git::switch_clean(fork_dir, branch, base)?;
            Ok(Some(base.clone()))
        }
        Source::ForkBranch { dir, branch: theirs } => {
            let tip = git::fetch_tip(fork_dir, dir, theirs)?;
            git::switch_clean(fork_dir, branch, &tip)?;
            Ok(Some(tip))
        }
    }
}

/// A fresh run journal, and no lock left over from a git command that was
/// running in the folder it was cloned from.
fn reset_pfs_state(fork_dir: &Path) -> Result<()> {
    paths::remove_tree(&runs::dir(fork_dir))?;
    let index_lock = git::git_dir(fork_dir).join("index.lock");
    if index_lock.exists() {
        fs::remove_file(&index_lock).context("can't drop the copied index.lock")?;
    }
    runs::ensure_dir(fork_dir)
}

fn record(repo: &Repo, fork: ledger::Fork) -> Result<()> {
    let _lock = store::lock(&repo.lock())?;
    ledger::append(repo, &Event::Fork(fork))
}

/// Plain: the created line, then the notes. Quiet: stdout is the number
/// alone, for `n=$(pfs new x -q)`, and the notes still reach a human on
/// stderr.
fn split_output(quiet: bool, id: u32, created: String, notes: Vec<String>) -> (Vec<String>, Vec<String>) {
    match quiet {
        true => (vec![id.to_string()], notes),
        false => (std::iter::once(created).chain(notes).collect(), Vec::new()),
    }
}

fn created_line(place: &Place, config: &Global, id: u32, name: Option<&str>, branch: &str) -> String {
    let label = names::label(id, name);
    let path = forks::show_path(&place.repo.fork_dir(id), place);
    let ports = ports::show(config.ports.block(id));
    format!("created {label} at {path} (branch {branch}, ports {ports})")
}

fn name_note(id: u32, raw: Option<&str>, name: Option<&str>) -> Option<String> {
    match (raw, name) {
        (Some(raw), Some(name)) if raw != name => Some(format!(
            "note: pfs picks the number and tidies the label, so \"{raw}\" became \"{name}\""
        )),
        (Some(raw), None) => Some(format!("note: \"{raw}\" left no usable label, so it's just {id}")),
        _ => None,
    }
}

fn source_notes(ledger: &Ledger, source: &Source) -> Vec<String> {
    let label = |id: u32| {
        ledger
            .get(&id)
            .map(|entry| entry.label())
            .unwrap_or_else(|| id.to_string())
    };
    match source {
        Source::Fork { id, dir } => {
            let copy = format!("a copy of fork {} as it is now, uncommitted work included", label(*id));
            let busy = (!forks::running(dir).is_empty()).then(|| {
                format!(
                    "note: fork {} still has things running, so this is its folder mid-work",
                    label(*id)
                )
            });
            std::iter::once(copy).chain(busy).collect()
        }
        Source::ForkBranch { branch, .. } => vec![format!(
            "a clean start at {branch}'s latest commit, taken from its fork (it isn't in the main repo yet)"
        )],
        Source::Main { .. } | Source::Rev { .. } => Vec::new(),
    }
}

/// Main's wip rides along by design, but a `git add -A` in the fork commits
/// it on the fork's branch: say so while it's cheap to act on.
fn carried_note(branch: &str, carried: &[String]) -> Option<String> {
    match carried {
        [] => None,
        [file] => Some(format!(
            "note: main's uncommitted {file} came along: a `git add -A` in the fork commits it on {branch}"
        )),
        files => Some(format!(
            "note: main's uncommitted work came along ({} files: {}): a `git add -A` in the fork commits it on {branch}",
            files.len(),
            paths::shown(files, 3)
        )),
    }
}

fn venv_note(venvs: &[PathBuf], fork_dir: &Path) -> Option<String> {
    let shown: Vec<String> = venvs.iter().map(|venv| paths::show(venv, fork_dir)).collect();
    (!shown.is_empty()).then(|| {
        format!(
            "note: pointed {} at the fork (venvs bake in absolute paths)",
            shown.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_numbers_are_refused() {
        assert!(refuse_bare_number("5").is_err());
        assert!(refuse_bare_number(" 12 ").is_err());
        assert!(refuse_bare_number("5-fix").is_ok());
        assert!(refuse_bare_number("v2").is_ok());
    }

    #[test]
    fn quiet_leaves_stdout_to_the_number() {
        let notes = || vec!["note: a".to_string(), "note: b".to_string()];
        assert_eq!(
            split_output(true, 12, "created 12-x".into(), notes()),
            (vec!["12".to_string()], notes())
        );
        let (out, err) = split_output(false, 12, "created 12-x".into(), notes());
        assert_eq!(out, ["created 12-x", "note: a", "note: b"]);
        assert!(err.is_empty());
        assert_eq!(split_output(true, 3, "created 3".into(), Vec::new()).0, ["3"]);
    }

    #[test]
    fn the_label_note_says_what_happened_to_the_name() {
        assert_eq!(
            name_note(8, Some("7-add-csv"), Some("add-csv")).unwrap(),
            "note: pfs picks the number and tidies the label, so \"7-add-csv\" became \"add-csv\""
        );
        assert_eq!(
            name_note(8, Some("!!"), None).unwrap(),
            "note: \"!!\" left no usable label, so it's just 8"
        );
        assert_eq!(name_note(8, Some("add-csv"), Some("add-csv")), None);
        assert_eq!(name_note(8, None, None), None);
    }

    #[test]
    fn main_s_carried_wip_gets_a_note() {
        let files = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(carried_note("pfs/3-x", &[]), None);
        assert_eq!(
            carried_note("pfs/3-x", &files(&["README.md"])).unwrap(),
            "note: main's uncommitted README.md came along: a `git add -A` in the fork commits it on pfs/3-x"
        );
        assert_eq!(
            carried_note("pfs/3-x", &files(&["README.md", "a", "b", "c"])).unwrap(),
            "note: main's uncommitted work came along (4 files: README.md, a, b +1 more): \
             a `git add -A` in the fork commits it on pfs/3-x"
        );
    }
}
