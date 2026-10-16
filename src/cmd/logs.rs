use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    cli::LogsArgs,
    forks, ledger,
    place::{self, Place},
    proc,
    runs::{self, Run},
};

const POLL: Duration = Duration::from_millis(250);

pub fn run(args: LogsArgs) -> Result<i32> {
    let place = place::here()?;
    let (id, number) = target(&place, args.target.as_deref())?;
    let ledger = ledger::load(&place.repo)?;
    let fork = forks::require(&place.repo, &ledger, id)?;
    let runs = runs::load(&fork.dir)?;
    let run = match number {
        Some(number) => runs
            .iter()
            .find(|run| run.start.run == number)
            .ok_or_else(|| anyhow!("fork {id} has no run {id}.{number}"))?,
        None => runs
            .iter()
            .rev()
            .find(|run| run.start.kind.has_log())
            .ok_or_else(|| anyhow!("fork {id} has no background runs yet"))?,
    };
    print_log(&fork.dir, id, run, args.follow)
}

fn target(place: &Place, raw: Option<&str>) -> Result<(u32, Option<u32>)> {
    match raw {
        Some(raw) => place::parse_run_ref(raw).ok_or_else(|| anyhow!("expected <n> or <n>.<run>, got {raw}")),
        None => Ok((place.target(None)?, None)),
    }
}

fn print_log(fork_dir: &Path, id: u32, run: &Run, follow: bool) -> Result<i32> {
    let path = run.log(fork_dir).ok_or_else(|| {
        anyhow!(
            "{id}.{} ran in the foreground, so its output went to the terminal",
            run.start.run
        )
    })?;
    let mut file = File::open(&path).with_context(|| format!("can't open {}", path.display()))?;
    let mut out = io::stdout().lock();
    io::copy(&mut file, &mut out)?;
    out.flush()?;
    match follow {
        true => follow_log(&mut file, &mut out, run),
        false => Ok(0),
    }
}

/// `tail -f` until the run is over and the log stops growing.
fn follow_log(file: &mut File, out: &mut impl Write, run: &Run) -> Result<i32> {
    loop {
        let alive = run.exit.is_none() && proc::alive(run.start.pid, run.start.proc_start);
        thread::sleep(POLL);
        let copied = copy_new(file, out)?;
        if !alive && copied == 0 {
            return Ok(0);
        }
    }
}

fn copy_new(file: &mut File, out: &mut impl Write) -> Result<u64> {
    let position = file.stream_position()?;
    let length = file.metadata()?.len();
    if length < position {
        bail!("the log was truncated");
    }
    file.seek(SeekFrom::Start(position))?;
    let copied = io::copy(&mut file.take(length - position), out)?;
    out.flush()?;
    Ok(copied)
}
