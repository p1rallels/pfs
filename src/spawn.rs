//! Starting things in a fork. Foreground runs are waited on by pfs itself.
//! Background runs get a waiter: a tiny `pfs __wait` process in its own
//! session, out of reach of a harness's process-group kill (pi's esc and
//! timeouts), terminal hangups and harness cleanup. It starts the command, records the start, waits, and
//! records the exit code, which macos only hands to a process's parent.

use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    cli::WaitArgs,
    clock, proc,
    runs::{self, Kind, Start},
    store,
};

#[derive(Debug, Clone)]
pub struct Job {
    pub fork_dir: PathBuf,
    pub cwd: PathBuf,
    pub argv: Vec<OsString>,
    /// What `pfs ls` and the run record show.
    pub label: String,
    pub kind: Kind,
    pub port: u32,
}

pub struct Started {
    pub run: u32,
    pub pid: i32,
}

fn command(job: &Job) -> Result<Command> {
    let (program, args) = job.argv.split_first().ok_or_else(|| anyhow!("empty command"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&job.cwd)
        .env("PORT", job.port.to_string());
    Ok(command)
}

/// Runs attached to your terminal; returns the command's exit code.
pub fn foreground(job: &Job) -> Result<i32> {
    runs::ensure_dir(&job.fork_dir)?;
    let (run, mut child) = {
        let _lock = store::lock(&runs::lock_path(&job.fork_dir))?;
        let run = runs::next_number(&job.fork_dir)?;
        let child = command(job)?
            .spawn()
            .with_context(|| format!("can't start {}", job.label))?;
        let pid = child.id() as i32;
        if let Err(error) = runs::record(&job.fork_dir, &start_event(job, run, pid)) {
            eprintln!("pfs: couldn't record run {run}: {error:#}");
        }
        (run, child)
    };
    ignore_terminal_interrupts();
    let status = child.wait().context("lost track of the command")?;
    if let Err(error) = runs::record(&job.fork_dir, &runs::exit_event(run, status)) {
        eprintln!("pfs: couldn't record how run {run} ended: {error:#}");
    }
    Ok(exit_code(status))
}

/// Hands the command to a waiter and returns once it has started.
pub fn background(job: &Job) -> Result<Started> {
    runs::ensure_dir(&job.fork_dir)?;
    let _lock = store::lock(&runs::lock_path(&job.fork_dir))?;
    let run = runs::next_number(&job.fork_dir)?;
    let mut waiter = waiter_command(job, run)?.spawn().context("can't start the waiter")?;
    let stdout = waiter.stdout.take().ok_or_else(|| anyhow!("waiter has no stdout"))?;
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .context("can't read the waiter's handshake")?;
    match line.trim().split_once(' ') {
        Some(("ok", pid)) => Ok(Started {
            run,
            pid: pid.parse().context("bad pid from the waiter")?,
        }),
        Some(("err", message)) => {
            let _ = waiter.wait();
            bail!("{message}")
        }
        _ => {
            let _ = waiter.wait();
            bail!("the waiter exited before starting {}", job.label)
        }
    }
}

/// The waiter stands in the run's folder, so `pfs rm` counts it with its
/// run, not as someone in whatever folder `pfs run` was typed in.
fn waiter_command(job: &Job, run: u32) -> Result<Command> {
    let exe = std::env::current_exe().context("can't find the pfs binary")?;
    let mut command = Command::new(exe);
    command
        .current_dir(&job.cwd)
        .arg("__wait")
        .arg("--fork-dir")
        .arg(&job.fork_dir)
        .arg("--cwd")
        .arg(&job.cwd)
        .arg("--run")
        .arg(run.to_string())
        .arg("--kind")
        .arg(job.kind.as_str())
        .arg("--label")
        .arg(&job.label)
        .arg("--")
        .args(&job.argv)
        .env("PORT", job.port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    Ok(command)
}

/// The waiter itself (`pfs __wait`). Reports `ok <pid>` or `err <why>` on
/// stdout, then lets go of it and waits for the command.
pub fn wait_main(args: WaitArgs) -> Result<i32> {
    let mut stdout = io::stdout();
    let child = Kind::parse(&args.kind)
        .ok_or_else(|| anyhow!("unknown run kind {}", args.kind))
        .and_then(|kind| start_child(&args, kind));
    match child {
        Err(error) => {
            let _ = writeln!(stdout, "err {}", format!("{error:#}").replace('\n', " "));
            let _ = stdout.flush();
            Ok(1)
        }
        Ok(mut child) => {
            writeln!(stdout, "ok {}", child.id())?;
            stdout.flush()?;
            silence_stdout();
            let status = child.wait().context("lost track of the command")?;
            runs::record(&args.fork_dir, &runs::exit_event(args.run, status))?;
            Ok(0)
        }
    }
}

fn start_child(args: &WaitArgs, kind: Kind) -> Result<Child> {
    let (program, rest) = args.argv.split_first().ok_or_else(|| anyhow!("empty command"))?;
    let log_path = runs::log_path(&args.fork_dir, args.run);
    let log = File::create(&log_path).with_context(|| format!("can't create {}", log_path.display()))?;
    let mut child = Command::new(program)
        .args(rest)
        .current_dir(&args.cwd)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("can't start {}", args.label))?;
    let pid = child.id() as i32;
    let start = runs::Event::Start(Start {
        run: args.run,
        kind,
        cmd: args.label.clone(),
        pid,
        proc_start: proc::start_time(pid),
        ts: clock::now(),
    });
    if let Err(error) = runs::record(&args.fork_dir, &start) {
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

fn start_event(job: &Job, run: u32, pid: i32) -> runs::Event {
    runs::Event::Start(Start {
        run,
        kind: job.kind,
        cmd: job.label.clone(),
        pid,
        proc_start: proc::start_time(pid),
        ts: clock::now(),
    })
}

fn silence_stdout() {
    if let Ok(null) = File::options().write(true).open("/dev/null") {
        unsafe { libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO) };
    }
}

/// While a foreground command runs, ctrl-c is for the command. pfs stays to
/// record how it ended. Set after spawning so the command keeps the defaults.
fn ignore_terminal_interrupts() {
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}
