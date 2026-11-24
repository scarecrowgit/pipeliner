//! Pipeline runs: one [`Driver`] per open run, with the real node runner and an observer that
//! keeps the store and the Inbox up to date.
//!
//! What is kept where:
//!
//! - the run (`Run`) in the store's runs table, upserted on every change;
//! - the executor's [`Snapshot`] as JSON in the store's run state, written before anything
//!   about a step is published;
//! - the graph the run executes as JSON next to it, so a run stays on its version also when
//!   the draft changes;
//! - which worktrees a run made under `v2/pipelines/runs/<run id>/places.json` (the runner's).
//!
//! On start every open run with a snapshot gets its driver back: runs that had jobs in flight
//! become `Interrupted` and wait for `resume_run`; runs waiting for a person keep waiting.

mod observer;
mod services;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use model::{Event, GraphDef, InboxId, PipelineId, ReviewId, Run, RunId, RunState};
use pipeline::exec::{Driver, DriverHandle, Machine, Snapshot};
use pipeline::runners::{Runner, RunnerConfig, Services, SessionQuestions};
use pipeline::{InstanceKey, Item, NodeRunner};

use crate::bus::lock;
use crate::engine::Inner;

pub(crate) use observer::resolve;

/// The graph a run executes, when it was kept and can be read.
pub(crate) fn run_graph(inner: &Inner, run: &RunId) -> Option<GraphDef> {
    inner.store.run_graph(run).ok().flatten().and_then(|text| parse_graph(&text).ok())
}

/// The executor's latest snapshot of a run, when it was kept and can be read.
pub(crate) fn run_snapshot(inner: &Inner, run: &RunId) -> Option<Snapshot> {
    inner.store.run_snapshot(run).ok().flatten().and_then(|text| serde_json::from_str(&text).ok())
}

/// The Inbox item of a review.
pub(crate) fn review_inbox_id(review: &ReviewId) -> InboxId {
    InboxId::new(format!("review:{review}"))
}

/// The Inbox item of an agent's question.
pub(crate) fn question_inbox_id(run: &RunId, key: &InstanceKey) -> InboxId {
    InboxId::new(format!("ask:{run}:{}:{}:{}", key.node, key.variant, key.attempt))
}

struct Open {
    driver: Driver,
}

/// The runs that have a driver.
#[derive(Default)]
pub(crate) struct Runs {
    open: Mutex<HashMap<RunId, Open>>,
    /// Held while a run is looked up and restored, so two callers never restore it twice.
    restoring: Mutex<()>,
}

impl Runs {
    fn handle(&self, id: &RunId) -> Option<DriverHandle> {
        lock(&self.open).get(id).map(|open| open.driver.handle())
    }

    /// Whether the run has a driver right now.
    #[cfg(test)]
    pub fn is_driven(&self, id: &RunId) -> bool {
        lock(&self.open).contains_key(id)
    }

    /// Stops every driver: running jobs are cancelled and their runs become `Interrupted`.
    pub fn stop_all(&self) {
        let open: Vec<Open> = lock(&self.open).drain().map(|(_, open)| open).collect();
        for open in open {
            let _ = open.driver.stop();
        }
    }
}

/// The deployed graph text: the graph as JSON.
pub(crate) fn graph_text(graph: &GraphDef) -> Result<String> {
    serde_json::to_string_pretty(graph).context("writing the graph")
}

/// Reads a deployed version's text.
pub(crate) fn parse_graph(text: &str) -> Result<GraphDef> {
    serde_json::from_str(text).context("reading the deployed graph")
}

/// The live version of a pipeline and its number.
pub(crate) fn live_graph(inner: &Inner, id: &PipelineId) -> Result<(GraphDef, u32)> {
    let version = inner
        .store
        .live_pipeline_version(id)?
        .ok_or_else(|| anyhow!("The pipeline {id} has no deployed version; deploy it first"))?;
    Ok((parse_graph(&version.def_text)?, version.version))
}

/// Refuses new runs once today's spend reached `Settings::daily_limit_usd`.
pub(crate) fn check_limit(inner: &Inner) -> Result<()> {
    let settings = inner.store.settings()?;
    let Some(limit) = settings.daily_limit_usd else { return Ok(()) };
    let now = inner.now();
    let start = base::time::day_start(now, base::time::utc_offset(now));
    let spent = inner.store.spend_between(start, now + 1)?.cost_usd;
    if spent >= limit {
        bail!(
            "Today's spend limit of {} is reached ({} spent): no new runs start until tomorrow, unless you raise the limit in Settings",
            base::fmt::cost(limit),
            base::fmt::cost(spent)
        );
    }
    Ok(())
}

/// Where the runner keeps worktrees and its records, with the secrets it must hide.
pub(crate) fn runner_config(inner: &Inner) -> RunnerConfig {
    let mut config = RunnerConfig::new(inner.paths.worktrees_dir(), inner.paths.v2_dir().join("pipelines"));
    config.secrets = inner.secrets.clone();
    config
}

/// Set once a run's state could not be stored: from then on no job of the run does any work,
/// so nothing runs twice when it is replayed from an older snapshot.
pub(crate) type Broken = Arc<std::sync::OnceLock<String>>;

/// The runner of one run, refusing work once its state could not be stored.
struct Guarded {
    runner: Arc<Runner>,
    broken: Broken,
}

impl NodeRunner for Guarded {
    fn run(&self, job: &pipeline::Job, sink: &dyn pipeline::JobSink, cancel: &base::cancel::CancelToken) -> pipeline::JobResult {
        if let Some(error) = self.broken.get() {
            return pipeline::JobResult::Failed {
                error: format!("The run was stopped because its state could not be saved: {error}"),
                output: Default::default(),
            };
        }
        self.runner.run(job, sink, cancel)
    }

    fn discard(&self, run: &RunId, output: &pipeline::NodeOutput) {
        self.runner.discard(run, output);
    }
}

fn runner(inner: &Arc<Inner>, graph: &GraphDef) -> Arc<Runner> {
    let weak = Arc::downgrade(inner);
    let sessions: Arc<dyn pipeline::runners::AgentSessions> = Arc::new(services::Sessions(weak.clone()));
    let services = Services {
        questions: Arc::new(SessionQuestions(sessions.clone())),
        sessions,
        renderer: inner.renderer.clone(),
        records: Arc::new(services::Inbox(weak.clone())),
        tracker: Arc::new(services::Issues(weak)),
        gh: inner.gh.clone(),
    };
    Arc::new(Runner::new(graph, services, runner_config(inner)))
}

fn drive(inner: &Arc<Inner>, graph: GraphDef, machine: Machine) -> Result<DriverHandle> {
    if inner.is_shut_down() {
        bail!("The app is shutting down");
    }
    let id = machine.run().id.clone();
    let runner = runner(inner, &graph);
    let broken = Broken::default();
    let observer = observer::RunObserver {
        inner: Arc::downgrade(inner),
        followed: None,
        asked: BTreeMap::new(),
        merged: false,
        broken: broken.clone(),
        graph,
    };
    let node_runner: Arc<dyn NodeRunner> = Arc::new(Guarded { runner, broken });
    let driver = Driver::spawn(machine, node_runner, Box::new(observer), inner.clock.clone())?;
    let handle = driver.handle();
    lock(&inner.runs.open).insert(id, Open { driver });
    Ok(handle)
}

/// Starts `item` through `graph` as `run` (the engine's shell of it).
pub(crate) fn launch(inner: &Arc<Inner>, graph: GraphDef, run: Run, item: Item) -> Result<Run> {
    check_limit(inner)?;
    inner.store.set_run_graph(&run.id, &graph_text(&graph)?)?;
    let machine = Machine::new(graph.clone(), run, item);
    let shell = machine.run().clone();
    inner.store.upsert_run(&shell)?;
    inner.publish(Event::RunChanged { run: shell.clone() });
    drive(inner, graph, machine)?;
    Ok(shell)
}

/// A driver for a run that has none: from its stored snapshot.
fn restore(inner: &Arc<Inner>, id: &RunId) -> Result<DriverHandle> {
    let graph = inner.store.run_graph(id)?.ok_or_else(|| anyhow!("The run {id} has no saved state to continue from"))?;
    let snapshot = inner.store.run_snapshot(id)?.ok_or_else(|| anyhow!("The run {id} has no saved state to continue from"))?;
    let graph = parse_graph(&graph)?;
    let snapshot: Snapshot = serde_json::from_str(&snapshot).context("reading the run's saved state")?;
    let machine = Machine::restore(graph.clone(), snapshot);
    inner.store.set_run_snapshot(id, &serde_json::to_string(machine.snapshot())?)?;
    inner.store.upsert_run(machine.run())?;
    inner.publish(Event::RunChanged { run: machine.run().clone() });
    drive(inner, graph, machine)
}

/// Sends a command to the run's driver, restoring the driver when it has none or when the
/// one found was just being dropped.
pub(crate) fn command(
    inner: &Arc<Inner>,
    id: &RunId,
    send: impl Fn(&DriverHandle) -> Result<(), pipeline::exec::ExecError>,
) -> Result<()> {
    match send(&handle(inner, id)?) {
        Err(pipeline::exec::ExecError::DriverStopped) => {
            let stale = {
                let _restoring = lock(&inner.runs.restoring);
                lock(&inner.runs.open).remove(id)
            };
            if let Some(open) = stale {
                let _ = open.driver.stop();
            }
            send(&handle(inner, id)?).map_err(|e| anyhow!("{e}"))
        }
        other => other.map_err(|e| anyhow!("{e}")),
    }
}

/// Whether the run takes no more work unless a person resumes it.
fn closed(state: RunState) -> bool {
    matches!(state, RunState::Done | RunState::Failed | RunState::Cancelled)
}

/// Drops the driver of a run that is closed; a later command restores it from the store.
/// A test run that is done or cancelled also loses its throwaway worktrees.
pub(crate) fn reap(inner: &Arc<Inner>, id: &RunId) {
    let open = {
        let _restoring = lock(&inner.runs.restoring);
        let run = inner.store.run(id).ok().flatten();
        if !run.as_ref().is_some_and(|r| closed(r.state)) {
            return;
        }
        lock(&inner.runs.open).remove(id)
    };
    if let Some(open) = open {
        let _ = open.driver.stop();
    }
    if let Ok(Some(run)) = inner.store.run(id)
        && run.test
        && matches!(run.state, RunState::Done | RunState::Cancelled)
    {
        pipeline::runners::cleanup(&runner_config(inner), id);
    }
}

/// Throws a test run away: cancels it when it is still open, stops its driver and removes
/// its worktrees and branches. Real runs keep their branches and cannot be discarded.
pub(crate) fn discard(inner: &Arc<Inner>, id: &RunId) -> Result<()> {
    let run = inner.store.run(id)?.ok_or_else(|| anyhow!("There is no run {id}"))?;
    if !run.test {
        bail!("Only test runs can be discarded; a real run keeps its branches");
    }
    if run.state.is_open() {
        command(inner, id, |h| h.cancel())?;
    }
    let open = {
        let _restoring = lock(&inner.runs.restoring);
        lock(&inner.runs.open).remove(id)
    };
    if let Some(open) = open {
        let _ = open.driver.stop();
    }
    pipeline::runners::cleanup(&runner_config(inner), id);
    Ok(())
}

/// The driver of a run, restored when it has none.
pub(crate) fn handle(inner: &Arc<Inner>, id: &RunId) -> Result<DriverHandle> {
    let _restoring = lock(&inner.runs.restoring);
    if let Some(handle) = inner.runs.handle(id) {
        return Ok(handle);
    }
    if inner.store.run(id)?.is_none() {
        bail!("There is no run {id}");
    }
    restore(inner, id)
}

/// Gives every open run with a snapshot its driver back.
pub(crate) fn recover(inner: &Arc<Inner>) {
    // Test runs that are gone, or closed for good, leave nothing behind.
    let keep = |id: &RunId| inner.store.run(id).ok().flatten().is_some_and(|r| !matches!(r.state, RunState::Done | RunState::Cancelled));
    pipeline::runners::sweep_test_runs(&runner_config(inner), &keep);
    let Ok(runs) = inner.store.open_runs() else { return };
    for run in runs {
        if inner.store.run_snapshot(&run.id).ok().flatten().is_none() {
            continue;
        }
        if let Err(error) = restore(inner, &run.id) {
            inner.notice_error(&format!("The run {} could not be restored: {error:#}", run.id));
        }
    }
}
