//! Where am I? Identity comes from the path: the nearest repo root at or
//! above the current folder is fork n of main M when its parent folder has a
//! `.main` naming M, M is a repo, and M's record names that parent (see
//! `layout`). Otherwise it's a main folder. A repo nested inside a fork (a
//! test fixture, say) is a main folder of its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::layout;

#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
    /// Its forks folder, `<forks dir>/<key>`, once a `pfs new` made one:
    /// resolved once, here, from the main's record.
    pub forks: Option<PathBuf>,
}

impl Repo {
    pub fn ledger(&self) -> PathBuf {
        layout::ledger_path(&self.root)
    }

    pub fn fork_dir(&self, id: u32) -> Option<PathBuf> {
        self.forks.as_deref().map(|forks| fork_in(forks, id))
    }
}

/// Fork n's folder in a forks folder.
pub fn fork_in(forks: &Path, id: u32) -> PathBuf {
    forks.join(id.to_string())
}

#[derive(Debug, Clone)]
pub struct Place {
    pub repo: Repo,
    pub fork: Option<u32>,
    pub cwd: PathBuf,
}

impl Place {
    /// The fork a command targets: the one given, else the one you're in.
    pub fn target(&self, given: Option<&str>) -> Result<u32> {
        match (given, self.fork) {
            (Some(raw), _) => parse_fork_ref(raw).ok_or_else(|| anyhow!("not a fork number: {raw}")),
            (None, Some(id)) => Ok(id),
            (None, None) => bail!("which fork? you're in the main folder, so pass its number (`pfs ls` lists them)"),
        }
    }

    pub fn require_main(&self, what: &str) -> Result<()> {
        match self.fork {
            None => Ok(()),
            Some(id) => bail!(
                "`pfs {what}` runs from the main folder ({}), you're in fork {id}",
                self.repo.root.display()
            ),
        }
    }
}

pub fn here() -> Result<Place> {
    let cwd = std::env::current_dir().context("can't read the current folder")?;
    locate(&cwd)
}

/// `here`, or `None` outside any repo.
pub fn try_here() -> Result<Option<Place>> {
    let cwd = std::env::current_dir().context("can't read the current folder")?;
    let resolved = cwd
        .canonicalize()
        .with_context(|| format!("can't resolve {}", cwd.display()))?;
    match nearest_repo(&resolved) {
        None => Ok(None),
        Some(_) => locate(&cwd).map(Some),
    }
}

pub fn locate(dir: &Path) -> Result<Place> {
    let cwd = dir
        .canonicalize()
        .with_context(|| format!("can't resolve {}", dir.display()))?;
    let top = nearest_repo(&cwd).ok_or_else(|| anyhow!("not in a git repo. pfs forks the repo you're in"))?;
    if let Some((root, id)) = layout::legacy_fork(&top) {
        bail!("{}", layout::legacy_fork_error(&top, &root, id));
    }
    let (repo, fork) = match fork_of(&top) {
        Some((root, forks, id)) => (
            Repo {
                root,
                forks: Some(forks),
            },
            Some(id),
        ),
        None => {
            layout::migrate(&top)?;
            let forks = layout::home_of(&top);
            (Repo { root: top, forks }, None)
        }
    };
    Ok(Place { repo, fork, cwd })
}

/// The closest folder at or above `dir` that has a `.git`.
fn nearest_repo(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `<forks folder>/<n>` → `(main, forks folder, n)`, when the main that
/// folder's `.main` names claims it.
fn fork_of(top: &Path) -> Option<(PathBuf, PathBuf, u32)> {
    let id = parse_dir_id(top.file_name()?.to_str()?)?;
    let forks = top.parent()?;
    let main = layout::main_named(forks)?.canonicalize().ok()?;
    layout::claims(&main, forks).then(|| (main, forks.to_path_buf(), id))
}

/// A fork folder's name: the bare number, as pfs writes it.
pub fn parse_dir_id(name: &str) -> Option<u32> {
    name.parse::<u32>().ok().filter(|id| *id > 0 && id.to_string() == name)
}

/// What people type: `5` or the label `5-csv-addition`.
pub fn parse_fork_ref(raw: &str) -> Option<u32> {
    let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
    let rest = &raw[digits.len()..];
    let label_ok = rest.is_empty() || rest.starts_with('-');
    match (digits.is_empty(), label_ok) {
        (false, true) => digits.parse::<u32>().ok().filter(|id| *id > 0),
        _ => None,
    }
}

/// `5` or `5.2` (fork 5, run 2).
pub fn parse_run_ref(raw: &str) -> Option<(u32, Option<u32>)> {
    match raw.split_once('.') {
        Some((fork, run)) => {
            let run = run.parse::<u32>().ok().filter(|run| *run > 0)?;
            Some((parse_fork_ref(fork)?, Some(run)))
        }
        None => Some((parse_fork_ref(raw)?, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_refs() {
        assert_eq!(parse_fork_ref("5"), Some(5));
        assert_eq!(parse_fork_ref("5-csv-addition"), Some(5));
        assert_eq!(parse_fork_ref("csv"), None);
        assert_eq!(parse_fork_ref("5csv"), None);
        assert_eq!(parse_fork_ref("0"), None);
    }

    #[test]
    fn run_refs() {
        assert_eq!(parse_run_ref("5"), Some((5, None)));
        assert_eq!(parse_run_ref("5.2"), Some((5, Some(2))));
        assert_eq!(parse_run_ref("5.x"), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfs-place-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn mkdirs(dirs: &[PathBuf]) {
        dirs.iter().for_each(|dir| std::fs::create_dir_all(dir).unwrap());
    }

    /// A main at `<base>/repo` with forks folder `<base>/forks/repo`, as
    /// `pfs new` leaves them.
    fn laid_out(base: &Path) -> (PathBuf, PathBuf) {
        let main = base.join("repo");
        let forks = base.join("forks").join("repo");
        mkdirs(&[main.join(".git").join("pfs"), forks.clone()]);
        std::fs::write(main.join(".git/pfs/forks-dir"), format!("{}\n", forks.display())).unwrap();
        std::fs::write(forks.join(layout::MAIN), format!("{}\n", main.display())).unwrap();
        (main, forks)
    }

    #[test]
    fn nearest_repo_decides_identity() {
        let base = scratch("identity");
        let (main, forks) = laid_out(&base);
        let fork = forks.join("3");
        let nested = fork.join("fixtures").join("toy");
        mkdirs(&[fork.join(".git"), nested.join(".git"), fork.join("src")]);
        let in_fork = locate(&fork.join("src")).unwrap();
        assert_eq!(in_fork.fork, Some(3));
        assert_eq!(in_fork.repo.root, main);
        assert_eq!(in_fork.repo.fork_dir(5), Some(forks.join("5")));
        let in_main = locate(&main).unwrap();
        assert_eq!((in_main.fork, in_main.repo.forks), (None, Some(forks.clone())));
        let in_nested = locate(&nested).unwrap();
        assert_eq!((in_nested.fork, in_nested.repo.forks), (None, None));
        assert_eq!(in_nested.repo.root, nested);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_its_main_doesnt_claim_is_no_fork() {
        let base = scratch("unclaimed");
        let (main, forks) = laid_out(&base);
        let fork = forks.join("3");
        mkdirs(&[fork.join(".git")]);
        std::fs::write(main.join(".git/pfs/forks-dir"), "/somewhere/else\n").unwrap();
        let place = locate(&fork).unwrap();
        assert_eq!((place.fork, place.repo.root), (None, fork.clone()));
        std::fs::remove_file(main.join(".git/pfs/forks-dir")).unwrap();
        assert_eq!(locate(&fork).unwrap().fork, None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_moved_main_finds_its_forks_again() {
        let base = scratch("moved");
        let (main, forks) = laid_out(&base);
        let fork = forks.join("2");
        mkdirs(&[fork.join(".git")]);
        let moved = base.join("moved");
        std::fs::rename(&main, &moved).unwrap();
        assert_eq!(locate(&fork).unwrap().fork, None);
        assert_eq!(locate(&moved).unwrap().repo.forks, Some(forks.clone()));
        let again = locate(&fork).unwrap();
        assert_eq!((again.fork, again.repo.root), (Some(2), moved));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_0_3_fork_is_refused_and_its_main_names_what_to_land() {
        let base = scratch("legacy");
        let main = base.join("repo");
        let old = main.join(layout::LEGACY).join("4");
        mkdirs(&[main.join(".git"), old.join(".git")]);
        let in_old = locate(&old).unwrap_err().to_string();
        assert!(in_old.contains("fork 4 from pfs 0.3"), "{in_old}");
        let in_main = locate(&main).unwrap_err().to_string();
        assert!(in_main.contains("forks from pfs 0.3 (4)"), "{in_main}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dir_ids_are_canonical_numbers() {
        assert_eq!(parse_dir_id("12"), Some(12));
        assert_eq!(parse_dir_id("012"), None);
        assert_eq!(parse_dir_id(".rm-3"), None);
    }
}
