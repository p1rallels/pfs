//! Every repo's forks, read from the forks dir: what `pfs ls` shows outside
//! any repo. Fork n of a main lives in `<forks dir>/<key>/<n>`, one per-repo
//! folder per main, and the two name each other: `<key>/.main` holds the
//! main's path, the main's `.git/pfs/forks-dir` the per-repo folder's. When a
//! main is deleted, moved or replaced, its forks stay behind with work that
//! exists nowhere else, and this is where they turn up again.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
};

use anyhow::Result;

use crate::{forks, git, names, paths, place::parse_dir_id};

/// In a per-repo folder: the main's absolute path, one line.
const MAIN_FILE: &str = ".main";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerRepo {
    pub folder: PathBuf,
    /// From `.main`. `None` when it's missing or holds no absolute path.
    pub main: Option<PathBuf>,
    pub state: State,
    pub forks: Vec<Found>,
}

/// Whether the main `.main` names is still these forks' main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Here,
    /// Nothing at that path: deleted, or moved.
    Gone,
    Replaced(Replaced),
    /// No `.main` to go by.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replaced {
    NotARepo,
    /// A repo whose record names another per-repo folder, or (`None`) none.
    OtherRepo(Option<PathBuf>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub id: u32,
    pub dir: PathBuf,
    /// Checked only when the main isn't here: a main that is has `pfs ls`.
    pub work: Option<Work>,
}

/// A fork's work, without the ledger. `None` where git couldn't tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    pub branch: Option<String>,
    pub commits: Option<u32>,
    pub uncommitted: Option<bool>,
}

/// Every per-repo folder in the forks dir, by name. A folder with neither a
/// `.main` nor a fork isn't one (or is still being made) and is left out.
pub fn scan(forks_dir: &Path) -> Result<Vec<PerRepo>> {
    let mut found: Vec<PerRepo> = paths::entries(forks_dir)?
        .into_iter()
        .filter(|path| path.is_dir())
        .map(per_repo)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    Ok(with_work(found))
}

fn per_repo(folder: PathBuf) -> Result<Option<PerRepo>> {
    let main = recorded_path(&folder.join(MAIN_FILE));
    let forks = fork_dirs(&folder)?;
    let state = main.as_deref().map_or(State::Unknown, |main| state(&folder, main));
    Ok((main.is_some() || !forks.is_empty()).then_some(PerRepo {
        folder,
        main,
        state,
        forks,
    }))
}

/// Here only when the main's own record names this folder, as resolved
/// paths (`/tmp` is `/private/tmp`).
fn state(folder: &Path, main: &Path) -> State {
    match (main.exists(), git::git_dir(main).exists()) {
        (false, _) => State::Gone,
        (true, false) => State::Replaced(Replaced::NotARepo),
        (true, true) => match recorded_path(&forks_dir_record(main)) {
            Some(named) if same_folder(&named, folder) => State::Here,
            named => State::Replaced(Replaced::OtherRepo(named)),
        },
    }
}

fn forks_dir_record(main: &Path) -> PathBuf {
    git::git_dir(main).join("pfs").join("forks-dir")
}

fn recorded_path(file: &Path) -> Option<PathBuf> {
    fs::read_to_string(file).ok().as_deref().and_then(first_path)
}

/// The absolute path on a record's first line.
fn first_path(text: &str) -> Option<PathBuf> {
    text.lines().next().map(PathBuf::from).filter(|path| path.is_absolute())
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Numbered folders only: `.rm-<n>` is a delete in progress, `README.md` a sign.
fn fork_dirs(folder: &Path) -> Result<Vec<Found>> {
    let mut found: Vec<Found> = paths::entries(folder)?
        .into_iter()
        .filter(|path| path.is_dir())
        .filter_map(|dir| {
            let id = dir.file_name()?.to_str().and_then(parse_dir_id)?;
            Some(Found { id, dir, work: None })
        })
        .collect();
    found.sort_by_key(|fork| fork.id);
    Ok(found)
}

/// A few git calls per fork of a main that isn't here, all forks at once.
/// Forks of a main that is get none.
fn with_work(repos: Vec<PerRepo>) -> Vec<PerRepo> {
    let works: Vec<Vec<Option<Work>>> = thread::scope(|scope| {
        let checks: Vec<Vec<_>> = repos
            .iter()
            .map(|repo| {
                let check = repo.state != State::Here;
                repo.forks
                    .iter()
                    .map(|fork| check.then(|| scope.spawn(move || work(&fork.dir, fork.id))))
                    .collect()
            })
            .collect();
        checks
            .into_iter()
            .map(|forks| {
                forks
                    .into_iter()
                    .map(|check| check.map(|check| check.join().unwrap_or_default()))
                    .collect()
            })
            .collect()
    });
    repos
        .into_iter()
        .zip(works)
        .map(|(repo, works)| PerRepo {
            forks: repo
                .forks
                .into_iter()
                .zip(works)
                .map(|(fork, work)| Found { work, ..fork })
                .collect(),
            ..repo
        })
        .collect()
}

/// A folder without its own `.git` gets no git call: git would look for a
/// repo above it, and find the wrong one.
fn work(dir: &Path, id: u32) -> Work {
    match git::git_dir(dir).is_dir() {
        false => Work::default(),
        true => {
            let branch = own_branch(dir, id);
            let fork = forks::Fork {
                id,
                dir: dir.to_path_buf(),
                entry: None,
            };
            Work {
                commits: branch.as_deref().and_then(|branch| own_commits(dir, branch).ok()),
                uncommitted: forks::uncommitted(&fork).ok(),
                branch,
            }
        }
    }
}

/// `pfs/<n>-<name>` (or `pfs/<n>`): the one checked out, else the only one.
fn own_branch(dir: &Path, id: u32) -> Option<String> {
    git::current_branch(dir)
        .filter(|branch| is_own(branch, id))
        .or_else(|| {
            only(
                git::branches(dir, "pfs/")
                    .into_iter()
                    .filter(|branch| is_own(branch, id)),
            )
        })
}

fn is_own(branch: &str, id: u32) -> bool {
    let number = id.to_string();
    branch
        .strip_prefix("pfs/")
        .and_then(|label| label.strip_prefix(number.as_str()))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

fn only<T>(items: impl IntoIterator<Item = T>) -> Option<T> {
    let mut items = items.into_iter();
    match (items.next(), items.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

fn own_commits(dir: &Path, branch: &str) -> Result<u32> {
    let revs = only_on(branch);
    git::count_commits(dir, &revs.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Commits on `branch` and on no other branch, local or remote-tracking.
/// The ledger's base lives in the main, so the branches the fork was born
/// with (main's, as they were) stand in for it. A branch pushed somewhere
/// counts as kept: its remote-tracking copy holds it.
fn only_on(branch: &str) -> Vec<String> {
    vec![
        "--ignore-missing".to_string(),
        format!("refs/heads/{branch}"),
        "--not".to_string(),
        format!("--exclude={branch}"),
        "--branches".to_string(),
        "--remotes".to_string(),
    ]
}

/// What `pfs ls` prints outside any repo: the forks dir, one line per main
/// with forks, and under each main that isn't here why, its forks' work and
/// what to do about it. `show` turns a path into what's printed (`~/…`).
pub fn render(forks_dir: &Path, repos: &[PerRepo], show: impl Fn(&Path) -> String) -> Vec<String> {
    let shown: Vec<&PerRepo> = repos
        .iter()
        .filter(|repo| repo.state != State::Here || !repo.forks.is_empty())
        .collect();
    let rows: Vec<(String, String, String)> = shown
        .iter()
        .map(|repo| {
            let main = repo.main.as_deref().map(&show).unwrap_or_else(|| "?".to_string());
            (key(repo), main, count_note(repo.forks.len()))
        })
        .collect();
    let key_width = rows.iter().map(|(key, _, _)| key.chars().count()).max().unwrap_or(0);
    let main_width = rows.iter().map(|(_, main, _)| main.chars().count()).max().unwrap_or(0);
    let header = match shown.is_empty() {
        true => format!("no forks in {}", show(forks_dir)),
        false => format!("forks in {}", show(forks_dir)),
    };
    let blocks = shown.iter().zip(&rows).flat_map(|(repo, (key, main, count))| {
        let line = format!("{key:<key_width$}   {main:<main_width$}   {count}");
        std::iter::once(line).chain(details(repo, &show).into_iter().map(|detail| format!("  {detail}")))
    });
    std::iter::once(header).chain(blocks).collect()
}

fn key(repo: &PerRepo) -> String {
    repo.folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.folder.display().to_string())
}

fn count_note(count: usize) -> String {
    match count {
        0 => "no forks".to_string(),
        1 => "1 fork".to_string(),
        count => format!("{count} forks"),
    }
}

/// Nothing for a main that's here: `pfs ls` in it says the rest.
fn details(repo: &PerRepo, show: &dyn Fn(&Path) -> String) -> Vec<String> {
    match why(repo, show) {
        None => Vec::new(),
        Some(why) => std::iter::once(why)
            .chain(repo.forks.iter().map(fork_line))
            .chain(std::iter::once(what_to_do(repo, show)))
            .collect(),
    }
}

fn why(repo: &PerRepo, show: &dyn Fn(&Path) -> String) -> Option<String> {
    match &repo.state {
        State::Here => None,
        State::Gone => Some(
            "its folder is gone: it may have moved. running any pfs command in the moved main relinks these forks to it"
                .to_string(),
        ),
        State::Replaced(Replaced::NotARepo) => {
            Some("its folder was replaced: something else is there, not a repo".to_string())
        }
        State::Replaced(Replaced::OtherRepo(Some(other))) => Some(format!(
            "its folder was replaced: the repo there keeps its forks in {}, so it or this folder was moved or copied",
            show(other)
        )),
        State::Replaced(Replaced::OtherRepo(None)) => {
            Some("its folder was replaced: the repo there has no record of these forks".to_string())
        }
        State::Unknown => Some(format!(
            "no record of whose forks these are: {} is missing",
            show(&repo.folder.join(MAIN_FILE))
        )),
    }
}

/// `3-wait (2 commits · uncommitted)`, the label read off the fork's branch.
fn fork_line(fork: &Found) -> String {
    format!("{} ({})", fork_label(fork), work_note(fork.work.as_ref()))
}

fn fork_label(fork: &Found) -> String {
    let prefix = format!("pfs/{}-", fork.id);
    let name = fork
        .work
        .as_ref()
        .and_then(|work| work.branch.as_deref())
        .and_then(|branch| branch.strip_prefix(prefix.as_str()));
    names::label(fork.id, name)
}

/// Like `pfs ls`: what pfs couldn't tell is a `?`, never blank.
fn work_note(work: Option<&Work>) -> String {
    let Some(work) = work else {
        return "?".to_string();
    };
    let commits = match work.commits {
        None | Some(0) => None,
        Some(1) => Some("1 commit".to_string()),
        Some(count) => Some(format!("{count} commits")),
    };
    let uncommitted = (work.uncommitted == Some(true)).then(|| "uncommitted".to_string());
    let unknown = (work.commits.is_none() || work.uncommitted.is_none()).then(|| "?".to_string());
    let notes: Vec<String> = commits.into_iter().chain(uncommitted).chain(unknown).collect();
    match notes.is_empty() {
        true => "no work".to_string(),
        false => notes.join(" · "),
    }
}

/// A fork pfs couldn't check may have work, so it counts as having some.
fn has_work(fork: &Found) -> bool {
    fork.work
        .as_ref()
        .is_none_or(|work| work.commits != Some(0) || work.uncommitted != Some(false))
}

/// One line: the work is only in this folder, how to keep a fork's (shown on
/// the first fork with work), how to drop it all.
fn what_to_do(repo: &PerRepo, show: &dyn Fn(&Path) -> String) -> String {
    let folder = show(&repo.folder);
    match (repo.forks.iter().find(|fork| has_work(fork)), repo.forks.is_empty()) {
        (_, true) => format!("nothing in it: drop it by deleting {folder}. pfs itself never deletes these"),
        (None, false) => {
            format!("no fork in it has work: drop them by deleting {folder}. pfs itself never deletes these")
        }
        (Some(fork), false) => {
            let branch = fork
                .work
                .as_ref()
                .and_then(|work| work.branch.clone())
                .unwrap_or_else(|| "<its branch>".to_string());
            format!(
                "this work exists only in {folder}: keep a fork's by committing it and pushing or fetching its \
                 branch somewhere (git -C {} push <remote or repo> {branch}), or drop it all by deleting \
                 {folder}. pfs itself never deletes these",
                shell_path(&show(&fork.dir))
            )
        }
    }
}

/// A shown path, safe to paste into a shell: quoted when it needs it, with
/// a leading `~/` left out of the quotes so it still expands.
fn shell_path(shown: &str) -> String {
    let plain = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c))
    };
    let quote = |text: &str| match plain(text) {
        true => text.to_string(),
        false => format!("'{}'", text.replace('\'', r"'\''")),
    };
    match shown.strip_prefix("~/") {
        Some(rest) => format!("~/{}", quote(rest)),
        None => quote(shown),
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfs-everywhere-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
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
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} in {}", dir.display());
    }

    fn commit(dir: &Path, message: &str) {
        git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    }

    /// A main's record of its per-repo folder, the way the other side
    /// writes it: one line.
    fn link(main: &Path, folder: &Path) {
        write(&forks_dir_record(main), &format!("{}\n", folder.display()));
        write(&folder.join(MAIN_FILE), &format!("{}\n", main.display()));
    }

    fn states(found: &[PerRepo]) -> Vec<(String, State)> {
        found.iter().map(|repo| (key(repo), repo.state.clone())).collect()
    }

    #[test]
    fn a_missing_forks_dir_is_an_empty_list() {
        let missing = std::env::temp_dir().join(format!("pfs-everywhere-none-{}", std::process::id()));
        assert!(scan(&missing).unwrap().is_empty());
    }

    #[test]
    fn each_main_is_here_gone_or_replaced() {
        let base = scratch("states");
        let forks_dir = base.join("forks");
        let (here, empty, gone, plain, other, unrecorded) = (
            base.join("here"),
            base.join("empty"),
            base.join("gone"),
            base.join("plain"),
            base.join("other"),
            base.join("unrecorded"),
        );
        link(&here, &forks_dir.join("here"));
        link(&empty, &forks_dir.join("empty"));
        write(
            &forks_dir.join("gone").join(MAIN_FILE),
            &format!("{}\n", gone.display()),
        );
        write(&plain.join("notes.txt"), "not a repo");
        write(&forks_dir.join("plain").join(MAIN_FILE), &plain.display().to_string());
        link(&other, &forks_dir.join("other-2"));
        write(&forks_dir.join("other").join(MAIN_FILE), &other.display().to_string());
        fs::create_dir_all(unrecorded.join(".git")).unwrap();
        write(
            &forks_dir.join("unrecorded").join(MAIN_FILE),
            &unrecorded.display().to_string(),
        );
        [
            "here/1",
            "here/3",
            "here/.rm-2",
            "gone/4",
            "plain/1",
            "other/2",
            "nomain/5",
            "junk/x",
            "unrecorded/1",
        ]
        .iter()
        .for_each(|dir| fs::create_dir_all(forks_dir.join(dir)).unwrap());
        write(&forks_dir.join("here").join("README.md"), "whose forks these are");
        write(&forks_dir.join("README.md"), "a file, not a per-repo folder");

        let found = scan(&forks_dir).unwrap();
        fs::remove_dir_all(&base).unwrap();
        let other_2 = forks_dir.join("other-2");
        assert_eq!(
            states(&found),
            vec![
                ("empty".to_string(), State::Here),
                ("gone".to_string(), State::Gone),
                ("here".to_string(), State::Here),
                ("nomain".to_string(), State::Unknown),
                ("other".to_string(), State::Replaced(Replaced::OtherRepo(Some(other_2)))),
                ("other-2".to_string(), State::Here),
                ("plain".to_string(), State::Replaced(Replaced::NotARepo)),
                ("unrecorded".to_string(), State::Replaced(Replaced::OtherRepo(None))),
            ]
        );
        let here_forks = &found[2].forks;
        assert_eq!(here_forks.iter().map(|fork| fork.id).collect::<Vec<_>>(), vec![1, 3]);
        assert!(here_forks.iter().all(|fork| fork.work.is_none()));
        assert_eq!(found[2].main, Some(here));
        assert_eq!(found[3].main, None);
        assert_eq!(found[1].forks[0].work, Some(Work::default()));
    }

    #[test]
    fn here_compares_resolved_paths() {
        let base = scratch("resolved");
        fs::create_dir_all(base.join("real").join("forks").join("repo")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("tmp")).unwrap();
        let folder = base.join("tmp").join("forks").join("repo");
        let main = base.join("repo");
        let resolved = folder.canonicalize().unwrap();
        link(&main, &resolved);
        let via_dots = base.join("tmp").join("forks").join("..").join("forks").join("repo");
        let states = (
            state(&folder, &main),
            state(&via_dots, &main),
            state(&base.join("tmp").join("forks").join("repo-2"), &main),
        );
        fs::remove_dir_all(&base).unwrap();
        assert_ne!(folder, resolved);
        assert_eq!(states.0, State::Here);
        assert_eq!(states.1, State::Here);
        assert_eq!(states.2, State::Replaced(Replaced::OtherRepo(Some(resolved))));
    }

    #[test]
    fn work_is_what_only_the_forks_branch_has() {
        let base = scratch("work");
        let forks_dir = base.join("forks");
        let folder = forks_dir.join("demo");
        write(
            &folder.join(MAIN_FILE),
            &format!("{}\n", base.join("moved-away").display()),
        );

        let three = folder.join("3");
        fs::create_dir_all(&three).unwrap();
        git(&three, &["init", "-q", "-b", "main"]);
        commit(&three, "one");
        commit(&three, "two");
        git(&three, &["branch", "pfs/2-old"]);
        git(&three, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&three, &["checkout", "-q", "-b", "pfs/3-wait"]);
        commit(&three, "three");
        commit(&three, "four");
        write(&three.join("wip.txt"), "not committed");

        let four = folder.join("4");
        fs::create_dir_all(&four).unwrap();
        git(&four, &["init", "-q", "-b", "main"]);
        commit(&four, "one");
        git(&four, &["checkout", "-q", "-b", "pfs/4"]);

        let pushed = folder.join("6");
        fs::create_dir_all(&pushed).unwrap();
        git(&pushed, &["init", "-q", "-b", "main"]);
        commit(&pushed, "one");
        git(&pushed, &["checkout", "-q", "-b", "pfs/6-shipped"]);
        commit(&pushed, "two");
        commit(&pushed, "three");
        git(&pushed, &["update-ref", "refs/remotes/origin/pfs/6-shipped", "HEAD~1"]);
        git(&pushed, &["checkout", "-q", "main"]);

        write(&folder.join("5").join("notes.txt"), "a folder, not a repo");

        let found = scan(&forks_dir).unwrap();
        fs::remove_dir_all(&base).unwrap();
        let works: Vec<(u32, Work)> = found[0]
            .forks
            .iter()
            .map(|fork| (fork.id, fork.work.clone().unwrap()))
            .collect();
        let work = |branch: Option<&str>, commits, uncommitted| Work {
            branch: branch.map(str::to_string),
            commits,
            uncommitted,
        };
        assert_eq!(
            works,
            vec![
                (3, work(Some("pfs/3-wait"), Some(2), Some(true))),
                (4, work(Some("pfs/4"), Some(0), Some(false))),
                (5, work(None, None, None)),
                (6, work(Some("pfs/6-shipped"), Some(1), Some(false))),
            ]
        );
        let lines: Vec<String> = found[0].forks.iter().map(fork_line).collect();
        assert_eq!(
            lines,
            vec![
                "3-wait (2 commits · uncommitted)",
                "4 (no work)",
                "5 (?)",
                "6-shipped (1 commit)"
            ]
        );
    }

    #[test]
    fn a_forks_own_branch_is_its_number_then_a_dash() {
        assert!(is_own("pfs/3-wait", 3));
        assert!(is_own("pfs/3", 3));
        assert!(!is_own("pfs/30-x", 3));
        assert!(!is_own("pfs/3x", 3));
        assert!(!is_own("main", 3));
        assert_eq!(only([1]), Some(1));
        assert_eq!(only([1, 2]), None);
        assert_eq!(only(Vec::<u32>::new()), None);
    }

    #[test]
    fn records_hold_one_absolute_path() {
        assert_eq!(
            first_path("/Users/u/src/demo\n"),
            Some(PathBuf::from("/Users/u/src/demo"))
        );
        assert_eq!(first_path("/a b/c\r\n"), Some(PathBuf::from("/a b/c")));
        assert_eq!(first_path("src/demo\n"), None);
        assert_eq!(first_path(""), None);
    }

    #[test]
    fn paths_paste_into_a_shell() {
        assert_eq!(shell_path("~/.pfs/forks/demo/3"), "~/.pfs/forks/demo/3");
        assert_eq!(shell_path("~/.pfs/forks/My Demo/3"), "~/'.pfs/forks/My Demo/3'");
        assert_eq!(shell_path("/Volumes/x/it's/3"), r"'/Volumes/x/it'\''s/3'");
    }

    fn show(path: &Path) -> String {
        match path.strip_prefix("/Users/u") {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }

    fn fork(id: u32, folder: &str, work: Option<(Option<&str>, Option<u32>, Option<bool>)>) -> Found {
        Found {
            id,
            dir: PathBuf::from(folder).join(id.to_string()),
            work: work.map(|(branch, commits, uncommitted)| Work {
                branch: branch.map(str::to_string),
                commits,
                uncommitted,
            }),
        }
    }

    fn per_repo(key: &str, main: Option<&str>, state: State, forks: Vec<Found>) -> PerRepo {
        PerRepo {
            folder: PathBuf::from("/Users/u/.pfs/forks").join(key),
            main: main.map(PathBuf::from),
            state,
            forks,
        }
    }

    #[test]
    fn renders_every_main_and_what_to_do_about_the_ones_that_arent_here() {
        let demo = "/Users/u/.pfs/forks/demo";
        let repos = vec![
            per_repo(
                "demo",
                Some("/Users/u/src/demo"),
                State::Gone,
                vec![
                    fork(3, demo, Some((Some("pfs/3-wait"), Some(2), Some(true)))),
                    fork(4, demo, Some((Some("pfs/4"), Some(0), Some(false)))),
                    fork(5, demo, Some((None, None, None))),
                ],
            ),
            per_repo("empty", Some("/Users/u/src/empty"), State::Here, vec![]),
            per_repo(
                "pfs-cli",
                Some("/Users/u/code/tools/pfs-cli"),
                State::Here,
                vec![fork(18, "", None), fork(19, "", None)],
            ),
            per_repo(
                "web",
                Some("/Users/u/src/web"),
                State::Replaced(Replaced::OtherRepo(Some(PathBuf::from("/Users/u/.pfs/forks/web-2")))),
                vec![fork(
                    1,
                    "/Users/u/.pfs/forks/web",
                    Some((Some("pfs/1"), Some(0), Some(false))),
                )],
            ),
            per_repo("x", None, State::Unknown, vec![]),
        ];
        assert_eq!(
            render(Path::new("/Users/u/.pfs/forks"), &repos, show),
            vec![
                "forks in ~/.pfs/forks",
                "demo      ~/src/demo             3 forks",
                "  its folder is gone: it may have moved. running any pfs command in the moved main relinks these forks to it",
                "  3-wait (2 commits · uncommitted)",
                "  4 (no work)",
                "  5 (?)",
                "  this work exists only in ~/.pfs/forks/demo: keep a fork's by committing it and pushing or fetching \
                 its branch somewhere (git -C ~/.pfs/forks/demo/3 push <remote or repo> pfs/3-wait), or drop it all \
                 by deleting ~/.pfs/forks/demo. pfs itself never deletes these",
                "pfs-cli   ~/code/tools/pfs-cli   2 forks",
                "web       ~/src/web              1 fork",
                "  its folder was replaced: the repo there keeps its forks in ~/.pfs/forks/web-2, so it or this \
                 folder was moved or copied",
                "  1 (no work)",
                "  no fork in it has work: drop them by deleting ~/.pfs/forks/web. pfs itself never deletes these",
                "x         ?                      no forks",
                "  no record of whose forks these are: ~/.pfs/forks/x/.main is missing",
                "  nothing in it: drop it by deleting ~/.pfs/forks/x. pfs itself never deletes these",
            ]
        );
    }

    #[test]
    fn a_fork_pfs_couldnt_check_is_the_one_to_keep_and_has_no_branch_to_name() {
        let folder = "/Users/u/.pfs/forks/demo";
        let repo = per_repo(
            "demo",
            Some("/Users/u/src/demo"),
            State::Replaced(Replaced::NotARepo),
            vec![
                fork(2, folder, Some((Some("pfs/2-a"), Some(0), Some(false)))),
                fork(7, folder, None),
            ],
        );
        let lines = details(&repo, &show);
        assert_eq!(lines[0], "its folder was replaced: something else is there, not a repo");
        assert_eq!(lines[2], "7 (?)");
        assert!(lines[3].contains("(git -C ~/.pfs/forks/demo/7 push <remote or repo> <its branch>)"));
    }

    #[test]
    fn nothing_to_show_says_so() {
        let here_and_empty = [per_repo("a", Some("/Users/u/a"), State::Here, vec![])];
        assert_eq!(
            render(Path::new("/Users/u/.pfs/forks"), &here_and_empty, show),
            vec!["no forks in ~/.pfs/forks"]
        );
        assert_eq!(
            render(Path::new("/Volumes/x/forks"), &[], show),
            vec!["no forks in /Volumes/x/forks"]
        );
        let no_record = per_repo(
            "a",
            Some("/Users/u/a"),
            State::Replaced(Replaced::OtherRepo(None)),
            vec![],
        );
        assert_eq!(
            details(&no_record, &show)[0],
            "its folder was replaced: the repo there has no record of these forks"
        );
    }
}
