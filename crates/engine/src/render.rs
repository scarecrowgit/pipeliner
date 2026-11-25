//! Renders static HTML mockups to PNG with a headless Chromium.

use std::path::Path;

use anyhow::{bail, Result};

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// Renders `input` to `output`. `command` (from config/node) may use `{in}`, `{out}`, `{w}`, `{h}`.
pub fn render(command: &str, input: &Path, output: &Path, w: u32, h: u32) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let _ = std::fs::remove_file(output);
    let url = format!("file://{}", input.canonicalize()?.display());
    let flags = format!(
        "--headless=new --disable-gpu --no-sandbox --hide-scrollbars --window-size={w},{h} --virtual-time-budget=8000 --screenshot={}",
        q(&output.display().to_string())
    );
    let cmd = if !command.trim().is_empty() {
        command
            .replace("{in}", &q(&input.display().to_string()))
            .replace("{out}", &q(&output.display().to_string()))
            .replace("{w}", &w.to_string())
            .replace("{h}", &h.to_string())
    } else if let Some(bin) = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"]
        .into_iter()
        .find(|b| on_path(b))
    {
        format!("{bin} {flags} {}", q(&url))
    } else if on_path("nix-shell") {
        format!("nix-shell -p chromium --run {}", q(&format!("chromium {flags} {}", q(&url))))
    } else {
        bail!("no Chromium found to render mockups; install chromium or set a render command in Settings");
    };
    // Browsers break with an app-specific LD_LIBRARY_PATH (e.g. from a nix shell).
    let mut env = std::collections::BTreeMap::new();
    env.insert("LD_LIBRARY_PATH".to_string(), String::new());
    let never = std::sync::atomic::AtomicBool::new(false);
    let out = crate::tools::run_command(
        input.parent().unwrap_or(Path::new("/")),
        "",
        &env,
        &cmd,
        std::time::Duration::from_secs(180),
        &never,
    )?;
    if !output.exists() {
        bail!("render failed: {}", out.lines().last().unwrap_or(""));
    }
    Ok(())
}
