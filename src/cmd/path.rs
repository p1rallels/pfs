use anyhow::Result;

use crate::{cli::PathArgs, forks, ledger, place};

/// The folder alone, absolute, for scripts and editors: `cd "$(pfs path 5)"`.
pub fn run(args: PathArgs) -> Result<i32> {
    let place = place::here()?;
    let id = place.target(args.fork.as_deref())?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    println!("{}", fork.dir.display());
    Ok(0)
}
