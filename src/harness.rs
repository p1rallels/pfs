//! Harnesses: the agent programs people run in forks (pi so far). pfs never
//! starts one: an agent in a fork is a background run like any other. What
//! pfs takes from a harness:
//!
//! - lineage: the env var it sets for the commands it runs, naming the
//!   conversation, so a `pfs new` run by one of its agents records that
//!   conversation as the fork's parent.
//! - its conversations in a fork, and whether each one's turn is still open,
//!   so `pfs rm` doesn't delete a fork under an agent that's thinking (pi-web
//!   hosts a whole conversation in one process whose working folder is
//!   elsewhere, so no process check can see it).
//!
//! This table is the only place pfs knows about any harness. Adding one is
//! one entry in `HARNESSES`.

use std::{path::PathBuf, time::SystemTime};

use anyhow::Result;

pub struct Harness {
    pub name: &'static str,
    pub session_env: &'static str,
    /// Its conversations whose working folder is one of these folders or
    /// inside one, open or not. The folders are one fork, as given and as
    /// resolved.
    pub conversations: fn(&[PathBuf]) -> Result<Vec<Filed>>,
}

/// One conversation as its harness filed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    pub id: String,
    pub last_write: SystemTime,
    pub turn: Turn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// The model is about to answer: the last message is a prompt or a tool's result.
    Thinking,
    /// The model called tools and they haven't all answered.
    RunningTools,
    Done,
}

pub const HARNESSES: &[Harness] = &[Harness {
    name: "pi",
    session_env: "PI_SESSION_ID",
    conversations: pi::conversations,
}];

/// The conversation that ran pfs, when a harness says: lineage for the ledger.
pub fn parent_session() -> Option<String> {
    session_in(|name| std::env::var(name).ok())
}

fn session_in(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    HARNESSES
        .iter()
        .find_map(|harness| env(harness.session_env).filter(|id| !id.is_empty()))
}

/// pi files a folder's conversations in `<agent dir>/sessions/--<path>--/`,
/// or flat in PI_CODING_AGENT_SESSION_DIR. Each file's first line says
/// which folder it ran in; its last user, assistant or tool result message
/// says where the turn stands.
mod pi {
    use std::{
        collections::BTreeSet,
        ffi::OsStr,
        fs::File,
        io::{self, BufRead, BufReader, ErrorKind, Read, Seek},
        path::{Path, PathBuf},
    };

    use anyhow::{Context, Result, anyhow};
    use serde::Deserialize;

    use super::{Filed, Turn};
    use crate::{paths, store};

    const AGENT_DIR_ENV: &str = "PI_CODING_AGENT_DIR";
    const SESSION_DIR_ENV: &str = "PI_CODING_AGENT_SESSION_DIR";
    /// pi's own limit on a conversation's first line.
    const HEADER_LIMIT: u64 = 1024 * 1024;
    /// How far back the turn reader starts; it reads more while a line runs on.
    const TAIL_CHUNK: usize = 64 * 1024;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Folders {
        /// `<agent dir>/sessions`: a `--<path>--` folder per working folder.
        pub sessions: PathBuf,
        /// PI_CODING_AGENT_SESSION_DIR: files straight in it, from any folder.
        pub flat: Option<PathBuf>,
    }

    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        cwd: PathBuf,
    }

    #[derive(Deserialize)]
    struct Entry {
        #[serde(rename = "type")]
        kind: String,
        message: Option<Message>,
    }

    #[derive(Deserialize)]
    struct Message {
        role: String,
        #[serde(rename = "stopReason")]
        stop_reason: Option<String>,
    }

    pub fn conversations(roots: &[PathBuf]) -> Result<Vec<Filed>> {
        let folders = folders(|name| std::env::var(name).ok())?;
        conversations_in(&folders, roots)
    }

    pub fn folders(env: impl Fn(&str) -> Option<String>) -> Result<Folders> {
        let var = |name: &str| env(name).filter(|value| !value.is_empty());
        let home = || {
            var("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("HOME is not set, so pfs can't find pi's conversations"))
        };
        let agent = match var(AGENT_DIR_ENV) {
            Some(raw) => expand(&raw, home)?,
            None => home()?.join(".pi").join("agent"),
        };
        let flat = var(SESSION_DIR_ENV).map(|raw| expand(&raw, home)).transpose()?;
        Ok(Folders {
            sessions: agent.join("sessions"),
            flat,
        })
    }

    /// `~` and `~/…`, the way pi expands them.
    fn expand(raw: &str, home: impl Fn() -> Result<PathBuf>) -> Result<PathBuf> {
        match raw.strip_prefix('~') {
            Some("") => home(),
            Some(rest) if rest.starts_with('/') => Ok(home()?.join(rest.trim_start_matches('/'))),
            _ => Ok(PathBuf::from(raw)),
        }
    }

    /// Conversations whose first line puts them in one of `roots` or a
    /// folder inside it.
    pub fn conversations_in(folders: &Folders, roots: &[PathBuf]) -> Result<Vec<Filed>> {
        let filed = paths::entries(&folders.sessions)?.into_iter().filter(|dir| {
            dir.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| roots.iter().any(|root| may_hold(name, root)))
        });
        let files: BTreeSet<PathBuf> = filed
            .chain(folders.flat.clone())
            .map(|dir| paths::entries(&dir))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .filter(|file| file.extension() == Some(OsStr::new("jsonl")))
            .collect();
        files
            .iter()
            .map(|file| read(file, roots))
            .filter_map(Result::transpose)
            .collect()
    }

    /// pi's getDefaultSessionDirPath (and pi-web's sessionDirForCwd).
    pub fn folder_name(cwd: &Path) -> String {
        let raw = cwd.to_string_lossy();
        let rest = raw.strip_prefix(['/', '\\']).unwrap_or(&raw);
        format!("--{}--", rest.replace(['/', '\\', ':'], "-"))
    }

    /// `root`'s own folder, or the folder of one inside it. The encoding is
    /// lossy (`5/x` and `5-x` share a name), so a file's first line decides.
    fn may_hold(name: &str, root: &Path) -> bool {
        let own = folder_name(root);
        let stem = own.strip_suffix('-').unwrap_or(&own);
        name.starts_with(stem) && name.ends_with("--")
    }

    fn read(path: &Path, roots: &[PathBuf]) -> Result<Option<Filed>> {
        let context = || format!("can't read pi conversation {}", path.display());
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(context),
        };
        let Some(header) = header(&file)
            .with_context(context)?
            .filter(|header| paths::inside(&header.cwd, roots))
        else {
            return Ok(None);
        };
        let last_write = file.metadata().and_then(|meta| meta.modified()).with_context(context)?;
        let turn = turn(&file).with_context(context)?;
        Ok(Some(Filed {
            id: header.id,
            last_write,
            turn,
        }))
    }

    /// The first line, when it's a pi conversation's header.
    fn header(file: &File) -> io::Result<Option<Header>> {
        let mut line = Vec::new();
        BufReader::new(file.take(HEADER_LIMIT)).read_until(b'\n', &mut line)?;
        Ok(serde_json::from_slice::<Header>(&line)
            .ok()
            .filter(|header| header.kind == "session"))
    }

    /// pi appends whole entries as each message ends, never token by token,
    /// so the last message entry is the turn's state. No deciding message
    /// at all reads done.
    pub fn turn<R: Read + Seek>(reader: R) -> io::Result<Turn> {
        store::lines_from_end(reader, TAIL_CHUNK)
            .find_map(|line| line.map(|line| verdict(&line)).transpose())
            .unwrap_or(Ok(Turn::Done))
    }

    /// What one line says about the turn, if anything: other roles (system,
    /// bashExecution, custom), other entry types and lines that don't parse
    /// say nothing.
    fn verdict(line: &[u8]) -> Option<Turn> {
        let entry: Entry = serde_json::from_slice(line).ok()?;
        let message = entry.message.filter(|_| entry.kind == "message")?;
        match (message.role.as_str(), message.stop_reason.as_deref()) {
            ("user" | "toolResult", _) => Some(Turn::Thinking),
            ("assistant", Some("toolUse")) => Some(Turn::RunningTools),
            ("assistant", _) => Some(Turn::Done),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{fs, io::Cursor};

        use super::*;

        const HEADER: &str = r#"{"type":"session","version":3,"id":"01a13e1f-a572-75c1-9fac-cfd8cb035217","timestamp":"2026-10-15T05:53:43.794Z","cwd":"/w/repo/.forks/5"}"#;

        fn message(role: &str, stop: Option<&str>) -> String {
            let stop = stop
                .map(|stop| format!(r#","stopReason":"{stop}""#))
                .unwrap_or_default();
            format!(
                r#"{{"type":"message","id":"e6244b91","parentId":"ac217994","timestamp":"2026-10-15T06:07:37.850Z","message":{{"role":"{role}","content":[{{"type":"text","text":"ok"}}]{stop}}}}}"#
            )
        }

        fn entry(kind: &str) -> String {
            format!(r#"{{"type":"{kind}","id":"420d0644","parentId":null,"timestamp":"2026-10-15T05:53:43.905Z"}}"#)
        }

        fn turn_of(lines: &[String]) -> Turn {
            let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
            turn(Cursor::new(text.into_bytes())).unwrap()
        }

        fn after_header(lines: &[String]) -> Turn {
            turn_of(&[vec![HEADER.to_string()], lines.to_vec()].concat())
        }

        #[test]
        fn a_prompt_or_a_tool_result_means_thinking() {
            assert_eq!(after_header(&[message("user", None)]), Turn::Thinking);
            assert_eq!(
                after_header(&[message("assistant", Some("toolUse")), message("toolResult", None)]),
                Turn::Thinking
            );
        }

        #[test]
        fn an_assistant_that_called_tools_means_running_tools() {
            assert_eq!(
                after_header(&[message("user", None), message("assistant", Some("toolUse"))]),
                Turn::RunningTools
            );
        }

        #[test]
        fn any_other_assistant_ending_closes_the_turn() {
            ["stop", "length", "aborted", "error", "somethingNew"]
                .iter()
                .for_each(|stop| {
                    assert_eq!(
                        after_header(&[message("user", None), message("assistant", Some(stop))]),
                        Turn::Done,
                        "{stop}"
                    );
                });
            assert_eq!(
                after_header(&[message("user", None), message("assistant", None)]),
                Turn::Done
            );
        }

        #[test]
        fn other_roles_and_entries_are_skipped() {
            let trailing = [
                message("system", None),
                message("bashExecution", None),
                message("custom", None),
                entry("model_change"),
                entry("session_info"),
                entry("compaction"),
                entry("thinking_level_change"),
            ];
            assert_eq!(
                after_header(&[vec![message("assistant", Some("toolUse"))], trailing.to_vec()].concat()),
                Turn::RunningTools
            );
            assert_eq!(
                after_header(&[vec![message("assistant", Some("stop"))], trailing.to_vec()].concat()),
                Turn::Done
            );
        }

        #[test]
        fn a_malformed_last_line_is_skipped() {
            let lines = [message("user", None), r#"{"type":"message","message":{"ro"#.to_string()];
            assert_eq!(after_header(&lines), Turn::Thinking);
            assert_eq!(
                after_header(&[message("user", None), r#"{"type":"message"}"#.to_string()]),
                Turn::Thinking
            );
        }

        #[test]
        fn no_deciding_message_reads_done() {
            assert_eq!(turn(Cursor::new(Vec::new())).unwrap(), Turn::Done);
            assert_eq!(turn_of(&[HEADER.to_string()]), Turn::Done);
            assert_eq!(
                after_header(&[entry("model_change"), message("system", None)]),
                Turn::Done
            );
        }

        #[test]
        fn a_last_line_without_a_newline_still_decides() {
            let text = format!("{HEADER}\n{}", message("user", None));
            assert_eq!(turn(Cursor::new(text.into_bytes())).unwrap(), Turn::Thinking);
        }

        #[test]
        fn a_big_last_line_is_read_whole() {
            let output = "x".repeat(3 * 1024 * 1024 + 17);
            let big = format!(
                r#"{{"type":"message","id":"a1","message":{{"role":"toolResult","content":[{{"type":"text","text":"{output}"}}]}}}}"#
            );
            assert_eq!(
                after_header(&[message("assistant", Some("toolUse")), big]),
                Turn::Thinking
            );
        }

        #[test]
        fn folder_names_match_pi() {
            assert_eq!(folder_name(Path::new("/private/tmp")), "--private-tmp--");
            assert_eq!(folder_name(Path::new(r"C:\w\a:b")), "--C--w-a-b--");
            assert_eq!(folder_name(Path::new(r"\w\x")), "--w-x--");
        }

        #[test]
        fn a_fork_may_hold_its_own_folder_and_its_subfolders_only() {
            let fork = Path::new("/w/repo/.forks/5");
            assert!(may_hold("--w-repo-.forks-5--", fork));
            assert!(may_hold("--w-repo-.forks-5-src-ui--", fork));
            assert!(!may_hold("--w-repo-.forks-50--", fork));
            assert!(!may_hold("--w-repo--", fork));
            assert!(!may_hold("--w-repo-.forks-5-", fork));
        }

        #[test]
        fn folders_come_from_the_env_given() {
            let env = |pairs: &'static [(&str, &str)]| {
                move |name: &str| {
                    pairs
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| value.to_string())
                }
            };
            assert_eq!(
                folders(env(&[("HOME", "/h")])).unwrap(),
                Folders {
                    sessions: PathBuf::from("/h/.pi/agent/sessions"),
                    flat: None
                }
            );
            assert_eq!(
                folders(env(&[
                    ("HOME", "/h"),
                    ("PI_CODING_AGENT_DIR", "~/agent"),
                    ("PI_CODING_AGENT_SESSION_DIR", "/t/s")
                ]))
                .unwrap(),
                Folders {
                    sessions: PathBuf::from("/h/agent/sessions"),
                    flat: Some(PathBuf::from("/t/s"))
                }
            );
            assert_eq!(
                folders(env(&[
                    ("PI_CODING_AGENT_DIR", "/a"),
                    ("PI_CODING_AGENT_SESSION_DIR", "")
                ]))
                .unwrap()
                .flat,
                None
            );
            assert!(folders(env(&[])).is_err());
        }

        fn scratch(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("pfs-harness-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn write(dir: &Path, name: &str, cwd: &str, id: &str, last: &str) {
            fs::create_dir_all(dir).unwrap();
            let header = format!(
                r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-10-15T05:53:43.794Z","cwd":"{cwd}"}}"#
            );
            fs::write(dir.join(name), format!("{header}\n{last}\n")).unwrap();
        }

        #[test]
        fn finds_the_forks_conversations_by_their_first_line() {
            let base = scratch("pi");
            let sessions = base.join("sessions");
            let flat = base.join("flat");
            let fork = "/w/repo/.forks/5";
            let open = message("user", None);
            let tools = message("assistant", Some("toolUse"));
            let done = message("assistant", Some("stop"));
            write(
                &sessions.join(folder_name(Path::new(fork))),
                "a.jsonl",
                fork,
                "0000000a",
                &open,
            );
            write(
                &sessions.join("--w-repo-.forks-5-src--"),
                "b.jsonl",
                "/w/repo/.forks/5/src",
                "0000000b",
                &tools,
            );
            write(
                &sessions.join("--w-repo-.forks-5-x--"),
                "c.jsonl",
                "/w/repo/.forks/5-x",
                "0000000c",
                &open,
            );
            write(
                &sessions.join("--w-repo-.forks-50--"),
                "d.jsonl",
                "/w/repo/.forks/50",
                "0000000d",
                &open,
            );
            write(&sessions.join("--w-repo--"), "e.jsonl", "/w/repo", "0000000e", &open);
            write(
                &sessions.join(folder_name(Path::new(fork))),
                "notes.txt",
                fork,
                "0000000f",
                &open,
            );
            write(&flat, "g.jsonl", fork, "0000000g", &done);
            write(&flat, "h.jsonl", "/w/repo", "0000000h", &open);
            fs::write(flat.join("i.jsonl"), "not a header\n").unwrap();
            let folders = Folders {
                sessions: sessions.clone(),
                flat: Some(flat.clone()),
            };
            let found = conversations_in(&folders, &[PathBuf::from(fork)]);
            let missing = conversations_in(
                &Folders {
                    sessions: base.join("nowhere"),
                    flat: Some(base.join("nowhere-either")),
                },
                &[PathBuf::from(fork)],
            );
            let _ = fs::remove_dir_all(&base);
            let mut found: Vec<(String, Turn)> =
                found.unwrap().into_iter().map(|filed| (filed.id, filed.turn)).collect();
            found.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                found,
                vec![
                    ("0000000a".to_string(), Turn::Thinking),
                    ("0000000b".to_string(), Turn::RunningTools),
                    ("0000000g".to_string(), Turn::Done),
                ]
            );
            assert!(missing.unwrap().is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_harness_that_names_a_conversation_wins() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(session_in(env(&[("PI_SESSION_ID", "01a1")])).as_deref(), Some("01a1"));
        assert_eq!(session_in(env(&[("PI_SESSION_ID", "")])), None);
        assert_eq!(session_in(env(&[])), None);
    }
}
