use anyhow::Result;

use crate::{cli::PortArgs, config, place};

/// Just the number, for scripts: `PORT=$(pfs port) bun dev`.
pub fn run(args: PortArgs) -> Result<i32> {
    let place = place::here()?;
    let id = place.target(args.fork.as_deref())?;
    println!("{}", config::load_global()?.ports.base_port(id));
    Ok(0)
}
