use anyhow::{Result, anyhow};

use crate::{
    cli::DiffArgs,
    forks::{self, START_REF},
    git, ledger, place, spawn,
};

/// Everything the fork changed since it was created: commits, uncommitted
/// edits and new files, against the state it started from (which includes
/// whatever was uncommitted in the main folder at fork time). git runs in
/// the fork, so its pager and diff drivers are the fork's.
pub fn run(args: DiffArgs) -> Result<i32> {
    let place = place::here()?;
    let (given, rest) = forks::split_ref(&args.args);
    let id = place.target(given)?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    let start = forks::start_tree(&fork)
        .ok_or_else(|| anyhow!("can't tell where fork {} started (no {START_REF}, no base)", fork.id))?;
    let now = forks::now_tree(&fork, "diff")?;
    let (options, pathspecs) = match rest.iter().position(|arg| arg == "--") {
        Some(split) => (&rest[..split], &rest[split + 1..]),
        None => (rest, &[] as &[String]),
    };
    let argv: Vec<&str> = std::iter::once("diff")
        .chain(options.iter().map(String::as_str))
        .chain([start.as_str(), now.as_str(), "--"])
        .chain(pathspecs.iter().map(String::as_str))
        .collect();
    let status = git::passthrough(&fork.dir, &argv)?;
    Ok(spawn::exit_code(status))
}
