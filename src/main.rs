mod cli;
mod clock;
mod clone;
mod cmd;
mod config;
mod forks;
mod git;
mod harness;
mod layout;
mod ledger;
mod names;
mod occupants;
mod paths;
mod place;
mod ports;
mod proc;
mod runs;
mod spawn;
mod store;
mod venv;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // A closed pipe (`pfs ls | head -1`) ends pfs quietly, like any unix tool.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let cli = cli::Cli::parse();
    match cmd::dispatch(cli.command) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) if is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pfs: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}
