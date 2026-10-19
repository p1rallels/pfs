//! APFS clones: `clonefile(2)` copies a whole tree copy-on-write in one
//! syscall. Each top-level entry is cloned on its own so some can be skipped
//! (pfs 0.3's `.forks/`: cloning it would copy old forks into the new one).
//! Clones only work within one disk.

use std::{
    ffi::{CString, OsStr},
    fmt, fs, io,
    os::unix::{ffi::OsStrExt, fs::FileTypeExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

const CLONE_NOFOLLOW: u32 = 0x0001;

/// clonefile's EXDEV: the source and the clone would be on different disks.
/// Its own error, so the caller can say what to change.
#[derive(Debug)]
pub struct OtherDisk {
    pub src: PathBuf,
}

impl fmt::Display for OtherDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "can't clone {} onto another disk", self.src.display())
    }
}

impl std::error::Error for OtherDisk {}

/// Why a clonefile failed, as far as pfs cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    OtherDisk,
    NotApfs,
    Other,
}

fn failure(error: &io::Error) -> Failure {
    match error.raw_os_error() {
        Some(libc::EXDEV) => Failure::OtherDisk,
        Some(libc::ENOTSUP) => Failure::NotApfs,
        _ => Failure::Other,
    }
}

pub fn tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<()> {
    let entries = fs::read_dir(src).with_context(|| format!("can't read {}", src.display()))?;
    entries
        .map(|entry| entry.with_context(|| format!("can't read {}", src.display())))
        .filter(|entry| match entry {
            Ok(entry) => !skip.iter().any(|name| entry.file_name() == OsStr::new(name)),
            Err(_) => true,
        })
        .try_for_each(|entry| {
            let name = entry?.file_name();
            entry_clone(&src.join(&name), &dst.join(&name))
        })
}

/// One entry: a single clonefile when possible. When a directory can't be
/// cloned in one go (it holds a socket, say), walk into it instead.
fn entry_clone(src: &Path, dst: &Path) -> Result<()> {
    let error = match clonefile(src, dst) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let kind = fs::symlink_metadata(src)
        .with_context(|| format!("can't stat {}", src.display()))?
        .file_type();
    let special = kind.is_socket() || kind.is_fifo();
    match (special, failure(&error), kind.is_dir()) {
        (true, _, _) => Ok(()),
        (_, Failure::OtherDisk, _) => Err(OtherDisk { src: src.to_path_buf() }.into()),
        (_, Failure::NotApfs, _) => bail!(
            "can't clone {}: {error}. pfs needs the repo on an APFS volume",
            src.display()
        ),
        (_, Failure::Other, true) => walk_clone(src, dst),
        (_, Failure::Other, false) => Err(error).with_context(|| format!("can't clone {}", src.display())),
    }
}

fn walk_clone(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        crate::paths::remove_tree(dst)?;
    }
    fs::create_dir(dst).with_context(|| format!("can't create {}", dst.display()))?;
    let permissions = fs::metadata(src)?.permissions();
    let result = fs::read_dir(src)
        .with_context(|| format!("can't read {}", src.display()))?
        .try_for_each(|entry| {
            let name = entry?.file_name();
            entry_clone(&src.join(&name), &dst.join(&name))
        });
    fs::set_permissions(dst, permissions)?;
    result
}

fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    match unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), CLONE_NOFOLLOW) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_disk_is_its_own_failure() {
        let failed = |errno: i32| failure(&io::Error::from_raw_os_error(errno));
        assert_eq!(failed(libc::EXDEV), Failure::OtherDisk);
        assert_eq!(failed(libc::ENOTSUP), Failure::NotApfs);
        assert_eq!(failed(libc::EEXIST), Failure::Other);
    }

    #[test]
    fn the_other_disk_error_can_be_told_apart() {
        let error: anyhow::Error = OtherDisk {
            src: PathBuf::from("/w/repo/src"),
        }
        .into();
        let error = error.context("couldn't create fork 3");
        assert!(error.chain().any(|cause| cause.is::<OtherDisk>()));
        assert_eq!(
            error.chain().last().unwrap().to_string(),
            "can't clone /w/repo/src onto another disk"
        );
    }
}
