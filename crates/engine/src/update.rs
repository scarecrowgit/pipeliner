//! Self-update: pull the app's own repository, build a release binary, hand back its path.
//! Pipelines that improve the app merge into this repo; this brings those changes into the
//! running app (the app restarts into the new binary).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};

use crate::{git, templates, tools, Engine};

#[derive(Clone, Debug)]
pub struct UpdateStatus {
    pub repo: PathBuf,
    pub branch: String,
    /// Commits on the remote branch not in the local checkout.
    pub behind: usize,
    /// Uncommitted local changes (update refuses to pull over them).
    pub dirty: bool,
    pub head: String,
}

pub fn status(engine: &Engine) -> Result<UpdateStatus> {
    let repo = templates::self_repo(&engine.config())
        .context("this build doesn't know its repository; set it in Settings")?;
    let branch = git::git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let _ = git::git(&repo, &["fetch", "--quiet", "origin", &branch]);
    let behind = git::git(
        &repo,
        &["rev-list", "--count", &format!("HEAD..origin/{branch}")],
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0);
    let dirty = !git::git(&repo, &["status", "--porcelain"])
        .unwrap_or_default()
        .trim()
        .is_empty();
    let head = git::git(&repo, &["log", "-1", "--format=%h %s"]).unwrap_or_default();
    Ok(UpdateStatus {
        repo,
        branch,
        behind,
        dirty,
        head,
    })
}

/// Pulls (fast-forward only) and builds `--release`. Returns the new binary's path.
/// `log` receives progress lines.
pub fn update(engine: &Engine, log: &mut dyn FnMut(&str)) -> Result<PathBuf> {
    let st = status(engine)?;
    if st.behind > 0 {
        if st.dirty {
            bail!("the app's repo has uncommitted changes; commit or stash them first");
        }
        log(&format!("Pulling {} new commit(s)…", st.behind));
        git::git(&st.repo, &["pull", "--ff-only", "--quiet"])?;
    }
    log("Building release (this can take a few minutes)…");
    let shell = crate::chat::shell_for(Some(&st.repo));
    let target = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .unwrap_or_else(|| st.repo.join("target").display().to_string());
    let mut env = std::collections::BTreeMap::new();
    env.insert("CARGO_TARGET_DIR".to_string(), target.clone());
    let never = AtomicBool::new(false);
    let out = tools::run_command(
        &st.repo,
        &shell,
        &env,
        "cargo build --release -p pipeliner",
        Duration::from_secs(3600),
        &never,
    )?;
    if !out.starts_with("exit code 0") {
        bail!("build failed:\n{}", tools::truncate_middle(&out, 3000));
    }
    let bin = PathBuf::from(target).join("release/pipeliner");
    if !bin.exists() {
        bail!("build finished but {} is missing", bin.display());
    }
    engine
        .store()
        .log("app.updated", None, None, format!("Built {}", st.head));
    log("Built. Restarting…");
    Ok(bin)
}
