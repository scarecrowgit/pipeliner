//! Merging a branch into its base without touching what is checked out.

use std::path::Path;

use anyhow::{Result, bail};
use base::cancel::CancelToken;

use crate::exec::{LOCAL, Tool, git_committing};
use crate::worktree;

/// What [`merge_into_base`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// A merge commit was made on the base branch; `head` is its hash.
    Merged { head: String },
    /// The base already contains the branch.
    UpToDate,
    /// The branches change the same lines. Nothing was changed anywhere.
    Conflict { files: Vec<String> },
}

/// Merges `branch` into the local branch `base` with a merge commit.
///
/// The merge is computed in memory (`git merge-tree`, git 2.38 or newer), so a conflict
/// leaves no half-finished merge behind and a dirty main checkout is never in the way. When
/// `base` is checked out somewhere, that checkout moves to the merge commit with
/// `git reset --keep`: uncommitted changes in other files stay as they are, and if they are
/// in files the merge touches, the whole thing fails with nothing changed.
pub fn merge_into_base(repo: &Path, branch: &str, base: &str, cancel: &CancelToken) -> Result<MergeOutcome> {
    let (branch, base) = (branch.trim(), base.trim());
    if branch.is_empty() || base.is_empty() || branch.starts_with('-') || base.starts_with('-') {
        bail!("a merge needs a branch and a base branch");
    }
    if branch == base {
        bail!("cannot merge `{branch}` into itself");
    }
    let base_ref = format!("refs/heads/{base}");
    if !Tool::git(repo).args(["show-ref", "--verify", "--quiet", &base_ref]).succeeds(cancel) {
        bail!("the base `{base}` is not a local branch of {}", repo.display());
    }
    let Some(base_oid) = super::resolve_commit(repo, &base_ref, cancel) else {
        bail!("the base `{base}` has no commits");
    };
    let Some(branch_oid) = super::resolve_commit(repo, branch, cancel) else {
        bail!("the branch `{branch}` does not exist in {}", repo.display());
    };
    if Tool::git(repo)
        .args(["merge-base", "--is-ancestor", &branch_oid, &base_oid])
        .succeeds(cancel)
    {
        return Ok(MergeOutcome::UpToDate);
    }

    let out = Tool::git(repo)
        .args(["merge-tree", "--write-tree", "--name-only", "-z", &base_oid, &branch_oid])
        .timeout(LOCAL)
        .run(cancel)?;
    let tree = match out.code {
        Some(0) => out.stdout.split('\0').next().unwrap_or_default().trim().to_string(),
        Some(1) => {
            return Ok(MergeOutcome::Conflict { files: conflicted_files(&out.stdout) });
        }
        _ => {
            let text = out.stderr.trim();
            if text.contains("unknown option") || text.contains("usage: git merge-tree") {
                bail!("merging needs git 2.38 or newer (git merge-tree --write-tree)");
            }
            return Err(crate::exec::failure("git merge-tree --write-tree", &out));
        }
    };
    if tree.is_empty() {
        bail!("git merge-tree printed no tree");
    }

    let message = format!("Merge branch '{branch}' into {base}\n");
    let merged = git_committing(repo, cancel)
        .args(["commit-tree", &tree, "-p", &base_oid, "-p", &branch_oid])
        .stdin(message)
        .timeout(LOCAL)
        .check(cancel)?;
    let merged = merged.trim().to_string();

    let checkout = worktree::list(repo, cancel)
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(base));
    match checkout {
        Some(checkout) => {
            Tool::git(&checkout.path)
                .args(["reset", "--keep", "--quiet", &merged])
                .timeout(LOCAL)
                .check(cancel)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "`{base}` is checked out in {} with changes the merge would overwrite; nothing was merged ({e})",
                        checkout.path.display()
                    )
                })?;
        }
        None => {
            Tool::git(repo)
                .args(["update-ref", "-m", &format!("merge {branch}"), &base_ref, &merged, &base_oid])
                .check(cancel)?;
        }
    }
    Ok(MergeOutcome::Merged { head: merged })
}

/// The file names of `git merge-tree --write-tree --name-only -z` output: the tree id, then
/// the conflicted paths, then an empty field, then messages.
fn conflicted_files(output: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for field in output.split('\0').skip(1) {
        if field.is_empty() {
            break;
        }
        if !files.iter().any(|f| f == field) {
            files.push(field.to_string());
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_output_is_split_at_the_empty_field() {
        let out = "abc\0src/a.rs\0src/b.rs\0src/a.rs\0\0Auto-merging x\0CONFLICT (content): x\0";
        assert_eq!(conflicted_files(out), ["src/a.rs", "src/b.rs"]);
        assert!(conflicted_files("abc\0").is_empty());
    }
}
