//! Python venvs bake absolute paths into their scripts: `bin/pip`'s shebang,
//! `activate`'s VIRTUAL_ENV, editable installs' `.pth` files. In a clone they
//! still point at the main folder, so `pip install` in a fork would write to
//! the main folder's venv and `pytest` would import the main folder's code.
//! After cloning, pfs rewrites those paths to the fork (what virtualenv-clone
//! does). Only small text files are touched, so it costs a few ms.

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

const MAX_FILE: u64 = 1024 * 1024;
const SKIP_DIRS: &[&str] = &[".git", "node_modules", ".forks"];

/// Rewrites every in-repo venv of the fork; returns the venvs it fixed.
pub fn relocate(root: &Path, fork_dir: &Path) -> Result<Vec<PathBuf>> {
    venvs(fork_dir)
        .into_iter()
        .map(|venv| relocate_one(root, fork_dir, &venv).map(|_| venv))
        .collect()
}

/// Folders holding a `pyvenv.cfg`, one or two levels down.
fn venvs(fork_dir: &Path) -> Vec<PathBuf> {
    let level = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter(|entry| !SKIP_DIRS.iter().any(|skip| entry.file_name() == *skip))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    };
    let top = level(fork_dir);
    let second: Vec<PathBuf> = top
        .iter()
        .filter(|dir| !dir.join("pyvenv.cfg").is_file())
        .flat_map(|dir| level(dir))
        .collect();
    top.into_iter()
        .chain(second)
        .filter(|dir| dir.join("pyvenv.cfg").is_file())
        .collect()
}

fn relocate_one(root: &Path, fork_dir: &Path, venv: &Path) -> Result<()> {
    let rel = venv.strip_prefix(fork_dir).unwrap_or(venv);
    let recorded_venv = recorded_path(venv).unwrap_or_else(|| root.join(rel));
    let recorded_root = strip_suffix(&recorded_venv, rel).unwrap_or_else(|| root.to_path_buf());
    let froms = [recorded_root, root.to_path_buf()]
        .into_iter()
        .fold(Vec::<PathBuf>::new(), |mut acc, from| {
            if !acc.contains(&from) {
                acc.push(from);
            }
            acc
        });
    candidates(venv)
        .iter()
        .try_for_each(|file| rewrite(file, &froms, fork_dir).map(|_| ()))
}

/// Where the venv thought it lived, from `activate`'s VIRTUAL_ENV line.
fn recorded_path(venv: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(venv.join("bin").join("activate")).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("VIRTUAL_ENV=")?;
        let unquoted = value.trim_matches(|c| c == '"' || c == '\'');
        unquoted.starts_with('/').then(|| PathBuf::from(unquoted))
    })
}

fn strip_suffix(path: &Path, suffix: &Path) -> Option<PathBuf> {
    let path_parts: Vec<_> = path.components().collect();
    let suffix_parts: Vec<_> = suffix.components().collect();
    let keep = path_parts.len().checked_sub(suffix_parts.len())?;
    (path_parts[keep..] == suffix_parts[..]).then(|| path_parts[..keep].iter().collect())
}

/// bin/ scripts, pyvenv.cfg, and the top-level files of site-packages where
/// editable installs keep their paths.
fn candidates(venv: &Path) -> Vec<PathBuf> {
    let files_in = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    };
    let editable = |path: &PathBuf| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        name.ends_with(".pth") || name.ends_with(".egg-link") || name.starts_with("__editable__")
    };
    let site_packages: Vec<PathBuf> = fs::read_dir(venv.join("lib"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path().join("site-packages"))
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    files_in(&venv.join("bin"))
        .into_iter()
        .chain(std::iter::once(venv.join("pyvenv.cfg")))
        .chain(site_packages.iter().flat_map(|dir| files_in(dir)).filter(editable))
        .collect()
}

/// Replaces each of `froms` with `to` where it's a whole path (followed by
/// `/`, a quote, whitespace, `:` or the end), so `/a/repo` never matches
/// inside `/a/repo-other`. One pass, so a replacement is never replaced
/// again. Writes a new file (the clone's blocks stay with main).
fn rewrite(file: &Path, froms: &[PathBuf], to: &Path) -> Result<bool> {
    let meta = fs::symlink_metadata(file)?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Ok(false);
    }
    let bytes = fs::read(file)?;
    if bytes.iter().take(1024).any(|byte| *byte == 0) {
        return Ok(false);
    }
    let patterns: Vec<&[u8]> = froms.iter().map(|from| from.as_os_str().as_encoded_bytes()).collect();
    let replaced = replace_paths(&bytes, &patterns, to.as_os_str().as_encoded_bytes());
    if replaced == bytes {
        return Ok(false);
    }
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let temp = file.with_file_name(format!(".{name}.pfs-tmp"));
    let mut out = fs::File::create(&temp).with_context(|| format!("can't write {}", temp.display()))?;
    out.write_all(&replaced)?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(meta.permissions().mode()))?;
    fs::rename(&temp, file).with_context(|| format!("can't replace {}", file.display()))?;
    Ok(true)
}

fn replace_paths(bytes: &[u8], froms: &[&[u8]], to: &[u8]) -> Vec<u8> {
    let is_boundary = |next: Option<&u8>| match next {
        None => true,
        Some(byte) => matches!(byte, b'/' | b'"' | b'\'' | b':' | b';') || byte.is_ascii_whitespace(),
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hit = froms
            .iter()
            .find(|from| bytes[index..].starts_with(from) && is_boundary(bytes.get(index + from.len())));
        match hit {
            Some(from) => {
                out.extend_from_slice(to);
                index += from.len();
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_whole_paths_only() {
        let text = b"#!/a/repo/.venv/bin/python\nX=/a/repo-other\nY='/a/repo'\n";
        let out = replace_paths(text, &[b"/a/repo"], b"/a/repo/.forks/3");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "#!/a/repo/.forks/3/.venv/bin/python\nX=/a/repo-other\nY='/a/repo/.forks/3'\n"
        );
    }

    #[test]
    fn never_replaces_a_replacement() {
        let out = replace_paths(
            b"/tmp/r/x /private/tmp/r/y",
            &[b"/tmp/r", b"/private/tmp/r"],
            b"/private/tmp/r/.forks/1",
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "/private/tmp/r/.forks/1/x /private/tmp/r/.forks/1/y"
        );
    }

    #[test]
    fn strips_path_suffixes() {
        assert_eq!(
            strip_suffix(Path::new("/tmp/toy/.venv"), Path::new(".venv")),
            Some(PathBuf::from("/tmp/toy"))
        );
        assert_eq!(strip_suffix(Path::new("/tmp/toy/env"), Path::new(".venv")), None);
    }

    #[test]
    fn relocates_a_fake_venv() {
        let base = std::env::temp_dir().join(format!("pfs-venv-test-{}", std::process::id()));
        let root = base.join("repo");
        let fork = root.join(".forks").join("1");
        let bin = fork.join(".venv").join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(fork.join(".venv").join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        let shebang = format!("#!{}/.venv/bin/python3\n", root.display());
        fs::write(bin.join("pip"), &shebang).unwrap();
        fs::set_permissions(bin.join("pip"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            bin.join("activate"),
            format!("VIRTUAL_ENV=\"{}/.venv\"\n", root.display()),
        )
        .unwrap();
        let fixed = relocate(&root, &fork).unwrap();
        assert_eq!(fixed, vec![fork.join(".venv")]);
        let pip = fs::read_to_string(bin.join("pip")).unwrap();
        assert_eq!(pip, format!("#!{}/.venv/bin/python3\n", fork.display()));
        assert_eq!(
            fs::metadata(bin.join("pip")).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(&base).unwrap();
    }
}
