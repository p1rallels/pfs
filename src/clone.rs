//! APFS clones: `clonefile(2)` copies a whole tree copy-on-write in one
//! syscall. Each top-level entry is cloned on its own so `.forks/` can be
//! skipped (cloning it would copy old forks into the new one).

use std::{
    ffi::{CString, OsStr},
    fs, io,
    os::unix::{ffi::OsStrExt, fs::FileTypeExt},
    path::Path,
};

use anyhow::{Context, Result, bail};

const CLONE_NOFOLLOW: u32 = 0x0001;

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
    match (special, unsupported(&error), kind.is_dir()) {
        (true, _, _) => Ok(()),
        (_, true, _) => bail!(
            "can't clone {}: {error}. pfs needs the repo on an APFS volume",
            src.display()
        ),
        (_, false, true) => walk_clone(src, dst),
        (_, false, false) => Err(error).with_context(|| format!("can't clone {}", src.display())),
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

fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EXDEV))
}

fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    match unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), CLONE_NOFOLLOW) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}
