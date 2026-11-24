//! Pipeline runs: the latest run of each issue and the open runs ([`RunsModel`]), and one opened
//! run with its node instances and a bounded log tail ([`RunModel`]).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::anyhow;
use gpui::{Context, EventEmitter};

use crate::api::{Api, IssueKey, NodeId, Run, RunId, RunLogLine};
use crate::load::{LoadState, failed, query};

/// Log lines a [`RunModel`] keeps; older ones fall off the front.
pub const LOG_CAPACITY: usize = 2000;
/// Log lines read when a run is opened.
pub const LOG_TAIL: usize = 500;

/// Runs for the board, Home and the sidebar. Updated by `RunChanged`.
pub struct RunsModel {
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    latest: HashMap<IssueKey, Arc<Run>>,
    /// Open runs without an issue, newest first.
    open: Vec<Arc<Run>>,
    buffer: Vec<Run>,
}

impl RunsModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self { api, state: LoadState::Loading, started: false, latest: HashMap::new(), open: Vec::new(), buffer: Vec::new() }
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
        query(cx, &self.api, |api| api.latest_runs(), |this: &mut Self, result, cx| {
            match result {
                Ok(runs) => {
                    for run in runs {
                        this.upsert(run);
                    }
                    this.state = LoadState::Ready;
                    for run in std::mem::take(&mut this.buffer) {
                        this.upsert(run);
                    }
                }
                Err(error) => this.state = failed(&error),
            }
            cx.notify();
        });
    }

    pub(crate) fn apply(&mut self, runs: Vec<Run>, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(runs);
            return;
        }
        for run in runs {
            self.upsert(run);
        }
        cx.notify();
    }

    fn upsert(&mut self, run: Run) {
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
}

/// One opened run: its summary and node instances, and the tail of its log. Created on demand
/// by [`crate::run`] and dropped when no view holds it.
pub struct RunModel {
    id: RunId,
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    run: Option<Arc<Run>>,
    log: VecDeque<RunLogLine>,
    buffer: Vec<RunInput>,
}

impl EventEmitter<RunLogEvent> for RunModel {}

impl RunModel {
    pub(crate) fn new(id: RunId, api: Arc<dyn Api>) -> Self {
        Self { id, api, state: LoadState::Loading, started: false, run: None, log: VecDeque::new(), buffer: Vec::new() }
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
                Ok((run, api.run_log_tail(&id, LOG_TAIL)?))
            },
            |this: &mut Self, result, cx| {
                match result {
                    Ok((run, lines)) => {
                        this.run = Some(Arc::new(run));
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
                RunInput::Changed(run) => self.run = Some(Arc::new(run)),
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
