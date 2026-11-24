//! Pipeline runs: one [`Driver`] per open run, with the real node runner and an observer that
//! keeps the store and the Inbox up to date.
//!
//! What is kept where:
//!
//! - the run (`Run`) in the store's runs table, upserted on every change;
//! - the executor's [`Snapshot`] as JSON in the marker `run-snapshot:<run id>` (there is no
//!   store column for it), written before anything about a step is published;
//! - the graph the run executes as JSON in the marker `run-graph:<run id>`, so a run stays on
//!   its version also when the draft changes;
//! - which worktrees a run made under `v2/pipelines/runs/<run id>/places.json` (the runner's).
//!
//! On start every open run with a snapshot gets its driver back: runs that had jobs in flight
//! become `Interrupted` and wait for `resume_run`; runs waiting for a person keep waiting.

mod observer;
mod services;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use model::{Event, GraphDef, InboxId, PipelineId, ReviewId, Run, RunId};
use pipeline::exec::{Driver, DriverHandle, Machine, Snapshot};
use pipeline::runners::{Runner, RunnerConfig, Services, SessionQuestions};
use pipeline::{InstanceKey, Item, NodeRunner};

use crate::bus::lock;
use crate::engine::Inner;

pub(crate) use observer::resolve;

/// The marker holding a run's executor state.
pub(crate) fn snapshot_marker(run: &RunId) -> String {
    format!("run-snapshot:{run}")
}

/// The marker holding the graph a run executes.
pub(crate) fn graph_marker(run: &RunId) -> String {
    format!("run-graph:{run}")
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
    let config = RunnerConfig::new(inner.paths.worktrees_dir(), inner.paths.v2_dir().join("pipelines"));
    Arc::new(Runner::new(graph, services, config))
}

fn drive(inner: &Arc<Inner>, graph: GraphDef, machine: Machine) -> Result<DriverHandle> {
    if inner.is_shut_down() {
        bail!("The app is shutting down");
    }
    let id = machine.run().id.clone();
    let runner = runner(inner, &graph);
    let observer = observer::RunObserver {
        inner: Arc::downgrade(inner),
        followed: None,
        asked: BTreeMap::new(),
        runner: runner.clone(),
        graph,
    };
    let node_runner: Arc<dyn NodeRunner> = runner;
    let driver = Driver::spawn(machine, node_runner, Box::new(observer), inner.clock.clone())?;
    let handle = driver.handle();
    lock(&inner.runs.open).insert(id, Open { driver });
    Ok(handle)
}

/// Starts `item` through `graph` as `run` (the engine's shell of it).
pub(crate) fn launch(inner: &Arc<Inner>, graph: GraphDef, run: Run, item: Item) -> Result<Run> {
    check_limit(inner)?;
    inner.store.set_marker(&graph_marker(&run.id), &graph_text(&graph)?)?;
    let machine = Machine::new(graph.clone(), run, item);
    let shell = machine.run().clone();
    inner.store.upsert_run(&shell)?;
    inner.publish(Event::RunChanged { run: shell.clone() });
    drive(inner, graph, machine)?;
    Ok(shell)
}

/// A driver for a run that has none: from its stored snapshot.
fn restore(inner: &Arc<Inner>, id: &RunId) -> Result<DriverHandle> {
    let graph = inner.store.marker(&graph_marker(id))?.ok_or_else(|| anyhow!("The run {id} has no saved state to continue from"))?;
    let snapshot = inner.store.marker(&snapshot_marker(id))?.ok_or_else(|| anyhow!("The run {id} has no saved state to continue from"))?;
    let graph = parse_graph(&graph)?;
    let snapshot: Snapshot = serde_json::from_str(&snapshot).context("reading the run's saved state")?;
    let machine = Machine::restore(graph.clone(), snapshot);
    inner.store.set_marker(&snapshot_marker(id), &serde_json::to_string(machine.snapshot())?)?;
    inner.store.upsert_run(machine.run())?;
    inner.publish(Event::RunChanged { run: machine.run().clone() });
    drive(inner, graph, machine)
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
    let Ok(runs) = inner.store.open_runs() else { return };
    for run in runs {
        if inner.store.marker(&snapshot_marker(&run.id)).ok().flatten().is_none() {
            continue;
        }
        if let Err(error) = restore(inner, &run.id) {
            inner.notice_error(&format!("The run {} could not be restored: {error:#}", run.id));
        }
    }
}
