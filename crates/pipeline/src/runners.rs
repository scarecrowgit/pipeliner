//! The real [`NodeRunner`]: agents, routers, commands, renders, merges, pull requests, reports
//! and issue updates.
//!
//! One [`Runner`] serves one run of one graph (it needs the graph's workdirs). What it needs
//! from the world comes through [`Services`]: agent sessions, router questions, rendering, the
//! Inbox and blobs, the tracker, and `gh`. Git and shell commands run directly, in the
//! instance's worktree (see [`places`](self) for how worktrees are named and found again).
//!
//! | Node | Job |
//! | --- | --- |
//! | Source | passes the item on |
//! | Agent, Orchestrator | a session in its worktree that finishes with `step_done` |
//! | Router | rules in order: `when` conditions, `ask` questions answered strictly yes or no |
//! | Action `command` | a shell command; a tests command reports counts, a failing one is a result |
//! | Action `render` | changed HTML files to before/after PNGs |
//! | Action `merge` | the instance's branch into the workdir's base branch |
//! | Action `pull_request` | commit, push, `gh pr create`, an Inbox item |
//! | Action `report`, `update_issue` | an Inbox item; the tracker |
//!
//! A test run works in throwaway worktrees and only logs what merge, push, pull request,
//! report and issue updates would have done; nothing leaves the machine.
//!
//! **Confinement.** Pipeline agents run without a person to ask: every tool call their access
//! level grants is allowed. Bash and file tools are confined only by the worktree they start
//! in, which is a working directory, not a sandbox: a command can read and write anywhere the
//! user can. Give agents `ReadOnly` or `ReadAndRun` where that matters.
//!
//! No job panics on bad input; every failure is [`JobResult::Failed`] with a message a person
//! can act on.

mod actions;
mod agent;
mod git;
mod pattern;
mod places;
mod publish;
mod router;
mod services;
mod task;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base::cancel::CancelToken;
use model::{GraphDef, LogLevel, NodeKind, RunId, WorkdirDef};
use vcs::pr::Gh;

use crate::api::{Job, JobNote, JobResult, JobSink, NodeOutput, NodeRunner};

pub use pattern::{Pattern, parse_tests};
pub use services::{
    ANSWER_TOOL, AgentSessions, Answer, Firefox, InboxPost, Picture, Question, Questions, Records, Renderer,
    SessionOpen, SessionQuestions, Tracker, render_size,
};

/// Where the runner keeps what it makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerConfig {
    /// Root of the worktrees it creates (`~/.local/share/harness/v2/worktrees`).
    pub worktrees: PathBuf,
    /// Its own records: which worktrees belong to which run.
    pub state: PathBuf,
    /// Longest a workdir's `setup` command may take.
    pub setup_timeout: Duration,
    /// Timeout of a command action that sets none.
    pub command_timeout: Duration,
}

impl RunnerConfig {
    /// Default timeouts: 30 minutes for setup and for commands.
    pub fn new(worktrees: PathBuf, state: PathBuf) -> Self {
        Self { worktrees, state, setup_timeout: Duration::from_secs(1800), command_timeout: Duration::from_secs(1800) }
    }
}

/// What the runner needs from the world.
#[derive(Clone)]
pub struct Services {
    pub sessions: Arc<dyn AgentSessions>,
    pub questions: Arc<dyn Questions>,
    pub renderer: Arc<dyn Renderer>,
    pub records: Arc<dyn Records>,
    pub tracker: Arc<dyn Tracker>,
    pub gh: Arc<dyn Gh + Send + Sync>,
}

/// The node runner of one run.
pub struct Runner {
    pipeline: String,
    pipeline_id: String,
    workdirs: Vec<WorkdirDef>,
    services: Services,
    config: RunnerConfig,
    places: places::Places,
}

impl Runner {
    /// A runner for runs of `graph`.
    pub fn new(graph: &GraphDef, services: Services, config: RunnerConfig) -> Runner {
        let places = places::Places::new(config.state.join("runs"));
        Runner { pipeline: graph.name.clone(), pipeline_id: graph.id.to_string(), workdirs: graph.workdirs.clone(), services, config, places }
    }

    /// Removes every worktree and branch a run made, and the runner's records of them. For
    /// test runs once they ended; best effort.
    pub fn cleanup(&self, run: &RunId) {
        for entry in self.places.entries(run) {
            places::remove(&entry);
        }
        self.places.forget(run);
        let _ = std::fs::remove_dir_all(self.config.worktrees.join("_test").join(base::id::slug(run.as_str(), 40)));
    }
}

impl NodeRunner for Runner {
    fn run(&self, job: &Job, sink: &dyn JobSink, cancel: &CancelToken) -> JobResult {
        let ctx = Ctx { runner: self, job, sink, cancel };
        if cancel.is_cancelled() {
            return JobResult::Cancelled { output: NodeOutput::default() };
        }
        let result = match &job.node.kind {
            NodeKind::Source(_) => {
                JobResult::Done { port: model::graph::PORT_OUT.into(), output: NodeOutput { summary: job.item.title.clone(), ..Default::default() } }
            }
            NodeKind::Agent(node) | NodeKind::Orchestrator(node) => agent::run(&ctx, node),
            NodeKind::Router(node) => router::run(&ctx, node),
            NodeKind::Action(action) => actions::run(&ctx, action),
            other => failed(format!("{} nodes are not run as jobs", other.label())),
        };
        match result {
            JobResult::Failed { output, .. } if cancel.is_cancelled() => JobResult::Cancelled { output },
            other => other,
        }
    }

    fn discard(&self, run: &RunId, output: &NodeOutput) {
        // Only what this runner made is removed; a workdir used in place is never touched.
        if let Some(path) = &output.worktree
            && let Some(entry) = self.places.remove(run, path)
        {
            places::remove(&entry);
        }
    }
}

/// One job being run.
pub(crate) struct Ctx<'a> {
    pub runner: &'a Runner,
    pub job: &'a Job,
    pub sink: &'a dyn JobSink,
    pub cancel: &'a CancelToken,
}

impl Ctx<'_> {
    pub fn log(&self, level: LogLevel, text: impl Into<String>) {
        self.sink.note(JobNote::Log { level, text: text.into() });
    }

    pub fn services(&self) -> &Services {
        &self.runner.services
    }

    /// "Mockup → GPUI build · Pull request".
    pub fn source(&self) -> String {
        format!("{} · {}", self.runner.pipeline, self.job.node.name)
    }

    /// `HAR-12: Fix the retry`, or the item's title.
    pub fn title(&self) -> String {
        match &self.job.item.issue {
            Some(issue) => format!("{}: {}", issue.key, self.job.item.title),
            None => self.job.item.title.clone(),
        }
    }
}

/// A failure with no output.
pub(crate) fn failed(error: impl Into<String>) -> JobResult {
    JobResult::Failed { error: error.into(), output: NodeOutput::default() }
}
