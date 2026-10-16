use anyhow::Result;

use crate::{
    cli::OpenArgs,
    config,
    forks::{self, Fork, Plan},
    ledger,
    place::{self, Place},
    ports,
    runs::Kind,
    spawn,
};

pub fn run(args: OpenArgs) -> Result<i32> {
    let place = place::here()?;
    let id = place.target(args.fork.as_deref())?;
    launch(&place, id)
}

/// The door: your shell in the fork with $PORT set. Like `cd .forks/<n>`
/// plus $PORT, and nothing else: what you run in there is yours, and the door
/// knows nothing about it.
pub fn launch(place: &Place, id: u32) -> Result<i32> {
    let config = config::load_global()?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    let shell = shell();
    let plan = Plan {
        argv: vec![shell.clone().into()],
        label: format!("{shell} (open)"),
        kind: Kind::Open,
    };
    let job = forks::job(place, &config, &fork, plan);
    note_running(&fork);
    eprintln!(
        "fork {} · branch {} · ports {} · exit to leave",
        fork.label(),
        fork.branch(),
        ports::show(config.ports.block(id))
    );
    spawn::foreground(&job)
}

fn shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string())
}

/// What pfs has going in the fork, from its own run records, so you don't
/// walk in on someone's work unawares.
fn note_running(fork: &Fork) {
    let running: Vec<String> = forks::running(&fork.dir)
        .iter()
        .map(|run| {
            format!(
                "{}.{} {} (pid {})",
                fork.id,
                run.start.run,
                run.start.kind.as_str(),
                run.start.pid
            )
        })
        .collect();
    if !running.is_empty() {
        eprintln!("note: still running in this fork: {}", running.join(", "));
    }
}
