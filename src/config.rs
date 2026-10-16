//! `~/.config/pfs/config.toml`, optional: your machine's `[ports]`. Tables
//! pfs no longer reads are ignored, never an error.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item};

use crate::{paths, ports::Plan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub ports: Plan,
}

pub fn global_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => paths::home()?.join(".config"),
    };
    Ok(base.join("pfs").join("config.toml"))
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

pub fn load_global() -> Result<Global> {
    load(&global_path()?)
}

fn load(path: &Path) -> Result<Global> {
    let ports = ports(&parse(path)?.unwrap_or_default());
    match ports.is_valid() {
        true => Ok(Global { ports }),
        false => bail!("[ports] in {} doesn't fit below port 65536", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfs-config-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn old_tables_are_ignored_and_ports_stay() {
        let path = scratch(
            "old",
            "# mine\n[wall]\nallow = [\"~/.m2\", \"/opt/cache\"]\nlock_git_settings = false\n\n[ports]\nbase = 41000\nsize = 5\n",
        );
        let loaded = load(&path);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
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
        let missing = std::env::temp_dir().join(format!("pfs-config-none-{}/config.toml", std::process::id()));
        assert_eq!(load(&missing).unwrap().ports, Plan::default());
        let path = scratch("bad", "[ports]\nbase = 65000\n");
        let loaded = load(&path);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert!(loaded.unwrap_err().to_string().starts_with("[ports] in "));
    }
}
