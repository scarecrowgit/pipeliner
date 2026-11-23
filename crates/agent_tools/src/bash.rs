//! `bash` tool: `sh -c` in the workspace root with output cap, timeout and cancel.

use std::collections::VecDeque;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub(crate) const MAX_TIMEOUT_SECS: u64 = 600;
const HEAD_BYTES: usize = 15 * 1024;
const TAIL_BYTES: usize = 15 * 1024;
const POLL: Duration = Duration::from_millis(50);

pub(crate) struct BashResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// Keeps the first `HEAD_BYTES` and the last `TAIL_BYTES` of everything pushed.
#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
}

impl Capture {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.head.len() < HEAD_BYTES {
            let take = (HEAD_BYTES - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        self.tail.extend(bytes);
        if self.tail.len() > TAIL_BYTES {
            let excess = self.tail.len() - TAIL_BYTES;
            self.tail.drain(..excess);
            self.dropped += excess;
        }
    }

    fn into_string(self) -> String {
        let mut s = String::from_utf8_lossy(&self.head).into_owned();
        if self.dropped > 0 {
            s.push_str(&format!("\n\n... [{} bytes of output truncated] ...\n\n", self.dropped));
        }
        let tail: Vec<u8> = self.tail.into_iter().collect();
        s.push_str(&String::from_utf8_lossy(&tail));
        s
    }
}

fn spawn_reader(
    mut src: impl Read + Send + 'static,
    cap: Arc<Mutex<Capture>>,
    done: Arc<AtomicUsize>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut c) = cap.lock() {
                        c.push(&buf[..n]);
                    }
                }
            }
        }
        done.fetch_add(1, Ordering::SeqCst);
    });
}

fn signal_group(pgid: u32, signal: &str) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -{signal} -{pgid} 2>/dev/null"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) fn run(
    root: &Path,
    command: &str,
    env: &[(String, String)],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<BashResult, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Failed to start sh: {e}"))?;
    let pgid = child.id();

    let cap = Arc::new(Mutex::new(Capture::default()));
    let done = Arc::new(AtomicUsize::new(0));
    let mut readers = 0;
    if let Some(out) = child.stdout.take() {
        spawn_reader(out, cap.clone(), done.clone());
        readers += 1;
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, cap.clone(), done.clone());
        readers += 1;
    }

    let start = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
        } else if start.elapsed() >= timeout {
            timed_out = true;
        }
        if cancelled || timed_out {
            signal_group(pgid, "TERM");
            let grace = Instant::now();
            while grace.elapsed() < Duration::from_millis(500) {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            signal_group(pgid, "KILL");
            let _ = child.kill();
            break child.wait().ok();
        }
        std::thread::sleep(POLL);
    };
    // A leftover group member could still hold the pipes open; make sure it's gone and
    // don't wait for the readers forever (e.g. a daemonized process in its own session).
    if !timed_out && !cancelled {
        let wait = Instant::now();
        while done.load(Ordering::SeqCst) < readers && wait.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if done.load(Ordering::SeqCst) < readers {
            signal_group(pgid, "KILL");
        }
    }
    let wait = Instant::now();
    while done.load(Ordering::SeqCst) < readers && wait.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(10));
    }

    let output = match cap.lock() {
        Ok(mut c) => std::mem::take(&mut *c).into_string(),
        Err(_) => String::new(),
    };
    Ok(BashResult {
        output,
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        cancelled,
    })
}
