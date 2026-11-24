//! Where an instance works: worktrees and branches per run, workdir and variant, and the
//! registry that finds them again after a restart and tells `discard` what is ours to remove.
//!
//! A worktree is reused, in this order:
//!
//! 1. the one a sent-back attempt builds on (`Retry::from_worktree`), when it is this variant's;
//!    another variant's becomes the base of a new branch instead;
//! 2. one an upstream instance of the same variant worked in for the same workdir (an
//!    instance outside the variant scope gives the base branch of a variant's new worktree);
//! 3. the one this node made before in the same attempt (a resumed or continued job).
//!
//! Otherwise a new branch `{branch_prefix}{issue key or run id}[-{variant letter}][-r{attempt}]`
//! is cut from the base, made unique, checked out in a new worktree, and the workdir's `setup`
//! command runs there (again on a reuse when it did not finish). Test runs use
//! `harness-test/{run}/…` branches in a separate folder, also for workdirs used in place.
//! A registered worktree that is gone is an error, never a silent fresh start.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context as _, Result, bail};
use base::process::Cmd;
use model::{LogLevel, RunId, StartFrom, WorkdirDef};
use serde::{Deserialize, Serialize};

use super::Ctx;
use super::git::git;

/// Longest slug the issue key or run id contributes to a branch name.
const KEY_SLUG: usize = 40;

/// A worktree this runner made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub workdir: String,
    pub node: String,
    pub variant: u32,
    pub attempt: u32,
    pub path: PathBuf,
    pub branch: String,
    /// The checkout it was cut from.
    pub repo: PathBuf,
    /// The workdir's setup command ran there to the end; a reuse runs it again otherwise.
    #[serde(default = "yes")]
    pub setup_done: bool,
}

fn yes() -> bool {
    true
}

/// The name of the file beside `places.json` that holds the run's id.
const RUN_FILE: &str = "run-id";

/// The registry of one runner's worktrees, one JSON file per run.
pub(crate) struct Places {
    root: PathBuf,
    lock: Mutex<()>,
}

impl Places {
    pub fn new(root: PathBuf) -> Self {
        Self { root, lock: Mutex::new(()) }
    }

    fn dir(&self, run: &RunId) -> PathBuf {
        self.root.join(base::id::slug(run.as_str(), 80))
    }

    fn file(&self, run: &RunId) -> PathBuf {
        self.dir(run).join("places.json")
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn entries(&self, run: &RunId) -> Result<Vec<Entry>> {
        let _guard = self.guard();
        self.read(run)
    }

    /// The run's entries; none when it has no file yet. A file that does not parse is an error:
    /// reading it as empty would forget every worktree of the run.
    fn read(&self, run: &RunId) -> Result<Vec<Entry>> {
        read_file(&self.file(run))
    }

    /// Writes the entries to a temporary file and renames it over the old one, so a crash
    /// leaves either the old list or the new one.
    fn write(&self, run: &RunId, entries: &[Entry]) -> Result<()> {
        let dir = self.dir(run);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let id_file = dir.join(RUN_FILE);
        if !id_file.exists() {
            std::fs::write(&id_file, run.as_str()).with_context(|| format!("writing {}", id_file.display()))?;
        }
        let file = self.file(run);
        let temp = dir.join(format!("places.json.{}.tmp", base::id::new_id()));
        let text = serde_json::to_string_pretty(entries)?;
        std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, &file).with_context(|| format!("replacing {}", file.display())).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    }

    /// Records `entry`, replacing any entry for the same folder.
    pub fn add(&self, run: &RunId, entry: Entry) -> Result<()> {
        let _guard = self.guard();
        let mut entries = self.read(run)?;
        entries.retain(|e| e.path != entry.path);
        entries.push(entry);
        self.write(run, &entries)
    }

    /// Forgets the entry of `path` and returns it.
    pub fn remove(&self, run: &RunId, path: &Path) -> Result<Option<Entry>> {
        let _guard = self.guard();
        let mut entries = self.read(run)?;
        let Some(pos) = entries.iter().position(|e| e.path == path) else { return Ok(None) };
        let entry = entries.remove(pos);
        self.write(run, &entries)?;
        Ok(Some(entry))
    }

    pub fn find(&self, run: &RunId, path: &Path) -> Result<Option<Entry>> {
        Ok(self.entries(run)?.into_iter().find(|e| e.path == path))
    }

    /// Removes the registry of a run.
    pub fn forget(&self, run: &RunId) {
        let _guard = self.guard();
        let _ = std::fs::remove_dir_all(self.dir(run));
    }

    /// Every run with a registry: its folder, the run id when known, and its entries (`None`
    /// when the file does not parse).
    pub fn all(&self) -> Vec<(PathBuf, Option<String>, Option<Vec<Entry>>)> {
        let _guard = self.guard();
        let Ok(dirs) = std::fs::read_dir(&self.root) else { return Vec::new() };
        dirs.flatten()
            .filter(|d| d.file_type().is_ok_and(|t| t.is_dir()))
            .map(|d| {
                let dir = d.path();
                let id = std::fs::read_to_string(dir.join(RUN_FILE)).ok().map(|s| s.trim().to_string());
                let entries = read_file(&dir.join("places.json")).ok();
                (dir, id, entries)
            })
            .collect()
    }
}

fn read_file(file: &Path) -> Result<Vec<Entry>> {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).with_context(|| {
            format!("The worktree records of this run are damaged ({}); fix or remove the file", file.display())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", file.display())),
    }
}

/// Where one instance works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub path: PathBuf,
    /// `None` when the workdir is used as it is.
    pub branch: Option<String>,
    /// The branch changes are measured against and merged into.
    pub base: String,
}

/// The error of a registered worktree that is no longer on disk.
fn gone(entry: &Entry) -> anyhow::Error {
    anyhow::anyhow!(
        "The worktree of {} is gone ({}); its work cannot continue there. Send the work back to start over, or start a new run",
        entry.node,
        entry.path.display()
    )
}

/// `entry` when its folder still exists, else the error that it is gone.
fn present(entry: Entry) -> Result<Entry> {
    if entry.path.is_dir() { Ok(entry) } else { Err(gone(&entry)) }
}

impl Ctx<'_> {
    /// The workdir definition behind an id.
    pub fn workdir(&self, id: &str) -> Result<&WorkdirDef> {
        match self.runner.workdirs.iter().find(|w| w.id == id) {
            Some(wd) => Ok(wd),
            None if id.is_empty() => bail!("The node names no workdir"),
            None => bail!("The pipeline has no workdir “{id}”"),
        }
    }

    /// The base branch of a workdir: as configured, else what its checkout has out.
    pub fn base_of(&self, wd: &WorkdirDef) -> String {
        if !wd.base_branch.trim().is_empty() {
            return wd.base_branch.trim().to_string();
        }
        git(&wd.path, &["symbolic-ref", "--short", "-q", "HEAD"], self.cancel)
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "HEAD".to_string())
    }

    /// Whether the job works in a worktree of `wd`. A test run always does: a workdir used in
    /// place is the person's own checkout, so it gets a throwaway worktree when it is a git
    /// repository and is refused otherwise.
    pub fn uses_worktree(&self, wd: &WorkdirDef) -> Result<bool> {
        if wd.worktree {
            return Ok(true);
        }
        if !self.job.test {
            return Ok(false);
        }
        if git(&wd.path, &["rev-parse", "--git-dir"], self.cancel).is_err() {
            bail!(
                "A test run works only in throwaway worktrees, and {} ({}) is not a git repository; make it one, or run the pipeline for real",
                wd.name,
                wd.path.display()
            );
        }
        Ok(true)
    }

    /// The place an upstream instance worked in for `wd`, as this job sees it: one of the
    /// same variant to work in, else one from before the variant scope to branch from.
    fn upstream_place(&self, wd: &WorkdirDef) -> Result<(Option<Entry>, Option<Entry>)> {
        let entries = self.runner.places.entries(&self.job.run)?;
        let variant = self.job.key.variant;
        let mut base_from = None;
        for up in self.job.upstream.iter().rev() {
            let Some(path) = &up.output.worktree else { continue };
            let Some(entry) = entries.iter().find(|e| &e.path == path && e.workdir == wd.id) else { continue };
            if variant == 0 || up.key.variant == variant {
                return Ok((Some(entry.clone()), None));
            }
            if base_from.is_none() {
                base_from = Some(entry.clone());
            }
        }
        Ok((None, base_from))
    }

    /// Where an agent (or a command without an upstream worktree) works in `wd`, made when
    /// needed. A registered worktree that is gone is an error, never a fresh start.
    pub fn work_place(&self, wd: &WorkdirDef) -> Result<Place> {
        let base = self.base_of(wd);
        if !self.uses_worktree(wd)? {
            if !wd.path.is_dir() {
                bail!("The workdir “{}” ({}) is not a folder", wd.name, wd.path.display());
            }
            return Ok(Place { path: wd.path.clone(), branch: None, base });
        }
        let run = &self.job.run;
        let key = &self.job.key;
        let mut branch_from: Option<String> = None;
        if let Some(retry) = &self.job.retry
            && retry.start != StartFrom::Over
            && let Some(from) = &retry.from_worktree
            && let Some(entry) = self.runner.places.find(run, from)?.filter(|e| e.workdir == wd.id)
        {
            if entry.variant == key.variant {
                return self.reuse(wd, present(entry)?, base);
            }
            branch_from = retry.from_branch.clone().or(Some(entry.branch));
        }
        let (same, before) = self.upstream_place(wd)?;
        if branch_from.is_none() {
            if let Some(entry) = same {
                return self.reuse(wd, present(entry)?, base);
            }
            branch_from = before.map(|e| e.branch);
        }
        // Starting over never reuses; a resumed job of that attempt does.
        let over = self.job.retry.as_ref().is_some_and(|r| r.start == StartFrom::Over) && self.job.resume_session.is_none();
        let mine = self.runner.places.entries(run)?.into_iter().rev().find(|e| {
            !over && e.workdir == wd.id && e.node == key.node.as_str() && e.variant == key.variant && e.attempt == key.attempt
        });
        if let Some(entry) = mine {
            return self.reuse(wd, present(entry)?, base);
        }
        self.create(wd, branch_from.as_deref().unwrap_or(&base), base.clone())
    }

    /// A place an action works in: what an upstream instance of this variant used, else
    /// `None` for a worktree workdir nobody worked in yet. A registered worktree that is gone
    /// is an error.
    pub fn found_place(&self, wd: &WorkdirDef) -> Result<Option<Place>> {
        let base = self.base_of(wd);
        if !self.uses_worktree(wd)? {
            return Ok(Some(Place { path: wd.path.clone(), branch: None, base }));
        }
        let (same, _) = self.upstream_place(wd)?;
        let entry = match same {
            Some(entry) => Some(entry),
            None => self.runner.places.entries(&self.job.run)?.into_iter().rev().find(|e| e.workdir == wd.id && e.variant == self.job.key.variant),
        };
        let Some(entry) = entry else { return Ok(None) };
        let entry = present(entry)?;
        Ok(Some(Place { path: entry.path, branch: Some(entry.branch), base }))
    }

    /// A place to read `wd` in for this job: the run's worktree when there is one, else the
    /// workdir's own folder.
    pub fn read_place(&self, wd: &WorkdirDef) -> Result<PathBuf> {
        Ok(self.found_place(wd)?.map_or_else(|| wd.path.clone(), |place| place.path))
    }

    fn reuse(&self, wd: &WorkdirDef, mut entry: Entry, base: String) -> Result<Place> {
        let key = &self.job.key;
        let mut changed = false;
        if entry.attempt != key.attempt || entry.node != key.node.as_str() {
            entry.attempt = key.attempt;
            entry.node = key.node.to_string();
            changed = true;
        }
        if !entry.setup_done {
            self.log(LogLevel::Info, "The setup did not finish here last time; running it again.");
            self.setup(wd, &entry.path)?;
            entry.setup_done = true;
            changed = true;
        }
        if changed {
            self.runner.places.add(&self.job.run, entry.clone())?;
        }
        Ok(Place { path: entry.path, branch: Some(entry.branch), base })
    }

    fn wanted_branch(&self, wd: &WorkdirDef) -> String {
        let job = self.job;
        let key = job.item.issue.as_ref().map_or_else(|| job.run.to_string(), |issue| issue.key.to_string());
        let mut name = base::id::slug(&key, KEY_SLUG);
        if job.key.variant > 0 {
            name.push('-');
            name.push_str(&model::variant_letter(job.key.variant).to_lowercase());
        }
        if job.key.attempt > 1 {
            name.push_str(&format!("-r{}", job.key.attempt));
        }
        if job.test {
            format!("harness-test/{}/{name}", base::id::slug(job.run.as_str(), KEY_SLUG))
        } else {
            format!("{}{name}", wd.branch_prefix.trim())
        }
    }

    fn create(&self, wd: &WorkdirDef, from: &str, base: String) -> Result<Place> {
        let repo = &wd.path;
        let branch = vcs::unique_branch(repo, &self.wanted_branch(wd), self.cancel);
        let repo_name = repo.file_name().map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
        let dir = if self.job.test {
            test_dir(&self.runner.config.worktrees, &self.job.run).join(format!("{repo_name}-{}", branch.rsplit('/').next().unwrap_or("work")))
        } else {
            vcs::default_dir(&self.runner.config.worktrees, &repo_name, &branch)
        };
        let made = vcs::worktree::create(repo, &dir, &branch, from, self.cancel)
            .with_context(|| format!("making a worktree of {} for {}", wd.name, self.job.node.name))?;
        let key = &self.job.key;
        let mut entry = Entry {
            workdir: wd.id.clone(),
            node: key.node.to_string(),
            variant: key.variant,
            attempt: key.attempt,
            path: made.path.clone(),
            branch: branch.clone(),
            repo: repo.clone(),
            setup_done: wd.setup.trim().is_empty(),
        };
        // Registered before the setup, so a discard finds it; marked done only after it.
        self.runner.places.add(&self.job.run, entry.clone())?;
        self.log(LogLevel::Info, format!("Working in a new worktree on {branch} (from {from})."));
        if !entry.setup_done {
            self.setup(wd, &made.path)?;
            entry.setup_done = true;
            self.runner.places.add(&self.job.run, entry)?;
        }
        Ok(Place { path: made.path, branch: Some(branch), base })
    }

    /// Runs the workdir's setup command in a worktree.
    fn setup(&self, wd: &WorkdirDef, dir: &Path) -> Result<()> {
        let script = wd.setup.trim();
        if script.is_empty() {
            return Ok(());
        }
        self.log(LogLevel::Info, format!("Setting up: {script}"));
        let mut cmd = Cmd::shell(script).cwd(dir).timeout(self.runner.config.setup_timeout);
        for (k, v) in &wd.env {
            cmd = cmd.env(k, v);
        }
        let out = cmd.run(self.cancel).with_context(|| format!("starting the setup of {}", wd.name))?;
        if out.cancelled {
            bail!("cancelled during setup");
        }
        if !out.success() {
            let why = if out.timed_out { "timed out".to_string() } else { format!("exited with {:?}", out.code) };
            bail!("The setup command of {} {why}: {}", wd.name, super::services::tail(&out.combined(), 600));
        }
        Ok(())
    }
}

/// The folder of a test run's throwaway worktrees.
pub(crate) fn test_dir(worktrees: &Path, run: &RunId) -> PathBuf {
    worktrees.join("_test").join(base::id::slug(run.as_str(), KEY_SLUG))
}

/// Removes a worktree and its branch that this runner made. Best effort.
pub(crate) fn remove(entry: &Entry) {
    let cancel = base::cancel::CancelToken::new();
    let _ = vcs::worktree::remove(&entry.repo, &entry.path, true, &cancel);
    let _ = git(&entry.repo, &["branch", "-D", &entry.branch], &cancel);
}
