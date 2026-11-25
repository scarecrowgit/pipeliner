//! Git and GitHub helpers: worktrees per run/variant, commits, diffs, pull requests.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};
use harness_core::config::expand_tilde;
use harness_core::Workdir;

/// Runs a command and returns stdout; fails with stderr on non-zero exit.
pub fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    run(cwd, "git", args)
}

pub fn is_repo(path: &Path) -> bool {
    git(path, &["rev-parse", "--is-inside-work-tree"]).map(|s| s == "true").unwrap_or(false)
}

pub fn repo_root(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?))
}

/// The path inside the repository root that `workdir.path` points at ("" when it is the root).
pub fn subdir(workdir: &Workdir) -> Result<PathBuf> {
    let p = expand_tilde(&workdir.path);
    let root = repo_root(&p)?;
    let canon = p.canonicalize().unwrap_or(p.clone());
    Ok(canon.strip_prefix(&root).map(|s| s.to_path_buf()).unwrap_or_default())
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}

/// Creates a worktree at `dest` on a new branch `branch` starting from `from`.
/// If the branch already exists, the worktree checks it out instead.
pub fn add_worktree(workdir: &Workdir, dest: &Path, branch: &str, from: &str) -> Result<()> {
    let repo = repo_root(&expand_tilde(&workdir.path))?;
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest_s = dest.to_string_lossy().to_string();
    if branch_exists(&repo, branch) {
        git(&repo, &["worktree", "add", &dest_s, branch])?;
    } else {
        git(&repo, &["worktree", "add", "-b", branch, &dest_s, from])?;
    }
    Ok(())
}

pub fn remove_worktree(workdir: &Workdir, dest: &Path) {
    if let Ok(repo) = repo_root(&expand_tilde(&workdir.path)) {
        let _ = git(&repo, &["worktree", "remove", "--force", &dest.to_string_lossy()]);
    }
}

/// Commits everything in `cwd`. Returns false when there was nothing to commit.
pub fn commit_all(cwd: &Path, message: &str) -> Result<bool> {
    git(cwd, &["add", "-A"])?;
    if git(cwd, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(false);
    }
    git(
        cwd,
        &["-c", "user.name=Pipeliner", "-c", "user.email=pipeliner@localhost", "commit", "-q", "--no-verify", "-m", message],
    )?;
    Ok(true)
}

/// Unified diff of the working tree (including untracked files) against `base`.
pub fn diff_against(cwd: &Path, base: &str) -> String {
    let _ = git(cwd, &["add", "-A", "--intent-to-add"]);
    let d = git(cwd, &["diff", "--no-color", base]).unwrap_or_default();
    d
}

/// Files changed in the working tree + commits since `base`.
pub fn changed_files(cwd: &Path, base: &str) -> Vec<String> {
    let _ = git(cwd, &["add", "-A", "--intent-to-add"]);
    git(cwd, &["diff", "--name-only", base])
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn head(cwd: &Path) -> Result<String> {
    git(cwd, &["rev-parse", "HEAD"])
}

pub fn diff_stat(diff: &str) -> (usize, usize, usize) {
    let mut files = 0;
    let mut add = 0;
    let mut del = 0;
    for l in diff.lines() {
        if l.starts_with("diff --git") {
            files += 1;
        } else if l.starts_with('+') && !l.starts_with("+++") {
            add += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            del += 1;
        }
    }
    (files, add, del)
}

/// Pushes `branch` and opens a PR with `gh`. Returns the PR URL.
pub fn open_pr(cwd: &Path, branch: &str, base: &str, title: &str, body: &str, draft: bool) -> Result<String> {
    git(cwd, &["push", "-u", "--force-with-lease", "origin", branch])?;
    // Reuse an existing PR for this branch.
    if let Ok(url) = run(cwd, "gh", &["pr", "view", branch, "--json", "url,state", "-q", "select(.state == \"OPEN\") | .url"]) {
        if url.starts_with("http") {
            let _ = run(cwd, "gh", &["pr", "edit", branch, "--title", title, "--body", body]);
            return Ok(url);
        }
    }
    let mut args = vec!["pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body];
    if draft {
        args.push("--draft");
    }
    let out = run(cwd, "gh", &args)?;
    let url = out.lines().rev().find(|l| l.starts_with("http")).unwrap_or(&out).to_string();
    Ok(url)
}

pub fn merge_pr(cwd: &Path, url: &str, method: &str) -> Result<String> {
    let flag = match method {
        "merge" => "--merge",
        "rebase" => "--rebase",
        _ => "--squash",
    };
    // A draft PR can't be merged; mark it ready first.
    let _ = run(cwd, "gh", &["pr", "ready", url]);
    run(cwd, "gh", &["pr", "merge", url, flag, "--delete-branch=false"])
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slugs() {
        assert_eq!(slug("Fix: Counters update chaotically!"), "fix-counters-update-chaotically");
    }
    #[test]
    fn stats() {
        let d = "diff --git a/x b/x\n--- a/x\n+++ b/x\n+a\n-b\n+c\n";
        assert_eq!(diff_stat(d), (1, 2, 1));
    }
}
