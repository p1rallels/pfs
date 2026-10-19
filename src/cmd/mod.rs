mod diff;
mod logs;
mod ls;
mod new;
mod open;
mod path;
mod port;
mod rm;
mod run;
mod settle;

use anyhow::Result;

use crate::{cli::Command, spawn};

/// Every command returns the exit code pfs should end with.
pub fn dispatch(command: Command) -> Result<i32> {
    match command {
        Command::New(args) => new::run(args),
        Command::Ls(args) => ls::run(args),
        Command::Port(args) => port::run(args),
        Command::Path(args) => path::run(args),
        Command::Open(args) => open::run(args),
        Command::Run(args) => run::run(args),
        Command::Logs(args) => logs::run(args),
        Command::Settle(args) => settle::run(args),
        Command::Diff(args) => diff::run(args),
        Command::Rm(args) => rm::run(args),
        Command::Wait(args) => spawn::wait_main(args),
    }
}
