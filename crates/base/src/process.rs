//! The one way to run a child process: timeout, cancellation and capped output.
//!
//! Unix only. The child starts in its own process group, so a timeout or a cancel stops the
//! whole tree (a shell, the `cargo` it started, the compilers below that) and not just the
//! direct child. Output is drained on reader threads, so a chatty child never blocks, and only
//! the first and last part of each stream is kept.

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::cancel::CancelToken;

mod capture;
#[cfg(test)]
mod tests;

use capture::Reader;

/// How often the wait loop looks at the child, the clock and the cancel token.
const POLL: Duration = Duration::from_millis(10);
/// Time between SIGTERM and SIGKILL.
const KILL_GRACE: Duration = Duration::from_millis(500);
/// How long the readers get to reach the end of their pipes after the child exited.
const DRAIN_GRACE: Duration = Duration::from_millis(250);
/// Default bytes kept from the start and from the end of each stream.
const DEFAULT_CAP: usize = 15 * 1024;

/// A command to run. Build it with [`Cmd::new`] or [`Cmd::shell`], then [`Cmd::run`] it.
#[derive(Clone, Debug)]
pub struct Cmd {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    env_remove: Vec<OsString>,
    stdin: Option<String>,
    timeout: Option<Duration>,
    head_bytes: usize,
    tail_bytes: usize,
}

/// What a finished (or stopped) command left behind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    /// The exit code; `None` when a signal ended the process (a timeout or a cancel usually).
    pub code: Option<i32>,
    /// Standard output, invalid UTF-8 replaced, capped to head and tail.
    pub stdout: String,
    /// Standard error, same treatment.
    pub stderr: String,
    /// The timeout ran out and the process tree was killed.
    pub timed_out: bool,
    /// The cancel token fired and the process tree was killed.
    pub cancelled: bool,
    /// Part of stdout or stderr was dropped because it exceeded the capture limit.
    pub truncated: bool,
    /// Time from start to the end of the run.
    pub elapsed: Duration,
}

impl Output {
    /// Whether the command ran to the end and exited with code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.cancelled
    }

    /// Stdout, then stderr, for a tool result. A line break is put between them when stdout
    /// does not end with one.
    pub fn combined(&self) -> String {
        let mut text = self.stdout.clone();
        if !text.is_empty() && !self.stderr.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&self.stderr);
        text
    }
}

impl Cmd {
    /// A command that runs `program`, looked up in `PATH`.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            stdin: None,
            timeout: None,
            head_bytes: DEFAULT_CAP,
            tail_bytes: DEFAULT_CAP,
        }
    }

    /// A command that runs `script` with `sh -c`.
    pub fn shell(script: &str) -> Self {
        Self::new("sh").arg("-c").arg(script)
    }

    /// Adds one argument.
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Adds several arguments.
    pub fn args<I, S>(mut self, a: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }

    /// Runs in `dir` instead of the current directory.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Sets an environment variable on top of the inherited environment.
    pub fn env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// Removes an inherited environment variable.
    pub fn env_remove(mut self, k: impl Into<OsString>) -> Self {
        self.env_remove.push(k.into());
        self
    }

    /// Feeds `text` to the child's stdin, then closes it. Without this, stdin is closed from
    /// the start.
    pub fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    /// Kills the process tree after `dur`. There is no timeout by default.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Keeps the first `head_bytes` and the last `tail_bytes` of stdout and of stderr (each
    /// 15 KB by default) and drops the middle, setting [`Output::truncated`].
    pub fn capture_limit(mut self, head_bytes: usize, tail_bytes: usize) -> Self {
        self.head_bytes = head_bytes;
        self.tail_bytes = tail_bytes;
        self
    }

    /// Runs the command to the end, a timeout or a cancel, whichever comes first.
    ///
    /// Fails only when the process cannot be started (program not found, bad directory) or its
    /// pipes cannot be set up; a command that exits non-zero is an `Ok` with its code. A token
    /// that is already cancelled returns at once without starting anything. After a normal
    /// exit, processes left behind that still hold the output pipes are killed.
    pub fn run(self, cancel: &CancelToken) -> io::Result<Output> {
        let started = Instant::now();
        let mut out = Output::default();
        if cancel.is_cancelled() {
            out.cancelled = true;
            return Ok(out);
        }
        let mut child = self.command().spawn()?;
        let pgid = i32::try_from(child.id()).ok().filter(|&p| p > 1);
        let (stdout, stderr) = match self.start_io(&mut child) {
            Ok(io) => io,
            Err(e) => {
                stop(&mut child, pgid);
                return Err(e);
            }
        };

        let deadline = self.timeout.and_then(|d| started.checked_add(d));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(e) => {
                    stop(&mut child, pgid);
                    return Err(e);
                }
            }
            if cancel.is_cancelled() {
                out.cancelled = true;
                break stop(&mut child, pgid);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                out.timed_out = true;
                break stop(&mut child, pgid);
            }
            std::thread::sleep(POLL);
        };
        out.code = status.and_then(|s| s.code());

        // The pipes close with the last process of the tree. If something still holds them
        // after the grace period, it is a leftover of this command: stop it.
        if !wait_finished(&[&stdout, &stderr], DRAIN_GRACE) {
            stop(&mut child, pgid);
            wait_finished(&[&stdout, &stderr], KILL_GRACE);
        }
        let (so, to) = stdout.take();
        let (se, te) = stderr.take();
        out.stdout = so;
        out.stderr = se;
        out.truncated = to || te;
        out.elapsed = started.elapsed();
        Ok(out)
    }

    /// The `std` command for this description: own process group, piped output.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        for key in &self.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }

    /// Starts the reader threads and the stdin writer for a spawned child.
    fn start_io(&self, child: &mut Child) -> io::Result<(Reader, Reader)> {
        let missing = || io::Error::other("child process pipe is missing");
        let out = child.stdout.take().ok_or_else(missing)?;
        let err = child.stderr.take().ok_or_else(missing)?;
        let stdout = Reader::spawn(out, self.head_bytes, self.tail_bytes)?;
        let stderr = Reader::spawn(err, self.head_bytes, self.tail_bytes)?;
        if let Some(text) = &self.stdin {
            let mut pipe = child.stdin.take().ok_or_else(missing)?;
            let text = text.clone();
            // Detached on purpose: it ends when the text is written or the child is gone.
            std::thread::Builder::new().name("base-process-stdin".into()).spawn(move || {
                // A child that exits without reading its input is not an error.
                let _ = pipe.write_all(text.as_bytes());
            })?;
        }
        Ok((stdout, stderr))
    }
}

/// Waits until all readers reached the end of their pipes or `limit` passed.
fn wait_finished(readers: &[&Reader], limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while !readers.iter().all(|r| r.finished()) {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// Stops the child's whole process group: SIGTERM, then SIGKILL after [`KILL_GRACE`] if
/// anything of the group is still there. Reaps the child and returns its status.
fn stop(child: &mut Child, pgid: Option<i32>) -> Option<ExitStatus> {
    let Some(pgid) = pgid else {
        // No usable group id (cannot happen for a real pid); fall back to the direct child.
        let _ = child.kill();
        return child.wait().ok();
    };
    signal_group(pgid, libc::SIGTERM);
    let end = Instant::now() + KILL_GRACE;
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait().ok().flatten();
        }
        if status.is_some() && !group_alive(pgid) {
            return status;
        }
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    signal_group(pgid, libc::SIGKILL);
    status.or_else(|| child.wait().ok())
}

/// Sends `signal` to every process of the group `pgid`.
fn signal_group(pgid: i32, signal: i32) -> bool {
    // SAFETY: `killpg` only sends a signal; it takes two integers and touches no memory. `pgid`
    // is the pid of a child we started with `process_group(0)` and is always above 1, so it
    // never names our own group or every process.
    unsafe { libc::killpg(pgid, signal) == 0 }
}

/// Whether any process of the group still exists (a zombie counts until it is reaped).
fn group_alive(pgid: i32) -> bool {
    signal_group(pgid, 0)
}

/// Asks every process of a group to stop (SIGTERM). `pgid` is the pid of a child started with
/// `process_group(0)`; values of 1 or below are ignored. Returns whether the signal was sent.
pub fn terminate_group(pgid: i32) -> bool {
    pgid > 1 && signal_group(pgid, libc::SIGTERM)
}

/// Kills every process of a group (SIGKILL). Same rules as [`terminate_group`].
pub fn kill_group(pgid: i32) -> bool {
    pgid > 1 && signal_group(pgid, libc::SIGKILL)
}
