//! `<main>/.git/pfs/ledger.jsonl`: every fork that ever existed and its
//! removal. Append-only, under its lock, so two `pfs new` at once never
//! share a number. Lines of a type pfs no longer writes (an old `allow`)
//! read as `Unknown` and change nothing.

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{layout, names, paths, place::Repo, place::parse_dir_id, store};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Fork(Fork),
    Rm(Rm),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The fork this one is a copy of (`--of`). Folders stay flat; lineage
    /// lives here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
    /// The conversation that ran `pfs new`, when a harness says (lineage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    pub ts: String,
}

impl Fork {
    /// `from` as typed, plus the commit it meant when it was a moving rev
    /// (`HEAD~1`, a branch).
    pub fn started_from(&self) -> Option<String> {
        let from = self.from.as_deref()?;
        Some(match self.base.as_deref() {
            Some(base) if !base.starts_with(from) => format!("{from} ({})", base.chars().take(7).collect::<String>()),
            _ => from.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rm {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_branch: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub fork: Fork,
    pub removed: Option<Rm>,
}

impl Entry {
    pub fn label(&self) -> String {
        names::label(self.fork.id, self.fork.name.as_deref())
    }
}

pub type Ledger = BTreeMap<u32, Entry>;

pub fn load(repo: &Repo) -> Result<Ledger> {
    Ok(fold(store::read(&repo.ledger())?))
}

pub fn append(repo: &Repo, event: &Event) -> Result<()> {
    store::append(&repo.ledger(), event)
}

pub fn lock(repo: &Repo) -> Result<store::Lock> {
    layout::lock(&repo.root)
}

pub fn fold(events: Vec<Event>) -> Ledger {
    events.into_iter().fold(BTreeMap::new(), |mut ledger, event| {
        match event {
            Event::Fork(fork) => {
                ledger.entry(fork.id).or_insert(Entry { fork, removed: None });
            }
            Event::Rm(rm) => {
                if let Some(entry) = ledger.get_mut(&rm.id) {
                    entry.removed = Some(rm);
                }
            }
            Event::Unknown => {}
        }
        ledger
    })
}

/// Numbered fork folders in a forks folder, including ones being deleted
/// (`.rm-<n>`).
pub fn ids_on_disk(forks: &Path) -> Vec<u32> {
    paths::entries(forks)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let name = entry.file_name()?.to_str()?;
            parse_dir_id(name.strip_prefix(".rm-").unwrap_or(name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fork(id: u32) -> Event {
        Event::Fork(Fork {
            id,
            name: Some(format!("f{id}")),
            branch: format!("pfs/{id}-f{id}"),
            base: None,
            from: None,
            of: None,
            parent_session: None,
            ts: "t".into(),
        })
    }

    #[test]
    fn folds_lifecycle() {
        let events = vec![
            fork(1),
            fork(2),
            Event::Rm(Rm {
                id: 1,
                saved_branch: None,
                ts: "t".into(),
            }),
            Event::Unknown,
        ];
        let ledger = fold(events);
        assert!(ledger[&1].removed.is_some());
        assert!(ledger[&2].removed.is_none());
        assert_eq!(ledger[&2].label(), "2-f2");
    }

    #[test]
    fn old_allow_lines_fold_the_same_as_without_them() {
        let lines = [
            r#"{"type":"fork","id":2,"name":"x","branch":"pfs/2-x","base":"abc","ts":"t"}"#,
            r#"{"type":"allow","id":2,"path":"/Users/x/.m2","ts":"t"}"#,
            r#"{"type":"fork","id":3,"branch":"pfs/3","ts":"t"}"#,
            r#"{"type":"allow","id":3,"path":"/opt/cache","ts":"t"}"#,
            r#"{"type":"rm","id":2,"saved_branch":"pfs/2-x","ts":"t"}"#,
        ];
        let events =
            |lines: &[&str]| -> Vec<Event> { lines.iter().map(|line| serde_json::from_str(line).unwrap()).collect() };
        let without: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| !line.contains(r#""type":"allow""#))
            .collect();
        assert!(matches!(events(&lines[1..2])[..], [Event::Unknown]));
        assert_eq!(fold(events(&lines)), fold(events(&without)));
        assert_eq!(fold(events(&lines)).len(), 2);
    }

    #[test]
    fn from_names_the_commit_a_moving_rev_meant() {
        let started = |from: &str, base: &str| Fork {
            from: Some(from.to_string()),
            base: Some(base.to_string()),
            ..match fork(9) {
                Event::Fork(fork) => fork,
                _ => unreachable!(),
            }
        };
        assert_eq!(
            started("HEAD~1", "4e7547fdeadbeef").started_from().as_deref(),
            Some("HEAD~1 (4e7547f)")
        );
        assert_eq!(
            started("4e7547f", "4e7547fdeadbeef").started_from().as_deref(),
            Some("4e7547f")
        );
    }

    #[test]
    fn unknown_types_parse() {
        let event: Event = serde_json::from_str(r#"{"type":"heat","id":3}"#).unwrap();
        assert!(matches!(event, Event::Unknown));
    }

    #[test]
    fn fork_line_shape() {
        let line = serde_json::to_string(&fork(8)).unwrap();
        assert!(line.starts_with(r#"{"type":"fork","id":8,"name":"f8","branch":"pfs/8-f8""#));
        assert!(!line.contains("parent_session"));
    }

    #[test]
    fn old_agent_forks_still_parse() {
        let line = r#"{"type":"fork","id":3,"branch":"pfs/3-x","task":"fix x","session":"01a1","parent_session":"01a0","ts":"t"}"#;
        let Event::Fork(fork) = serde_json::from_str(line).unwrap() else {
            panic!("an old --agent fork line should still parse");
        };
        assert_eq!((fork.id, fork.parent_session.as_deref()), (3, Some("01a0")));
    }
}
