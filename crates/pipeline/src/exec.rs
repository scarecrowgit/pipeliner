//! Running commands for pipeline steps (git, gh, warm-up builds) and rendering mockup
//! screens to PNG with headless Firefox.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// stderr, else stdout, last lines — for error messages.
    pub fn tail(&self) -> String {
        let s = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        let lines: Vec<&str> = s.trim().lines().collect();
        lines[lines.len().saturating_sub(12)..].join("\n")
    }
}

/// Run `cmd` (stdin closed) until it exits, `timeout` passes or `cancel` is set.
pub fn run(mut cmd: Command, timeout: Duration, cancel: &AtomicBool) -> anyhow::Result<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| anyhow!("{:?}: {e}", cmd.get_program()))?;
    let mut out_pipe = child.stdout.take().expect("piped");
    let mut err_pipe = child.stderr.take().expect("piped");
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
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("cancelled");
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok(Output {
        ok: status.success(),
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
}

/// `git -C dir args…`, failing with git's message.
pub fn git(dir: &Path, args: &[&str], cancel: &AtomicBool) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    let out = run(cmd, Duration::from_secs(300), cancel)?;
    if !out.ok {
        bail!("git {}: {}", args.join(" "), out.tail());
    }
    Ok(out.stdout)
}

/// `sh -c command` in `dir` with extra env.
pub fn shell(
    dir: &Path,
    command: &str,
    env: &[(String, String)],
    timeout: Duration,
    cancel: &AtomicBool,
) -> anyhow::Result<Output> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    run(cmd, timeout, cancel)
}

/// `render size: 1440x960` from a mockup's head comment (default 1440×960).
pub fn render_size(html: &str) -> (u32, u32) {
    html.find("render size:")
        .and_then(|i| {
            let rest = html[i + 12..].trim_start();
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == 'x'))?;
            let (w, h) = rest[..end].split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .filter(|(w, h)| *w >= 200 && *h >= 200 && *w <= 4000 && *h <= 4000)
        .unwrap_or((1440, 960))
}

/// Screenshot `html` into `png` with headless Firefox at the mockup's render size.
pub fn render(html: &Path, png: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(html)?;
    let (w, h) = render_size(&text);
    if let Some(dir) = png.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let profile = std::env::temp_dir().join(format!(
        "harness-render-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos() ^ (png.as_os_str().len() as u128)
    ));
    std::fs::create_dir_all(&profile)?;
    let mut cmd = Command::new("firefox");
    cmd.args(["--headless", "--no-remote", "-profile"])
        .arg(&profile)
        .arg(format!("--window-size={w},{h}"))
        .arg("--screenshot")
        .arg(png)
        .arg(format!("file://{}", html.display()))
        // The app's own library path (gpui's) breaks Firefox's bundled NSS.
        .env_remove("LD_LIBRARY_PATH");
    let _ = std::fs::remove_file(png);
    let out = run(cmd, Duration::from_secs(90), cancel);
    let _ = std::fs::remove_dir_all(&profile);
    let out = out?;
    if !png.exists() {
        bail!("firefox made no screenshot: {}", out.tail());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(render_size("<head><!-- render size: 560x640 -->"), (560, 640));
        assert_eq!(render_size("<head>"), (1440, 960));
    }
}
