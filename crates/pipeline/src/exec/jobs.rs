//! What a job's notes and its end do to the run: spending, places, questions, failures.

use model::graph::PORT_ELSE;
use model::{LogLevel, Millis, NodeInstance, NodeKind, NodeStatus, SpanKind, Usage};

use super::state::{Halt, Hold, Meta, SpawnLink};
use super::{Effect, Machine};
use crate::api::{JobNote, JobResult, NodeOutput, SpawnedState};

/// How a job ended, without its output.
enum Ending {
    Done(String),
    Ask(String),
    OverBudget,
    Failed(String),
    Cancelled,
}

fn split(result: JobResult) -> (Ending, NodeOutput) {
    match result {
        JobResult::Done { port, output } => (Ending::Done(port), output),
        JobResult::Ask { question, output } => (Ending::Ask(question), output),
        JobResult::OverBudget { output } => (Ending::OverBudget, output),
        JobResult::Failed { error, output } => (Ending::Failed(error), output),
        JobResult::Cancelled { output } => (Ending::Cancelled, output),
    }
}

fn usage_over(total: Usage, counted: Usage) -> Usage {
    Usage {
        input: total.input.saturating_sub(counted.input),
        output: total.output.saturating_sub(counted.output),
        cache_read: total.cache_read.saturating_sub(counted.cache_read),
        cache_write: total.cache_write.saturating_sub(counted.cache_write),
    }
}

impl Machine {
    /// The job of instance `idx` reported its end. The caller checked that one was in flight.
    pub(super) fn finish_job(&mut self, idx: usize, result: JobResult, now: Millis, fx: &mut Vec<Effect>) {
        let (ending, output) = split(result);
        self.s.meta[idx].in_flight = false;
        self.absorb(idx, output);
        self.close_spawned(idx, matches!(ending, Ending::Done(_) | Ending::Ask(_)), now);

        if self.s.meta[idx].superseded {
            // A job that was cancelled by a send-back: history only.
            if let Ending::Failed(error) = &ending {
                self.log(fx, now, Some(idx), LogLevel::Warn, format!("Stopped attempt ended with an error: {error}"));
            }
            let placed = |out: &NodeOutput| out.worktree.is_some() || out.branch.is_some();
            if std::mem::take(&mut self.s.meta[idx].discard_late)
                && !self.s.meta[idx].discarded
                && let Some(output) = self.s.meta[idx].output.clone().filter(placed)
            {
                self.s.meta[idx].discarded = true;
                fx.push(Effect::Discard(Box::new(output)));
            }
            self.check_budget(now, fx);
            return;
        }

        let status = self.s.run.nodes[idx].status;
        let expected = status == NodeStatus::Running;
        match ending {
            // Finished work counts even when the job had been asked to stop.
            Ending::Done(port) => self.complete(idx, port, now, fx),
            Ending::Ask(question) => {
                self.s.meta[idx].hold = Some(Hold::Question { text: question.clone() });
                self.s.meta[idx].answer = None;
                self.s.meta[idx].resume = false;
                let view = &mut self.s.run.nodes[idx];
                view.summary = question.clone();
                // In a stopped run the question is kept and asked again on resume.
                if self.s.halt.is_none() {
                    view.status = NodeStatus::Waiting;
                    view.finished = None;
                    self.log(fx, now, Some(idx), LogLevel::Info, format!("Asks: {question}"));
                    fx.push(Effect::Asked { key: self.key_of(idx), question });
                }
            }
            Ending::OverBudget if expected => {
                self.s.meta[idx].hold = Some(Hold::Budget);
                self.s.run.nodes[idx].status = NodeStatus::Waiting;
                let text = "Out of budget; waiting for it to be raised.".to_string();
                self.log(fx, now, Some(idx), LogLevel::Warn, text);
            }
            Ending::Failed(error) if expected => self.fail_instance(idx, error, now, fx),
            Ending::Cancelled if expected => {
                // Nobody here asked for it: whoever runs the jobs is shutting down.
                self.log(fx, now, Some(idx), LogLevel::Warn, "The job was stopped from outside.".to_string());
                self.s.run.nodes[idx].status = NodeStatus::Interrupted;
                self.halt_run(Halt::Interrupted, None, now, fx);
            }
            Ending::Failed(error) => {
                // It was already told to stop; it will run again when the run goes on.
                self.log(fx, now, Some(idx), LogLevel::Warn, format!("Stopped with an error: {error}"));
            }
            Ending::OverBudget | Ending::Cancelled => {}
        }
        self.check_budget(now, fx);
    }

    /// Books what a job's output says it spent and where it worked, and keeps the output.
    fn absorb(&mut self, idx: usize, mut output: NodeOutput) {
        let meta = &self.s.meta[idx];
        let extra_cost = output.cost_usd - meta.job_cost;
        let extra_usage = usage_over(output.usage, meta.job_usage);
        if extra_cost > 0.0 {
            self.spend(idx, extra_cost, Usage::default());
        }
        self.spend(idx, 0.0, extra_usage);
        let view = &mut self.s.run.nodes[idx];
        if output.session.is_some() {
            view.session = output.session.clone();
        }
        if output.worktree.is_some() {
            view.worktree = output.worktree.clone();
        }
        if output.branch.is_some() {
            view.branch = output.branch.clone();
        }
        // The stored output speaks for the whole instance, not only its last job.
        output.session = view.session.clone();
        output.worktree = view.worktree.clone();
        output.branch = view.branch.clone();
        output.cost_usd = view.cost_usd;
        self.s.meta[idx].output = Some(output);
    }

    fn spend(&mut self, idx: usize, cost_usd: f64, usage: Usage) {
        let cost_usd = cost_usd.max(0.0);
        self.s.run.nodes[idx].cost_usd += cost_usd;
        self.s.run.cost_usd += cost_usd;
        self.s.run.usage.add(usage);
    }

    fn complete(&mut self, idx: usize, port: String, now: Millis, fx: &mut Vec<Effect>) {
        let node = self.s.run.nodes[idx].node.clone();
        if let Some(NodeKind::Router(router)) = self.def(&node).map(|def| &def.kind)
            && port != PORT_ELSE
            && !router.rules.iter().any(|rule| rule.id == port)
        {
            let error = format!("The router answered with a rule it does not have: “{port}”.");
            // A result that arrives after the stop was requested cannot fail the run again.
            if self.s.halt.is_none() {
                self.fail_instance(idx, error, now, fx);
            } else {
                self.log(fx, now, Some(idx), LogLevel::Warn, error);
            }
            return;
        }
        let took = match self.def(&node).map(|def| &def.kind) {
            Some(NodeKind::Router(router)) => Some(match router.rules.iter().enumerate().find(|(_, rule)| rule.id == port) {
                Some((pos, rule)) => format!("Took rule {}: {}.", pos + 1, rule.label),
                None => "No rule matched; took else.".to_string(),
            }),
            _ => None,
        };
        if let Some(text) = took {
            self.log(fx, now, Some(idx), LogLevel::Info, text);
        }
        let order = self.next_order();
        let output = self.s.meta[idx].output.clone().unwrap_or_default();
        let meta = &mut self.s.meta[idx];
        meta.order = order;
        meta.hold = None;
        meta.answer = None;
        meta.resume = false;
        let view = &mut self.s.run.nodes[idx];
        view.status = NodeStatus::Done;
        view.port = Some(port);
        view.started.get_or_insert(now);
        view.finished = Some(now);
        view.summary = output.summary.clone();
        if output.pr_url.is_some() {
            self.s.run.pr_url = output.pr_url;
        }
    }

    /// Marks instance `idx` failed. Inside a variant scope with other variants alive only the
    /// variant is lost; otherwise the run fails.
    pub(super) fn fail_instance(&mut self, idx: usize, error: String, now: Millis, fx: &mut Vec<Effect>) {
        let view = &mut self.s.run.nodes[idx];
        view.status = NodeStatus::Failed;
        view.finished = Some(now);
        view.summary = error.clone();
        let (name, variant, node) = (view.name.clone(), view.variant, view.node.clone());
        self.log(fx, now, Some(idx), LogLevel::Error, format!("Failed: {error}"));

        let lanes = self.lanes();
        let opener = lanes.get(&node).and_then(|lane| lane.opener.clone()).filter(|_| variant != 0);
        let Some(opener) = opener else {
            self.halt_run(Halt::Failed, Some(format!("{name} failed: {error}")), now, fx);
            return;
        };
        let failed = |variant: u32| {
            (0..self.s.meta.len()).any(|at| {
                let view = &self.s.run.nodes[at];
                self.is_current(at)
                    && view.variant == variant
                    && view.status == NodeStatus::Failed
                    && lanes.get(&view.node).is_some_and(|lane| lane.opener.as_ref() == Some(&opener))
            })
        };
        let alive = lanes.get(&opener).is_some_and(|lane| lane.variants.iter().any(|&variant| !failed(variant)));
        if alive {
            let letter = model::variant_letter(variant);
            self.log(fx, now, Some(idx), LogLevel::Warn, format!("Variant {letter} dropped out; the others go on."));
            return;
        }
        let opener_name = self.def(&opener).map_or_else(|| name.clone(), |def| def.name.clone());
        let error = format!("Every variant of {opener_name} failed. The last error, from {name}: {error}");
        self.halt_run(Halt::Failed, Some(error), now, fx);
    }

    /// When the run's cap is reached: running jobs are cancelled and wait for a raise.
    pub(super) fn check_budget(&mut self, now: Millis, fx: &mut Vec<Effect>) {
        if self.s.halt.is_some() || !self.over_budget() {
            return;
        }
        for idx in 0..self.s.meta.len() {
            if !self.is_current(idx) || self.s.run.nodes[idx].status != NodeStatus::Running {
                continue;
            }
            if self.s.meta[idx].in_flight {
                fx.push(Effect::CancelJob(self.key_of(idx)));
            }
            self.s.meta[idx].hold = Some(Hold::Budget);
            self.s.run.nodes[idx].status = NodeStatus::Waiting;
        }
        if !self.s.budget_noted {
            self.s.budget_noted = true;
            let cap = self.s.budget_usd.unwrap_or_default();
            let text = format!("The run's budget of {} is used up; waiting for it to be raised.", base::fmt::cost(cap));
            self.log(fx, now, None, LogLevel::Warn, text);
        }
    }

    /// A note from the job in flight of instance `idx` (log lines are handled by the caller).
    pub(super) fn take_note(&mut self, idx: usize, note: JobNote, now: Millis, fx: &mut Vec<Effect>) {
        match note {
            JobNote::Log { level, text } => self.log(fx, now, Some(idx), level, text),
            JobNote::Place { session, worktree, branch } => {
                let view = &mut self.s.run.nodes[idx];
                if session.is_some() {
                    view.session = session;
                }
                if worktree.is_some() {
                    view.worktree = worktree;
                }
                if branch.is_some() {
                    view.branch = branch;
                }
            }
            JobNote::Spent { cost_usd, usage } => {
                let cost_usd = cost_usd.max(0.0);
                let meta = &mut self.s.meta[idx];
                meta.job_cost += cost_usd;
                meta.job_usage.add(usage);
                self.spend(idx, cost_usd, usage);
                self.check_budget(now, fx);
            }
            JobNote::Spawned { id, name, session, state, summary } => {
                let parent = self.key_of(idx);
                let found = (0..self.s.meta.len())
                    .find(|&at| self.s.meta[at].spawn.as_ref().is_some_and(|link| link.id == id && link.parent == parent));
                let at = found.unwrap_or_else(|| {
                    self.s.run.nodes.push(NodeInstance { question: None,
                        node: id.as_str().into(),
                        name: name.clone(),
                        variant: parent.variant,
                        attempt: parent.attempt,
                        status: NodeStatus::Running,
                        kind: SpanKind::Spawned,
                        port: None,
                        session: None,
                        worktree: None,
                        branch: None,
                        started: Some(now),
                        finished: None,
                        cost_usd: 0.0,
                        summary: String::new(),
                        spawned_by: Some(parent.node.clone()),
                    });
                    self.s.meta.push(Meta { spawn: Some(SpawnLink { id, parent }), ..Meta::default() });
                    self.s.run.nodes.len() - 1
                });
                let view = &mut self.s.run.nodes[at];
                view.name = name;
                if session.is_some() {
                    view.session = session;
                }
                if !summary.is_empty() {
                    view.summary = summary;
                }
                (view.status, view.finished) = match state {
                    SpawnedState::Running => (NodeStatus::Running, None),
                    SpawnedState::Done => (NodeStatus::Done, Some(now)),
                    SpawnedState::Failed => (NodeStatus::Failed, Some(now)),
                };
            }
        }
    }

    /// Sub-agents cannot outlive the job that created them: when it ends, those it left running
    /// are done with it, or stopped with it.
    fn close_spawned(&mut self, idx: usize, finished: bool, now: Millis) {
        let parent = self.key_of(idx);
        for at in 0..self.s.meta.len() {
            let child = self.s.meta[at].spawn.as_ref().is_some_and(|link| link.parent == parent);
            let view = &mut self.s.run.nodes[at];
            if child && view.status == NodeStatus::Running {
                view.status = if finished { NodeStatus::Done } else { NodeStatus::Cancelled };
                view.finished = Some(now);
            }
        }
    }
}
