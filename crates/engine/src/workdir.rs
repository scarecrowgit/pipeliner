//! Folders sessions work in: preparing a [`WorkdirChoice`] (cutting a worktree when asked),
//! and the folder queries of the "new chat" page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use base::cancel::CancelToken;
use model::{Event, NoticeLevel, RepoHost, WorkMode, Workdir};
use store::{ParentFilter, SessionFilter};

use crate::api::{ChildDir, FolderInfo, RepoFacts, WorkdirApi, WorkdirChoice, WorkdirEntry};
use crate::engine::{Engine, Inner};

/// Prefix of the branches chats get.
const CHAT_BRANCH_PREFIX: &str = "chat/";
/// Recent folders listed at most.
const RECENT_MAX: usize = 12;
/// Sessions looked at for recent folders.
const RECENT_SCAN: usize = 300;

impl Inner {
    /// Turns a choice into the folder the session works in. Worktree mode cuts a new worktree
    /// from the checkout on a new branch; the other modes take the folder as it is.
    pub fn prepare_workdir(&self, choice: &WorkdirChoice, hint: &str) -> Result<Workdir> {
        let cancel = CancelToken::new();
        let path = self.paths.expand(&choice.path.to_string_lossy());
        if !path.is_dir() {
            bail!("{} is not a folder", self.paths.display(&path));
        }
        let path = path.canonicalize().unwrap_or(path);
        let facts = vcs::facts(&path, &cancel);
        let runtime = "local".to_string();
        if choice.mode != WorkMode::Worktree {
            return Ok(Workdir {
                repo: facts.as_ref().map_or_else(|| folder_name(&path), |f| f.name.clone()),
                branch: facts.as_ref().and_then(|f| f.branch.clone()),
                host: facts.as_ref().map_or(RepoHost::Local, |f| f.host),
                mode: choice.mode,
                source: facts.as_ref().filter(|f| f.is_worktree).map(|f| f.main_checkout.clone()),
                runtime,
                path,
            });
        }
        let Some(facts) = facts else {
            bail!("{} is not a git repository, so a worktree cannot be made there", self.paths.display(&path));
        };
        let repo = facts.main_checkout.clone();
        let base = choice
            .base
            .clone()
            .filter(|b| !b.trim().is_empty())
            .or_else(|| facts.branch.clone())
            .unwrap_or_else(|| "HEAD".to_string());
        let wanted = choice
            .branch
            .clone()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| vcs::branch_name(CHAT_BRANCH_PREFIX, hint));
        let branch = vcs::unique_branch(&repo, &wanted, &cancel);
        let dir = vcs::default_dir(&self.paths.worktrees_dir(), &facts.name, &branch);
        let worktree = vcs::worktree::create(&repo, &dir, &branch, &base, &cancel)?;
        Ok(Workdir {
            path: worktree.path,
            repo: facts.name,
            branch: Some(branch),
            host: facts.host,
            mode: WorkMode::Worktree,
            source: Some(repo),
            runtime,
        })
    }

    /// Who works in `path` right now: the runnable of a running session there.
    fn owner_of(&self, path: &Path) -> Option<String> {
        let catalog = self.catalog();
        self.live.running().into_iter().find_map(|id| {
            let session = self.store.session(&id).ok()??;
            let workdir = session.workdir?;
            (workdir.path == path).then(|| catalog.get(&session.runnable).map_or(session.title, |r| r.name.clone()))
        })
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl WorkdirApi for Engine {
    fn recent_workdirs(&self) -> Vec<WorkdirEntry> {
        let inner = &self.inner;
        let filter = SessionFilter { parent: ParentFilter::TopLevel, limit: Some(RECENT_SCAN), ..Default::default() };
        let sessions = inner.store.sessions(&filter).unwrap_or_default();
        // Most recent first; the first session seen for a path is its latest.
        let mut order: Vec<PathBuf> = Vec::new();
        let mut seen: BTreeMap<PathBuf, (Workdir, u32)> = BTreeMap::new();
        for session in sessions {
            let Some(workdir) = session.workdir else { continue };
            match seen.get_mut(&workdir.path) {
                Some((_, count)) => *count += 1,
                None => {
                    order.push(workdir.path.clone());
                    seen.insert(workdir.path.clone(), (workdir, 1));
                }
            }
        }
        let cancel = CancelToken::new();
        order
            .into_iter()
            .take(RECENT_MAX)
            .filter_map(|path| {
                let (workdir, sessions) = seen.remove(&path)?;
                let facts = vcs::facts(&workdir.path, &cancel);
                Some(WorkdirEntry {
                    status: facts.as_ref().map_or_else(|| "no repository".to_string(), vcs::status_line),
                    branch: facts.as_ref().and_then(|f| f.branch.clone()).or(workdir.branch),
                    owner: inner.owner_of(&workdir.path),
                    is_worktree: workdir.source.is_some(),
                    repo: workdir.repo,
                    host: workdir.host,
                    path: workdir.path,
                    sessions,
                })
            })
            .collect()
    }

    fn worktrees(&self, repo: &Path) -> Vec<WorkdirEntry> {
        let inner = &self.inner;
        let cancel = CancelToken::new();
        let sessions = inner.store.sessions(&SessionFilter::default()).unwrap_or_default();
        vcs::worktree::list(repo, &cancel)
            .into_iter()
            .map(|wt| {
                let facts = vcs::facts(&wt.path, &cancel);
                let used = sessions.iter().filter(|s| s.workdir.as_ref().is_some_and(|w| w.path == wt.path)).count();
                WorkdirEntry {
                    repo: facts.as_ref().map_or_else(|| folder_name(&wt.path), |f| f.name.clone()),
                    branch: wt.branch.clone(),
                    host: facts.as_ref().map_or(RepoHost::Local, |f| f.host),
                    is_worktree: !wt.is_main,
                    status: facts.as_ref().map_or_else(String::new, vcs::status_line),
                    owner: inner.owner_of(&wt.path),
                    sessions: used as u32,
                    path: wt.path,
                }
            })
            .collect()
    }

    fn repo_facts(&self, path: &Path) -> Option<RepoFacts> {
        vcs::facts(path, &CancelToken::new())
    }

    fn branches(&self, repo: &Path) -> Vec<String> {
        vcs::branches(repo, &CancelToken::new())
    }

    fn folder_info(&self, path: &Path) -> FolderInfo {
        vcs::folder(path, &CancelToken::new())
    }

    fn suggest_branch(&self, repo: &Path, hint: &str) -> String {
        vcs::unique_branch(repo, &vcs::branch_name(CHAT_BRANCH_PREFIX, hint), &CancelToken::new())
    }

    fn folder_exists(&self, path: &Path) -> bool {
        self.inner.paths.expand(&path.to_string_lossy()).is_dir()
    }

    fn list_dir(&self, dir: &Path, prefix: &str, limit: usize) -> Result<Vec<ChildDir>> {
        let dir = self.inner.paths.expand(&dir.to_string_lossy());
        vcs::child_dirs(&dir, prefix, limit).with_context(|| format!("Could not read {}", self.inner.paths.display(&dir)))
    }

    fn clone_repo(&self, url: &str, dest: &Path) -> Result<()> {
        let inner = self.inner.clone();
        let (url, dest) = (url.to_string(), dest.to_path_buf());
        std::thread::Builder::new().name("clone".into()).spawn(move || {
            let cancel = CancelToken::new();
            let (level, text) = match vcs::clone::clone(&url, &dest, &cancel) {
                Ok(()) => (NoticeLevel::Success, format!("Cloned into {}", inner.paths.display(&dest))),
                Err(error) => (NoticeLevel::Error, format!("Could not clone: {error:#}")),
            };
            inner.publish(Event::Notice { level, text });
        })?;
        Ok(())
    }
}

/// Puts back the worktree of `branch` at `dir`, whose folder was deleted, from the checkout
/// `repo`. Fails with a reason a person can act on when the branch is gone too.
pub(crate) fn restore_worktree(repo: &Path, dir: &Path, branch: &str) -> Result<()> {
    let cancel = CancelToken::new();
    if !repo.is_dir() {
        bail!("its worktree {} is gone, and so is the checkout {} it came from", dir.display(), repo.display());
    }
    if !vcs::branches(repo, &cancel).iter().any(|b| b == branch) {
        bail!("its worktree {} is gone and the branch `{branch}` no longer exists in {}", dir.display(), repo.display());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // The entry of the deleted folder would make git refuse to check the branch out again.
    let _ = git(repo, &["worktree", "prune"]);
    let dir_arg = dir.to_string_lossy();
    git(repo, &["worktree", "add", &dir_arg, branch])
        .with_context(|| format!("putting back the worktree of `{branch}` at {}", dir.display()))?;
    Ok(())
}

/// Checks that `dir` is still a worktree of `repo` with `branch` checked out; fails with what
/// it is instead.
pub(crate) fn check_worktree(repo: &Path, dir: &Path, branch: &str) -> Result<()> {
    let common = |at: &Path| {
        git(at, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()
            .map(|out| PathBuf::from(out.trim()))
            .map(|p| p.canonicalize().unwrap_or(p))
    };
    let Some(own) = common(dir) else {
        bail!("{} is no longer the worktree of `{branch}`: it is not a git checkout; move it away to have it put back", dir.display());
    };
    if common(repo) != Some(own) {
        bail!("{} is no longer the worktree of `{branch}`: it belongs to another repository", dir.display());
    }
    let head = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if head.trim() != branch {
        bail!("{} has `{}` checked out instead of `{branch}`; check `{branch}` out there again", dir.display(), head.trim());
    }
    Ok(())
}

/// Runs git in `dir` without a terminal to prompt on; fails with its error output.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .context("starting git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
