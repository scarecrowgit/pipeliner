//! The worktree sweep: a real run that ended `Done` or `Cancelled` more than
//! `Settings::worktree_retention_days` ago, and whose branch is merged into its base or has a
//! pull request recorded, loses the worktrees its runner made. The branch stays unless it is
//! merged. A worktree with uncommitted changes, one the run's runner did not register, and any
//! run that is still open are never touched.
//!
//! The runner's registry (`<pipelines state>/runs/<run>/places.json`) says which worktrees a run
//! made; it is read as JSON here and an entry is dropped once its worktree is gone.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use base::cancel::CancelToken;
use base::process::Cmd;
use model::{Run, RunId};
use serde_json::Value;

use crate::api::SweptWorktree;
use crate::engine::Inner;
use crate::pipelines;

const DAY_MS: i64 = 24 * 3_600_000;

/// One registered worktree of a run, as the runner records it.
struct Place {
    workdir: String,
    path: PathBuf,
    branch: String,
    repo: PathBuf,
}

fn registry_file(inner: &Inner, run: &RunId) -> PathBuf {
    pipelines::runner_config(inner).state.join("runs").join(base::id::slug(run.as_str(), 80)).join("places.json")
}

fn read_places(file: &Path) -> Result<Option<Vec<Value>>> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(serde_json::from_str(&text).with_context(|| format!("reading {}", file.display()))?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", file.display())),
    }
}

fn place_of(value: &Value) -> Option<Place> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Some(Place {
        workdir: text("workdir").unwrap_or_default(),
        path: PathBuf::from(text("path")?),
        branch: text("branch")?,
        repo: PathBuf::from(text("repo")?),
    })
}

/// Writes the registry without the entries of `removed`, through a temporary file.
fn forget_places(file: &Path, entries: &[Value], removed: &[PathBuf]) -> Result<()> {
    let kept: Vec<&Value> = entries
        .iter()
        .filter(|e| !e.get("path").and_then(Value::as_str).is_some_and(|p| removed.iter().any(|r| r == Path::new(p))))
        .collect();
    let temp = file.with_extension(format!("json.{}.tmp", base::id::new_id()));
    std::fs::write(&temp, serde_json::to_string_pretty(&kept)?).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, file).with_context(|| format!("replacing {}", file.display()))
}

fn git(repo: &Path, args: &[&str]) -> Option<base::process::Output> {
    Cmd::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .timeout(std::time::Duration::from_secs(30))
        .run(&CancelToken::new())
        .ok()
}

/// Whether `branch` is contained in `base` (an ancestor of it) in `repo`.
fn merged_into(repo: &Path, branch: &str, base: &str) -> bool {
    git(repo, &["merge-base", "--is-ancestor", &format!("refs/heads/{branch}"), base]).is_some_and(|out| out.success())
}

/// The base branch the run's graph names for a workdir; `main` when it names none.
fn base_branch(inner: &Inner, run: &Run, workdir: &str) -> String {
    pipelines::run_graph(inner, &run.id)
        .and_then(|graph| graph.workdir(workdir).map(|wd| wd.base_branch.clone()))
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Sweeps (or, with `dry_run`, lists) the worktrees that may go. A worktree that cannot be
/// removed is left as it is and not reported.
pub(crate) fn sweep(inner: &Inner, dry_run: bool) -> Result<Vec<SweptWorktree>> {
    let days = inner.store.settings()?.worktree_retention_days;
    if days == 0 {
        return Ok(Vec::new());
    }
    let cutoff = inner.now() - i64::from(days) * DAY_MS;
    let cancel = CancelToken::new();
    let mut out = Vec::new();
    for run in inner.store.closed_runs_before(cutoff)? {
        let file = registry_file(inner, &run.id);
        // A damaged registry is left for a person to look at.
        let Ok(Some(entries)) = read_places(&file) else { continue };
        let mut removed = Vec::new();
        for place in entries.iter().filter_map(place_of) {
            if !place.path.is_dir() {
                continue;
            }
            let Some(facts) = vcs::facts(&place.path, &cancel) else { continue };
            if facts.dirty > 0 || facts.branch.as_deref() != Some(place.branch.as_str()) {
                continue;
            }
            let base = base_branch(inner, &run, &place.workdir);
            let merged = merged_into(&place.repo, &place.branch, &base);
            if !merged && run.pr_url.is_none() {
                continue;
            }
            if !dry_run {
                if vcs::worktree::remove(&place.repo, &place.path, false, &cancel).is_err() {
                    continue;
                }
                if merged {
                    let _ = git(&place.repo, &["branch", "-D", &place.branch]);
                }
                removed.push(place.path.clone());
            }
            out.push(SweptWorktree {
                run: run.id.clone(),
                path: place.path,
                branch: place.branch,
                branch_removed: merged,
                reason: if merged { format!("merged into {base}") } else { "has a pull request".to_string() },
            });
        }
        if !removed.is_empty() {
            forget_places(&file, &entries, &removed)?;
        }
    }
    Ok(out)
}
