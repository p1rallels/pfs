//! Where am I? Identity comes from the path: the nearest repo root at or
//! above the current folder. When that root is `<root>/.forks/<n>`, you're
//! in fork n of `<root>`; otherwise it's a main folder. A repo nested inside
//! a fork (a test fixture, say) is a main folder of its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

pub const FORKS_DIR: &str = ".forks";

#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub fn forks(&self) -> PathBuf {
        self.root.join(FORKS_DIR)
    }

    pub fn ledger(&self) -> PathBuf {
        self.forks().join("ledger.jsonl")
    }

    pub fn lock(&self) -> PathBuf {
        self.forks().join(".lock")
    }

    pub fn fork_dir(&self, id: u32) -> PathBuf {
        self.forks().join(id.to_string())
    }

    pub fn trash_dir(&self, id: u32) -> PathBuf {
        self.forks().join(format!(".rm-{id}"))
    }
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

pub fn locate(dir: &Path) -> Result<Place> {
    let cwd = dir
        .canonicalize()
        .with_context(|| format!("can't resolve {}", dir.display()))?;
    let top = nearest_repo(&cwd).ok_or_else(|| anyhow!("not in a git repo. pfs forks the repo you're in"))?;
    let (root, fork) = match fork_of(&top) {
        Some((root, id)) => (root, Some(id)),
        None => (top, None),
    };
    Ok(Place {
        repo: Repo { root },
        fork,
        cwd,
    })
}

/// The closest folder at or above `dir` that has a `.git`.
fn nearest_repo(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `<root>/.forks/<n>` → `(root, n)`, when `<root>` is a plain repo.
fn fork_of(top: &Path) -> Option<(PathBuf, u32)> {
    let id = parse_dir_id(top.file_name()?.to_str()?)?;
    let forks = top.parent()?;
    let root = forks.parent()?;
    let is_fork = forks.file_name()? == FORKS_DIR && root.join(".git").is_dir();
    is_fork.then(|| (root.to_path_buf(), id))
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

    #[test]
    fn nearest_repo_decides_identity() {
        let base = std::env::temp_dir().join(format!("pfs-place-test-{}", std::process::id()));
        let fork = base.join("repo").join(FORKS_DIR).join("3");
        let nested = fork.join("fixtures").join("toy");
        [
            base.join("repo").join(".git"),
            fork.join(".git"),
            nested.join(".git"),
            fork.join("src"),
        ]
        .iter()
        .for_each(|dir| std::fs::create_dir_all(dir).unwrap());
        let in_fork = locate(&fork.join("src")).unwrap();
        assert_eq!(in_fork.fork, Some(3));
        assert_eq!(in_fork.repo.root, base.join("repo").canonicalize().unwrap());
        let in_nested = locate(&nested).unwrap();
        assert_eq!(in_nested.fork, None);
        assert_eq!(in_nested.repo.root, nested.canonicalize().unwrap());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dir_ids_are_canonical_numbers() {
        assert_eq!(parse_dir_id("12"), Some(12));
        assert_eq!(parse_dir_id("012"), None);
        assert_eq!(parse_dir_id(".rm-3"), None);
    }
}
