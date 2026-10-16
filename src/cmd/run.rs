use std::{ffi::OsString, path::Path};

use anyhow::{Result, bail};

use crate::{
    cli::RunArgs,
    config,
    forks::{self, Plan},
    ledger, place,
    runs::Kind,
    spawn,
};

/// A single argument with any of these is a shell snippet: `pfs run 3 'a && b'`.
const SHELL_CHARS: &str = ";&|<>()$`\\\"'*?[]#~={}!";

pub fn run(args: RunArgs) -> Result<i32> {
    let place = place::here()?;
    let (given, rest) = forks::split_ref(&args.args);
    let (inline, command) = strip_flags(rest);
    if command.is_empty() {
        bail!("what should run? pfs run [<n>] [--bg] <cmd...>");
    }
    let background = args.bg || inline.bg;
    let quiet = args.quiet || inline.quiet;
    let id = place.target(given)?;
    let config = config::load_global()?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    let argv = argv(command);
    precheck(&argv, &forks::cwd_for(&place, &fork))?;
    let plan = Plan {
        argv,
        label: command.join(" "),
        kind: match background {
            true => Kind::Bg,
            false => Kind::Fg,
        },
    };
    let job = forks::job(&place, &config, &fork, plan);
    match background {
        true => {
            let started = spawn::background(&job)?;
            println!("{}", started_line(id, &started, quiet));
            Ok(0)
        }
        false => spawn::foreground(&job),
    }
}

/// Quiet is for scripts: `run=$(pfs run 5 --bg -q bun dev)` gets `5.1`.
fn started_line(id: u32, started: &spawn::Started, quiet: bool) -> String {
    match quiet {
        true => format!("{id}.{}", started.run),
        false => format!(
            "started {id}.{run} (pid {pid}) · output: pfs logs {id}.{run}",
            run = started.run,
            pid = started.pid
        ),
    }
}

/// pfs's own flags typed after the fork number.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Inline {
    bg: bool,
    quiet: bool,
}

/// `--bg` and `-q` right after the fork number, and `--` before the command.
fn strip_flags(args: &[String]) -> (Inline, &[String]) {
    match args.split_first() {
        Some((first, rest)) if first == "--bg" => {
            let (flags, command) = strip_flags(rest);
            (Inline { bg: true, ..flags }, command)
        }
        Some((first, rest)) if first == "-q" || first == "--quiet" => {
            let (flags, command) = strip_flags(rest);
            (Inline { quiet: true, ..flags }, command)
        }
        Some((first, rest)) if first == "--" => (Inline::default(), rest),
        _ => (Inline::default(), args),
    }
}

fn argv(command: &[String]) -> Vec<OsString> {
    match command {
        [single] if single.chars().any(|c| c.is_whitespace() || SHELL_CHARS.contains(c)) => {
            vec!["/bin/sh".into(), "-c".into(), single.into()]
        }
        _ => command.iter().map(OsString::from).collect(),
    }
}

/// "command not found" now, rather than as a failed background run.
fn precheck(argv: &[OsString], cwd: &Path) -> Result<()> {
    let Some(program) = argv.first().map(Path::new) else {
        bail!("empty command");
    };
    let found = match program.components().count() > 1 {
        true => cwd.join(program).exists(),
        false => std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
            .unwrap_or(false),
    };
    match found {
        true => Ok(()),
        false => bail!("command not found: {}", program.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn flags_after_the_fork_number() {
        let args = strings(&["--bg", "--", "bun", "dev"]);
        let (flags, command) = strip_flags(&args);
        assert_eq!(flags, Inline { bg: true, quiet: false });
        assert_eq!(command, strings(&["bun", "dev"]).as_slice());
        let args = strings(&["-q", "--bg", "pi", "-p", "-q"]);
        let (flags, command) = strip_flags(&args);
        assert_eq!(flags, Inline { bg: true, quiet: true });
        assert_eq!(command, strings(&["pi", "-p", "-q"]).as_slice());
        let args = strings(&["--", "-q"]);
        assert_eq!(strip_flags(&args), (Inline::default(), strings(&["-q"]).as_slice()));
    }

    #[test]
    fn quiet_prints_the_run_alone() {
        let started = spawn::Started { run: 2, pid: 42 };
        assert_eq!(started_line(5, &started, true), "5.2");
        assert_eq!(
            started_line(5, &started, false),
            "started 5.2 (pid 42) · output: pfs logs 5.2"
        );
    }

    #[test]
    fn one_snippet_goes_through_sh() {
        assert_eq!(argv(&strings(&["bun test && echo ok"]))[0], "/bin/sh");
        assert_eq!(argv(&strings(&["ls"]))[0], "ls");
        assert_eq!(argv(&strings(&["git", "commit", "-m", "fix: a b"])).len(), 4);
    }
}
