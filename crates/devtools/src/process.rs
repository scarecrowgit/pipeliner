//! Running helper commands with a timeout and a readable error, and scratch directories.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};

/// What a finished command printed.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The last lines of stderr, else stdout: for error messages.
    pub fn tail(&self) -> String {
        let s = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        tail(s, 12)
    }
}

/// The last `n` lines of `s`.
pub fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Run `cmd` with stdin closed until it exits or `timeout` passes (then kill it).
pub fn run(mut cmd: Command, timeout: Duration) -> anyhow::Result<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let program = format!("{:?}", cmd.get_program());
    let mut child = cmd.spawn().with_context(|| format!("start {program}"))?;
    let mut out_pipe = child.stdout.take().ok_or_else(|| anyhow!("no stdout pipe"))?;
    let mut err_pipe = child.stderr.take().ok_or_else(|| anyhow!("no stderr pipe"))?;
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out_pipe.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err_pipe.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} timed out after {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        ok: status.success(),
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
}

/// A fresh, empty directory for one run (socket, logs, a browser profile), removed on drop.
/// Lives under `$XDG_RUNTIME_DIR` when set, because Unix socket paths must stay short.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Create `<runtime or temp dir>/harness-dev-<what>-<pid>-<nanos>`.
    pub fn new(what: &str) -> anyhow::Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let path = base.join(format!("harness-dev-{what}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(ScratchDir { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// `path` made absolute against the current directory (it need not exist yet).
pub fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}
