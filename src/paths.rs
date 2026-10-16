use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

/// `~/x` for paths under home, the full path otherwise.
pub fn tildify(path: &Path) -> String {
    match home()
        .ok()
        .and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Short display form: relative to `base` when inside it.
pub fn show(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
        Ok(rest) => rest.display().to_string(),
        Err(_) => tildify(path),
    }
}

/// `remove_dir_all` that also copes with read-only directories
/// (go's module cache, some node_modules).
pub fn remove_tree(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => {
            make_dirs_writable(path);
            fs::remove_dir_all(path).with_context(|| format!("couldn't delete {}", path.display()))
        }
    }
}

fn make_dirs_writable(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode();
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    if let Ok(entries) = fs::read_dir(path) {
        entries.flatten().for_each(|entry| make_dirs_writable(&entry.path()));
    }
}

pub fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|meta| meta.len())
}

/// A folder's entries; a folder that isn't there has none.
pub fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(listing) => Ok(listing.flatten().map(|entry| entry.path()).collect()),
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("can't list {}", dir.display())),
    }
}

/// `path` is one of `roots` or inside one, component-wise: `.forks/5` holds
/// `.forks/5/src` but not `.forks/50`. Relative paths are inside nothing.
pub fn inside(path: &Path, roots: &[PathBuf]) -> bool {
    path.is_absolute() && roots.iter().any(|root| path.starts_with(root))
}

/// `a, b, c +2 more`: a list cut to `max` for one line.
pub fn shown(items: &[String], max: usize) -> String {
    let listed = items.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    match items.len().saturating_sub(max) {
        0 => listed,
        more => format!("{listed} +{more} more"),
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, f64); 4] = [
        ("T", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("G", 1024.0 * 1024.0 * 1024.0),
        ("M", 1024.0 * 1024.0),
        ("K", 1024.0),
    ];
    let value = bytes as f64;
    UNITS
        .iter()
        .find(|(_, size)| value >= *size)
        .map(|(unit, size)| format!("{:.1}{unit}", value / size))
        .unwrap_or_else(|| format!("{bytes}B"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_goes_by_whole_components() {
        let roots = [
            PathBuf::from("/tmp/r/.forks/5"),
            PathBuf::from("/private/tmp/r/.forks/5"),
        ];
        assert!(inside(Path::new("/tmp/r/.forks/5"), &roots));
        assert!(inside(Path::new("/private/tmp/r/.forks/5/web"), &roots));
        assert!(!inside(Path::new("/private/tmp/r/.forks/50"), &roots));
        assert!(!inside(Path::new("/tmp/r"), &roots));
        assert!(!inside(Path::new("tmp/r/.forks/5"), &[PathBuf::from("tmp/r/.forks/5")]));
    }

    #[test]
    fn a_missing_folder_has_no_entries() {
        assert!(entries(Path::new("/no/such/pfs/folder")).unwrap().is_empty());
    }

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(1536), "1.5K");
        assert_eq!(human_bytes(1_288_490_189), "1.2G");
    }

    #[test]
    fn shown_cuts_a_list_for_one_line() {
        let items = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(shown(&items(&["a", "b"]), 3), "a, b");
        assert_eq!(shown(&items(&["a", "b", "c", "d", "e"]), 3), "a, b, c +2 more");
        assert_eq!(shown(&[], 3), "");
    }
}
