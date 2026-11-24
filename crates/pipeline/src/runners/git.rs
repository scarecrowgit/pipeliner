//! The few git calls the runner makes itself; everything else goes through `vcs`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use base::cancel::CancelToken;
use base::process::Cmd;

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// The most a file read whole out of git may weigh.
const WHOLE_FILE_LIMIT: usize = 64 * 1024 * 1024;

/// `git -C dir args…`: stdout, or an error with git's message. Long output keeps its head
/// and tail only; use [`file_at`] for content that must arrive whole.
pub(crate) fn git(dir: &Path, args: &[&str], cancel: &CancelToken) -> Result<String> {
    run(dir, args, None, cancel)
}

/// `path` as it is on `rev`, whole. A file over the limit is an error, never a cut text.
pub(crate) fn file_at(dir: &Path, rev: &str, path: &str, cancel: &CancelToken) -> Result<String> {
    run(dir, &["show", &format!("{rev}:{path}")], Some(WHOLE_FILE_LIMIT), cancel)
}

fn run(dir: &Path, args: &[&str], whole: Option<usize>, cancel: &CancelToken) -> Result<String> {
    let mut cmd = Cmd::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .timeout(GIT_TIMEOUT);
    if let Some(limit) = whole {
        cmd = cmd.capture_limit(limit, 0);
    }
    let out = cmd.run(cancel)?;
    if out.cancelled {
        bail!("cancelled");
    }
    if !out.success() {
        bail!("git {} failed: {}", args.join(" "), super::services::tail(&out.combined(), 400));
    }
    if whole.is_some() && out.truncated {
        bail!("git {} gave more than {} MB", args.join(" "), WHOLE_FILE_LIMIT / (1024 * 1024));
    }
    Ok(out.stdout)
}

/// Whether the worktree has uncommitted changes, untracked files included.
pub(crate) fn dirty(dir: &Path, cancel: &CancelToken) -> Result<bool> {
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=all"], cancel)?.trim().is_empty())
}

/// Whether `path` exists on `base`. An unknown base or a failing git is an error, not "no".
pub(crate) fn exists_in(dir: &Path, base: &str, path: &str, cancel: &CancelToken) -> Result<bool> {
    let commit = format!("{base}^{{commit}}");
    git(dir, &["rev-parse", "--verify", "--quiet", &commit], cancel).map_err(|_| anyhow::anyhow!("the base `{base}` does not exist"))?;
    let listed = git(dir, &["ls-tree", "-z", "--name-only", base, "--", path], cancel)?;
    Ok(listed.split('\0').any(|name| name == path))
}

/// Files that differ from `base` in the worktree: committed on the branch, staged, unstaged or
/// untracked; deleted ones left out. Read with `-z`, so names are never C-quoted.
pub(crate) fn changed_paths(dir: &Path, base: &str, cancel: &CancelToken) -> Result<Vec<String>> {
    let status = git(dir, &["status", "--porcelain", "-z", "--untracked-files=all"], cancel)?;
    let mut files = parse_status_z(&status);
    if !base.is_empty()
        && let Ok(out) = git(dir, &["diff", "-z", "--name-only", "--diff-filter=AMR", &format!("{base}...HEAD")], cancel)
    {
        files.extend(out.split('\0').filter(|p| !p.is_empty()).map(str::to_string));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// The paths of `git status --porcelain -z`, deleted ones left out. A rename or copy is
/// followed by its old path, which is skipped.
fn parse_status_z(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    while let Some(field) = fields.next() {
        let (code, path) = (field.get(..2).unwrap_or_default(), field.get(3..).unwrap_or_default());
        if code.starts_with('R') || code.starts_with('C') {
            fields.next();
        }
        if !code.contains('D') && !path.is_empty() {
            files.push(path.to_string());
        }
    }
    files
}
