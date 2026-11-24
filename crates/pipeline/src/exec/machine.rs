//! The state machine of one run: its inputs, and the bookkeeping they share.

use model::{
    GraphDef, LogLevel, Millis, NodeDef, NodeId, NodeInstance, NodeKind, NodeStatus, Review, ReviewDecision, ReviewId,
    ReviewState, Run, RunLogLine, RunState, SpanKind,
};

use super::plan::{Plan, is_agent};
use super::state::{Halt, Hold, Meta, Snapshot};
use super::{Effect, ExecError};
use crate::api::{InstanceKey, Item, JobNote, JobResult};

/// Budgets are compared with this slack so that sums of cents do not miss the cap by rounding.
const CENT_SLACK: f64 = 1e-9;

/// Log sequence numbers are reserved in blocks of this size; see [`Snapshot::log_seq`].
const LOG_BLOCK: u64 = 256;

/// One run of one graph. Feed it what happened; do what it answers.
///
/// Every input takes the current time and returns the effects in the order they must be carried
/// out. An `Err` means the input was refused and nothing changed.
pub struct Machine {
    pub(super) graph: GraphDef,
    pub(super) plan: Plan,
    /// The source nodes the item enters at.
    pub(super) entry: Vec<NodeId>,
    pub(super) s: Snapshot,
    /// Sequence number of the last log line handed out; never above `s.log_seq`.
    log_last: u64,
}

impl Machine {
    /// A machine for `item` running through `graph`. `run` is the shell the engine created (id,
    /// pipeline, version, issue, title, input, test flag); nothing happens until [`Self::start`].
    pub fn new(graph: GraphDef, mut run: Run, item: Item) -> Machine {
        run.state = RunState::Queued;
        run.nodes.clear();
        run.finished = None;
        run.error = None;
        let snapshot = Snapshot {
            budget_usd: graph.limits.budget_usd,
            run,
            item,
            meta: Vec::new(),
            reviews: Vec::new(),
            scopes: Default::default(),
            attempts: Default::default(),
            retries: Vec::new(),
            halt: None,
            started: false,
            budget_noted: false,
            log_seq: 0,
            order: 0,
        };
        Self::build(graph, snapshot)
    }

    /// Continues from a stored snapshot after a restart. No job survived the restart, so
    /// instances that were running become `Interrupted` and so does the run; [`Self::resume`]
    /// re-issues their jobs. A run that owes no job (it waited for a person, or was closed) is
    /// left as it was, whatever cancelled jobs were still winding down when it was stored.
    pub fn restore(graph: GraphDef, snapshot: Snapshot) -> Machine {
        let mut machine = Self::build(graph, snapshot);
        // A snapshot written by hand or by an older build may have lost the pairing.
        machine.s.meta.resize_with(machine.s.run.nodes.len(), Meta::default);
        let mut interrupted = false;
        for (view, meta) in machine.s.run.nodes.iter_mut().zip(machine.s.meta.iter_mut()) {
            meta.in_flight = false;
            if view.status == NodeStatus::Running {
                view.status = NodeStatus::Interrupted;
                interrupted |= meta.spawn.is_none() && !meta.superseded;
            }
        }
        // An instance that waited to start behind a job that died (for its slot, or for its
        // own cancelled job to return) has nothing left to wake it: `resume` does.
        let over_budget = machine.over_budget();
        interrupted |= !over_budget
            && (0..machine.s.meta.len()).any(|idx| machine.is_current(idx) && machine.s.run.nodes[idx].status == NodeStatus::Pending);
        if interrupted && machine.s.halt.is_none() && machine.s.started {
            machine.s.halt = Some(Halt::Interrupted);
        }
        machine.s.run.state = machine.derive_state();
        machine
    }

    fn build(graph: GraphDef, snapshot: Snapshot) -> Machine {
        let plan = Plan::new(&graph);
        let source_kind = |id: &NodeId| match graph.node(id).map(|def| &def.kind) {
            Some(NodeKind::Source(source)) => Some(source.source),
            _ => None,
        };
        let mut entry: Vec<NodeId> =
            plan.sources.iter().filter(|id| source_kind(id) == Some(snapshot.item.source)).cloned().collect();
        if entry.is_empty() {
            // A manual or test run of a pipeline that only has an issue source still has to enter.
            entry.extend(plan.sources.first().cloned());
        }
        // Numbers up to the stored bound may have been used by lines the store already has.
        let log_last = snapshot.log_seq;
        Machine { graph, plan, entry, s: snapshot, log_last }
    }

    /// The whole state, for storing.
    pub fn snapshot(&self) -> &Snapshot {
        &self.s
    }

    /// The run as it is now.
    pub fn run(&self) -> &Run {
        &self.s.run
    }

    /// The graph the run is pinned to.
    pub fn graph(&self) -> &GraphDef {
        &self.graph
    }

    /// Reviews waiting for a decision.
    pub fn open_reviews(&self) -> Vec<&Review> {
        self.s.reviews.iter().map(|round| &round.review).filter(|review| review.state == ReviewState::Open).collect()
    }

    /// Instances waiting for a person, with the reason: a question to answer or a budget to raise.
    pub fn holds(&self) -> Vec<(InstanceKey, Hold)> {
        (0..self.s.meta.len())
            .filter(|&idx| self.is_current(idx) && self.s.run.nodes[idx].status == NodeStatus::Waiting)
            .filter_map(|idx| self.s.meta[idx].hold.clone().map(|hold| (self.key_of(idx), hold)))
            .collect()
    }

    /// Whether the run's spending cap is used up: nothing starts until [`Self::raise_budget`].
    pub fn budget_reached(&self) -> bool {
        self.over_budget()
    }

    /// Jobs started and not yet reported back, cancelled ones included.
    pub fn jobs_in_flight(&self) -> usize {
        self.s.meta.iter().filter(|meta| meta.in_flight).count()
    }

    // ------------------------------------------------------------------ inputs

    /// Lets the item in at the source.
    pub fn start(&mut self, now: Millis) -> Result<Vec<Effect>, ExecError> {
        if self.s.started {
            return Err(ExecError::AlreadyStarted);
        }
        if self.s.halt.is_some() {
            return Err(ExecError::RunClosed(self.s.run.state));
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.s.started = true;
        self.s.run.started = now;
        if let Some(problem) = self.plan.problem.clone() {
            self.halt_run(Halt::Failed, Some(problem), now, &mut fx);
        }
        Ok(self.end(before, fx, now))
    }

    /// A job ended. Results of jobs that were cancelled are expected and keep what the job
    /// left behind; a result for a key with no job in flight is refused.
    pub fn job_finished(&mut self, key: &InstanceKey, result: JobResult, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let idx = self.find(key).ok_or_else(|| ExecError::UnknownInstance(key.clone()))?;
        if !self.s.meta[idx].in_flight {
            return Err(ExecError::NoJob(key.clone()));
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.finish_job(idx, result, now, &mut fx);
        Ok(self.end(before, fx, now))
    }

    /// A running job reported something.
    pub fn note(&mut self, key: &InstanceKey, note: JobNote, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let idx = self.find(key).ok_or_else(|| ExecError::UnknownInstance(key.clone()))?;
        if !self.s.meta[idx].in_flight {
            return Err(ExecError::NoJob(key.clone()));
        }
        let mut fx = Vec::new();
        if let JobNote::Log { level, text } = note {
            self.log(&mut fx, now, Some(idx), level, text);
            return Ok(fx);
        }
        let before = self.s.run.clone();
        self.take_note(idx, note, now, &mut fx);
        Ok(self.end(before, fx, now))
    }

    /// A person decided an open review.
    pub fn decide(&mut self, review: &ReviewId, decision: ReviewDecision, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.decide_review(review, decision, now, &mut fx)?;
        Ok(self.end(before, fx, now))
    }

    /// A person answered the question of a waiting instance; its work continues in a new job
    /// with the same key.
    pub fn answer(&mut self, key: &InstanceKey, text: String, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let idx = self.find(key).ok_or_else(|| ExecError::UnknownInstance(key.clone()))?;
        let waiting = self.is_current(idx)
            && self.s.run.nodes[idx].status == NodeStatus::Waiting
            && matches!(self.s.meta[idx].hold, Some(Hold::Question { .. }));
        if !waiting {
            return Err(ExecError::NotWaiting(key.clone()));
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        let meta = &mut self.s.meta[idx];
        meta.hold = None;
        meta.answer = Some(text);
        meta.resume = true;
        self.s.run.nodes[idx].status = NodeStatus::Pending;
        self.log(&mut fx, now, Some(idx), LogLevel::Info, "Answered; continuing.".to_string());
        Ok(self.end(before, fx, now))
    }

    /// Allows `usd` more to be spent from now: the run's cap moves to what was spent plus `usd`,
    /// and so does the node cap of every instance that stopped for its budget. Also taken by a
    /// cancelled, failed or interrupted run, where a person raises first and resumes after;
    /// only a run that is done refuses it.
    pub fn raise_budget(&mut self, usd: f64, now: Millis) -> Result<Vec<Effect>, ExecError> {
        if !usd.is_finite() || usd <= 0.0 {
            return Err(ExecError::BadAmount);
        }
        if self.s.run.state == RunState::Done {
            return Err(ExecError::RunClosed(RunState::Done));
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        let spent = self.s.run.cost_usd;
        if let Some(budget) = self.s.budget_usd.as_mut() {
            *budget = budget.max(spent) + usd;
        }
        self.s.budget_noted = false;
        for idx in 0..self.s.meta.len() {
            if !self.is_current(idx) || self.s.meta[idx].hold != Some(Hold::Budget) {
                continue;
            }
            let cost = self.s.run.nodes[idx].cost_usd;
            let meta = &mut self.s.meta[idx];
            meta.hold = None;
            meta.node_budget = meta.node_budget.map(|cap| cap.max(cost) + usd);
            if self.s.run.nodes[idx].status == NodeStatus::Waiting {
                meta.resume = true;
                self.s.run.nodes[idx].status = NodeStatus::Pending;
            }
        }
        self.log(&mut fx, now, None, LogLevel::Info, format!("Budget raised by {}.", base::fmt::cost(usd)));
        Ok(self.end(before, fx, now))
    }

    /// Stops the run: running jobs are cancelled, open reviews are closed. [`Self::resume`]
    /// continues from here.
    pub fn cancel(&mut self, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let state = self.s.run.state;
        if matches!(state, RunState::Done | RunState::Failed | RunState::Cancelled) {
            return Err(ExecError::RunClosed(state));
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.log(&mut fx, now, None, LogLevel::Warn, "Run cancelled.".to_string());
        self.halt_run(Halt::Cancelled, None, now, &mut fx);
        Ok(self.end(before, fx, now))
    }

    /// Stops the run because its driver is going away: running jobs are cancelled and marked
    /// `Interrupted`, and nothing starts any more, also not when a job that was cancelled
    /// earlier reports. Reviews and questions stay open. Does nothing when the run has no job
    /// in flight and none running: a run that only waits for a person stays as it is.
    pub fn interrupt(&mut self, now: Millis) -> Vec<Effect> {
        let busy = (0..self.s.meta.len())
            .any(|idx| self.s.meta[idx].in_flight || (self.is_current(idx) && self.s.run.nodes[idx].status == NodeStatus::Running));
        if self.s.halt.is_some() || !busy {
            return Vec::new();
        }
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.halt_run(Halt::Interrupted, None, now, &mut fx);
        self.end(before, fx, now)
    }

    /// Continues a cancelled, failed or interrupted run from where it stopped. Stopped
    /// instances get a new job with the same key and their session in `Job::resume_session`;
    /// after a failure the failed instances are tried again.
    pub fn resume(&mut self, now: Millis) -> Result<Vec<Effect>, ExecError> {
        let Some(halt) = self.s.halt else { return Err(ExecError::NotStopped) };
        let before = self.s.run.clone();
        let mut fx = Vec::new();
        self.s.halt = None;
        self.s.run.error = None;
        self.s.run.finished = None;
        if self.plan.problem.is_some() {
            // Nothing can run in this graph; `start` already said why.
            let problem = self.plan.problem.clone();
            self.halt_run(Halt::Failed, problem, now, &mut fx);
            return Ok(self.end(before, fx, now));
        }
        self.log(&mut fx, now, None, LogLevel::Info, "Run resumed.".to_string());
        let mut reopen = Vec::new();
        for idx in 0..self.s.meta.len() {
            if !self.is_current(idx) {
                continue;
            }
            let status = self.s.run.nodes[idx].status;
            let stopped = matches!(status, NodeStatus::Cancelled | NodeStatus::Interrupted)
                || (status == NodeStatus::Failed && halt == Halt::Failed);
            if !stopped {
                continue;
            }
            if self.is_review(&self.s.run.nodes[idx].node) {
                reopen.push(idx);
                continue;
            }
            let view = &mut self.s.run.nodes[idx];
            view.finished = None;
            match self.s.meta[idx].hold.clone() {
                Some(hold) => {
                    view.status = NodeStatus::Waiting;
                    if let Hold::Question { text } = hold {
                        view.question = Some(text.clone());
                        fx.push(Effect::Asked { key: self.key_of(idx), question: text });
                    }
                }
                None => {
                    view.status = NodeStatus::Pending;
                    self.s.meta[idx].resume = true;
                }
            }
        }
        // A review that was closed by the stop is built again from what is there.
        for idx in reopen.into_iter().rev() {
            self.remove_instance(idx);
        }
        Ok(self.end(before, fx, now))
    }

    // ------------------------------------------------------------------ shared bookkeeping

    /// Finishes a step: lets the item move, settles the run's state and reports what changed.
    pub(super) fn end(&mut self, before: Run, mut fx: Vec<Effect>, now: Millis) -> Vec<Effect> {
        if self.s.started && self.s.halt.is_none() {
            self.settle(now, &mut fx);
        }
        let state = self.derive_state();
        self.s.run.state = state;
        let closed = matches!(state, RunState::Done | RunState::Failed | RunState::Cancelled);
        if closed && self.s.run.finished.is_none() {
            self.s.run.finished = Some(now);
        }
        if self.s.run != before {
            fx.push(Effect::Run(Box::new(self.s.run.clone())));
        }
        if closed && before.state != state {
            fx.push(Effect::Finished(state));
        }
        fx
    }

    pub(super) fn derive_state(&self) -> RunState {
        match self.s.halt {
            Some(Halt::Cancelled) => return RunState::Cancelled,
            Some(Halt::Failed) => return RunState::Failed,
            Some(Halt::Interrupted) => return RunState::Interrupted,
            None => {}
        }
        if !self.s.started {
            return RunState::Queued;
        }
        let (mut working, mut pending, mut held, mut review) = (false, false, false, false);
        for idx in (0..self.s.meta.len()).filter(|&idx| self.is_current(idx)) {
            let view = &self.s.run.nodes[idx];
            match view.status {
                NodeStatus::Running => working = true,
                NodeStatus::Pending => pending = true,
                NodeStatus::Waiting if self.is_review(&view.node) => review = true,
                NodeStatus::Waiting => held = true,
                _ => {}
            }
        }
        if held || (pending && self.over_budget()) {
            RunState::WaitingAnswer
        } else if review {
            RunState::WaitingReview
        } else if working || pending {
            RunState::Running
        } else {
            RunState::Done
        }
    }

    /// Stops the run. Running jobs are cancelled; unless the stop is an interruption, reviews
    /// are closed and waiting instances are parked until `resume`.
    pub(super) fn halt_run(&mut self, halt: Halt, error: Option<String>, now: Millis, fx: &mut Vec<Effect>) {
        self.s.halt = Some(halt);
        self.s.run.error = error;
        let stopped = if halt == Halt::Interrupted { NodeStatus::Interrupted } else { NodeStatus::Cancelled };
        for idx in 0..self.s.meta.len() {
            if self.s.meta[idx].superseded {
                continue;
            }
            let status = self.s.run.nodes[idx].status;
            if self.s.meta[idx].spawn.is_some() {
                if status == NodeStatus::Running {
                    self.s.run.nodes[idx].status = stopped;
                }
                continue;
            }
            match status {
                NodeStatus::Running => {
                    if self.s.meta[idx].in_flight {
                        fx.push(Effect::CancelJob(self.key_of(idx)));
                    }
                    let view = &mut self.s.run.nodes[idx];
                    view.status = stopped;
                    if halt != Halt::Interrupted {
                        view.finished = Some(now);
                    }
                }
                NodeStatus::Waiting if halt != Halt::Interrupted => {
                    self.close_review_of(idx, ReviewState::Skipped, fx);
                    let view = &mut self.s.run.nodes[idx];
                    view.status = NodeStatus::Cancelled;
                    view.finished = Some(now);
                    // Asked again when the run resumes.
                    view.question = None;
                }
                _ => {}
            }
        }
    }

    /// Marks the open review behind a review instance as closed without a decision.
    pub(super) fn close_review_of(&mut self, idx: usize, state: ReviewState, fx: &mut Vec<Effect>) {
        let view = &self.s.run.nodes[idx];
        let round = self.s.reviews.iter_mut().find(|round| {
            round.review.node == view.node && round.attempt == view.attempt && round.review.state == ReviewState::Open
        });
        if let Some(round) = round {
            round.review.state = state;
            fx.push(Effect::ReviewClosed(round.review.clone()));
        }
    }

    pub(super) fn over_budget(&self) -> bool {
        self.s.budget_usd.is_some_and(|budget| self.s.run.cost_usd + CENT_SLACK >= budget)
    }

    /// What a job of this instance may still spend, when anything limits it.
    pub(super) fn budget_left(&self, idx: usize) -> Option<f64> {
        let run_left = self.s.budget_usd.map(|budget| budget - self.s.run.cost_usd);
        let node_left = self.s.meta[idx].node_budget.map(|cap| cap - self.s.run.nodes[idx].cost_usd);
        let left = match (run_left, node_left) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return None,
        };
        Some(left.max(0.0))
    }

    pub(super) fn def(&self, node: &NodeId) -> Option<&NodeDef> {
        self.graph.node(node)
    }

    pub(super) fn is_review(&self, node: &NodeId) -> bool {
        self.def(node).is_some_and(|def| matches!(def.kind, NodeKind::Review(_)))
    }

    pub(super) fn is_agent_node(&self, node: &NodeId) -> bool {
        self.def(node).is_some_and(|def| is_agent(&def.kind))
    }

    /// A graph instance on the item's present path: not a spawned sub-agent, not replaced.
    pub(super) fn is_current(&self, idx: usize) -> bool {
        let meta = &self.s.meta[idx];
        !meta.superseded && meta.spawn.is_none()
    }

    /// The instance of `node` for `variant` on the item's present path.
    pub(super) fn current(&self, node: &NodeId, variant: u32) -> Option<usize> {
        (0..self.s.meta.len()).rev().find(|&idx| {
            let view = &self.s.run.nodes[idx];
            self.is_current(idx) && view.variant == variant && &view.node == node
        })
    }

    /// The graph instance with this key, replaced ones included.
    pub(super) fn find(&self, key: &InstanceKey) -> Option<usize> {
        (0..self.s.meta.len()).rev().find(|&idx| {
            let view = &self.s.run.nodes[idx];
            self.s.meta[idx].spawn.is_none() && view.node == key.node && view.variant == key.variant && view.attempt == key.attempt
        })
    }

    pub(super) fn key_of(&self, idx: usize) -> InstanceKey {
        let view = &self.s.run.nodes[idx];
        InstanceKey { node: view.node.clone(), variant: view.variant, attempt: view.attempt }
    }

    pub(super) fn attempt_of(&self, node: &NodeId) -> u32 {
        self.s.attempts.get(node).copied().unwrap_or(1)
    }

    /// Adds an instance of `def` in the node's present attempt.
    pub(super) fn push_instance(&mut self, def: &NodeDef, variant: u32, status: NodeStatus) -> usize {
        let kind = match def.kind {
            NodeKind::Agent(_) | NodeKind::Orchestrator(_) => SpanKind::Agent,
            NodeKind::Review(_) => SpanKind::Waiting,
            _ => SpanKind::Step,
        };
        let node_budget = match &def.kind {
            NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => agent.budget_usd,
            _ => None,
        };
        self.s.run.nodes.push(NodeInstance {
            question: None,
            node: def.id.clone(),
            name: def.name.clone(),
            variant,
            attempt: self.attempt_of(&def.id),
            status,
            kind,
            port: None,
            session: None,
            worktree: None,
            branch: None,
            started: None,
            finished: None,
            cost_usd: 0.0,
            summary: String::new(),
            spawned_by: None,
        });
        self.s.meta.push(Meta { node_budget, ..Meta::default() });
        self.s.run.nodes.len() - 1
    }

    /// Drops an instance that never did anything (a skipped placeholder, a closed review that
    /// is opened again).
    pub(super) fn remove_instance(&mut self, idx: usize) {
        self.s.run.nodes.remove(idx);
        self.s.meta.remove(idx);
    }

    pub(super) fn next_order(&mut self) -> u64 {
        self.s.order += 1;
        self.s.order
    }

    /// Adds a log line, about an instance when `idx` is given.
    pub(super) fn log(&mut self, fx: &mut Vec<Effect>, now: Millis, idx: Option<usize>, level: LogLevel, text: String) {
        self.log_last += 1;
        if self.log_last > self.s.log_seq {
            // Reserve a block, so that the snapshot changes once per block and not per line.
            self.s.log_seq = self.log_last + LOG_BLOCK - 1;
        }
        let (node, variant) = match idx {
            Some(idx) => {
                let view = &self.s.run.nodes[idx];
                let node = self.s.meta[idx].spawn.as_ref().map_or(&view.node, |link| &link.parent.node);
                (Some(node.clone()), view.variant)
            }
            None => (None, 0),
        };
        fx.push(Effect::Log(RunLogLine { seq: self.log_last, at: now, node, variant, level, text }));
    }
}
