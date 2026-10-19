//! pfs's home, `$PFS_HOME` (default `~/.pfs`), and its optional
//! `config.toml`: your machine's `[ports]`, and `[forks] dir`, where new
//! mains get their forks folder. Tables pfs no longer reads are ignored,
//! never an error.

use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{DocumentMut, Item};

use crate::{paths, ports::Plan};

const CONFIG: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub ports: Plan,
    /// Where a main's first `pfs new` puts its forks folder: `[forks] dir`,
    /// else `$PFS_HOME/forks`. A main that has one keeps it.
    pub forks_dir: PathBuf,
    /// The file this came from (it may not exist), for messages that send
    /// you there.
    pub path: PathBuf,
}

/// `$PFS_HOME`, or `~/.pfs` when it's unset or empty.
pub fn pfs_home() -> Result<PathBuf> {
    home_from(std::env::var_os("PFS_HOME"), paths::home)
}

fn home_from(raw: Option<OsString>, home: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    match raw.filter(|value| !value.is_empty()).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Ok(dir),
        Some(dir) => bail!("PFS_HOME is {}: make it an absolute path", dir.display()),
        None => Ok(home()?.join(".pfs")),
    }
}

fn parse(path: &Path) -> Result<Option<DocumentMut>> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map(Some)
            .with_context(|| format!("bad toml in {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("can't read {}", path.display())),
    }
}

fn item<'a>(doc: &'a DocumentMut, table: &str, key: &str) -> Option<&'a Item> {
    doc.get(table).and_then(|section| section.get(key))
}

fn number(doc: &DocumentMut, table: &str, key: &str) -> Option<u32> {
    item(doc, table, key)
        .and_then(Item::as_integer)
        .and_then(|value| u32::try_from(value).ok())
}

/// `[ports]`, with the defaults for whatever it leaves out.
fn ports(doc: &DocumentMut) -> Plan {
    let defaults = Plan::default();
    Plan {
        base: number(doc, "ports", "base").unwrap_or(defaults.base),
        size: number(doc, "ports", "size").unwrap_or(defaults.size),
        count: number(doc, "ports", "count").unwrap_or(defaults.count),
    }
}

/// `/abs/path` as is, `~/x` under home; anything else isn't a place.
fn expand(raw: &str, home: impl FnOnce() -> Result<PathBuf>) -> Option<Result<PathBuf>> {
    match raw.strip_prefix("~/") {
        Some(rest) => Some(home().map(|home| home.join(rest))),
        None if Path::new(raw).is_absolute() => Some(Ok(PathBuf::from(raw))),
        None => None,
    }
}

/// `[forks] dir`, when it's set.
fn forks_dir(doc: &DocumentMut, path: &Path, home: impl FnOnce() -> Result<PathBuf>) -> Result<Option<PathBuf>> {
    let Some(item) = item(doc, "forks", "dir") else {
        return Ok(None);
    };
    let raw = item
        .as_str()
        .ok_or_else(|| anyhow!("[forks] dir in {} should be a path in quotes", path.display()))?;
    expand(raw, home)
        .ok_or_else(|| {
            anyhow!(
                "[forks] dir in {} is \"{raw}\": make it an absolute path or one starting with ~/",
                path.display()
            )
        })?
        .map(Some)
}

pub fn load_global() -> Result<Global> {
    load(&pfs_home()?, paths::home)
}

fn load(pfs_home: &Path, home: impl FnOnce() -> Result<PathBuf>) -> Result<Global> {
    let path = pfs_home.join(CONFIG);
    let doc = parse(&path)?.unwrap_or_default();
    let ports = ports(&doc);
    if !ports.is_valid() {
        bail!("[ports] in {} doesn't fit below port 65536", path.display());
    }
    let forks_dir = forks_dir(&doc, &path, home)?.unwrap_or_else(|| pfs_home.join("forks"));
    Ok(Global { ports, forks_dir, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfs-config-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG), text).unwrap();
        dir
    }

    fn home() -> Result<PathBuf> {
        Ok(PathBuf::from("/Users/me"))
    }

    fn no_home() -> Result<PathBuf> {
        bail!("HOME is not set")
    }

    fn loaded(name: &str, text: &str) -> Result<Global> {
        let dir = scratch(name, text);
        let loaded = load(&dir, home);
        fs::remove_dir_all(&dir).unwrap();
        loaded
    }

    #[test]
    fn old_tables_are_ignored_and_ports_stay() {
        let loaded = loaded(
            "old",
            "# mine\n[wall]\nallow = [\"~/.m2\", \"/opt/cache\"]\nlock_git_settings = false\n\n[ports]\nbase = 41000\nsize = 5\n",
        );
        assert_eq!(
            loaded.unwrap().ports,
            Plan {
                base: 41000,
                size: 5,
                count: Plan::default().count,
            }
        );
    }

    #[test]
    fn no_file_is_the_defaults_and_bad_ports_are_an_error() {
        let missing = std::env::temp_dir().join(format!("pfs-config-none-{}", std::process::id()));
        let global = load(&missing, no_home).unwrap();
        assert_eq!(global.ports, Plan::default());
        assert_eq!(global.forks_dir, missing.join("forks"));
        assert_eq!(global.path, missing.join(CONFIG));
        let error = loaded("bad", "[ports]\nbase = 65000\n").unwrap_err().to_string();
        assert!(error.starts_with("[ports] in "), "{error}");
    }

    #[test]
    fn pfs_home_is_the_env_var_or_dot_pfs() {
        assert_eq!(home_from(None, home).unwrap(), PathBuf::from("/Users/me/.pfs"));
        assert_eq!(
            home_from(Some("".into()), home).unwrap(),
            PathBuf::from("/Users/me/.pfs")
        );
        assert_eq!(
            home_from(Some("/x/pfs".into()), no_home).unwrap(),
            PathBuf::from("/x/pfs")
        );
        assert!(home_from(Some("rel/pfs".into()), home).is_err());
        assert!(home_from(None, no_home).is_err());
    }

    #[test]
    fn forks_dir_is_absolute_or_under_home() {
        let dir = |text: &str| loaded("dir", text).map(|global| global.forks_dir);
        assert_eq!(
            dir("[forks]\ndir = \"/Volumes/fast/forks\"\n").unwrap(),
            PathBuf::from("/Volumes/fast/forks")
        );
        assert_eq!(
            dir("[forks]\ndir = \"~/work/forks\"\n").unwrap(),
            PathBuf::from("/Users/me/work/forks")
        );
        let relative = dir("[forks]\ndir = \"forks\"\n").unwrap_err().to_string();
        assert!(relative.starts_with("[forks] dir in "), "{relative}");
        assert!(relative.contains(CONFIG), "{relative}");
        assert!(dir("[forks]\ndir = \"~forks\"\n").is_err());
        assert!(dir("[forks]\ndir = 3\n").unwrap_err().to_string().contains(CONFIG));
    }

    #[test]
    fn a_forks_table_without_dir_is_the_default() {
        let dir = scratch("nodir", "[forks]\n");
        let global = load(&dir, no_home).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(global.forks_dir, dir.join("forks"));
    }
}
