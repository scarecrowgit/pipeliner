//! The screen's real data: the configured pipelines, their model steps and sub-agents (from
//! [`pipeline::Pipelines`]), the chat agents (built-in Build and Plan plus custom agents found
//! on disk) and the tasks sub-agents worked on in recent runs. [`crate::entries`] turns it
//! into the card list.

use std::path::{Path, PathBuf};

use agent_tools::agents::{AgentDef, AgentSource};
use agent_tools::skills::Skill;
use gpui::{Hsla, SharedString};
use pipeline::{
    AgentUsage, CoderTask, Delegate, PipelineDef, Pipelines, RunStatus, StageStatus, StepDef,
    StepKind,
};
use ui::Status;
use ui::theme::*;

/// The chat screen's built-in agents (`Mode` in `screen_chat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatKind {
    Build,
    Plan,
}

impl ChatKind {
    pub const ALL: [ChatKind; 2] = [ChatKind::Build, ChatKind::Plan];

    pub fn name(self) -> &'static str {
        match self {
            ChatKind::Build => "Build",
            ChatKind::Plan => "Plan",
        }
    }

    /// One line, as in the chat's agent picker.
    pub fn hint(self) -> &'static str {
        match self {
            ChatKind::Build => "Full access · edits and bash ask first",
            ChatKind::Plan => "Read-only · investigates and writes a plan",
        }
    }

    /// What its system prompt asks of it.
    pub fn duty(self) -> &'static str {
        match self {
            ChatKind::Build => {
                "A coding agent in your repository. It reads the code before changing it, prefers \
                 small exact edits, uses bash for builds, tests and git, keeps a todo list for \
                 multi-step work and reports finished work to the Inbox."
            }
            ChatKind::Plan => {
                "Investigates the code with read-only tools and replies with a numbered \
                 implementation plan: files, changes, risks and how to verify. It does not edit \
                 files or run commands."
            }
        }
    }

    pub fn tools(self) -> &'static str {
        match self {
            ChatKind::Build => {
                "Every workspace tool and your MCP servers; edits and commands ask you first"
            }
            ChatKind::Plan => "Read-only workspace tools, todo list and skills",
        }
    }
}

/// What a model step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepRole {
    /// An agent step that edits itself.
    Agent,
    /// An agent step that hands tasks to sub-agents.
    Lead,
    /// A yes/no decision.
    Decision,
}

impl StepRole {
    pub fn of(step: &StepDef) -> StepRole {
        match step.kind {
            StepKind::Decision => StepRole::Decision,
            _ if step.delegate.is_some() => StepRole::Lead,
            _ => StepRole::Agent,
        }
    }
}

/// A step of a pipeline: indices into [`Snapshot::pipelines`] and its `steps`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepRef {
    pub pipeline: usize,
    pub step: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Index into [`Snapshot::pipelines`].
    Pipeline(usize),
    /// An agent or decision step.
    Step(StepRef, StepRole),
    /// The sub-agents the step leads.
    Delegate(StepRef),
    Chat(ChatKind),
    /// Index into [`Catalog::agents`].
    Custom(usize),
    /// Index into [`Snapshot::tasks`].
    Task(usize),
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Pipeline(_) => "Pipeline",
            Kind::Step(_, StepRole::Agent) => "Agent step",
            Kind::Step(_, StepRole::Lead) => "Lead agent",
            Kind::Step(_, StepRole::Decision) => "Decision step",
            Kind::Delegate(_) => "Sub-agent",
            Kind::Chat(_) => "Chat agent",
            Kind::Custom(_) => "Custom chat agent",
            Kind::Task(_) => "Sub-agent task",
        }
    }

    /// The step a pipeline-step or sub-agent entry stands for.
    pub fn step_ref(self) -> Option<StepRef> {
        match self {
            Kind::Step(r, _) | Kind::Delegate(r) => Some(r),
            _ => None,
        }
    }
}

/// Run state of an entry: drives the status dot and the status-text colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Waiting,
    Idle,
    Done,
    Failed,
}

impl State {
    pub fn dot(self) -> Status {
        match self {
            State::Running => Status::Running,
            State::Waiting => Status::Waiting,
            State::Idle => Status::Draft,
            State::Done => Status::Live,
            State::Failed => Status::Error,
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            State::Running => INFO,
            State::Waiting => WARNING,
            State::Idle => TEXT_MUTED,
            State::Done => SUCCESS,
            State::Failed => DANGER,
        }
    }

    pub(crate) fn of_stage(s: StageStatus) -> State {
        match s {
            StageStatus::Running => State::Running,
            StageStatus::Waiting => State::Waiting,
            StageStatus::Done => State::Done,
            StageStatus::Failed => State::Failed,
            StageStatus::Pending | StageStatus::Skipped => State::Idle,
        }
    }
}

pub fn stage_word(s: StageStatus) -> &'static str {
    match s {
        StageStatus::Pending => "Pending",
        StageStatus::Running => "Running",
        StageStatus::Waiting => "Waiting for you",
        StageStatus::Done => "Done",
        StageStatus::Skipped => "Skipped",
        StageStatus::Failed => "Failed",
    }
}

/// One card.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: SharedString,
    pub name: SharedString,
    pub kind: Kind,
    pub desc: SharedString,
    pub state: State,
    pub status: SharedString,
    pub meta: SharedString,
    pub chips: Vec<SharedString>,
}

impl Entry {
    pub fn created(&self) -> bool {
        matches!(self.kind, Kind::Task(_))
    }
}

/// Grid filter (the tab bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Pipelines,
    Steps,
    SubAgents,
    ChatAgents,
    Created,
}

impl Filter {
    pub const TABS: [Filter; 6] = [
        Filter::All,
        Filter::Pipelines,
        Filter::Steps,
        Filter::SubAgents,
        Filter::ChatAgents,
        Filter::Created,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Pipelines => "Pipelines",
            Filter::Steps => "Pipeline agents",
            Filter::SubAgents => "Sub-agents",
            Filter::ChatAgents => "Chat agents",
            Filter::Created => "Created by agents",
        }
    }

    pub fn matches(self, e: &Entry) -> bool {
        match self {
            Filter::All => true,
            Filter::Pipelines => matches!(e.kind, Kind::Pipeline(_)),
            Filter::Steps => matches!(e.kind, Kind::Step(..)),
            Filter::SubAgents => matches!(e.kind, Kind::Delegate(_)),
            Filter::ChatAgents => matches!(e.kind, Kind::Chat(_) | Kind::Custom(_)),
            Filter::Created => e.created(),
        }
    }

    /// `HARNESS_AGENTS_TAB` test hook values.
    pub fn from_env(v: &str) -> Option<Filter> {
        match v {
            "all" => Some(Filter::All),
            "pipelines" => Some(Filter::Pipelines),
            "pipeline" | "agents" | "steps" => Some(Filter::Steps),
            "sub" | "subagents" | "sub-agents" => Some(Filter::SubAgents),
            "chat" => Some(Filter::ChatAgents),
            "created" => Some(Filter::Created),
            _ => None,
        }
    }
}

// ── Pipeline runs ───────────────────────────────────────────────────────

/// One step of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct StageInfo {
    pub id: String,
    pub name: String,
    pub status: StageStatus,
}

/// The latest run of one issue, without its log.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub issue: String,
    pub title: String,
    pub id: String,
    /// Pipeline id.
    pub pipeline: String,
    pub status: RunStatus,
    /// A worker is running for it right now.
    pub active: bool,
    pub created: u64,
    pub tokens: u64,
    pub cost: f64,
    pub usage: Vec<AgentUsage>,
    pub stages: Vec<StageInfo>,
    pub tasks: Vec<CoderTask>,
    /// Step id → name of the sub-agents it leads (from the run's pipeline snapshot).
    pub delegates: Vec<(String, String)>,
}

impl RunInfo {
    pub fn stage(&self, step: &str) -> StageStatus {
        self.stages
            .iter()
            .find(|s| s.id == step)
            .map(|s| s.status)
            .unwrap_or(StageStatus::Pending)
    }

    pub fn state(&self) -> State {
        match self.status {
            RunStatus::Running if self.active => State::Running,
            RunStatus::Running => State::Idle,
            RunStatus::WaitingApproval => State::Waiting,
            RunStatus::Done => State::Done,
            RunStatus::Failed => State::Failed,
            RunStatus::Cancelled => State::Idle,
        }
    }

    pub fn status_word(&self) -> &'static str {
        match self.status {
            RunStatus::Running if self.active => "Running",
            RunStatus::Running => "Stopped",
            RunStatus::WaitingApproval => "Waiting for approval",
            RunStatus::Done => "Done",
            RunStatus::Failed => "Failed",
            RunStatus::Cancelled => "Cancelled",
        }
    }

    /// Name of the sub-agents step `step` leads in this run.
    pub fn delegate_of(&self, step: &str) -> Option<&str> {
        self.delegates
            .iter()
            .find(|(s, _)| s == step)
            .map(|(_, n)| n.as_str())
    }
}

/// Where a sub-agent task lives in [`Snapshot::runs`].
#[derive(Clone, Copy, Debug)]
pub struct TaskRef {
    pub run: usize,
    pub index: usize,
}

/// What one agent used over the recent runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub runs: usize,
    pub tokens: u64,
    pub cost: f64,
    /// Models seen in the usage records, first use first.
    pub models: Vec<String>,
}

/// `who` of a usage record is sub-agent `name` (`Coder 2` for `Coder`).
pub fn is_delegate(who: &str, name: &str) -> bool {
    who.strip_prefix(name).is_some_and(|rest| {
        rest.is_empty() || rest.strip_prefix(' ').is_some_and(|n| n.parse::<u32>().is_ok())
    })
}

pub struct Snapshot {
    /// All pipelines, by name.
    pub pipelines: Vec<PipelineDef>,
    /// Pipeline files that could not be read or written.
    pub errors: Vec<String>,
    /// Latest run of every issue, newest issue first.
    pub runs: Vec<RunInfo>,
    /// Sub-agent tasks of those runs, newest run first.
    pub tasks: Vec<TaskRef>,
    /// The workspace agents and skills are discovered in.
    pub root: PathBuf,
}

impl Snapshot {
    pub fn read(p: &Pipelines) -> Self {
        let runs: Vec<RunInfo> = p
            .issues
            .iter()
            .filter_map(|issue| {
                let active = p.is_active(&issue.key);
                p.with_run(&issue.key, |r| RunInfo {
                    issue: issue.key.clone(),
                    title: issue.title.clone(),
                    id: r.id.clone(),
                    pipeline: r.pipeline.clone(),
                    status: r.status,
                    active,
                    created: r.created,
                    tokens: r.tokens,
                    cost: r.cost_usd,
                    usage: r.usage.clone(),
                    stages: r
                        .stages
                        .iter()
                        .map(|s| StageInfo {
                            id: s.id.clone(),
                            name: s.name.clone(),
                            status: s.status,
                        })
                        .collect(),
                    tasks: r.tasks.clone(),
                    delegates: r
                        .def
                        .iter()
                        .flat_map(|d| d.steps.iter())
                        .filter_map(|s| Some((s.id.clone(), s.delegate.as_ref()?.name.clone())))
                        .collect(),
                })
            })
            .collect();
        let root = workspace_root(p.default_pipeline());
        Self::from_runs(p.pipelines.clone(), p.pipeline_errors.clone(), runs, root)
    }

    pub fn from_runs(
        pipelines: Vec<PipelineDef>,
        errors: Vec<String>,
        runs: Vec<RunInfo>,
        root: PathBuf,
    ) -> Self {
        let tasks = runs
            .iter()
            .enumerate()
            .flat_map(|(run, r)| (0..r.tasks.len()).map(move |index| TaskRef { run, index }))
            .collect();
        Self {
            pipelines,
            errors,
            runs,
            tasks,
            root,
        }
    }

    pub fn pipeline(&self, ix: usize) -> Option<&PipelineDef> {
        self.pipelines.get(ix)
    }

    pub fn step(&self, r: StepRef) -> Option<(&PipelineDef, &StepDef)> {
        let def = self.pipelines.get(r.pipeline)?;
        Some((def, def.steps.get(r.step)?))
    }

    pub fn delegate(&self, r: StepRef) -> Option<(&PipelineDef, &StepDef, &Delegate)> {
        let (def, step) = self.step(r)?;
        Some((def, step, step.delegate.as_ref()?))
    }

    /// Latest runs of pipeline `id`.
    pub fn runs_in(&self, id: &str) -> Vec<&RunInfo> {
        self.runs.iter().filter(|r| r.pipeline == id).collect()
    }

    pub fn task(&self, ix: usize) -> Option<(&RunInfo, &CoderTask)> {
        let t = self.tasks.get(ix)?;
        let run = self.runs.get(t.run)?;
        Some((run, run.tasks.get(t.index)?))
    }

    /// The sub-agent that worked on task `ix` (`Coder <n>`: tasks are numbered from 1
    /// within the run).
    pub fn task_agent(&self, ix: usize) -> Option<String> {
        let t = self.tasks.get(ix)?;
        let (run, task) = self.task(ix)?;
        let name = run.delegate_of(&task.step).unwrap_or("Sub-agent");
        Some(format!("{name} {}", t.index + 1))
    }

    /// Name of the step that handed out task `ix`.
    pub fn task_lead(&self, ix: usize) -> Option<String> {
        let (run, task) = self.task(ix)?;
        run.stages
            .iter()
            .find(|s| s.id == task.step)
            .map(|s| s.name.clone())
    }

    /// Usage records of the task's sub-agent.
    pub fn task_usage(&self, ix: usize) -> Vec<&AgentUsage> {
        let (Some((run, _)), Some(who)) = (self.task(ix), self.task_agent(ix)) else {
            return Vec::new();
        };
        run.usage.iter().filter(|u| u.who == who).collect()
    }

    /// Tasks handed out by step `r` (newest run first).
    pub fn tasks_of(&self, r: StepRef) -> Vec<usize> {
        let Some((def, step)) = self.step(r) else {
            return Vec::new();
        };
        (0..self.tasks.len())
            .filter(|&ix| {
                self.task(ix)
                    .is_some_and(|(run, t)| run.pipeline == def.id && t.step == step.id)
            })
            .collect()
    }

    fn usage_where(&self, pipeline: &str, f: impl Fn(&AgentUsage) -> bool) -> Usage {
        let mut out = Usage::default();
        for run in self.runs.iter().filter(|r| r.pipeline == pipeline) {
            let mut used = false;
            for u in run.usage.iter().filter(|u| f(u)) {
                used = true;
                out.tokens += u.tokens;
                out.cost += u.cost_usd;
                if !out.models.contains(&u.model) {
                    out.models.push(u.model.clone());
                }
            }
            if used {
                out.runs += 1;
            }
        }
        out
    }

    /// Usage of a model step (records under the step's name).
    pub fn step_usage(&self, r: StepRef) -> Usage {
        match self.step(r) {
            Some((def, step)) => self.usage_where(&def.id, |u| u.who == step.name),
            None => Usage::default(),
        }
    }

    /// Usage of the sub-agents step `r` leads (`Coder 1`, `Coder 2`…).
    pub fn delegate_usage(&self, r: StepRef) -> Usage {
        match self.delegate(r) {
            Some((def, _, d)) => self.usage_where(&def.id, |u| is_delegate(&u.who, &d.name)),
            None => Usage::default(),
        }
    }

    /// Runs the step took part in (it started, or it has usage records).
    pub fn runs_of_step(&self, r: StepRef) -> Vec<&RunInfo> {
        let Some((def, step)) = self.step(r) else {
            return Vec::new();
        };
        self.runs_in(&def.id)
            .into_iter()
            .filter(|run| {
                !matches!(run.stage(&step.id), StageStatus::Pending | StageStatus::Skipped)
                    || run.usage.iter().any(|u| u.who == step.name)
            })
            .collect()
    }

    /// Runs the step's sub-agents worked in.
    pub fn runs_of_delegate(&self, r: StepRef) -> Vec<&RunInfo> {
        let Some((def, step, d)) = self.delegate(r) else {
            return Vec::new();
        };
        self.runs_in(&def.id)
            .into_iter()
            .filter(|run| {
                run.tasks.iter().any(|t| t.step == step.id)
                    || run.usage.iter().any(|u| is_delegate(&u.who, &d.name))
            })
            .collect()
    }

    /// Issues the step is working on right now.
    pub fn running_step(&self, r: StepRef) -> Vec<&RunInfo> {
        let Some((def, step)) = self.step(r) else {
            return Vec::new();
        };
        self.runs_in(&def.id)
            .into_iter()
            .filter(|run| run.active && run.stage(&step.id) == StageStatus::Running)
            .collect()
    }

    /// Issues the step's sub-agents are working on right now.
    pub fn running_delegate(&self, r: StepRef) -> Vec<&RunInfo> {
        let Some((_, step)) = self.step(r) else {
            return Vec::new();
        };
        self.running_step(r)
            .into_iter()
            .filter(|run| {
                run.tasks
                    .iter()
                    .any(|t| t.step == step.id && t.status == StageStatus::Running)
            })
            .collect()
    }

    pub fn active_in(&self, id: &str) -> usize {
        self.runs_in(id).iter().filter(|r| r.active).count()
    }

    pub fn waiting_in(&self, id: &str) -> usize {
        self.runs_in(id)
            .iter()
            .filter(|r| r.status == RunStatus::WaitingApproval)
            .count()
    }

    pub fn last_run_in(&self, id: &str) -> Option<u64> {
        self.runs_in(id).iter().map(|r| r.created).max()
    }

    pub fn tokens_in(&self, id: &str) -> u64 {
        self.runs_in(id).iter().map(|r| r.tokens).sum()
    }

    pub fn cost_in(&self, id: &str) -> f64 {
        self.runs_in(id).iter().map(|r| r.cost).sum()
    }

    pub fn pipeline_state(&self, id: &str) -> State {
        if self.active_in(id) > 0 {
            State::Running
        } else if self.waiting_in(id) > 0 {
            State::Waiting
        } else {
            State::Idle
        }
    }
}

/// The workspace: the default pipeline's folder pull requests are opened from, else its last
/// workdir, else the current folder.
pub fn workspace_root(def: Option<&PipelineDef>) -> PathBuf {
    def.and_then(|d| {
        d.steps
            .iter()
            .find(|s| s.kind == StepKind::PullRequest)
            .and_then(|s| d.workdir(&s.workdir))
            .or(d.workdirs.last())
            .map(|w| w.path.clone())
    })
    .filter(|p| !p.as_os_str().is_empty())
    .or_else(|| std::env::current_dir().ok())
    .unwrap_or_default()
}

// ── Chat agents and skills on disk ──────────────────────────────────────

pub struct Catalog {
    /// Workspace the agents and skills were discovered for.
    pub root: PathBuf,
    pub agents: Vec<AgentDef>,
    pub skills: Vec<Skill>,
}

impl Catalog {
    pub fn load(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            agents: agent_tools::agents::discover(root),
            skills: agent_tools::skills::discover(root),
        }
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

pub fn source_label(s: &AgentSource) -> &'static str {
    match s {
        AgentSource::User => "Your agent",
        AgentSource::Project => "Project agent",
    }
}

pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
