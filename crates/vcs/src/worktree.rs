//! Linked worktrees: where agents work on their own branch without touching the checkout.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use base::cancel::CancelToken;
use base::id::slug;

use crate::exec::{LOCAL, QUICK, Tool};

/// Longest slug a branch name gets from a title.
const SLUG_MAX: usize = 40;

/// One entry of `git worktree list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` on a detached head.
    pub branch: Option<String>,
    /// Full hash of the checked-out commit.
    pub head: String,
    /// The first entry: the checkout the repository lives in.
    pub is_main: bool,
}

/// An entry of the porcelain listing, bare repositories and stale entries included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawWorktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    /// The folder is gone; `git worktree prune` would drop the entry.
    pub prunable: bool,
}

pub(crate) fn parse_porcelain(text: &str) -> Vec<RawWorktree> {
    let mut list = Vec::new();
    for block in text.split("\n\n") {
        let mut entry = RawWorktree {
            path: PathBuf::new(),
            head: String::new(),
            branch: None,
            bare: false,
            prunable: false,
        };
        let mut has_path = false;
        for line in block.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                entry.path = PathBuf::from(path);
                has_path = true;
            } else if let Some(head) = line.strip_prefix("HEAD ") {
                entry.head = head.trim().to_string();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string());
            } else if line == "bare" {
                entry.bare = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                entry.prunable = true;
            }
        }
        if has_path {
            list.push(entry);
        }
    }
    list
}

/// The worktrees of the repository that contains `repo`, main checkout first. Empty when
/// `repo` is not a repository.
pub fn list(repo: &Path, cancel: &CancelToken) -> Vec<Worktree> {
    let Some(out) = Tool::git(repo).args(["worktree", "list", "--porcelain"]).query(cancel) else {
        return Vec::new();
    };
    parse_porcelain(&out)
        .into_iter()
        .enumerate()
        .filter(|(_, w)| !w.bare && !w.prunable)
        .map(|(i, w)| Worktree { path: w.path, branch: w.branch, head: w.head, is_main: i == 0 })
        .collect()
}

/// `wanted`, or `wanted-2`, `wanted-3`… when a branch of that name exists, locally or on a
/// remote, or when git could not create it next to an existing one (`chat` against `chat/x`;
/// `chat/x/y` against `chat/x` becomes `chat/x-y`).
pub fn unique_branch(repo: &Path, wanted: &str, cancel: &CancelToken) -> String {
    let taken = taken_refs(repo, cancel);
    let free = |name: &str| !conflicts(&taken, name);
    // Under an existing branch (`chat/fix` taken, `chat/fix/deep` wanted) no suffix helps:
    // fold the rest into one segment instead (`chat/fix-deep`).
    let mut wanted = wanted.to_string();
    while let Some((parent, rest)) = taken
        .iter()
        .find_map(|e| wanted.strip_prefix(e.as_str()).and_then(|r| r.strip_prefix('/')).map(|r| (e, r)))
    {
        wanted = format!("{parent}-{}", rest.replace('/', "-"));
    }
    if free(&wanted) {
        return wanted;
    }
    (2..1000)
        .map(|n| format!("{wanted}-{n}"))
        .find(|name| free(name))
        .unwrap_or_else(|| format!("{wanted}-{}", base::time::now_ms()))
}

/// Branch names that exist locally or as remote-tracking branches.
fn taken_refs(repo: &Path, cancel: &CancelToken) -> BTreeSet<String> {
    let Some(out) = Tool::git(repo)
        .args(["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])
        .query(cancel)
    else {
        return BTreeSet::new();
    };
    out.lines()
        .filter_map(|line| {
            if let Some(local) = line.strip_prefix("refs/heads/") {
                Some(local.to_string())
            } else {
                // refs/remotes/<remote>/<branch>
                let rest = line.strip_prefix("refs/remotes/")?;
                let (_, branch) = rest.split_once('/')?;
                (branch != "HEAD").then(|| branch.to_string())
            }
        })
        .collect()
}

/// A name conflicts with an existing one when it is equal or one is a folder of the other.
fn conflicts(taken: &BTreeSet<String>, name: &str) -> bool {
    taken.iter().any(|existing| {
        existing == name
            || existing.strip_prefix(name).is_some_and(|rest| rest.starts_with('/'))
            || name.strip_prefix(existing.as_str()).is_some_and(|rest| rest.starts_with('/'))
    })
}

/// `prefix` plus a slug of `title`: `chat/` and "Fix double charge retry" give
/// `chat/fix-double-charge-retry`.
pub fn branch_name(prefix: &str, title: &str) -> String {
    format!("{prefix}{}", slug(title, SLUG_MAX))
}

/// Makes a worktree at `dir` on the new branch `branch`, started from `base`.
///
/// Fails with a readable message when `repo` is not a repository, `base` does not exist,
/// `branch` exists already or is not a valid name, or `dir` exists and is not empty.
pub fn create(repo: &Path, dir: &Path, branch: &str, base: &str, cancel: &CancelToken) -> Result<Worktree> {
    let (branch, base) = (branch.trim(), base.trim());
    if branch.is_empty() {
        bail!("the branch name is empty");
    }
    if base.is_empty() {
        bail!("the base branch is empty");
    }
    if branch.starts_with('-') || base.starts_with('-') {
        bail!("`{}` is not a valid branch name", if branch.starts_with('-') { branch } else { base });
    }
    if Tool::git(repo).args(["rev-parse", "--git-dir"]).query(cancel).is_none() {
        bail!("{} is not a git repository", repo.display());
    }
    if !Tool::git(repo).args(["check-ref-format", "--branch", branch]).succeeds(cancel) {
        bail!("`{branch}` is not a valid branch name");
    }
    let commit = format!("{base}^{{commit}}");
    if !Tool::git(repo).args(["rev-parse", "--verify", "--quiet", &commit]).succeeds(cancel) {
        bail!("the base `{base}` does not exist in {}", repo.display());
    }
    let local = format!("refs/heads/{branch}");
    if Tool::git(repo).args(["show-ref", "--verify", "--quiet", &local]).succeeds(cancel) {
        bail!("the branch `{branch}` already exists");
    }
    if dir.exists() {
        let empty = dir.is_dir() && std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none());
        if !empty {
            bail!("{} already exists", dir.display());
        }
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Entries of worktrees whose folder was deleted would make git refuse to reuse a branch.
    let _ = Tool::git(repo).args(["worktree", "prune"]).run(cancel);
    Tool::git(repo)
        .args(["worktree", "add", "--no-track", "-b", branch])
        .arg(dir)
        .arg(base)
        .timeout(LOCAL)
        .check(cancel)?;
    let path = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let head = Tool::git(&path)
        .args(["rev-parse", "HEAD"])
        .timeout(QUICK)
        .check(cancel)?
        .trim()
        .to_string();
    Ok(Worktree { path, branch: Some(branch.to_string()), head, is_main: false })
}

/// Removes the worktree at `dir`. Without `force`, uncommitted changes make it fail; with it
/// they are thrown away. The branch stays. A folder that is already gone just has its entry
/// pruned.
pub fn remove(repo: &Path, dir: &Path, force: bool, cancel: &CancelToken) -> Result<()> {
    if !dir.exists() {
        Tool::git(repo).args(["worktree", "prune"]).check(cancel)?;
        return Ok(());
    }
    let mut tool = Tool::git(repo).args(["worktree", "remove"]);
    if force {
        // Twice: the second one also removes a locked worktree.
        tool = tool.args(["--force", "--force"]);
    }
    tool.arg(dir).timeout(LOCAL).check(cancel)?;
    Ok(())
}

/// Where a worktree goes: `<root>/<repo>/<branch with / replaced by ->`.
pub fn default_dir(worktrees_root: &Path, repo_name: &str, branch: &str) -> PathBuf {
    worktrees_root.join(repo_name).join(branch.replace('/', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, commit_file, git, write};

    fn cancel() -> CancelToken {
        CancelToken::new()
    }

    #[test]
    fn branch_names_come_from_titles() {
        assert_eq!(branch_name("chat/", "Fix double charge retry"), "chat/fix-double-charge-retry");
        assert_eq!(branch_name("har-4/", "Починить вход"), "har-4/pochinit-vkhod");
        assert_eq!(branch_name("chat/", "!!!"), "chat/item");
        assert!(branch_name("chat/", &"word ".repeat(30)).len() <= "chat/".len() + SLUG_MAX);
    }

    #[test]
    fn default_dir_flattens_the_branch() {
        assert_eq!(
            default_dir(Path::new("/w"), "payments-api", "chat/fix-retry"),
            PathBuf::from("/w/payments-api/chat-fix-retry")
        );
    }

    #[test]
    fn unique_branch_counts_up() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("unique");
        assert_eq!(unique_branch(&repo, "chat/fix", &cancel()), "chat/fix");
        git(&repo, &["branch", "chat/fix"]);
        assert_eq!(unique_branch(&repo, "chat/fix", &cancel()), "chat/fix-2");
        git(&repo, &["branch", "chat/fix-2"]);
        assert_eq!(unique_branch(&repo, "chat/fix", &cancel()), "chat/fix-3");
        // `main` exists, so it counts up too.
        assert_eq!(unique_branch(&repo, "main", &cancel()), "main-2");
    }

    #[test]
    fn unique_branch_avoids_ref_folder_clashes_and_remote_branches() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("clash");
        git(&repo, &["branch", "chat/fix"]);
        // `chat` cannot be created next to `chat/fix`, and `chat/fix/deep` cannot either.
        assert_eq!(unique_branch(&repo, "chat", &cancel()), "chat-2");
        assert_eq!(unique_branch(&repo, "chat/fix/deep", &cancel()), "chat/fix-deep");
        // A remote-tracking branch counts.
        git(&repo, &["update-ref", "refs/remotes/origin/feat/x", "HEAD"]);
        assert_eq!(unique_branch(&repo, "feat/x", &cancel()), "feat/x-2");
        // Not a repo: the wanted name is returned.
        assert_eq!(unique_branch(sandbox.path(), "x", &cancel()), "x");
    }

    #[test]
    fn create_list_and_remove() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("wt");
        let dir = default_dir(&sandbox.path().join("worktrees"), "wt", "chat/fix");
        let made = create(&repo, &dir, "chat/fix", "main", &cancel()).expect("create");
        assert_eq!(made.path, dir);
        assert_eq!(made.branch.as_deref(), Some("chat/fix"));
        assert!(!made.is_main);
        assert_eq!(made.head.len(), 40);
        assert!(dir.join("README.md").is_file());

        let all = list(&repo, &cancel());
        assert_eq!(all.len(), 2);
        assert!(all[0].is_main);
        assert_eq!(all[0].path, repo);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
        assert_eq!(all[1].path, dir);
        assert_eq!(all[1].branch.as_deref(), Some("chat/fix"));
        assert!(!all[1].is_main);
        // Listing from inside the worktree gives the same repo.
        assert_eq!(list(&dir, &cancel()).len(), 2);

        remove(&repo, &dir, false, &cancel()).expect("remove");
        assert!(!dir.exists());
        assert_eq!(list(&repo, &cancel()).len(), 1);
        // The branch stays, and removing a folder that is gone is fine.
        assert!(git(&repo, &["branch", "--list", "chat/fix"]).contains("chat/fix"));
        remove(&repo, &dir, false, &cancel()).expect("remove again");
    }

    #[test]
    fn create_does_not_track_the_base() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("track");
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let dir = sandbox.path().join("t");
        create(&repo, &dir, "chat/t", "origin/main", &cancel()).expect("create");
        let upstream = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["config", "--get", "branch.chat/t.merge"])
            .output()
            .unwrap();
        assert!(!upstream.status.success(), "no upstream expected");
    }

    #[test]
    fn create_errors_are_readable() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("errors");
        let dir = sandbox.path().join("w1");
        let err = |r: Result<Worktree>| r.expect_err("should fail").to_string();

        let e = err(create(&repo, &dir, "chat/a", "nope", &cancel()));
        assert!(e.contains("base `nope` does not exist"), "{e}");
        let e = err(create(&repo, &dir, "main", "main", &cancel()));
        assert!(e.contains("`main` already exists"), "{e}");
        let e = err(create(&repo, &dir, "bad name", "main", &cancel()));
        assert!(e.contains("not a valid branch name"), "{e}");
        let e = err(create(&repo, &dir, "", "main", &cancel()));
        assert!(e.contains("empty"), "{e}");
        let e = err(create(&repo, &dir, "-x", "main", &cancel()));
        assert!(e.contains("not a valid"), "{e}");
        let e = err(create(sandbox.path(), &dir, "chat/a", "main", &cancel()));
        assert!(e.contains("not a git repository"), "{e}");

        write(&dir, "file.txt", "occupied");
        let e = err(create(&repo, &dir, "chat/a", "main", &cancel()));
        assert!(e.contains("already exists"), "{e}");
        // An empty folder is fine.
        let empty = sandbox.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        create(&repo, &empty, "chat/a", "main", &cancel()).expect("empty dir is fine");
    }

    #[test]
    fn remove_refuses_dirty_worktrees_unless_forced() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("dirty-wt");
        let dir = sandbox.path().join("w");
        create(&repo, &dir, "chat/d", "main", &cancel()).expect("create");
        write(&dir, "scratch.txt", "work in progress");
        let err = remove(&repo, &dir, false, &cancel()).expect_err("dirty");
        assert!(err.to_string().contains("git worktree remove"), "{err}");
        assert!(dir.exists());
        remove(&repo, &dir, true, &cancel()).expect("forced");
        assert!(!dir.exists());
    }

    #[test]
    fn a_deleted_worktree_folder_does_not_block_reuse_of_the_branch() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("stale");
        let dir = sandbox.path().join("w");
        create(&repo, &dir, "chat/s", "main", &cancel()).expect("create");
        std::fs::remove_dir_all(&dir).unwrap();
        // The entry is stale: not listed, and a new worktree can be made elsewhere.
        assert_eq!(list(&repo, &cancel()).len(), 1);
        commit_file(&repo, "later.txt", "x");
        let again = sandbox.path().join("w2");
        create(&repo, &again, "chat/s2", "main", &cancel()).expect("create again");
    }

    #[test]
    fn porcelain_parser_reads_detached_and_bare_entries() {
        let text = "worktree /srv/repo.git\nbare\n\n\
            worktree /w/a\nHEAD aaa\nbranch refs/heads/chat/a\n\n\
            worktree /w/b\nHEAD bbb\ndetached\n\n\
            worktree /w/gone\nHEAD ccc\nbranch refs/heads/x\nprunable gitdir file points to non-existent location\n";
        let raw = parse_porcelain(text);
        assert_eq!(raw.len(), 4);
        assert!(raw[0].bare);
        assert_eq!(raw[1].branch.as_deref(), Some("chat/a"));
        assert_eq!(raw[2].branch, None);
        assert_eq!(raw[2].head, "bbb");
        assert!(raw[3].prunable);
    }
}
