//! What a branch changed: file lists, diffs, commits, merges and pushes.
//!
//! Everything is measured against the merge base with a base branch, so a worktree shows
//! exactly what its agent did, whether it committed yet or not.

mod diff;
mod merge;

use std::io::Read;
use std::path::{Component, Path};

use anyhow::{Result, bail};
use base::cancel::CancelToken;
use model::{FileChange, FileDiff};

use crate::exec::{LOCAL, NETWORK, Tool, git_committing};

pub use diff::parse_unified;
pub use merge::{MergeOutcome, merge_into_base};

/// Files changed in `worktree` against the merge base with `base`: committed, staged,
/// unstaged and untracked ones, renames as one entry under the new path, sorted by path.
/// Binary files count zero lines.
pub fn changed_files(worktree: &Path, base: &str, cancel: &CancelToken) -> Result<Vec<FileChange>> {
    let mb = merge_base(worktree, base, cancel)?;
    let out = Tool::git(worktree)
        .args(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--no-textconv"])
        .arg(&mb)
        .arg("--")
        .timeout(LOCAL)
        .check(cancel)?;
    let mut files = parse_numstat(&out);
    let untracked = Tool::git(worktree)
        .args(["ls-files", "--others", "--exclude-standard", "--full-name", "-z"])
        .timeout(LOCAL)
        .check(cancel)?;
    for path in untracked.split('\0').filter(|p| !p.is_empty() && !p.ends_with('/')) {
        if cancel.is_cancelled() {
            bail!("cancelled while listing changes");
        }
        if files.iter().any(|f| f.path == path) {
            continue;
        }
        let added = count_lines(&worktree_root(worktree, cancel).join(path));
        files.push(FileChange { path: path.to_string(), added, removed: 0 });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The change to one file against the merge base with `base`, parsed into hunks. An
/// untracked file is a new file; a file without changes gives an empty diff. For a binary
/// file `hunks` is empty.
pub fn file_diff(worktree: &Path, base: &str, path: &str, cancel: &CancelToken) -> Result<FileDiff> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative.components().any(|c| matches!(c, Component::ParentDir))
    {
        bail!("`{path}` is not a path inside the worktree");
    }
    let mb = merge_base(worktree, base, cancel)?;
    let literal = |tool: Tool| tool.env("GIT_LITERAL_PATHSPECS", "1");

    // A renamed file needs its old path in the pathspec, or git shows it as a new file.
    let names = literal(Tool::git(worktree))
        .args(["diff", "--name-status", "-z", "-M", "--no-ext-diff", "--no-textconv"])
        .arg(&mb)
        .arg("--")
        .timeout(LOCAL)
        .check(cancel)?;
    let mut specs = vec![path.to_string()];
    if let Some(old) = renamed_from(&names, path) {
        specs.push(old);
    }
    let text = literal(Tool::git(worktree))
        .args(["diff", "-M", "--no-ext-diff", "--no-textconv", "--no-color", "-U3"])
        .arg(&mb)
        .arg("--")
        .args(&specs)
        .timeout(LOCAL)
        .check(cancel)?;
    let mut parsed = parse_unified(&text);
    if parsed.is_empty() {
        let untracked = literal(Tool::git(worktree))
            .args(["ls-files", "--others", "--exclude-standard", "--full-name", "-z", "--"])
            .arg(path)
            .check(cancel)?;
        if !untracked.trim_matches('\0').is_empty() {
            // `--no-index` exits with 1 when the files differ.
            let out = literal(Tool::git(worktree))
                .args(["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "-U3", "--"])
                .arg("/dev/null")
                .arg(path)
                .timeout(LOCAL)
                .run(cancel)?;
            parsed = parse_unified(&out.stdout);
        }
    }
    let wanted = parsed.iter().position(|d| d.path == path).unwrap_or(0);
    Ok(if parsed.is_empty() {
        FileDiff { path: path.to_string(), ..FileDiff::default() }
    } else {
        parsed.swap_remove(wanted)
    })
}

/// Stages everything and commits it with `message`. Returns the new commit's hash, or `None`
/// when there was nothing to commit.
///
/// Runs without an editor, without a prompt and without commit signing, so it cannot wait for
/// a person; repository hooks still run. A machine without a configured git identity commits
/// as `Harness`.
pub fn commit_all(worktree: &Path, message: &str, cancel: &CancelToken) -> Result<Option<String>> {
    if message.trim().is_empty() {
        bail!("the commit message is empty");
    }
    Tool::git(worktree).args(["add", "-A"]).timeout(LOCAL).check(cancel)?;
    // Exit 1 means there is something staged.
    let staged = Tool::git(worktree)
        .args(["diff", "--cached", "--quiet", "--no-ext-diff"])
        .timeout(LOCAL)
        .run(cancel)?;
    match staged.code {
        Some(0) => return Ok(None),
        Some(1) => {}
        _ => return Err(crate::exec::failure("git diff --cached --quiet", &staged)),
    }
    git_committing(worktree, cancel)
        .args(["-c", "commit.gpgsign=false", "commit", "--quiet", "--cleanup=whitespace", "-F", "-"])
        .stdin(message)
        .timeout(LOCAL)
        .check(cancel)?;
    let head = Tool::git(worktree).args(["rev-parse", "HEAD"]).check(cancel)?;
    Ok(Some(head.trim().to_string()))
}

/// Pushes `branch` to the remote (`origin`, else the only or first one) and sets it as the
/// branch's upstream. Never asks for a password: a remote that needs one fails.
pub fn push(worktree: &Path, branch: &str, cancel: &CancelToken) -> Result<()> {
    let branch = branch.trim();
    if branch.is_empty() || branch.starts_with('-') {
        bail!("`{branch}` is not a branch name");
    }
    let remote = default_remote(worktree, cancel)?;
    Tool::git(worktree)
        .args(["push", "--set-upstream", &remote])
        .arg(format!("refs/heads/{branch}:refs/heads/{branch}"))
        .timeout(NETWORK)
        .check(cancel)?;
    Ok(())
}

/// `origin`, else the first remote.
fn default_remote(dir: &Path, cancel: &CancelToken) -> Result<String> {
    let remotes = Tool::git(dir).arg("remote").check(cancel)?;
    let names: Vec<&str> = remotes.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if names.contains(&"origin") {
        return Ok("origin".to_string());
    }
    match names.first() {
        Some(name) => Ok((*name).to_string()),
        None => bail!("this repository has no remote to push to"),
    }
}

/// The commit where `base` and HEAD parted. `base` may also be given without its remote
/// (`main` when only `origin/main` exists).
pub(crate) fn merge_base(dir: &Path, base: &str, cancel: &CancelToken) -> Result<String> {
    let base = base.trim();
    if base.is_empty() || base.starts_with('-') {
        bail!("`{base}` is not a valid base branch");
    }
    let resolved = resolve_commit(dir, base, cancel)
        .or_else(|| resolve_commit(dir, &format!("origin/{base}"), cancel));
    let Some(base_commit) = resolved else {
        bail!("the base `{base}` does not exist in {}", dir.display());
    };
    match Tool::git(dir).args(["merge-base", &base_commit, "HEAD"]).query(cancel) {
        Some(out) if !out.trim().is_empty() => Ok(out.trim().to_string()),
        _ => bail!("`{base}` and this branch have no common history"),
    }
}

/// The commit a name points at, or `None`.
pub(crate) fn resolve_commit(dir: &Path, name: &str, cancel: &CancelToken) -> Option<String> {
    let spec = format!("{name}^{{commit}}");
    let out = Tool::git(dir).args(["rev-parse", "--verify", "--quiet", &spec]).query(cancel)?;
    let hash = out.trim();
    (!hash.is_empty()).then(|| hash.to_string())
}

fn worktree_root(dir: &Path, cancel: &CancelToken) -> std::path::PathBuf {
    Tool::git(dir)
        .args(["rev-parse", "--show-toplevel"])
        .query(cancel)
        .map(|s| std::path::PathBuf::from(s.trim()))
        .unwrap_or_else(|| dir.to_path_buf())
}

/// Parses `git diff --numstat -z`. A rename is `added TAB removed TAB NUL old NUL new NUL`.
pub(crate) fn parse_numstat(text: &str) -> Vec<FileChange> {
    let mut files = Vec::new();
    let mut fields = text.split('\0');
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // Skip the old name; the entry is known by the new one.
            let _old = fields.next();
            match fields.next() {
                Some(new) => new,
                None => break,
            }
        } else {
            path
        };
        // Binary files show `-` for both counts.
        files.push(FileChange {
            path: path.to_string(),
            added: added.parse().unwrap_or(0),
            removed: removed.parse().unwrap_or(0),
        });
    }
    files
}

/// The old path of `path` when `git diff --name-status -z -M` reports it as renamed or copied.
fn renamed_from(names: &str, path: &str) -> Option<String> {
    let mut fields = names.split('\0');
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        if status.starts_with('R') || status.starts_with('C') {
            let old = fields.next()?;
            let new = fields.next()?;
            if new == path {
                return Some(old.to_string());
            }
        } else {
            fields.next();
        }
    }
    None
}

/// Lines in a file; 0 for a binary file or one that cannot be read. A symlink counts as one.
fn count_lines(path: &Path) -> u32 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.file_type().is_symlink() {
        return 1;
    }
    let Ok(mut file) = std::fs::File::open(path) else { return 0 };
    let mut buf = [0u8; 64 * 1024];
    let (mut lines, mut last, mut first) = (0u32, b'\n', true);
    loop {
        let Ok(n) = file.read(&mut buf) else { return 0 };
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if first && chunk[..n.min(8000)].contains(&0) {
            return 0;
        }
        first = false;
        lines = lines.saturating_add(u32::try_from(chunk.iter().filter(|&&b| b == b'\n').count()).unwrap_or(u32::MAX));
        last = chunk[n - 1];
    }
    if last != b'\n' { lines.saturating_add(1) } else { lines }
}

#[cfg(test)]
mod tests;
