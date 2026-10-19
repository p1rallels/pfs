use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "pfs",
    version,
    about = "Forks of a folder, for you and your agents",
    after_help = "Forks live outside the repo, in <forks dir>/<repo>/<n>; `pfs path <n>` prints one. \
                  The forks dir is $PFS_HOME/forks ($PFS_HOME defaults to ~/.pfs), or [forks] dir in \
                  $PFS_HOME/config.toml. Inside a fork the number is optional."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Fork the main folder (or a fork, with --of): an instant copy on its own branch")]
    New(NewArgs),
    #[command(
        alias = "list",
        about = "List active forks: work to land, what's running; * marks the one you're in. Outside a repo: every repo's forks"
    )]
    Ls(LsArgs),
    #[command(about = "Print a fork's base port (the rest of its block follows)")]
    Port(PortArgs),
    #[command(about = "Print a fork's folder, for cd \"$(pfs path 5)\" and editors")]
    Path(PathArgs),
    #[command(about = "Open a shell in a fork, $PORT set (exit to leave)")]
    Open(OpenArgs),
    #[command(about = "Run a command in a fork, $PORT set")]
    Run(RunArgs),
    #[command(about = "Show a background run's output")]
    Logs(LogsArgs),
    #[command(
        name = "wait",
        about = "Wait until forks' background runs (or the runs named) have ended",
        after_help = "A fork waits on the bg runs going when wait starts; a run is waited on, \
                      and reported, even if it already ended. \
                      Exits 0 when every run waited on ended with 0, 1 if any failed or died, 124 on timeout."
    )]
    Settle(SettleArgs),
    #[command(about = "What a fork changed since it was created, uncommitted included")]
    Diff(DiffArgs),
    #[command(
        about = "Save each fork's branch to the main repo, then delete the fork",
        after_help = "Refuses a fork while anyone works in it (a process with its folder there, a pfs run still going), \
                      while a conversation there is mid-turn, or while it has uncommitted changes. \
                      --force skips the last two, never the first, and nothing is ever killed."
    )]
    Rm(RmArgs),
    #[command(name = "__wait", hide = true)]
    Wait(WaitArgs),
}

#[derive(Debug, Args)]
pub struct NewArgs {
    #[arg(value_name = "NAME", help = "Label, e.g. add-csv (pfs picks the number)")]
    pub name: Option<String>,

    #[arg(
        long,
        value_name = "REV",
        conflicts_with = "of",
        help = "Start clean at this commit (or a live fork's pfs/<n>-… branch) instead of copying the main folder as-is"
    )]
    pub from: Option<String>,

    #[arg(
        long,
        value_name = "N",
        help = "Copy fork N as it is, uncommitted work included, instead of the main folder"
    )]
    pub of: Option<String>,

    #[arg(long, conflicts_with = "quiet", help = "Open a shell in the new fork right away")]
    pub open: bool,

    #[arg(
        short,
        long,
        help = "Print only the fork number (notes go to stderr), for n=$(pfs new x -q)"
    )]
    pub quiet: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(long, help = "Include removed forks")]
    pub all: bool,

    #[arg(long, help = "One JSON object per fork")]
    pub json: bool,

    #[arg(
        long,
        help = "Then list files two or more forks changed since they started, with those forks (for a merge order)"
    )]
    pub overlap: bool,
}

#[derive(Debug, Args)]
pub struct PortArgs {
    #[arg(value_name = "N", help = "Fork number; defaults to the fork you're in")]
    pub fork: Option<String>,
}

#[derive(Debug, Args)]
pub struct PathArgs {
    #[arg(value_name = "N", help = "Fork number; defaults to the fork you're in")]
    pub fork: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenArgs {
    #[arg(value_name = "N", help = "Fork number; defaults to the fork you're in")]
    pub fork: Option<String>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, help = "Run in the background; output goes to a log")]
    pub bg: bool,

    #[arg(short, long, help = "With --bg, print only the run's <n>.<run>")]
    pub quiet: bool,

    #[arg(
        value_name = "[N] CMD",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Fork number (optional inside a fork), then the command. One quoted argument runs through sh"
    )]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    #[arg(
        value_name = "N[.RUN]",
        help = "Fork, or fork.run; defaults to your fork's latest run"
    )]
    pub target: Option<String>,

    #[arg(short, long, help = "Keep printing until the run ends")]
    pub follow: bool,
}

#[derive(Debug, Args)]
pub struct SettleArgs {
    #[arg(
        value_name = "N[.RUN]",
        help = "Forks, or single bg runs (what `pfs run --bg -q` prints); \
                defaults to the fork you're in, or from the main folder every active fork"
    )]
    pub targets: Vec<String>,

    #[arg(long, value_name = "SECS", help = "Give up after this many seconds (exit 124)")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Args)]
pub struct DiffArgs {
    #[arg(
        value_name = "[N] [GIT DIFF ARGS]",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Fork number (optional inside a fork), then e.g. --stat or -- <paths>"
    )]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RmArgs {
    #[arg(
        value_name = "N",
        required = true,
        help = "Fork numbers; each is checked and removed on its own, in this order"
    )]
    pub forks: Vec<String>,

    #[arg(
        long,
        help = "Remove even with uncommitted changes or a conversation left mid-turn (a crash leaves one open). \
                Never while anyone works in the fork, and never kills anything"
    )]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    #[arg(long)]
    pub fork_dir: PathBuf,

    #[arg(long)]
    pub cwd: PathBuf,

    #[arg(long)]
    pub run: u32,

    #[arg(long)]
    pub kind: String,

    #[arg(long)]
    pub label: String,

    #[arg(last = true, required = true)]
    pub argv: Vec<OsString>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls(argv: &[&str]) -> Option<LsArgs> {
        match Cli::try_parse_from(["pfs", "ls"].iter().chain(argv)).ok()?.command {
            Command::Ls(args) => Some(args),
            _ => None,
        }
    }

    #[test]
    fn ls_overlap_goes_with_the_other_flags() {
        let args = ls(&["--overlap", "--json", "--all"]).unwrap();
        assert!(args.overlap && args.json && args.all);
        assert!(!ls(&[]).unwrap().overlap);
    }

    fn rm(argv: &[&str]) -> Option<RmArgs> {
        match Cli::try_parse_from(["pfs", "rm"].iter().chain(argv)).ok()?.command {
            Command::Rm(args) => Some(args),
            _ => None,
        }
    }

    #[test]
    fn rm_takes_several_forks_and_force_anywhere() {
        let args = rm(&["3", "5", "7", "--force"]).unwrap();
        assert_eq!(args.forks, ["3", "5", "7"]);
        assert!(args.force);
        let args = rm(&["--force", "3-csv"]).unwrap();
        assert_eq!(args.forks, ["3-csv"]);
        assert!(args.force);
        assert!(!rm(&["4"]).unwrap().force);
        assert!(rm(&[]).is_none());
    }

    fn wait(argv: &[&str]) -> Option<SettleArgs> {
        match Cli::try_parse_from(["pfs", "wait"].iter().chain(argv)).ok()?.command {
            Command::Settle(args) => Some(args),
            _ => None,
        }
    }

    #[test]
    fn wait_takes_forks_and_runs() {
        let args = wait(&["5", "6.2", "--timeout", "3"]).unwrap();
        assert_eq!(args.targets, ["5", "6.2"]);
        assert_eq!(args.timeout, Some(3));
        assert!(wait(&[]).unwrap().targets.is_empty());
    }

    #[test]
    fn path_takes_an_optional_fork() {
        let path = |argv: &[&str]| match Cli::try_parse_from(["pfs", "path"].iter().chain(argv)).ok()?.command {
            Command::Path(args) => Some(args.fork),
            _ => None,
        };
        assert_eq!(path(&["5"]), Some(Some("5".to_string())));
        assert_eq!(path(&[]), Some(None));
        assert_eq!(path(&["5", "6"]), None);
    }

    #[test]
    fn allow_is_gone() {
        assert!(Cli::try_parse_from(["pfs", "allow"]).is_err());
        assert!(Cli::try_parse_from(["pfs", "new", "x", "--allow", "/tmp"]).is_err());
        assert!(Cli::try_parse_from(["pfs", "new", "x"]).is_ok());
    }
}
