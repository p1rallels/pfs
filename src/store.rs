//! Append-only jsonl files and the flock that guards id allocation.

use std::{
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

/// Every parseable line, in order. A missing file is an empty log, and so
/// is one under a `.git` that's a file (a worktree's).
/// Lines that don't parse (a crash mid-write, a newer pfs) are skipped.
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("can't read {}", path.display())),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// A file's lines from the last to the first, without their newlines.
/// Reads `chunk` bytes from the end, then as much again as it holds while a
/// line runs on, so a long last line costs about its own length and the
/// rest of the file is never read.
pub fn lines_from_end<R: Read + Seek>(mut reader: R, chunk: usize) -> impl Iterator<Item = io::Result<Vec<u8>>> {
    let chunk = chunk.max(1);
    let mut start: Option<u64> = None;
    let mut buf: Vec<u8> = Vec::new();
    let mut unscanned = 0;
    let mut finished = false;
    std::iter::from_fn(move || {
        loop {
            if finished {
                return None;
            }
            if let Some(at) = buf[..unscanned].iter().rposition(|byte| *byte == b'\n') {
                let line = buf.split_off(at + 1);
                buf.truncate(at);
                unscanned = at;
                return Some(Ok(line));
            }
            let read = match start {
                Some(0) => {
                    finished = true;
                    return Some(Ok(std::mem::take(&mut buf)));
                }
                Some(end) => read_before(&mut reader, end, chunk.max(buf.len())),
                None => reader
                    .seek(SeekFrom::End(0))
                    .and_then(|end| read_before(&mut reader, end, chunk)),
            };
            match read {
                Ok((from, fresh)) => {
                    unscanned = fresh.len();
                    start = Some(from);
                    buf = [fresh, std::mem::take(&mut buf)].concat();
                }
                Err(error) => {
                    finished = true;
                    return Some(Err(error));
                }
            }
        }
    })
}

/// Up to `want` bytes ending at `end`, and where they start.
fn read_before<R: Read + Seek>(reader: &mut R, end: u64, want: usize) -> io::Result<(u64, Vec<u8>)> {
    let from = end.saturating_sub(u64::try_from(want).unwrap_or(u64::MAX));
    let mut fresh = vec![0; usize::try_from(end - from).unwrap_or(0)];
    reader.seek(SeekFrom::Start(from))?;
    reader.read_exact(&mut fresh)?;
    Ok((from, fresh))
}

/// One line, one `write` on an O_APPEND fd, so concurrent appenders can't
/// interleave inside a line.
pub fn append<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("can't open {}", path.display()))?;
    file.write_all(&line)
        .with_context(|| format!("can't append to {}", path.display()))
}

/// Exclusive advisory lock, released when dropped (or when the process dies).
pub struct Lock {
    _file: File,
}

pub fn lock(path: &Path) -> Result<Lock> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("can't open lock {}", path.display()))?;
    loop {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc == 0 {
            return Ok(Lock { _file: file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error).with_context(|| format!("can't lock {}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn reads_lines_from_the_end_at_any_chunk_size() {
        let texts = [
            "",
            "\n",
            "a",
            "a\n",
            "a\nbb\n\nccc",
            "{\"x\":1}\n{\"y\":22}\n",
            "\nlead",
        ];
        texts.iter().for_each(|text| {
            let forward: Vec<Vec<u8>> = text.split('\n').map(|line| line.as_bytes().to_vec()).collect();
            let expected: Vec<Vec<u8>> = forward.into_iter().rev().collect();
            [1, 2, 3, 5, 64].iter().for_each(|chunk| {
                let backward: Vec<Vec<u8>> = lines_from_end(Cursor::new(text.as_bytes().to_vec()), *chunk)
                    .collect::<io::Result<_>>()
                    .unwrap();
                assert_eq!(backward, expected, "{text:?} in chunks of {chunk}");
            });
        });
    }

    #[test]
    fn stops_reading_once_a_line_decides() {
        let mut text = "z".repeat(10_000);
        text.push_str("\nlast");
        let first = lines_from_end(Cursor::new(text.into_bytes()), 8).next();
        assert_eq!(first.map(Result::unwrap), Some(b"last".to_vec()));
    }
}
