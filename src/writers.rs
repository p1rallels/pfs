//! Who has files open for writing in a folder pfs is about to clone. A clone
//! taken while a process writes (a sqlite db in WAL mode, its -wal and -shm)
//! can catch a file mid-write. pfs can't make that copy consistent, but it
//! can say who was writing and what.
//!
//! One lsof over every process's numbered descriptors (no tree walk, so it
//! costs the same whatever is in the folder), kept when a regular file open
//! for writing is inside the folder, component-wise. pfs's own files don't
//! count: `.git/pfs/` holds the run journal and the logs every bg run's
//! processes write to, and the copy drops it anyway; `.forks` isn't copied.

use std::{
    ffi::OsStr,
    fs, io,
    io::Read,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use crate::{layout, occupants::LSOF, paths, runs};

/// Numbered descriptors only (no cwd, txt or mem rows). `-b` keeps lsof off
/// the kernel calls that block on a dead network mount, `-w` drops the
/// warnings that come with it.
const LSOF_ARGS: [&str; 7] = ["-b", "-w", "-d", "0-999999", "-Fpcfatn", "+c", "0"];
/// lsof takes ~0.1s; past this it's stuck, and `pfs new` goes on without it.
const PATIENCE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writer {
    pub pid: i32,
    pub command: String,
    /// Relative to the folder, sorted, each once.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
    ReadWrite,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Regular,
    Other,
}

/// One file a process has open, as lsof lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    pid: i32,
    command: String,
    access: Access,
    kind: Kind,
    path: PathBuf,
}

/// The file set being read: `f` opens one, `a`, `t` and `n` fill it in.
struct Pending {
    access: Access,
    kind: Kind,
    path: Option<PathBuf>,
}

#[derive(Default)]
struct Cursor {
    pid: Option<i32>,
    command: String,
    file: Option<Pending>,
    found: Vec<Open>,
}

/// A command that ended within its time.
struct Finished {
    pid: i32,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
}

/// The processes with regular files open for writing in `dir`, by pid, each
/// once. pfs itself and its lsof aren't writers.
pub fn in_folder(dir: &Path) -> Result<Vec<Writer>> {
    let root = fs::canonicalize(dir).with_context(|| format!("can't resolve {}", dir.display()))?;
    let lsof = list()?;
    let own = i32::try_from(std::process::id()).unwrap_or(-1);
    Ok(writing_in(&parse_lsof(&lsof.stdout), &root, &[own, lsof.pid]))
}

/// lsof exits 1 when it can't read some processes; what it did list counts.
fn list() -> Result<Finished> {
    let mut lsof = Command::new(LSOF);
    lsof.args(LSOF_ARGS);
    let done = bounded(lsof, PATIENCE)?;
    match parse_lsof(&done.stdout).is_empty() {
        true => bail!("lsof listed no open files ({}: {})", done.status, done.stderr),
        false => Ok(done),
    }
}

/// Runs `command` to the end, or stops it once `patience` is up. A stopped
/// one is reaped off the main thread: one stuck in the kernel can't be
/// killed until its call returns.
fn bounded(mut command: Command, patience: Duration) -> Result<Finished> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("can't run {program}"))?;
    let pid = i32::try_from(child.id()).unwrap_or(-1);
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let stdout = match stdout.recv_timeout(patience) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            abandon(child);
            return Err(error).with_context(|| format!("can't read what {program} printed"));
        }
        Err(_) => {
            abandon(child);
            bail!("{program} didn't finish in {patience:?}");
        }
    };
    let status = child.wait().with_context(|| format!("{program} didn't finish"))?;
    let stderr = match stderr.recv_timeout(patience) {
        Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).trim().to_string(),
        Ok(Err(error)) => format!("can't read its errors: {error}"),
        Err(_) => "its errors didn't arrive".to_string(),
    };
    Ok(Finished {
        pid,
        status,
        stdout,
        stderr,
    })
}

fn abandon(mut child: Child) {
    let _ = child.kill();
    thread::spawn(move || child.wait());
}

/// Reads a pipe to its end on its own thread, so a full pipe never stalls
/// the command and the reader can be given up on.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let read = pipe.ok_or_else(|| io::Error::other("no pipe")).and_then(|mut pipe| {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        });
        let _ = send.send(read);
    });
    receive
}

/// `lsof -F` output: `p<pid>` opens each process and `c<command>` names it,
/// then each file is `f<fd>` and its `a<access>`, `t<type>` and `n<path>`.
/// A file with no path or no process is dropped; other fields are skipped.
fn parse_lsof(listing: &[u8]) -> Vec<Open> {
    let end = listing
        .split(|byte| *byte == b'\n')
        .filter_map(<[u8]>::split_first)
        .fold(Cursor::default(), step);
    flush(end).found
}

fn step(cursor: Cursor, (field, value): (&u8, &[u8])) -> Cursor {
    match *field {
        b'p' => Cursor {
            pid: parse_pid(value),
            command: String::new(),
            ..flush(cursor)
        },
        b'c' => Cursor {
            command: String::from_utf8_lossy(value).into_owned(),
            ..cursor
        },
        b'f' => Cursor {
            file: Some(Pending {
                access: Access::Unknown,
                kind: Kind::Other,
                path: None,
            }),
            ..flush(cursor)
        },
        b'a' => fill(cursor, |file| Pending {
            access: access(value),
            ..file
        }),
        b't' => fill(cursor, |file| Pending {
            kind: kind(value),
            ..file
        }),
        b'n' => fill(cursor, |file| Pending {
            path: Some(PathBuf::from(OsStr::from_bytes(value))),
            ..file
        }),
        _ => cursor,
    }
}

fn fill(cursor: Cursor, change: impl FnOnce(Pending) -> Pending) -> Cursor {
    Cursor {
        file: cursor.file.map(change),
        ..cursor
    }
}

/// Ends the file set being read, keeping it when it's whole.
fn flush(cursor: Cursor) -> Cursor {
    let Cursor {
        pid,
        command,
        file,
        mut found,
    } = cursor;
    let whole = pid.zip(file).and_then(|(pid, file)| {
        Some(Open {
            pid,
            command: command.clone(),
            access: file.access,
            kind: file.kind,
            path: file.path?,
        })
    });
    found.extend(whole);
    Cursor {
        pid,
        command,
        file: None,
        found,
    }
}

fn parse_pid(raw: &[u8]) -> Option<i32> {
    std::str::from_utf8(raw).ok()?.parse().ok()
}

fn access(raw: &[u8]) -> Access {
    match raw {
        b"r" => Access::Read,
        b"w" => Access::Write,
        b"u" => Access::ReadWrite,
        _ => Access::Unknown,
    }
}

fn kind(raw: &[u8]) -> Kind {
    match raw {
        b"REG" => Kind::Regular,
        _ => Kind::Other,
    }
}

fn writes(access: Access) -> bool {
    matches!(access, Access::Write | Access::ReadWrite)
}

/// Regular files open for writing inside `root` (resolved, as lsof prints
/// paths), outside pfs's own, by processes other than `not_them`. One
/// writer per pid.
fn writing_in(open: &[Open], root: &Path, not_them: &[i32]) -> Vec<Writer> {
    let inside = [root.to_path_buf()];
    let skipped = skipped(root);
    let mut hits: Vec<(i32, &str, String)> = open
        .iter()
        .filter(|file| !not_them.contains(&file.pid) && writes(file.access) && file.kind == Kind::Regular)
        .filter(|file| paths::inside(&file.path, &inside) && !paths::inside(&file.path, &skipped))
        .map(|file| (file.pid, file.command.as_str(), paths::show(&file.path, root)))
        .collect();
    hits.sort();
    hits.dedup();
    hits.into_iter()
        .fold(Vec::new(), |mut writers: Vec<Writer>, (pid, command, file)| {
            match writers.last_mut() {
                Some(last) if last.pid == pid => last.files.push(file),
                _ => writers.push(Writer {
                    pid,
                    command: command.to_string(),
                    files: vec![file],
                }),
            }
            writers
        })
}

/// pfs's own files in the folder: its run journal and logs, and the forks
/// folder pfs 0.3 kept inside the repo.
fn skipped(root: &Path) -> [PathBuf; 2] {
    [runs::dir(root), root.join(layout::LEGACY)]
}

#[cfg(test)]
mod tests {
    use std::{os::unix::process::CommandExt, time::Instant};

    use super::*;

    fn open(pid: i32, access: Access, kind: Kind, path: &str) -> Open {
        Open {
            pid,
            command: format!("cmd{pid}"),
            access,
            kind,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn parses_lsof_fields_with_access_and_type() {
        let listing = b"p501\ncbun\nf0\nar\ntCHR\nn/dev/null\nf7\nau\ntREG\nn/private/tmp/r/var/v2.sqlite\n\
                        f8\naw\ntREG\nn/private/tmp/r/caf\xc3\xa9 \xc3\xbc.log\nf9\na \ntDIR\nn/private/tmp/r\n\
                        p502\nf3\naw\ntREG\nn/a\nf4\naw\ntREG\npbad\ncx\nf3\naw\ntREG\nn/b\n\
                        p503\ncsh\nf5\ntREG\nn/c\nf6\naw\nn/d\nf7\nau\ntREG\nn/e";
        assert_eq!(
            parse_lsof(listing),
            vec![
                Open {
                    command: "bun".into(),
                    ..open(501, Access::Read, Kind::Other, "/dev/null")
                },
                Open {
                    command: "bun".into(),
                    ..open(501, Access::ReadWrite, Kind::Regular, "/private/tmp/r/var/v2.sqlite")
                },
                Open {
                    command: "bun".into(),
                    ..open(501, Access::Write, Kind::Regular, "/private/tmp/r/café ü.log")
                },
                Open {
                    command: "bun".into(),
                    ..open(501, Access::Unknown, Kind::Other, "/private/tmp/r")
                },
                Open {
                    command: String::new(),
                    ..open(502, Access::Write, Kind::Regular, "/a")
                },
                Open {
                    command: "sh".into(),
                    ..open(503, Access::Unknown, Kind::Regular, "/c")
                },
                Open {
                    command: "sh".into(),
                    ..open(503, Access::Write, Kind::Other, "/d")
                },
                Open {
                    command: "sh".into(),
                    ..open(503, Access::ReadWrite, Kind::Regular, "/e")
                },
            ]
        );
        assert!(parse_lsof(b"").is_empty());
        assert!(parse_lsof(b"\n\nf3\naw\ntREG\nn/orphan\n").is_empty());
    }

    #[test]
    fn keeps_regular_files_open_for_writing_inside_the_folder() {
        let root = Path::new("/w/repo");
        let listed = [
            open(10, Access::ReadWrite, Kind::Regular, "/w/repo/var/v2.sqlite-wal"),
            open(10, Access::ReadWrite, Kind::Regular, "/w/repo/var/v2.sqlite"),
            open(10, Access::ReadWrite, Kind::Regular, "/w/repo/var/v2.sqlite"),
            open(10, Access::Read, Kind::Regular, "/w/repo/README.md"),
            open(11, Access::Write, Kind::Regular, "/w/repo/log/dev.log"),
            open(12, Access::Read, Kind::Regular, "/w/repo/src/main.rs"),
            open(13, Access::Unknown, Kind::Regular, "/w/repo/x"),
            open(14, Access::Write, Kind::Other, "/w/repo/fifo"),
            open(15, Access::Write, Kind::Regular, "/w/repo2/out.log"),
            open(16, Access::Write, Kind::Regular, "/w/other.log"),
            open(17, Access::Write, Kind::Regular, "w/repo/relative.log"),
            open(9, Access::Write, Kind::Regular, "/w/repo/a.log"),
        ];
        assert_eq!(
            writing_in(&listed, root, &[]),
            vec![
                Writer {
                    pid: 9,
                    command: "cmd9".into(),
                    files: vec!["a.log".into()],
                },
                Writer {
                    pid: 10,
                    command: "cmd10".into(),
                    files: vec!["var/v2.sqlite".into(), "var/v2.sqlite-wal".into()],
                },
                Writer {
                    pid: 11,
                    command: "cmd11".into(),
                    files: vec!["log/dev.log".into()],
                },
            ]
        );
        let pids = |writers: Vec<Writer>| writers.iter().map(|writer| writer.pid).collect::<Vec<_>>();
        assert_eq!(pids(writing_in(&listed, root, &[9, 11])), vec![10]);
        assert_eq!(pids(writing_in(&listed, Path::new("/w/repo2"), &[])), vec![15]);
    }

    #[test]
    fn pfs_s_own_files_in_the_folder_are_no_writers() {
        let root = Path::new("/w/repo");
        let listed = [
            open(20, Access::Write, Kind::Regular, "/w/repo/.git/pfs/1.log"),
            open(21, Access::ReadWrite, Kind::Regular, "/w/repo/.git/pfs/ledger.lock"),
            open(22, Access::Write, Kind::Regular, "/w/repo/.forks/5/var/db.sqlite"),
            open(23, Access::Write, Kind::Regular, "/w/repo/.git/index.lock"),
            open(24, Access::Write, Kind::Regular, "/w/repo/.git/pfsx/1.log"),
            open(25, Access::Write, Kind::Regular, "/w/repo/.forks2/a"),
        ];
        let found: Vec<(i32, Vec<String>)> = writing_in(&listed, root, &[])
            .into_iter()
            .map(|writer| (writer.pid, writer.files))
            .collect();
        assert_eq!(
            found,
            vec![
                (23, vec![".git/index.lock".to_string()]),
                (24, vec![".git/pfsx/1.log".to_string()]),
                (25, vec![".forks2/a".to_string()]),
            ]
        );
    }

    #[test]
    fn a_command_past_its_time_is_stopped() {
        let started = Instant::now();
        let mut sleep = Command::new("/bin/sleep");
        sleep.arg("30");
        let error = bounded(sleep, Duration::from_millis(200)).err().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(format!("{error:#}"), "/bin/sleep didn't finish in 200ms");
        let mut echo = Command::new("/bin/sh");
        echo.args(["-c", "echo out; echo err >&2; exit 3"]);
        let done = bounded(echo, Duration::from_secs(5)).unwrap();
        assert_eq!(
            (done.stdout.as_slice(), done.stderr.as_str(), done.status.code()),
            (&b"out\n"[..], "err", Some(3))
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("pfs-writers-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spawn_group(dir: &Path, script: &str) -> Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .current_dir(dir)
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn stop(mut child: Child) {
        unsafe { libc::kill(-i32::try_from(child.id()).unwrap(), libc::SIGKILL) };
        let _ = child.wait();
    }

    fn pid_of(child: &Child) -> i32 {
        i32::try_from(child.id()).unwrap()
    }

    /// lsof until each of `pids` has a file open in `dir`, for up to 3s.
    fn settled(dir: &Path, pids: &[i32]) -> bool {
        let roots = [fs::canonicalize(dir).unwrap()];
        (0..30).any(|_| {
            let open = parse_lsof(&list().unwrap().stdout);
            let ready = pids.iter().all(|pid| {
                open.iter()
                    .any(|file| file.pid == *pid && paths::inside(&file.path, &roots))
            });
            if !ready {
                thread::sleep(Duration::from_millis(100));
            }
            ready
        })
    }

    #[test]
    fn finds_a_real_writer_and_not_a_reader() {
        let dir = scratch("lsof");
        fs::create_dir_all(dir.join("var")).unwrap();
        fs::create_dir_all(runs::dir(&dir)).unwrap();
        fs::write(dir.join("README.md"), "hi\n").unwrap();
        let writer = spawn_group(
            &dir,
            "exec 3>>var/db.sqlite 4<>var/db.sqlite-wal 5<README.md; exec sleep 30",
        );
        let reader = spawn_group(&dir, "exec 3<README.md; exec sleep 30");
        let waiter = spawn_group(&dir, "exec 3>>.git/pfs/1.log; exec sleep 30");
        let pids = [pid_of(&writer), pid_of(&reader), pid_of(&waiter)];
        let ready = settled(&dir, &pids);
        let found = in_folder(&dir);
        [writer, reader, waiter].into_iter().for_each(stop);
        let _ = fs::remove_dir_all(&dir);
        assert!(ready, "the three never showed in lsof");
        assert_eq!(
            found.unwrap(),
            vec![Writer {
                pid: pids[0],
                command: "sleep".into(),
                files: vec!["var/db.sqlite".into(), "var/db.sqlite-wal".into()],
            }]
        );
    }
}
