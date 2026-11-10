//! Where forks live: outside every repo, in the forks dir (`~/.pfs/forks`),
//! one folder per main, `<forks dir>/<key>/`. In it: `.main` names the main
//! (the back-pointer), `README.md` is a sign for whoever finds the folder,
//! `<n>/` are the forks and `.rm-<n>/` rm's deletes in progress. The main
//! records that folder in `.git/pfs/forks-dir`, so a later `[forks] dir`
//! never strands its forks: the record wins. The ledger lives next to the
//! record, where `git clean` can't reach it and it moves with the repo.
//! Also what pfs 0.3 left: `<main>/.forks/`.

use std::{
    fs::{self, DirBuilder},
    io::{ErrorKind, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    config::Global,
    git, paths,
    place::parse_dir_id,
    store::{self, Lock},
};

pub const MAIN: &str = ".main";
pub const SIGN: &str = "README.md";
const RECORD: &str = "forks-dir";
const LEDGER: &str = "ledger.jsonl";
/// Not `.lock`: in a fork, `.git/pfs/.lock` is the runs' lock.
const LEDGER_LOCK: &str = "ledger.lock";
/// pfs 0.3 kept forks, the ledger and its lock in `<main>/.forks/`.
pub const LEGACY: &str = ".forks";
/// `<name>`, `<name>-2`, … this many before pfs gives up on a key.
const MAX_KEYS: usize = 1000;

/// A main's own pfs folder: inside `.git`, so nothing tracks or cleans it.
fn pfs_dir(root: &Path) -> PathBuf {
    root.join(".git").join("pfs")
}

fn record_path(root: &Path) -> PathBuf {
    pfs_dir(root).join(RECORD)
}

pub fn ledger_path(root: &Path) -> PathBuf {
    pfs_dir(root).join(LEDGER)
}

/// The ledger's lock, held to claim a fork number, to write the ledger and
/// to settle the main's forks folder.
pub fn lock(root: &Path) -> Result<Lock> {
    let dir = pfs_dir(root);
    fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
    store::lock(&dir.join(LEDGER_LOCK))
}

/// The first line of a file, when it's an absolute path.
fn path_in(file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(file).ok()?;
    let path = PathBuf::from(text.lines().next()?.trim());
    path.is_absolute().then_some(path)
}

/// The forks folder a main's record names, as written.
pub fn recorded(root: &Path) -> Option<PathBuf> {
    path_in(&record_path(root))
}

/// The main a forks folder's `.main` names, as written.
pub fn main_named(home: &Path) -> Option<PathBuf> {
    path_in(&home.join(MAIN))
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// One place, once symlinks are resolved (/tmp is /private/tmp).
fn same(a: &Path, b: &Path) -> bool {
    resolved(a) == resolved(b)
}

/// `main` is a repo whose record names `home`.
pub fn claims(main: &Path, home: &Path) -> bool {
    main.join(".git").is_dir() && recorded(main).is_some_and(|record| same(&record, home))
}

/// Whose a forks folder is, as one main sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    Ours,
    /// No `.main`: not made yet, or lost.
    Unsigned,
    /// Another main is there and claims it: we're a copy of that main.
    Taken,
    /// `.main` names a folder that's gone, or that no longer claims it.
    Stale,
}

fn claim(home: &Path, root: &Path) -> Claim {
    match main_named(home) {
        None => Claim::Unsigned,
        Some(main) if same(&main, root) => Claim::Ours,
        Some(main) if claims(&main, home) => Claim::Taken,
        Some(_) => Claim::Stale,
    }
}

/// A main's forks folder: what its record names, unless that folder's
/// `.main` names another main that's there and claims it (this one is a
/// copy, and a copy never takes over the original's forks). A main that
/// moved signs its folder again, so its forks know it.
pub fn home_of(root: &Path) -> Option<PathBuf> {
    let (home, claim) = found_home(root)?;
    if matches!(claim, Claim::Unsigned | Claim::Stale) && home.is_dir() {
        let _ = sign(&home, root);
    }
    Some(home)
}

/// `home_of` without signing: the same folder, its `.main` left as it is.
pub fn home_seen(root: &Path) -> Option<PathBuf> {
    found_home(root).map(|(home, _)| home)
}

fn found_home(root: &Path) -> Option<(PathBuf, Claim)> {
    let home = resolved(&recorded(root)?);
    match claim(&home, root) {
        Claim::Taken => None,
        claim => Some((home, claim)),
    }
}

/// The sign in every forks folder.
pub fn sign_text(main: &Path) -> String {
    let name = key_name(main);
    let main = paths::tildify(main);
    format!(
        "# forks of {name}\n\n\
         pfs forks of {main} live here. each numbered folder is a full copy of\n\
         that repo on its own branch (pfs/<n>-...).\n\n\
         remove a fork with `pfs rm <n>` from the main folder, never by hand: a\n\
         fork's commits exist nowhere else until `pfs rm` lands its branch there.\n\n\
         `.main` names the main folder, so pfs can tell its forks. leave it be.\n"
    )
}

/// Writes a file whole or not at all: readers of `.main` and the record
/// never see half a line.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    fs::File::create(&temp)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .and_then(|()| fs::rename(&temp, path))
        .with_context(|| format!("can't write {}", path.display()))
}

fn sign(home: &Path, root: &Path) -> Result<()> {
    write_atomic(&home.join(MAIN), &format!("{}\n", root.display()))?;
    write_atomic(&home.join(SIGN), &sign_text(root))
}

/// Every folder pfs makes on the way, `$PFS_HOME` and the forks dir
/// included, is 0700: forks carry `.env`, and `~` is often 750.
fn make_private(dir: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("can't create {}", dir.display()))
}

/// The disk a path is on, or would be: its nearest folder that exists.
fn disk(path: &Path) -> Option<u64> {
    path.ancestors()
        .find_map(|at| fs::metadata(at).ok())
        .map(|meta| meta.dev())
}

/// Clones only work within one disk. A disk pfs can't tell is the clone's
/// to find out.
fn one_disk(a: &Path, b: &Path) -> bool {
    match (disk(a), disk(b)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

pub fn other_disk(root: &Path, forks_dir: &Path, config: &Path) -> String {
    format!(
        "{} and the forks dir {} are on different disks, and clones only work within one. \
         set `[forks] dir` in {} to a folder on the repo's disk",
        paths::tildify(root),
        paths::tildify(forks_dir),
        paths::tildify(config)
    )
}

/// A fork, or one being deleted: what makes a forks folder worth keeping.
fn holds_forks(home: &Path) -> bool {
    paths::entries(home).unwrap_or_default().iter().any(|entry| {
        entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| parse_dir_id(name.strip_prefix(".rm-").unwrap_or(name)).is_some())
    })
}

/// The folder a main's next fork goes in, made and signed if it isn't yet.
/// Run under the ledger lock, so two `pfs new` at once settle on one.
pub fn home_for_new(root: &Path, config: &Global) -> Result<PathBuf> {
    match home_of(root) {
        Some(home) if one_disk(root, &home) => {
            make_private(&home)?;
            if claim(&home, root) != Claim::Ours || !home.join(SIGN).is_file() {
                sign(&home, root)?;
            }
            Ok(home)
        }
        Some(home) if holds_forks(&home) => bail!(
            "{} and its forks folder {} are on different disks now (the repo moved?), and clones only work \
             within one. land its forks with `pfs rm <n>`, then the next `pfs new` starts a folder in the forks dir",
            paths::tildify(root),
            paths::tildify(&home)
        ),
        Some(home) => {
            forget(&home);
            allocate(root, config)
        }
        None => allocate(root, config),
    }
}

/// An empty forks folder left on another disk: unsigned first, so it's
/// nobody's even if something else in it keeps it from going.
fn forget(home: &Path) {
    let _ = fs::remove_file(home.join(MAIN));
    let _ = fs::remove_file(home.join(SIGN));
    let _ = fs::remove_dir(home);
}

/// A key for a main: its folder's name.
fn key_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string())
}

/// `<name>`, then `<name>-2`, `<name>-3`… for mains that share a name.
fn keys(name: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(name.to_string()).chain((2..).map(move |n| format!("{name}-{n}")))
}

/// A new forks folder in the forks dir, under the first key nobody has,
/// then the record that makes it this main's.
fn allocate(root: &Path, config: &Global) -> Result<PathBuf> {
    if !one_disk(root, &config.forks_dir) {
        bail!("{}", other_disk(root, &config.forks_dir, &config.path));
    }
    make_private(&config.forks_dir)?;
    let dir = resolved(&config.forks_dir);
    let name = key_name(root);
    let home = keys(&name)
        .take(MAX_KEYS)
        .map(|key| take(&dir.join(key), root))
        .find_map(Result::transpose)
        .ok_or_else(|| anyhow!("{} has no free folder for {name} after {MAX_KEYS} tries", dir.display()))??;
    let record = record_path(root);
    if let Some(dir) = record.parent() {
        fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
    }
    write_atomic(&record, &format!("{}\n", home.display()))?;
    Ok(home)
}

/// `home` for this main: made just now, or already signed for it (a crash
/// before the record). Anyone else's, or nobody's yet, is left alone.
fn take(home: &Path, root: &Path) -> Result<Option<PathBuf>> {
    match DirBuilder::new().mode(0o700).create(home) {
        Ok(()) => sign(home, root).map(|()| Some(home.to_path_buf())),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Ok((claim(home, root) == Claim::Ours).then(|| home.to_path_buf()))
        }
        Err(error) => Err(error).with_context(|| format!("can't create {}", home.display())),
    }
}

/// What pfs 0.3 left in `<main>/.forks/`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Leftovers {
    forks: Vec<u32>,
    trash: Vec<PathBuf>,
    ledger: Option<PathBuf>,
    lock: Option<PathBuf>,
}

fn leftovers(entries: Vec<(PathBuf, bool)>) -> Leftovers {
    let mut found = entries
        .into_iter()
        .fold(Leftovers::default(), |mut found, (path, is_dir)| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            match (name.as_str(), is_dir) {
                ("ledger.jsonl", false) => found.ledger = Some(path),
                (".lock", false) => found.lock = Some(path),
                (name, true) if name.starts_with(".rm-") => found.trash.push(path),
                (name, true) => found.forks.extend(parse_dir_id(name)),
                _ => {}
            }
            found
        });
    found.forks.sort_unstable();
    found
}

/// pfs 0.3's forks can't be read here, so they're the one thing that stops
/// a main.
fn legacy_forks(root: &Path, ids: &[u32]) -> String {
    let ids = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
    format!(
        "{} has forks from pfs 0.3 ({ids}). this pfs keeps forks outside the repo and can't read those: \
         land them with pfs 0.3 first (`pfs rm {ids}` from {})",
        paths::tildify(&root.join(LEGACY)),
        paths::tildify(root)
    )
}

/// `<main>/.forks/<n>`, a fork pfs 0.3 made.
pub fn legacy_fork(top: &Path) -> Option<(PathBuf, u32)> {
    let id = parse_dir_id(top.file_name()?.to_str()?)?;
    let forks = top.parent()?;
    let root = forks.parent()?;
    (forks.file_name()? == LEGACY && root.join(".git").is_dir()).then(|| (root.to_path_buf(), id))
}

pub fn legacy_fork_error(top: &Path, root: &Path, id: u32) -> String {
    format!(
        "this is fork {id} from pfs 0.3 ({}). this pfs keeps forks outside the repo and can't read it: \
         land it with pfs 0.3 first (`pfs rm {id}` from {})",
        paths::tildify(top),
        paths::tildify(root)
    )
}

/// Why `migrate` left `<main>/.forks/` as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Left {
    /// Git tracks something in it: the repo's own files, maybe from
    /// upstream. pfs 0.3 kept its folder out of git (`/.forks/` in
    /// `.git/info/exclude`).
    Tracked,
    /// Its ledger sits beside the one pfs keeps now, and lines from a folder
    /// anyone can commit or copy into a repo never join a live ledger.
    BesideLedger,
}

/// One line for stderr: which folder, and why.
pub fn left_note(root: &Path, left: Left) -> String {
    let folder = paths::tildify(&root.join(LEGACY));
    match left {
        Left::Tracked => {
            format!("{folder} left as it is: git tracks files in it, so it's the repo's own, not pfs 0.3's")
        }
        Left::BesideLedger => format!(
            "{folder} left as it is: .git/pfs/ledger.jsonl already exists and pfs never merges another ledger into it \
             (if pfs 0.3 wrote {LEGACY}/{LEDGER}, append it there yourself, then remove {LEGACY})"
        ),
    }
}

/// The old ledger moves to `.git/pfs/`, under its lock, so ids carry on;
/// 0.3's half-done deletes are finished, and `.forks/` goes once it's empty.
/// Old forks still there stop everything until pfs 0.3 lands them. A
/// `.forks/` that git tracks, or whose ledger would land beside a live one,
/// is left as it is, and the reason returned.
pub fn migrate(root: &Path) -> Result<Option<Left>> {
    let old = root.join(LEGACY);
    if !old.is_dir() {
        return Ok(None);
    }
    let tracked =
        git::tracks(root, LEGACY).with_context(|| format!("can't tell whether git tracks {}", paths::tildify(&old)))?;
    if tracked {
        return Ok(Some(Left::Tracked));
    }
    let entries = paths::entries(&old)?
        .into_iter()
        .map(|path| {
            let is_dir = path.is_dir();
            (path, is_dir)
        })
        .collect();
    let found = leftovers(entries);
    if !found.forks.is_empty() {
        bail!("{}", legacy_forks(root, &found.forks));
    }
    if let Some(old_ledger) = &found.ledger {
        let _lock = lock(root)?;
        if carry_ledger(old_ledger, &ledger_path(root))? == Carry::Kept {
            return Ok(Some(Left::BesideLedger));
        }
    }
    found.trash.iter().for_each(|trash| {
        let _ = paths::remove_tree(trash);
    });
    if let Some(old_lock) = &found.lock {
        let _ = fs::remove_file(old_lock);
    }
    let _ = fs::remove_dir(&old);
    Ok(None)
}

/// What became of `.forks/ledger.jsonl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carry {
    Moved,
    /// Gone already: another pfs moved it first.
    Gone,
    /// Left where it is: a ledger is already there.
    Kept,
}

/// Moved whole, never into or over a ledger that's there: the only honest
/// reason for both is two pfs versions racing in one repo, while a
/// committed or copied-in `.forks/ledger.jsonl` would steer rm, kill,
/// settle and every label. The link refuses a ledger made a moment ago too.
fn carry_ledger(old: &Path, new: &Path) -> Result<Carry> {
    if matches!(old.symlink_metadata(), Err(error) if error.kind() == ErrorKind::NotFound) {
        return Ok(Carry::Gone);
    }
    match fs::hard_link(old, new) {
        Ok(()) => fs::remove_file(old)
            .map(|()| Carry::Moved)
            .with_context(|| format!("can't remove {}", old.display())),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(Carry::Kept),
        Err(error) => Err(error).with_context(|| format!("can't move {} to {}", old.display(), new.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::ports::Plan;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfs-layout-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn repo(at: &Path) -> PathBuf {
        fs::create_dir_all(at.join(".git")).unwrap();
        at.to_path_buf()
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} in {}", dir.display());
    }

    /// A repo git can answer for: `migrate` asks it what it tracks.
    fn git_repo(at: &Path) -> PathBuf {
        fs::create_dir_all(at).unwrap();
        git(at, &["init", "-q"]);
        at.to_path_buf()
    }

    /// Every path under `dir`, files with their bytes: equal before and
    /// after means nothing in it was written, moved or removed.
    fn files(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let mut entries = paths::entries(dir).unwrap();
        entries.sort();
        entries
            .into_iter()
            .flat_map(|path| match path.is_dir() {
                true => std::iter::once((path.clone(), None))
                    .chain(files(&path))
                    .collect::<Vec<_>>(),
                false => vec![(path.clone(), Some(fs::read(&path).unwrap()))],
            })
            .collect()
    }

    /// `.forks/` as a 0.3 leftover looks once its forks are landed: a
    /// ledger with a fork 9 in it, a lock and a half-done delete.
    fn plant_leftover(at: &Path) -> String {
        let line = "{\"type\":\"fork\",\"id\":9,\"branch\":\"pfs/9-planted\"}\n";
        let old = at.join(LEGACY);
        fs::create_dir_all(old.join(".rm-2")).unwrap();
        fs::write(old.join(".rm-2").join("x"), "x").unwrap();
        fs::write(old.join(".lock"), "").unwrap();
        fs::write(old.join(LEDGER), line).unwrap();
        line.to_string()
    }

    fn config(forks_dir: &Path) -> Global {
        Global {
            ports: Plan::default(),
            forks_dir: forks_dir.to_path_buf(),
            path: PathBuf::from("/x/pfs/config.toml"),
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn keys_count_up_from_the_name() {
        assert_eq!(keys("toy").take(3).collect::<Vec<_>>(), ["toy", "toy-2", "toy-3"]);
        assert_eq!(key_name(Path::new("/w/pfs-cli")), "pfs-cli");
        assert_eq!(key_name(Path::new("/")), "repo");
    }

    #[test]
    fn the_sign_says_whose_forks_and_how_to_remove_them() {
        let sign = sign_text(Path::new("/w/toy"));
        assert!(sign.starts_with("# forks of toy\n"), "{sign}");
        assert!(sign.contains("pfs forks of /w/toy live here"), "{sign}");
        assert!(
            sign.contains("`pfs rm <n>` from the main folder, never by hand"),
            "{sign}"
        );
        assert!(sign.contains("commits exist nowhere else"), "{sign}");
    }

    #[test]
    fn a_first_new_makes_a_private_signed_folder_and_records_it() {
        let base = scratch("first");
        let main = repo(&base.join("toy"));
        let forks = base.join("home").join("forks");
        let home = home_for_new(&main, &config(&forks)).unwrap();
        assert_eq!(home, forks.join("toy"));
        assert_eq!(recorded(&main), Some(home.clone()));
        assert_eq!(main_named(&home), Some(main.clone()));
        assert!(home.join(SIGN).is_file());
        assert_eq!([mode(&base.join("home")), mode(&forks), mode(&home)], [0o700; 3]);
        assert_eq!(home_of(&main), Some(home.clone()));
        assert_eq!(home_for_new(&main, &config(&base.join("elsewhere"))).unwrap(), home);
        assert!(!base.join("elsewhere").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn mains_that_share_a_name_get_their_own_folders() {
        let base = scratch("twins");
        let forks = base.join("forks");
        let one = repo(&base.join("a").join("toy"));
        let two = repo(&base.join("b").join("toy"));
        let three = repo(&base.join("c").join("toy"));
        assert_eq!(home_for_new(&one, &config(&forks)).unwrap(), forks.join("toy"));
        assert_eq!(home_for_new(&two, &config(&forks)).unwrap(), forks.join("toy-2"));
        fs::create_dir(forks.join("toy-3")).unwrap();
        assert_eq!(home_for_new(&three, &config(&forks)).unwrap(), forks.join("toy-4"));
        assert_eq!(main_named(&forks.join("toy-2")), Some(two.clone()));
        assert_eq!(home_for_new(&one, &config(&forks)).unwrap(), forks.join("toy"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_copy_never_takes_over_and_a_move_takes_its_folder_back() {
        let base = scratch("copy");
        let forks = base.join("forks");
        let main = repo(&base.join("toy"));
        let home = home_for_new(&main, &config(&forks)).unwrap();
        let copy = repo(&base.join("copy").join("toy"));
        fs::create_dir_all(pfs_dir(&copy)).unwrap();
        fs::copy(record_path(&main), record_path(&copy)).unwrap();
        assert_eq!(home_seen(&copy), None);
        assert_eq!(home_of(&copy), None);
        assert_eq!(home_for_new(&copy, &config(&forks)).unwrap(), forks.join("toy-2"));
        assert_eq!(main_named(&home), Some(main.clone()));

        let moved = base.join("moved");
        fs::rename(&main, &moved).unwrap();
        assert!(!claims(&main, &home));
        assert_eq!(home_seen(&moved), Some(home.clone()));
        assert_eq!(main_named(&home), Some(main.clone()));
        assert_eq!(home_of(&moved), Some(home.clone()));
        assert_eq!(main_named(&home), Some(moved.clone()));
        assert!(
            fs::read_to_string(home.join(SIGN))
                .unwrap()
                .contains(&moved.display().to_string())
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn another_main_s_orphans_are_left_alone() {
        let base = scratch("orphan");
        let forks = base.join("forks");
        fs::create_dir_all(forks.join("toy")).unwrap();
        fs::write(forks.join("toy").join(MAIN), "/gone/toy\n").unwrap();
        let main = repo(&base.join("toy"));
        assert_eq!(home_for_new(&main, &config(&forks)).unwrap(), forks.join("toy-2"));
        assert_eq!(main_named(&forks.join("toy")), Some(PathBuf::from("/gone/toy")));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn old_leftovers_sort_into_forks_trash_ledger_and_lock() {
        let at = |name: &str| PathBuf::from("/w/repo/.forks").join(name);
        let found = leftovers(vec![
            (at("ledger.jsonl"), false),
            (at(".lock"), false),
            (at("12"), true),
            (at("3"), true),
            (at(".rm-4"), true),
            (at("notes"), true),
            (at("5"), false),
        ]);
        assert_eq!(
            found,
            Leftovers {
                forks: vec![3, 12],
                trash: vec![at(".rm-4")],
                ledger: Some(at("ledger.jsonl")),
                lock: Some(at(".lock")),
            }
        );
    }

    #[test]
    fn the_old_ledger_moves_and_old_forks_stop_it() {
        let base = scratch("legacy");
        let main = git_repo(&base.join("toy"));
        let old = main.join(LEGACY);
        fs::create_dir_all(old.join(".rm-2")).unwrap();
        fs::write(old.join("ledger.jsonl"), "{\"type\":\"fork\",\"id\":7}\n").unwrap();
        fs::write(old.join(".lock"), "").unwrap();
        fs::create_dir(old.join("3")).unwrap();
        let refused = migrate(&main).unwrap_err().to_string();
        assert!(refused.contains("forks from pfs 0.3 (3)"), "{refused}");
        assert!(refused.contains("`pfs rm 3`"), "{refused}");
        assert!(old.join("ledger.jsonl").is_file());
        fs::remove_dir(old.join("3")).unwrap();
        migrate(&main).unwrap();
        assert!(!old.exists());
        assert_eq!(
            fs::read_to_string(ledger_path(&main)).unwrap(),
            "{\"type\":\"fork\",\"id\":7}\n"
        );
        migrate(&main).unwrap();
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn an_old_ledger_found_late_never_joins_the_new_one() {
        let base = scratch("late");
        let old = base.join("old.jsonl");
        let new = base.join("new.jsonl");
        fs::write(&old, "b\n").unwrap();
        fs::write(&new, "a").unwrap();
        assert_eq!(carry_ledger(&old, &new).unwrap(), Carry::Kept);
        assert_eq!(fs::read_to_string(&new).unwrap(), "a");
        assert_eq!(fs::read_to_string(&old).unwrap(), "b\n");
        fs::remove_file(&new).unwrap();
        assert_eq!(carry_ledger(&old, &new).unwrap(), Carry::Moved);
        assert_eq!(fs::read_to_string(&new).unwrap(), "b\n");
        assert!(!old.exists());
        assert_eq!(carry_ledger(&old, &new).unwrap(), Carry::Gone);
        assert_eq!(fs::read_to_string(&new).unwrap(), "b\n");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_ledger_beside_the_live_one_stays_where_it_is() {
        let base = scratch("beside");
        let main = git_repo(&base.join("toy"));
        plant_leftover(&main);
        fs::create_dir_all(pfs_dir(&main)).unwrap();
        fs::write(ledger_path(&main), "{\"type\":\"fork\",\"id\":1}\n").unwrap();
        let seen = || (files(&main.join(LEGACY)), fs::read(ledger_path(&main)).unwrap());
        let before = seen();
        assert_eq!(migrate(&main).unwrap(), Some(Left::BesideLedger));
        assert_eq!(seen(), before, "a ledger from .forks joined the live one");
        assert_eq!(migrate(&main).unwrap(), Some(Left::BesideLedger));
        assert_eq!(seen(), before);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_forks_folder_git_tracks_is_the_repo_s_own() {
        let base = scratch("tracked");
        let upstream = git_repo(&base.join("upstream"));
        plant_leftover(&upstream);
        git(&upstream, &["add", "-A"]);
        git(&upstream, &["commit", "-qm", "a ledger for whoever clones"]);
        git(&base, &["clone", "-q", "upstream", "main"]);
        let main = base.join("main");
        let before = files(&main.join(LEGACY));
        assert_eq!(migrate(&main).unwrap(), Some(Left::Tracked));
        assert_eq!(files(&main.join(LEGACY)), before, "migrate wrote in a tracked .forks");
        assert!(!pfs_dir(&main).exists(), "migrate made .git/pfs for a tracked .forks");

        fs::create_dir_all(main.join(LEGACY).join("3")).unwrap();
        fs::write(main.join(LEGACY).join("3").join("notes.md"), "n\n").unwrap();
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-qm", "a folder named like a 0.3 fork"]);
        assert_eq!(migrate(&main).unwrap(), Some(Left::Tracked));
        assert!(main.join(LEGACY).join(LEDGER).is_file());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_0_3_leftover_with_no_ledger_yet_still_moves() {
        let base = scratch("leftover");
        let main = git_repo(&base.join("toy"));
        let line = plant_leftover(&main);
        assert_eq!(migrate(&main).unwrap(), None);
        assert!(!main.join(LEGACY).exists());
        assert_eq!(fs::read_to_string(ledger_path(&main)).unwrap(), line);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_notes_name_the_folder_and_why() {
        let root = Path::new("/w/toy");
        let tracked = left_note(root, Left::Tracked);
        assert!(tracked.starts_with("/w/toy/.forks left as it is: "), "{tracked}");
        assert!(tracked.contains("git tracks files in it"), "{tracked}");
        let beside = left_note(root, Left::BesideLedger);
        assert!(beside.starts_with("/w/toy/.forks left as it is: "), "{beside}");
        assert!(beside.contains(".git/pfs/ledger.jsonl already exists"), "{beside}");
        assert!(
            beside.contains("append it there yourself, then remove .forks"),
            "{beside}"
        );
        assert!(!tracked.contains('\n') && !beside.contains('\n'));
    }

    #[test]
    fn a_0_3_fork_is_known_by_its_path() {
        let base = scratch("oldfork");
        let main = repo(&base.join("toy"));
        let fork = repo(&main.join(LEGACY).join("4"));
        assert_eq!(legacy_fork(&fork), Some((main.clone(), 4)));
        assert_eq!(legacy_fork(&main), None);
        assert_eq!(legacy_fork(&repo(&base.join("other").join("4"))), None);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn disks_are_told_apart_by_the_nearest_folder_that_exists() {
        let base = scratch("disk");
        assert!(disk(&base).is_some());
        assert_eq!(disk(&base.join("not").join("yet")), disk(&base));
        assert!(one_disk(&base, &base.join("not").join("yet")));
        fs::remove_dir_all(&base).unwrap();
    }
}
