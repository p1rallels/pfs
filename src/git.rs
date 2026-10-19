//! Thin wrappers over the git cli. pfs never parses git's internals.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

/// Stat checks that survive an apfs clone: clones get new inodes and ctimes,
/// so git's default check would rehash every tracked file once per fork.
const CLONE_FRIENDLY_STAT: [&str; 4] = ["-c", "core.checkStat=minimal", "-c", "core.trustCtime=false"];

fn command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn run(mut command: Command, what: &str) -> Result<String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("can't run git ({what})"))?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string()),
        false => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {what} failed: {}", stderr.trim())
        }
    }
}

pub fn out(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = command(dir);
    command.args(args);
    run(command, &args.join(" "))
}

fn out_with_index(dir: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let mut command = command(dir);
    command
        .args(CLONE_FRIENDLY_STAT)
        .args(args)
        .env("GIT_INDEX_FILE", index);
    run(command, &args.join(" "))
}

/// Attached to the terminal (output, pager), for `pfs diff`.
pub fn passthrough(dir: &Path, args: &[&str]) -> Result<ExitStatus> {
    command(dir)
        .args(args)
        .status()
        .with_context(|| format!("can't run git {}", args.first().unwrap_or(&"")))
}

/// `None` on an unborn branch (a repo with no commits yet).
pub fn head(dir: &Path) -> Option<String> {
    out(dir, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]).ok()
}

pub fn commit(dir: &Path, rev: &str) -> Result<String> {
    out(dir, &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")]).map_err(|_| anyhow!("not a commit: {rev}"))
}

pub fn tree_of(dir: &Path, rev: &str) -> Option<String> {
    out(dir, &["rev-parse", "--verify", "-q", &format!("{rev}^{{tree}}")]).ok()
}

/// Untracked files count, ignored ones don't.
pub fn is_dirty(dir: &Path) -> Result<bool> {
    let status = out(dir, &["status", "--porcelain", "--untracked-files=normal"])?;
    Ok(!status.is_empty())
}

pub fn current_branch(dir: &Path) -> Option<String> {
    out(dir, &["symbolic-ref", "-q", "--short", "HEAD"]).ok()
}

pub fn branch_tip(dir: &Path, branch: &str) -> Option<String> {
    out(
        dir,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
    .ok()
}

pub fn branches(dir: &Path, prefix: &str) -> Vec<String> {
    out(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}"),
        ],
    )
    .map(|text| text.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

pub fn is_ancestor(dir: &Path, older: &str, newer: &str) -> bool {
    out(dir, &["merge-base", "--is-ancestor", older, newer]).is_ok()
}

/// The tree of the whole working folder (tracked + untracked, minus ignored),
/// built in a scratch index so the real one is never touched.
pub fn worktree_tree(dir: &Path, scratch: &Path) -> Result<String> {
    let real_index = git_dir(dir).join("index");
    let _ = fs::remove_file(scratch);
    if real_index.is_file() {
        fs::copy(&real_index, scratch).context("can't copy the git index")?;
    }
    let tree = out_with_index(dir, scratch, &["add", "-A"]).and_then(|_| out_with_index(dir, scratch, &["write-tree"]));
    let _ = fs::remove_file(scratch);
    tree
}

pub fn git_dir(dir: &Path) -> PathBuf {
    dir.join(".git")
}

/// Points a ref at any object (pfs keeps start snapshots as trees).
pub fn set_ref(dir: &Path, name: &str, object: &str) -> Result<()> {
    out(dir, &["update-ref", name, object]).map(|_| ())
}

pub fn ref_target(dir: &Path, name: &str) -> Option<String> {
    out(dir, &["rev-parse", "--verify", "-q", name]).ok()
}

/// Paths that differ between two trees (`from: None` is an unborn branch,
/// so every path in `to`). Names only: no content diff, nothing run.
pub fn changed_paths(dir: &Path, from: Option<&str>, to: &str) -> Result<Vec<String>> {
    let listed = match from {
        Some(from) => out(dir, &["diff-tree", "-r", "-z", "--no-renames", "--name-only", from, to])?,
        None => out(dir, &["ls-tree", "-r", "-z", "--name-only", to])?,
    };
    Ok(listed
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn count_commits(dir: &Path, revs: &[&str]) -> Result<u32> {
    let args: Vec<&str> = ["rev-list", "--count"]
        .into_iter()
        .chain(revs.iter().copied())
        .chain(["--"])
        .collect();
    let count = out(dir, &args)?;
    count
        .parse()
        .with_context(|| format!("git rev-list --count said {count:?}"))
}

/// `--no-track`: a fork's branch starts where it's told and follows nothing.
/// Its work lands through `pfs rm`, not a pull or a push.
pub fn switch_new_branch(dir: &Path, branch: &str) -> Result<()> {
    match head(dir) {
        Some(_) => out(dir, &["checkout", "-q", "--no-track", "-b", branch]).map(|_| ()),
        None => out(dir, &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")]).map(|_| ()),
    }
}

/// A clean start at `rev`: tracked changes and untracked files go, ignored
/// files (deps, .env) stay.
pub fn switch_clean(dir: &Path, branch: &str, rev: &str) -> Result<()> {
    out(dir, &["checkout", "-q", "-f", "--no-track", "-b", branch, rev])?;
    out(dir, &["clean", "-q", "-f", "-d"]).map(|_| ())
}

/// Copies `branch` from another repo on disk into this one.
pub fn fetch_branch(into: &Path, from: &Path, branch: &str) -> Result<()> {
    let spec = format!("refs/heads/{branch}:refs/heads/{branch}");
    fetch(into, from, &spec)
}

/// Fetches `branch` of the repo at `from` into this one and returns its tip.
pub fn fetch_tip(into: &Path, from: &Path, branch: &str) -> Result<String> {
    const INCOMING: &str = "refs/pfs/incoming";
    fetch(into, from, &format!("+refs/heads/{branch}:{INCOMING}"))
        .map_err(|_| anyhow!("couldn't copy {branch} from {}", from.display()))?;
    let tip = commit(into, INCOMING)?;
    out(into, &["update-ref", "-d", INCOMING])?;
    Ok(tip)
}

fn fetch(into: &Path, from: &Path, spec: &str) -> Result<()> {
    let from = from.to_string_lossy();
    out(
        into,
        &["fetch", "-q", "--no-tags", "--no-write-fetch-head", &from, spec],
    )
    .map(|_| ())
}

/// Files a merge of `theirs` into `ours` would conflict on, worked out
/// without touching the folder. `None` when git can't tell.
pub fn merge_conflicts(dir: &Path, ours: &str, theirs: &str) -> Option<Vec<String>> {
    let output = command(dir)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            ours,
            theirs,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => Some(Vec::new()),
        Some(1) => Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .skip(1)
                .take_while(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

/// Paths with work the folder hasn't committed: changed since HEAD (staged or
/// not), or untracked and not ignored.
pub fn uncommitted_paths(dir: &Path) -> Result<Vec<String>> {
    let changed = out(dir, &["diff", "-z", "--no-renames", "--name-only", "HEAD"])?;
    let untracked = out(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    Ok(nul_list(&changed).chain(nul_list(&untracked)).collect())
}

/// Paths `theirs` changed since it split from `ours`: what merging it writes.
pub fn incoming_paths(dir: &Path, ours: &str, theirs: &str) -> Result<Vec<String>> {
    let range = format!("{ours}...{theirs}");
    let listed = out(dir, &["diff", "-z", "--no-renames", "--name-only", &range])?;
    Ok(nul_list(&listed).collect())
}

fn nul_list(listed: &str) -> impl Iterator<Item = String> + '_ {
    listed.split('\0').filter(|path| !path.is_empty()).map(str::to_string)
}
