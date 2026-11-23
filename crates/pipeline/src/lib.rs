//! Pipelines: configurable sequences of steps (agents, decisions, renders, approvals,
//! commands, merges, pull requests, reports) over named workdirs — see [`def`]. Issues on the
//! board run through one pipeline each (see [`stages`] for the engine). [`Pipelines`] is the
//! app-wide service the screens use: it owns pipelines, issues and runs, starts worker
//! threads and re-renders observers when a run changes.

pub mod agent;
pub mod config;
pub mod def;
pub mod exec;
pub mod model;
pub mod stages;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, AppContext as _, Context, Entity, Global};

pub use def::{Delegate, PipelineDef, StepDef, StepKind, Tools, WorkdirDef};
pub use model::{
    AgentUsage, CoderTask, Decision, Issue, IssueStatus, LogKind, LogLine, Render, Run, RunStatus,
    RunWorkdir, Stage, StageStatus,
};
pub use stages::Job;

enum Msg {
    Changed,
    Finished(String),
}

pub struct Pipelines {
    /// All pipelines, by name.
    pub pipelines: Vec<PipelineDef>,
    /// Pipeline files that could not be read or written.
    pub pipeline_errors: Vec<String>,
    /// Newest first.
    pub issues: Vec<Issue>,
    runs: HashMap<String, Arc<Mutex<Run>>>,
    /// Runs with a live worker → its cancel flag.
    active: HashMap<String, Arc<AtomicBool>>,
    tx: smol::channel::Sender<Msg>,
    _rx_task: gpui::Task<()>,
}

struct GlobalPipelines(Entity<Pipelines>);
impl Global for GlobalPipelines {}

/// Create the service (once, by the app shell).
pub fn init(cx: &mut App) -> Entity<Pipelines> {
    let e = cx.new(Pipelines::new);
    cx.set_global(GlobalPipelines(e.clone()));
    e
}

pub fn global(cx: &App) -> Option<Entity<Pipelines>> {
    cx.try_global::<GlobalPipelines>().map(|g| g.0.clone())
}

impl Pipelines {
    fn new(cx: &mut Context<Self>) -> Self {
        let (pipelines, pipeline_errors) = def::load_all();
        let mut runs = HashMap::new();
        for id in model::load_issues().into_iter().filter_map(|i| i.run) {
            if let Some(mut run) = model::load_run(&id) {
                // A worker cannot survive an app restart.
                if run.status == RunStatus::Running {
                    run.status = RunStatus::Failed;
                    run.error = Some("Interrupted: the app was closed during the run. Retry to continue.".into());
                    for s in run.stages.iter_mut().filter(|s| s.status == StageStatus::Running) {
                        s.status = StageStatus::Failed;
                        s.summary = "interrupted".into();
                    }
                    let _ = run.save();
                    let _ = model::update_issue(&run.issue, |i| i.status = IssueStatus::Failed);
                }
                runs.insert(id, Arc::new(Mutex::new(run)));
            }
        }
        let issues = model::load_issues();
        let (tx, rx) = smol::channel::unbounded::<Msg>();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                // Coalesce bursts of log lines into one re-render.
                let mut finished = Vec::new();
                if let Msg::Finished(id) = msg {
                    finished.push(id);
                }
                while let Ok(m) = rx.try_recv() {
                    if let Msg::Finished(id) = m {
                        finished.push(id);
                    }
                }
                let ok = this.update(cx, |this, cx| {
                    this.issues = model::load_issues();
                    for id in &finished {
                        this.active.remove(id);
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
                if !finished.is_empty() {
                    cx.update(ui::nav::inbox_changed).ok();
                }
            }
        });
        Self {
            pipelines,
            pipeline_errors,
            issues,
            runs,
            active: HashMap::new(),
            tx,
            _rx_task: task,
        }
    }

    // ── Queries ─────────────────────────────────────────────────────────

    pub fn issue(&self, key: &str) -> Option<&Issue> {
        self.issues.iter().find(|i| i.key == key)
    }

    /// Read the latest run of `issue` without copying it.
    pub fn with_run<R>(&self, issue: &str, f: impl FnOnce(&Run) -> R) -> Option<R> {
        let id = self.issue(issue)?.run.clone()?;
        let run = self.runs.get(&id)?;
        let guard = run.lock().ok()?;
        Some(f(&guard))
    }

    /// Latest run of every issue (copies, newest issue first) — for dashboards.
    pub fn latest_runs(&self) -> Vec<Run> {
        self.issues
            .iter()
            .filter_map(|i| i.run.as_ref())
            .filter_map(|id| self.runs.get(id))
            .filter_map(|r| r.lock().ok().map(|r| r.clone()))
            .collect()
    }

    /// The workspace label: the default pipeline's main repository (`pipeliner`) — its
    /// workdir that pull requests are opened from, else its last workdir.
    pub fn workspace_name(&self) -> String {
        let Some(def) = self.default_pipeline() else {
            return "Harness".into();
        };
        let pr_wd = def
            .steps
            .iter()
            .find(|s| s.kind == StepKind::PullRequest)
            .and_then(|s| def.workdir(&s.workdir));
        pr_wd
            .or(def.workdirs.last())
            .and_then(|w| w.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Harness".into())
    }

    // ── Pipelines ───────────────────────────────────────────────────────

    pub fn pipeline(&self, id: &str) -> Option<&PipelineDef> {
        self.pipelines.iter().find(|p| p.id == id)
    }

    /// The pipeline new issues use unless another is picked: the built-in one if present,
    /// else the first.
    pub fn default_pipeline(&self) -> Option<&PipelineDef> {
        self.pipeline("mockup-gpui-build").or(self.pipelines.first())
    }

    /// The pipeline an issue runs through.
    pub fn pipeline_of(&self, issue: &Issue) -> Option<&PipelineDef> {
        self.pipeline(&issue.pipeline).or(self.default_pipeline())
    }

    /// Issues whose latest run is active and uses `pipeline`.
    pub fn active_in(&self, pipeline: &str) -> usize {
        self.issues
            .iter()
            .filter(|i| self.is_active(&i.key))
            .filter(|i| self.with_run(&i.key, |r| r.pipeline == pipeline).unwrap_or(false))
            .count()
    }

    /// Create a pipeline named `name`: a copy of `template` (a pipeline id), or an empty
    /// one with the default pipeline's workdirs. Returns its id.
    pub fn create_pipeline(
        &mut self,
        name: &str,
        template: Option<&str>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        let name = if name.trim().is_empty() { "New pipeline" } else { name.trim() };
        let mut def = match template.and_then(|t| self.pipeline(t)) {
            Some(t) => t.clone(),
            None => PipelineDef {
                workdirs: self
                    .default_pipeline()
                    .map(|d| d.workdirs.clone())
                    .unwrap_or_default(),
                ..PipelineDef::default()
            },
        };
        def.id = def::new_pipeline_id(name, &self.pipelines);
        def.name = name.to_string();
        def.save()?;
        let id = def.id.clone();
        self.pipelines.push(def);
        self.sort_pipelines();
        cx.notify();
        Ok(id)
    }

    /// Save an edited pipeline (runs in progress keep the version they started with).
    pub fn save_pipeline(&mut self, def: PipelineDef, cx: &mut Context<Self>) -> anyhow::Result<()> {
        def.save()?;
        match self.pipelines.iter_mut().find(|p| p.id == def.id) {
            Some(p) => *p = def,
            None => self.pipelines.push(def),
        }
        self.sort_pipelines();
        cx.notify();
        Ok(())
    }

    /// Delete a pipeline; refused while a run of it is active.
    pub fn delete_pipeline(&mut self, id: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if self.active_in(id) > 0 {
            anyhow::bail!("a run of this pipeline is active");
        }
        let Some(def) = self.pipeline(id).cloned() else { return Ok(()) };
        std::fs::remove_file(def.path())?;
        self.pipelines.retain(|p| p.id != id);
        cx.notify();
        Ok(())
    }

    fn sort_pipelines(&mut self) {
        self.pipelines.sort_by_key(|p| p.name.to_lowercase());
    }

    pub fn run_id(&self, issue: &str) -> Option<String> {
        self.issue(issue)?.run.clone()
    }

    /// A worker is running for the issue's latest run.
    pub fn is_active(&self, issue: &str) -> bool {
        self.run_id(issue).is_some_and(|id| self.active.contains_key(&id))
    }

    pub fn count(&self, status: IssueStatus) -> usize {
        self.issues.iter().filter(|i| i.status == status).count()
    }

    // ── Commands ────────────────────────────────────────────────────────

    /// Create an issue; `start` runs the pipeline on it right away.
    /// Create an issue on `pipeline` (`None` = the default pipeline); `start` runs it now.
    #[allow(clippy::too_many_arguments)]
    pub fn create_issue(
        &mut self,
        title: &str,
        body: &str,
        kind: &str,
        pipeline: Option<&str>,
        start: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        if title.trim().is_empty() {
            anyhow::bail!("The title is empty");
        }
        let pipeline = pipeline
            .and_then(|p| self.pipeline(p))
            .or(self.default_pipeline())
            .map(|p| p.id.clone())
            .ok_or_else(|| anyhow::anyhow!("There is no pipeline yet"))?;
        let issue = model::create_issue(title, body, kind, &pipeline)?;
        let key = issue.key.clone();
        self.issues = model::load_issues();
        if start {
            self.start(&key, cx);
        }
        cx.notify();
        Ok(key)
    }

    pub fn delete_issue(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.is_active(key) {
            return;
        }
        let _ = model::delete_issue(key);
        self.issues = model::load_issues();
        cx.notify();
    }

    /// Move an issue to another pipeline (not while it runs).
    pub fn set_issue_pipeline(&mut self, key: &str, pipeline: &str, cx: &mut Context<Self>) {
        if self.is_active(key) || self.pipeline(pipeline).is_none() {
            return;
        }
        let _ = model::update_issue(key, |i| i.pipeline = pipeline.to_string());
        self.issues = model::load_issues();
        cx.notify();
    }

    /// Start a fresh run on `key` from the first step of its pipeline.
    pub fn start(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.is_active(key) {
            return;
        }
        let Some(issue) = self.issue(key).cloned() else { return };
        let Some(def) = self.pipeline_of(&issue).cloned() else { return };
        let run = Run::new(key, &def);
        let _ = run.save();
        let id = run.id.clone();
        let _ = model::update_issue(key, |i| {
            i.run = Some(id.clone());
            i.pr_url = None;
        });
        self.runs.insert(id.clone(), Arc::new(Mutex::new(run)));
        self.issues = model::load_issues();
        self.spawn(issue, id, Job::Start);
        cx.notify();
    }

    pub fn approve(&mut self, key: &str, cx: &mut Context<Self>) {
        self.continue_with(key, RunStatus::WaitingApproval, Job::Approved, cx);
    }

    pub fn request_changes(&mut self, key: &str, notes: &str, cx: &mut Context<Self>) {
        let notes = notes.trim();
        if notes.is_empty() {
            return;
        }
        self.continue_with(key, RunStatus::WaitingApproval, Job::Redesign(notes.to_string()), cx);
    }

    /// Continue a failed or cancelled run from where it stopped.
    pub fn retry(&mut self, key: &str, cx: &mut Context<Self>) {
        let status = self.with_run(key, |r| r.status);
        if matches!(status, Some(RunStatus::Failed | RunStatus::Cancelled)) {
            self.continue_with(key, status.unwrap_or(RunStatus::Failed), Job::Retry, cx);
        }
    }

    pub fn cancel(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(flag) = self.run_id(key).and_then(|id| self.active.get(&id).cloned()) {
            flag.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    fn continue_with(&mut self, key: &str, expect: RunStatus, job: Job, cx: &mut Context<Self>) {
        if self.is_active(key) || self.with_run(key, |r| r.status) != Some(expect) {
            return;
        }
        let (Some(issue), Some(id)) = (self.issue(key).cloned(), self.run_id(key)) else { return };
        self.spawn(issue, id, job);
        cx.notify();
    }

    fn spawn(&mut self, issue: Issue, run_id: String, job: Job) {
        let Some(run) = self.runs.get(&run_id).cloned() else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        self.active.insert(run_id.clone(), cancel.clone());
        let tx = self.tx.clone();
        let notify_tx = tx.clone();
        let name = format!("pipeline-{}", issue.key);
        let spawned = std::thread::Builder::new().name(name).spawn(move || {
            let ctx = stages::Ctx {
                issue,
                run,
                cancel,
                notify: Box::new(move || {
                    let _ = notify_tx.try_send(Msg::Changed);
                }),
            };
            stages::dispatch(&ctx, job);
            let _ = tx.try_send(Msg::Finished(run_id));
        });
        if let Err(e) = spawned {
            eprintln!("pipeline: could not start a worker: {e}");
        }
    }
}
