//! The niri compositor: find the app's window by pid, resize it, screenshot it. Only ever
//! acts on a window id the caller found for its own pid.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde_json::Value;

use crate::process::{absolute, run};

/// A window as `niri msg --json windows` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct NiriWindow {
    pub id: u64,
    pub pid: Option<u32>,
    pub app_id: Option<String>,
    pub title: Option<String>,
    /// The client area in logical pixels, when niri reports it.
    pub size: Option<(u32, u32)>,
    pub focused: bool,
}

/// Read `niri msg --json windows` output.
pub fn parse_windows(json: &str) -> anyhow::Result<Vec<NiriWindow>> {
    let value: Value = serde_json::from_str(json).context("niri windows: not JSON")?;
    let list = value.as_array().ok_or_else(|| anyhow!("niri windows: not a list"))?;
    list.iter()
        .map(|w| {
            let size = w["layout"]["window_size"].as_array().and_then(|s| match s.as_slice() {
                [w, h] => Some((w.as_u64()? as u32, h.as_u64()? as u32)),
                _ => None,
            });
            Ok(NiriWindow {
                id: w["id"].as_u64().ok_or_else(|| anyhow!("niri window without id"))?,
                pid: w["pid"].as_u64().map(|p| p as u32),
                app_id: w["app_id"].as_str().map(str::to_string),
                title: w["title"].as_str().map(str::to_string),
                size,
                focused: w["is_focused"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

fn niri(args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("niri");
    // The Nix shell's library path is for GPUI, not for system binaries.
    cmd.arg("msg").args(args).env_remove("LD_LIBRARY_PATH");
    let out = run(cmd, Duration::from_secs(10))?;
    if !out.ok {
        bail!("niri msg {}: {}", args.join(" "), out.tail());
    }
    Ok(out.stdout)
}

/// Every window niri manages.
pub fn windows() -> anyhow::Result<Vec<NiriWindow>> {
    parse_windows(&niri(&["--json", "windows"])?)
}

/// The window of process `pid`, waiting up to `timeout` for it to appear.
pub fn window_for_pid(pid: u32, timeout: Duration) -> anyhow::Result<NiriWindow> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(w) = windows()?.into_iter().find(|w| w.pid == Some(pid)) {
            return Ok(w);
        }
        if Instant::now() > deadline {
            bail!("niri has no window for pid {pid}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Set window `id` to `width` logical pixels.
pub fn set_width(id: u64, width: u32) -> anyhow::Result<()> {
    niri(&["action", "set-window-width", "--id", &id.to_string(), &width.to_string()]).map(drop)
}

/// Screenshot window `id` into `png` (no clipboard, no notification); waits until the file
/// is written and decodes.
pub fn screenshot(id: u64, png: &Path) -> anyhow::Result<()> {
    let png = absolute(png)?;
    if let Some(dir) = png.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&png);
    let path = png.to_string_lossy();
    niri(&[
        "action",
        "screenshot-window",
        "--id",
        &id.to_string(),
        "--path",
        &path,
        "--write-to-disk",
        "true",
        "--copy-to-clipboard",
        "false",
        "--notify",
        "false",
    ])?;
    // niri writes the file after replying.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if png.exists() && image::open(&png).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("niri wrote no screenshot to {path}")
}
