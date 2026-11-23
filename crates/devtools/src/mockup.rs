//! Render a `screens-v2` mockup to PNG with headless Firefox, at the size its head comment
//! (`<!-- render size: 1440x960 -->`) asks for.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::process::{ScratchDir, absolute, run};

/// The size used when a mockup has no render-size comment.
pub const DEFAULT_SIZE: (u32, u32) = (1440, 960);

/// The `render size: WxH` from a mockup's head comment, if present and sane (200–4000 px).
pub fn render_size(html: &str) -> Option<(u32, u32)> {
    let start = html.find("render size:")? + "render size:".len();
    let rest = html[start..].trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == 'x')).unwrap_or(rest.len());
    let (w, h) = rest[..end].split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    let sane = |v: u32| (200..=4000).contains(&v);
    (sane(w) && sane(h)).then_some((w, h))
}

/// The mockup directory: `~/Pictures/designs/pipeliner-mockups/screens-v2`.
pub fn mockup_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Pictures/designs/pipeliner-mockups/screens-v2")
}

/// A mockup by name (`home`) or by path to an `.html` file.
pub fn resolve(name: &str) -> PathBuf {
    let path = Path::new(name);
    if name.ends_with(".html") || path.components().count() > 1 {
        return path.to_path_buf();
    }
    mockup_dir().join(format!("{name}.html"))
}

/// Screenshot `html` into `png`; returns the size rendered at. Uses a throwaway Firefox
/// profile and drops the Nix shell's `LD_LIBRARY_PATH` (GPUI's libraries break Firefox's NSS).
pub fn render(html: &Path, png: &Path) -> anyhow::Result<(u32, u32)> {
    let html = absolute(html)?;
    let png = absolute(png)?;
    let text = std::fs::read_to_string(&html).with_context(|| format!("read {}", html.display()))?;
    let (w, h) = render_size(&text).unwrap_or(DEFAULT_SIZE);
    if let Some(dir) = png.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let profile = ScratchDir::new("firefox")?;
    let mut cmd = Command::new("firefox");
    cmd.args(["--headless", "--no-remote", "-profile"])
        .arg(profile.path())
        .arg(format!("--window-size={w},{h}"))
        .arg("--screenshot")
        .arg(&png)
        .arg(format!("file://{}", html.display()))
        .env_remove("LD_LIBRARY_PATH");
    let _ = std::fs::remove_file(&png);
    let out = run(cmd, Duration::from_secs(90))?;
    if !png.exists() {
        bail!("firefox made no screenshot: {}", out.tail());
    }
    Ok((w, h))
}
