//! `.forks/<n>/.git/pfs/runs.jsonl`: what ran in one fork. It lives inside
//! the fork, in `.git` where git never tracks anything (an agent's
//! `git add -A` can't commit it), and it's deleted along with the fork.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{clock, paths, proc, store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Fg,
    /// Runs from before `--agent` was folded into `run --bg` say `agent`.
    #[serde(alias = "agent")]
    Bg,
    Open,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Fg => "fg",
            Kind::Bg => "bg",
            Kind::Open => "open",
        }
    }

    pub fn parse(raw: &str) -> Option<Kind> {
        match raw {
            "fg" => Some(Kind::Fg),
            "bg" => Some(Kind::Bg),
            "open" => Some(Kind::Open),
            _ => None,
        }
    }

    pub fn has_log(self) -> bool {
        matches!(self, Kind::Bg)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Start(Start),
    Exit(Exit),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Start {
    pub run: u32,
    pub kind: Kind,
    pub cmd: String,
    pub pid: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<u64>,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exit {
    pub run: u32,
    pub code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub ts: String,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub start: Start,
    pub exit: Option<Exit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Exited(i32),
    /// No exit line and the process is gone: killed with its waiter, or the
    /// mac went down mid-run.
    Died,
}

impl Run {
    pub fn status(&self) -> Status {
        match &self.exit {
            Some(exit) => Status::Exited(exit.code),
            None if proc::alive(self.start.pid, self.start.proc_start) => Status::Running,
            None => Status::Died,
        }
    }

    pub fn log(&self, fork_dir: &Path) -> Option<PathBuf> {
        self.start.kind.has_log().then(|| log_path(fork_dir, self.start.run))
    }
}

pub fn dir(fork_dir: &Path) -> PathBuf {
    fork_dir.join(".git").join("pfs")
}

pub fn journal(fork_dir: &Path) -> PathBuf {
    dir(fork_dir).join("runs.jsonl")
}

pub fn lock_path(fork_dir: &Path) -> PathBuf {
    dir(fork_dir).join(".lock")
}

pub fn log_path(fork_dir: &Path, run: u32) -> PathBuf {
    dir(fork_dir).join(format!("{run}.log"))
}

pub fn ensure_dir(fork_dir: &Path) -> Result<()> {
    fs::create_dir_all(dir(fork_dir))?;
    Ok(())
}

pub fn load(fork_dir: &Path) -> Result<Vec<Run>> {
    let events: Vec<Event> = store::read(&journal(fork_dir))?;
    Ok(fold(events))
}

pub fn fold(events: Vec<Event>) -> Vec<Run> {
    events.into_iter().fold(Vec::new(), |mut runs, event| {
        match event {
            Event::Start(start) => runs.push(Run { start, exit: None }),
            Event::Exit(exit) => {
                if let Some(run) = runs.iter_mut().rev().find(|run| run.start.run == exit.run) {
                    run.exit = Some(exit);
                }
            }
            Event::Unknown => {}
        }
        runs
    })
}

pub fn record(fork_dir: &Path, event: &Event) -> Result<()> {
    store::append(&journal(fork_dir), event)
}

/// Callers hold the fork's run lock, so two runs never share a number.
pub fn next_number(fork_dir: &Path) -> Result<u32> {
    let recorded = load(fork_dir)?.iter().map(|run| run.start.run).max().unwrap_or(0);
    let logged = fs::read_dir(dir(fork_dir))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name();
                    name.to_str()?.strip_suffix(".log")?.parse::<u32>().ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    Ok(recorded.max(logged) + 1)
}

pub fn exit_event(run: u32, status: std::process::ExitStatus) -> Event {
    use std::os::unix::process::ExitStatusExt;
    let signal = status.signal();
    let code = status.code().unwrap_or_else(|| 128 + signal.unwrap_or(0));
    Event::Exit(Exit {
        run,
        code,
        signal,
        ts: clock::now(),
    })
}

pub fn describe(status: Status) -> String {
    match status {
        Status::Running => "running".to_string(),
        Status::Exited(0) => "done".to_string(),
        Status::Exited(code) => format!("failed (exit {code})"),
        Status::Died => "died".to_string(),
    }
}

/// How a bg run ended, and when. A death has no end time, so it says when it
/// started.
pub fn ending_note(id: u32, run: &Run, status: Status) -> String {
    let what = format!("bg {id}.{} {}", run.start.run, describe(status));
    match &run.exit {
        Some(exit) => format!("{what} {} ago", clock::ago(&exit.ts)),
        None => format!("{what}, started {} ago", clock::ago(&run.start.ts)),
    }
}

pub fn log_size(fork_dir: &Path, run: &Run) -> Option<u64> {
    run.log(fork_dir).and_then(|path| paths::file_size(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(run: u32, pid: i32) -> Event {
        Event::Start(Start {
            run,
            kind: Kind::Bg,
            cmd: "x".into(),
            pid,
            proc_start: None,
            ts: clock::now(),
        })
    }

    #[test]
    fn folds_starts_and_exits() {
        let runs = fold(vec![
            start(1, 999_999),
            start(2, 999_998),
            Event::Exit(Exit {
                run: 1,
                code: 3,
                signal: None,
                ts: "t".into(),
            }),
        ]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].status(), Status::Exited(3));
        assert_eq!(runs[1].status(), Status::Died);
    }

    #[test]
    fn old_agent_runs_read_as_bg() {
        let line = r#"{"type":"start","run":1,"kind":"agent","cmd":"pi: x","pid":42,"session":"01a1","ts":"t"}"#;
        let Event::Start(start) = serde_json::from_str(line).unwrap() else {
            panic!("an old agent start line should still parse");
        };
        assert_eq!(start.kind, Kind::Bg);
        assert!(start.kind.has_log());
    }

    fn ended(run: u32, code: Option<i32>) -> Run {
        let Event::Start(start) = start(run, 999_999) else {
            unreachable!()
        };
        Run {
            start,
            exit: code.map(|code| Exit {
                run,
                code,
                signal: None,
                ts: clock::now(),
            }),
        }
    }

    #[test]
    fn endings_say_when() {
        assert_eq!(
            ending_note(5, &ended(2, Some(0)), Status::Exited(0)),
            "bg 5.2 done 0s ago"
        );
        assert_eq!(
            ending_note(5, &ended(1, Some(1)), Status::Exited(1)),
            "bg 5.1 failed (exit 1) 0s ago"
        );
        assert_eq!(
            ending_note(5, &ended(3, None), Status::Died),
            "bg 5.3 died, started 0s ago"
        );
    }

    #[test]
    fn start_line_shape() {
        let line = serde_json::to_string(&start(1, 42)).unwrap();
        assert!(line.starts_with(r#"{"type":"start","run":1,"kind":"bg","cmd":"x","pid":42"#));
    }
}
