//! The few git calls the runner makes itself; everything else goes through `vcs`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use base::cancel::CancelToken;
use base::process::Cmd;

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `git -C dir args…`: stdout, or an error with git's message.
pub(crate) fn git(dir: &Path, args: &[&str], cancel: &CancelToken) -> Result<String> {
    let out = Cmd::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .timeout(GIT_TIMEOUT)
        .run(cancel)?;
    if out.cancelled {
        bail!("cancelled");
    }
    if !out.success() {
        bail!("git {} failed: {}", args.join(" "), super::services::tail(&out.combined(), 400));
    }
    Ok(out.stdout)
}

/// Whether the worktree has uncommitted changes, untracked files included.
pub(crate) fn dirty(dir: &Path, cancel: &CancelToken) -> Result<bool> {
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=all"], cancel)?.trim().is_empty())
}

/// Files that differ from `base` in the worktree: committed on the branch, staged, unstaged or
/// untracked; deleted ones left out.
pub(crate) fn changed_paths(dir: &Path, base: &str, cancel: &CancelToken) -> Result<Vec<String>> {
    let mut files: Vec<String> = git(dir, &["status", "--porcelain", "--untracked-files=all"], cancel)?
        .lines()
        .filter(|line| !line.starts_with(" D") && !line.starts_with("D "))
        .filter_map(|line| line.get(3..).map(|p| p.rsplit(" -> ").next().unwrap_or(p).trim().trim_matches('"').to_string()))
        .collect();
    if !base.is_empty()
        && let Ok(out) = git(dir, &["diff", "--name-only", "--diff-filter=AMR", &format!("{base}...HEAD")], cancel)
    {
        files.extend(out.lines().map(str::to_string));
    }
    files.sort();
    files.dedup();
    Ok(files)
}
