//! Pipeline runs: the latest run of each issue and the open runs ([`RunsModel`]), and one opened
//! run with its node instances and a bounded log tail ([`RunModel`]).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::anyhow;
use gpui::{Context, EventEmitter};

use crate::api::{Api, IssueKey, NodeId, PipelineId, Run, RunId, RunLogLine, RunWait};
use crate::load::{LoadState, Loadable, failed, fetch, query};

/// Log lines a [`RunModel`] keeps; older ones fall off the front.
pub const LOG_CAPACITY: usize = 2000;
/// Log lines read when a run is opened.
pub const LOG_TAIL: usize = 500;
/// Runs of a pipeline [`RunsModel::pipeline_runs`] keeps, newest first.
pub const PIPELINE_RUNS: usize = 20;

/// Runs for the board, Home and the sidebar. Updated by `RunChanged`.
pub struct RunsModel {
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    latest: HashMap<IssueKey, Arc<Run>>,
    /// Open runs without an issue, newest first.
    open: Vec<Arc<Run>>,
    /// Why open runs wait for a person.
    waits: HashMap<RunId, RunWait>,
    /// Runs of the pipelines a view asked for, newest first.
    by_pipeline: HashMap<PipelineId, Loadable<Vec<Arc<Run>>>>,
    buffer: Vec<Run>,
    wait_buffer: Vec<(RunId, Option<RunWait>)>,
}

impl RunsModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            state: LoadState::Loading,
            started: false,
            latest: HashMap::new(),
            open: Vec::new(),
            waits: HashMap::new(),
            by_pipeline: HashMap::new(),
            buffer: Vec::new(),
            wait_buffer: Vec::new(),
        }
    }

    /// Why a run waits for a person (a review, a question, its budget); `None` when it does
    /// not, or is not loaded.
    pub fn waiting(&self, id: &RunId) -> Option<RunWait> {
        self.waits.get(id).copied()
    }

    /// The newest runs of a pipeline (test runs included, see `Run::test`), once
    /// [`Self::ensure_pipeline_runs`] asked for them. Kept current by `RunChanged`.
    pub fn pipeline_runs(&self, id: &PipelineId) -> Option<&Loadable<Vec<Arc<Run>>>> {
        self.by_pipeline.get(id)
    }

    /// The newest test run of a pipeline, for "Last test run #N", once loaded.
    pub fn last_test_run(&self, id: &PipelineId) -> Option<&Arc<Run>> {
        self.by_pipeline.get(id)?.value()?.iter().find(|run| run.test)
    }

    /// Reads a pipeline's newest runs unless they are loaded or loading.
    pub fn ensure_pipeline_runs(&mut self, id: &PipelineId, cx: &mut Context<Self>) {
        if self.by_pipeline.get(id).is_some_and(Loadable::started) {
            return;
        }
        let (api, key, wanted) = (self.api.clone(), id.clone(), id.clone());
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.by_pipeline.entry(key.clone()).or_default(),
            move |api| Ok(api.runs_for_pipeline(&wanted, PIPELINE_RUNS)?.into_iter().map(Arc::new).collect()),
        );
    }

    /// Whether the first load is done.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// The latest run on an issue.
    pub fn latest_for(&self, issue: &IssueKey) -> Option<&Arc<Run>> {
        self.latest.get(issue)
    }

    /// Open runs that belong to no issue (manual and test runs), newest first.
    pub fn open_without_issue(&self) -> &[Arc<Run>] {
        &self.open
    }

    /// Every open run the model knows, newest first.
    pub fn open_runs(&self) -> Vec<Arc<Run>> {
        let mut runs: Vec<Arc<Run>> =
            self.latest.values().filter(|run| run.state.is_open()).chain(self.open.iter()).cloned().collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started));
        runs
    }

    /// A run the model knows, by id.
    pub fn find(&self, id: &RunId) -> Option<&Arc<Run>> {
        self.latest.values().chain(self.open.iter()).find(|run| &run.id == id)
    }

    /// Reads the latest and open runs, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        query(cx, &self.api, |api| Ok((api.latest_runs()?, api.waiting_runs()?)), |this: &mut Self, result, cx| {
            match result {
                Ok((runs, waits)) => {
                    for run in runs {
                        this.upsert(run);
                    }
                    this.waits = waits.into_iter().collect();
                    this.state = LoadState::Ready;
                    for run in std::mem::take(&mut this.buffer) {
                        this.upsert(run);
                    }
                    for (id, wait) in std::mem::take(&mut this.wait_buffer) {
                        this.set_wait(id, wait);
                    }
                }
                Err(error) => this.state = failed(&error),
            }
            cx.notify();
        });
    }

    /// Changed runs and changed waits of one batch.
    pub(crate) fn apply_all(&mut self, runs: Vec<Run>, waits: Vec<(RunId, Option<RunWait>)>, cx: &mut Context<Self>) {
        let mut changed = false;
        for run in &runs {
            if let Some(slot) = self.by_pipeline.get_mut(&run.pipeline) {
                changed |= slot.edit(|known| {
                    known.retain(|other| other.id != run.id);
                    known.push(Arc::new(run.clone()));
                    known.sort_by_key(|run| std::cmp::Reverse((run.started, run.id.clone())));
                    known.truncate(PIPELINE_RUNS);
                });
            }
        }
        if !self.started {
            if changed {
                cx.notify();
            }
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(runs);
            self.wait_buffer.extend(waits);
            if changed {
                cx.notify();
            }
            return;
        }
        for run in runs {
            self.upsert(run);
        }
        for (id, wait) in waits {
            self.set_wait(id, wait);
        }
        cx.notify();
    }

    fn set_wait(&mut self, id: RunId, wait: Option<RunWait>) {
        match wait {
            Some(wait) => self.waits.insert(id, wait),
            None => self.waits.remove(&id),
        };
    }

    fn upsert(&mut self, run: Run) {
        if !run.state.needs_person() {
            self.waits.remove(&run.id);
        }
        let run = Arc::new(run);
        match run.issue.clone() {
            Some(issue) => {
                let newer = self
                    .latest
                    .get(&issue)
                    .is_none_or(|known| known.id == run.id || (run.number, run.started) >= (known.number, known.started));
                if newer {
                    self.latest.insert(issue, run);
                }
            }
            None => {
                self.open.retain(|known| known.id != run.id);
                if run.state.is_open() {
                    self.open.push(run);
                    self.open.sort_by_key(|run| std::cmp::Reverse(run.started));
                }
            }
        }
    }
}

/// What changed in a run's log, for a list view of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunLogEvent {
    /// `added` lines arrived at the end and `dropped` fell off the front.
    Appended { added: usize, dropped: usize },
}

pub(crate) enum RunInput {
    Changed(Run),
    Log(RunLogLine),
    Waiting(Option<RunWait>),
}

/// One opened run: its summary and node instances, and the tail of its log. Created on demand
/// by [`crate::run`] and dropped when no view holds it.
pub struct RunModel {
    id: RunId,
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    run: Option<Arc<Run>>,
    waiting: Option<RunWait>,
    log: VecDeque<RunLogLine>,
    buffer: Vec<RunInput>,
}

impl EventEmitter<RunLogEvent> for RunModel {}

impl RunModel {
    pub(crate) fn new(id: RunId, api: Arc<dyn Api>) -> Self {
        Self {
            id,
            api,
            state: LoadState::Loading,
            started: false,
            run: None,
            waiting: None,
            log: VecDeque::new(),
            buffer: Vec::new(),
        }
    }

    /// Why the run waits for a person (a review, a question, its budget); `None` when it
    /// does not.
    pub fn waiting(&self) -> Option<RunWait> {
        self.waiting
    }

    /// The run this model shows.
    pub fn id(&self) -> &RunId {
        &self.id
    }

    /// Where the run stands.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// The run, once loaded.
    pub fn run(&self) -> Option<&Arc<Run>> {
        self.run.as_ref()
    }

    /// The last [`LOG_CAPACITY`] lines at most, oldest first.
    pub fn log(&self) -> &VecDeque<RunLogLine> {
        &self.log
    }

    /// Lines of one node instance.
    pub fn log_for<'a>(&'a self, node: &'a NodeId, variant: u32) -> impl Iterator<Item = &'a RunLogLine> + 'a {
        self.log.iter().filter(move |line| line.node.as_ref() == Some(node) && line.variant == variant)
    }

    /// Reads the run and the tail of its log, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        let id = self.id.clone();
        query(
            cx,
            &self.api,
            move |api| {
                let run = api.run(&id)?.ok_or_else(|| anyhow!("This run no longer exists"))?;
                Ok((run, api.run_log_tail(&id, LOG_TAIL)?, api.run_waiting(&id)?))
            },
            |this: &mut Self, result, cx| {
                match result {
                    Ok((run, lines, waiting)) => {
                        this.run = Some(Arc::new(run));
                        this.waiting = waiting;
                        this.log = lines.into_iter().collect();
                        this.state = LoadState::Ready;
                        let buffered = std::mem::take(&mut this.buffer);
                        this.apply_inputs(buffered, cx);
                    }
                    Err(error) => this.state = failed(&error),
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<RunInput>, cx: &mut Context<Self>) {
        if !self.started || matches!(self.state, LoadState::Failed(_)) {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        self.apply_inputs(inputs, cx);
        cx.notify();
    }

    fn apply_inputs(&mut self, inputs: Vec<RunInput>, cx: &mut Context<Self>) {
        let mut added = 0;
        let mut dropped = 0;
        for input in inputs {
            match input {
                RunInput::Changed(run) => {
                    if !run.state.needs_person() {
                        self.waiting = None;
                    }
                    self.run = Some(Arc::new(run));
                }
                RunInput::Waiting(waiting) => self.waiting = waiting,
                RunInput::Log(line) => {
                    if self.log.back().is_some_and(|last| last.seq >= line.seq) {
                        continue;
                    }
                    self.log.push_back(line);
                    added += 1;
                    if self.log.len() > LOG_CAPACITY {
                        self.log.pop_front();
                        dropped += 1;
                    }
                }
            }
        }
        if added > 0 {
            cx.emit(RunLogEvent::Appended { added, dropped });
        }
    }
}
