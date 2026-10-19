//! One fork, as the commands see it: its folder and its ledger entry.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::{
    clock,
    config::Global,
    git,
    ledger::{Entry, Ledger},
    names, paths,
    place::{Place, Repo, parse_dir_id, parse_fork_ref},
    runs::{self, Kind, Run, Status},
    spawn::Job,
};

/// Where a fork's starting state is kept, as a tree, inside the fork's git:
/// the base commit's tree, or the dirty folder it was cloned from.
pub const START_REF: &str = "refs/pfs/start";

#[derive(Debug, Clone)]
pub struct Fork {
    pub id: u32,
    pub dir: PathBuf,
    pub entry: Option<Entry>,
}

impl Fork {
    pub fn name(&self) -> Option<&str> {
        self.entry.as_ref().and_then(|entry| entry.fork.name.as_deref())
    }

    pub fn label(&self) -> String {
        names::label(self.id, self.name())
    }

    pub fn branch(&self) -> String {
        self.entry
            .as_ref()
            .map(|entry| entry.fork.branch.clone())
            .unwrap_or_else(|| names::branch(self.id, None))
    }

    pub fn base(&self) -> Option<&str> {
        self.entry.as_ref().and_then(|entry| entry.fork.base.as_deref())
    }

    /// Where `pfs rm` moves the folder before deleting it: next to it, in
    /// the same forks folder.
    pub fn trash(&self) -> PathBuf {
        self.dir.with_file_name(format!(".rm-{}", self.id))
    }
}

pub fn require(repo: &Repo, ledger: &Ledger, id: u32) -> Result<Fork> {
    if let Some(dir) = repo.fork_dir(id).filter(|dir| dir.is_dir()) {
        return Ok(Fork {
            id,
            dir,
            entry: ledger.get(&id).cloned(),
        });
    }
    match ledger.get(&id).and_then(|entry| entry.removed.as_ref()) {
        Some(rm) => {
            let saved = rm
                .saved_branch
                .as_ref()
                .map(|branch| format!(". its branch lives on in the main repo as {branch}"))
                .unwrap_or_default();
            bail!("fork {id} was removed {} ago{saved}", clock::ago(&rm.ts))
        }
        None => bail!("no fork {id}. `pfs ls` lists them"),
    }
}

/// What's in the repo's forks folder; nothing before its first fork.
fn listing(repo: &Repo) -> Vec<PathBuf> {
    repo.forks
        .as_deref()
        .map(|forks| paths::entries(forks).unwrap_or_default())
        .unwrap_or_default()
}

/// Fork folders that exist right now.
pub fn active_ids(repo: &Repo) -> Vec<u32> {
    let mut ids: Vec<u32> = listing(repo)
        .iter()
        .filter(|path| path.is_dir())
        .filter_map(|path| path.file_name()?.to_str().and_then(parse_dir_id))
        .collect();
    ids.sort_unstable();
    ids
}

/// Finishes deletions an earlier `pfs rm` couldn't (best effort).
pub fn sweep_trash(repo: &Repo) {
    listing(repo)
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".rm-"))
        })
        .for_each(|path| {
            let _ = paths::remove_tree(path);
        });
}

/// `[N] rest…`: the leading fork number, if the first argument is one.
pub fn split_ref(args: &[String]) -> (Option<&str>, &[String]) {
    match args.split_first() {
        Some((first, rest)) if parse_fork_ref(first).is_some() => (Some(first.as_str()), rest),
        _ => (None, args),
    }
}

/// Things started in the fork you're in run where you are; anything else
/// starts at the fork's root.
pub fn cwd_for(place: &Place, fork: &Fork) -> PathBuf {
    match place.fork == Some(fork.id) {
        true => place.cwd.clone(),
        false => fork.dir.clone(),
    }
}

/// What to start in a fork.
pub struct Plan {
    pub argv: Vec<OsString>,
    pub label: String,
    pub kind: Kind,
}

/// A plan, placed: in the fork's folder (or where you are in it), with the
/// fork's port.
pub fn job(place: &Place, config: &Global, fork: &Fork, plan: Plan) -> Job {
    Job {
        fork_dir: fork.dir.clone(),
        cwd: cwd_for(place, fork),
        argv: plan.argv,
        label: plan.label,
        kind: plan.kind,
        port: config.ports.base_port(fork.id),
    }
}

/// Uncommitted, the one meaning `pfs rm` refuses on: some file matches
/// neither the last commit nor how the fork started. The start may carry the
/// main folder's uncommitted work from fork time (an untracked lockfile, a
/// wip edit): a file the fork never touched since isn't its work to lose,
/// even next to files it committed.
pub fn uncommitted(fork: &Fork) -> Result<bool> {
    let now = now_tree(fork, "uncommitted")?;
    uncommitted_at(fork, &now)
}

/// `uncommitted`, for a folder tree already built (`now_tree`).
pub fn uncommitted_at(fork: &Fork, now: &str) -> Result<bool> {
    let dir = fork.dir.as_path();
    let committed = git::tree_of(dir, "HEAD");
    let started = git::ref_target(dir, START_REF);
    match (committed.as_deref() == Some(now), started.as_deref()) {
        (true, _) => Ok(false),
        (false, None) => Ok(true),
        (false, Some(started)) if started == now => Ok(false),
        (false, Some(started)) => {
            let since_commit = git::changed_paths(dir, committed.as_deref(), now)?;
            let since_start = git::changed_paths(dir, Some(started), now)?;
            Ok(unsaved(&since_commit, &since_start))
        }
    }
}

/// The fork's whole folder as a tree, right now. The scratch index is named
/// for `what` and this process, so commands looking at once don't share one.
pub fn now_tree(fork: &Fork, what: &str) -> Result<String> {
    runs::ensure_dir(&fork.dir)?;
    let scratch = runs::dir(&fork.dir).join(format!("{what}-{}.index", std::process::id()));
    git::worktree_tree(&fork.dir, &scratch)
}

/// How the fork started, as a tree: what `pfs diff` compares against.
/// `None` only for a fork of a repo with no commits and no files.
pub fn start_tree(fork: &Fork) -> Option<String> {
    git::ref_target(&fork.dir, START_REF).or_else(|| fork.base().and_then(|base| git::tree_of(&fork.dir, base)))
}

/// Files the fork changed since it started, committed or not: what
/// `pfs diff --name-only` lists, except a rename counts as both its paths.
pub fn changed(fork: &Fork, now: &str) -> Result<Vec<String>> {
    git::changed_paths(&fork.dir, start_tree(fork).as_deref(), now)
}

/// Files changed both since the last commit and since the start.
fn unsaved(since_commit: &[String], since_start: &[String]) -> bool {
    since_commit.iter().any(|path| since_start.contains(path))
}

/// Commits on the fork's branch since its base: what `pfs rm` would land.
pub fn commits(fork: &Fork) -> Result<u32> {
    let revs = since_base(&fork.branch(), fork.base());
    git::count_commits(&fork.dir, &revs.iter().map(String::as_str).collect::<Vec<_>>())
}

/// A fork of an empty repo has no base, and its branch has no commits until
/// the first one, so there a missing branch counts as none.
fn since_base(branch: &str, base: Option<&str>) -> Vec<String> {
    let tip = format!("refs/heads/{branch}");
    match base {
        Some(base) => vec![tip, format!("^{base}")],
        None => vec!["--ignore-missing".to_string(), tip],
    }
}

/// A fork's runs that are still going, from its run records.
pub fn running(fork_dir: &Path) -> Vec<Run> {
    runs::load(fork_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|run| run.status() == Status::Running)
        .collect()
}

pub fn show_path(path: &Path, place: &Place) -> String {
    paths::show(path, &place.cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trees(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_file_is_saved_by_the_last_commit_or_by_the_start() {
        let paths = |names: &[&str]| trees(names);
        assert!(!unsaved(&paths(&["bun.lock"]), &paths(&["src/a.rs"])));
        assert!(unsaved(&paths(&["README.md"]), &paths(&["README.md"])));
        assert!(unsaved(&paths(&["bun.lock", "x"]), &paths(&["src/a.rs", "x"])));
        assert!(!unsaved(&[], &paths(&["src/a.rs"])));
    }

    #[test]
    fn a_job_runs_where_you_are_in_your_fork_and_at_the_root_of_any_other() {
        let repo = Repo {
            root: PathBuf::from("/r"),
            forks: Some(PathBuf::from("/f/r")),
        };
        let place = Place {
            fork: Some(3),
            cwd: PathBuf::from("/f/r/3/src"),
            repo: repo.clone(),
        };
        let config = Global {
            ports: crate::ports::Plan::default(),
            forks_dir: PathBuf::from("/f"),
            path: PathBuf::from("/p/config.toml"),
        };
        let job = |id: u32| {
            let fork = Fork {
                id,
                dir: repo.fork_dir(id).unwrap(),
                entry: None,
            };
            let plan = Plan {
                argv: vec!["true".into()],
                label: "true".into(),
                kind: Kind::Fg,
            };
            job(&place, &config, &fork, plan)
        };
        let own = job(3);
        assert_eq!((own.cwd, own.port), (PathBuf::from("/f/r/3/src"), 40030));
        let other = job(5);
        assert_eq!(
            (other.fork_dir, other.cwd, other.port),
            (PathBuf::from("/f/r/5"), PathBuf::from("/f/r/5"), 40050)
        );
        assert_eq!(other.argv, vec![OsString::from("true")]);
    }

    #[test]
    fn the_trash_sits_next_to_the_fork() {
        let fork = Fork {
            id: 5,
            dir: PathBuf::from("/f/r/5"),
            entry: None,
        };
        assert_eq!(fork.trash(), PathBuf::from("/f/r/.rm-5"));
    }

    #[test]
    fn commits_count_from_the_base() {
        assert_eq!(
            since_base("pfs/3-x", Some("abc")),
            trees(&["refs/heads/pfs/3-x", "^abc"])
        );
        assert_eq!(
            since_base("pfs/3-x", None),
            trees(&["--ignore-missing", "refs/heads/pfs/3-x"])
        );
    }
}
