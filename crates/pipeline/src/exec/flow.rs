//! How the item moves: which instances exist, which may start, and what a job is handed.

use std::collections::{BTreeMap, BTreeSet};

use model::graph::PORT_OUT;
use model::{LogLevel, Millis, NodeDef, NodeId, NodeKind, NodeStatus, SourceKind};

use super::plan::{attached, is_agent, variant_set};
use super::state::Halt;
use super::{Effect, Machine, PORT_SKIPPED};
use crate::api::{Job, Upstream};

/// Passes of [`Machine::settle`] before it gives up; a pass that changes nothing ends it.
const SETTLE_PASSES: usize = 64;

/// The variants a node runs in right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lane {
    /// `[0]` outside a variant scope.
    pub variants: Vec<u32>,
    /// The node that opened the scope, when inside one.
    pub opener: Option<NodeId>,
}

impl Lane {
    fn single() -> Lane {
        Lane { variants: vec![0], opener: None }
    }
}

pub(super) type Lanes = BTreeMap<NodeId, Lane>;

enum EdgeState {
    Pending,
    Delivered,
    Dead,
}

#[derive(PartialEq)]
enum Inputs {
    Waiting,
    Dead,
    Ready,
}

enum ReviewInputs {
    Waiting,
    Dead,
    /// The instance variants that reached the review.
    Arrived(Vec<u32>),
}

impl Machine {
    /// The lane of every reachable node under the present variant counts.
    pub(super) fn lanes(&self) -> Lanes {
        let mut lanes = Lanes::new();
        for id in &self.plan.order {
            let Some(def) = self.def(id) else { continue };
            let lane = match &def.kind {
                // A review collapses the scope: it and what follows run once.
                NodeKind::Review(_) => Lane::single(),
                kind => match self.scope_into(id, &lanes) {
                    Some(lane) => lane,
                    None => match kind {
                        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => {
                            let variants = self.s.scopes.get(id).cloned().unwrap_or_else(|| variant_set(agent.variants));
                            // An opener the item never reaches opens nothing: it is skipped
                            // once, and what follows it is not in a scope.
                            if variants == [0] || self.inputs(def, 0, &lanes) == Inputs::Dead {
                                Lane::single()
                            } else {
                                Lane { variants, opener: Some(id.clone()) }
                            }
                        }
                        _ => Lane::single(),
                    },
                },
            };
            lanes.insert(id.clone(), lane);
        }
        lanes
    }

    /// The open variant scope the item is in when it reaches `node`, if any. A scope comes in
    /// only along an edge that some variant can still take: a branch that is dead for every
    /// variant (not taken by a router, skipped) brings no scope with it.
    pub(super) fn scope_into(&self, node: &NodeId, lanes: &Lanes) -> Option<Lane> {
        self.plan.incoming(node).iter().filter(|(pred, _)| !self.is_review(pred)).find_map(|(pred, port)| {
            let lane = lanes.get(pred).filter(|lane| lane.opener.is_some())?;
            let live = lane.variants.iter().any(|&variant| !matches!(self.edge_state(pred, port, variant, lanes), EdgeState::Dead));
            live.then(|| lane.clone())
        })
    }

    /// Which variant of `pred` feeds variant `variant` of a node after it.
    fn feed_variant(&self, pred: &NodeId, variant: u32, lanes: &Lanes) -> Option<u32> {
        let variants = lanes.get(pred).map(|lane| lane.variants.as_slice()).unwrap_or(&[0]);
        if variants.contains(&variant) {
            Some(variant)
        } else if variants == [0] {
            Some(0)
        } else {
            None
        }
    }

    fn edge_state(&self, pred: &NodeId, port: &str, variant: u32, lanes: &Lanes) -> EdgeState {
        let Some(from) = self.feed_variant(pred, variant, lanes) else { return EdgeState::Dead };
        let Some(idx) = self.current(pred, from) else { return EdgeState::Pending };
        let view = &self.s.run.nodes[idx];
        match view.status {
            NodeStatus::Done if view.port.as_deref() == Some(port) => EdgeState::Delivered,
            // A failed instance that did not fail the run is a variant that dropped out.
            NodeStatus::Done | NodeStatus::Skipped | NodeStatus::Failed => EdgeState::Dead,
            _ => EdgeState::Pending,
        }
    }

    /// Whether a person skipped a review the item would have to pass to get to `node`.
    fn cut_off(&self, node: &NodeId) -> bool {
        self.plan.order.iter().any(|review| {
            review != node
                && self.is_review(review)
                && self.current(review, 0).is_some_and(|idx| {
                    let view = &self.s.run.nodes[idx];
                    view.status == NodeStatus::Done && view.port.as_deref() == Some(PORT_SKIPPED)
                })
                && self.plan.reaches(review, node)
        })
    }

    fn inputs(&self, def: &NodeDef, variant: u32, lanes: &Lanes) -> Inputs {
        if self.cut_off(&def.id) {
            return Inputs::Dead;
        }
        if matches!(def.kind, NodeKind::Source(_)) {
            return if self.entry.contains(&def.id) { Inputs::Ready } else { Inputs::Dead };
        }
        // The node that opens the scope fans out from the single lane before it.
        let opens = variant != 0 && lanes.get(&def.id).is_some_and(|lane| lane.opener.as_ref() == Some(&def.id));
        let mut delivered = false;
        for (pred, port) in self.plan.incoming(&def.id) {
            // Inside a scope a variant arrives through its own work, not through a branch
            // that every variant shares.
            let own = variant == 0 || opens || lanes.get(pred).is_some_and(|lane| lane.variants.contains(&variant));
            match self.edge_state(pred, port, variant, lanes) {
                EdgeState::Pending => return Inputs::Waiting,
                EdgeState::Delivered => delivered |= own,
                EdgeState::Dead => {}
            }
        }
        if delivered { Inputs::Ready } else { Inputs::Dead }
    }

    /// The variants a review waits for: those of the scope it closes, or the single lane.
    pub(super) fn review_scope(&self, node: &NodeId, lanes: &Lanes) -> Lane {
        self.scope_into(node, lanes).unwrap_or_else(Lane::single)
    }

    fn review_inputs(&self, node: &NodeId, lanes: &Lanes) -> ReviewInputs {
        if self.cut_off(node) {
            return ReviewInputs::Dead;
        }
        let scope = self.review_scope(node, lanes);
        let mut arrived = Vec::new();
        for &variant in &scope.variants {
            let mut delivered = false;
            for (pred, port) in self.plan.incoming(node) {
                let own = variant == 0 || lanes.get(pred).is_some_and(|lane| lane.variants.contains(&variant));
                match self.edge_state(pred, port, variant, lanes) {
                    EdgeState::Pending => return ReviewInputs::Waiting,
                    EdgeState::Delivered => delivered |= own,
                    EdgeState::Dead => {}
                }
            }
            if delivered {
                arrived.push(variant);
            }
        }
        if arrived.is_empty() { ReviewInputs::Dead } else { ReviewInputs::Arrived(arrived) }
    }

    /// Lets the item move as far as it can: creates the instances whose inputs are settled,
    /// marks dead paths, opens reviews, and starts the jobs that may run.
    pub(super) fn settle(&mut self, now: Millis, fx: &mut Vec<Effect>) {
        for _ in 0..SETTLE_PASSES {
            let lanes = self.lanes();
            let mut changed = self.heal_skipped(&lanes);
            for id in self.plan.order.clone() {
                if self.s.halt.is_some() {
                    return;
                }
                let Some(def) = self.def(&id).cloned() else { continue };
                if let NodeKind::Review(review) = &def.kind {
                    if self.current(&id, 0).is_some() {
                        continue;
                    }
                    match self.review_inputs(&id, &lanes) {
                        ReviewInputs::Waiting => {}
                        ReviewInputs::Dead => {
                            self.push_instance(&def, 0, NodeStatus::Skipped);
                            changed = true;
                        }
                        ReviewInputs::Arrived(variants) => {
                            self.open_review(&def, review, &variants, &lanes, now, fx);
                            changed = true;
                        }
                    }
                    continue;
                }
                let variants = lanes.get(&id).map(|lane| lane.variants.clone()).unwrap_or_default();
                for variant in variants {
                    if self.s.halt.is_some() {
                        return;
                    }
                    if self.current(&id, variant).is_some() {
                        continue;
                    }
                    match self.inputs(&def, variant, &lanes) {
                        Inputs::Waiting => {}
                        Inputs::Dead => {
                            self.push_instance(&def, variant, NodeStatus::Skipped);
                            changed = true;
                        }
                        Inputs::Ready => {
                            self.arrive(&def, variant, now, fx);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.start_jobs(now, fx);
    }

    /// Removes skipped placeholders whose inputs are no longer all dead (a failed variant is
    /// tried again), so that they are judged afresh.
    fn heal_skipped(&mut self, lanes: &Lanes) -> bool {
        let mut changed = false;
        loop {
            let stale = (0..self.s.meta.len()).find(|&idx| {
                let view = &self.s.run.nodes[idx];
                if !self.is_current(idx) || view.status != NodeStatus::Skipped {
                    return false;
                }
                let Some(def) = self.def(&view.node) else { return false };
                match def.kind {
                    NodeKind::Review(_) => !matches!(self.review_inputs(&def.id, lanes), ReviewInputs::Dead),
                    // Also a placeholder of a variant the node no longer runs in.
                    _ => {
                        !lanes.get(&def.id).is_some_and(|lane| lane.variants.contains(&view.variant))
                            || self.inputs(def, view.variant, lanes) != Inputs::Dead
                    }
                }
            });
            let Some(idx) = stale else { break };
            self.remove_instance(idx);
            changed = true;
        }
        changed
    }

    /// The item reached `def` in `variant`.
    fn arrive(&mut self, def: &NodeDef, variant: u32, now: Millis, fx: &mut Vec<Effect>) {
        match &def.kind {
            NodeKind::Source(source) => {
                let missing = match source.source {
                    SourceKind::Jira => Some("Jira"),
                    SourceKind::Sentry => Some("Sentry"),
                    _ => None,
                };
                if let Some(missing) = missing {
                    let error = format!("{}: the {missing} source is not available yet.", def.name);
                    self.log(fx, now, None, LogLevel::Error, error.clone());
                    self.halt_run(Halt::Failed, Some(error), now, fx);
                    return;
                }
                let idx = self.push_instance(def, variant, NodeStatus::Done);
                let order = self.next_order();
                let view = &mut self.s.run.nodes[idx];
                view.port = Some(PORT_OUT.to_string());
                view.started = Some(now);
                view.finished = Some(now);
                self.s.meta[idx].order = order;
            }
            NodeKind::Pipeline(_) => {
                let idx = self.push_instance(def, variant, NodeStatus::Pending);
                self.refuse_pipeline(idx, now, fx);
            }
            _ => {
                let idx = self.push_instance(def, variant, NodeStatus::Pending);
                let planned = self.s.retries.iter().position(|plan| plan.node == def.id && plan.variant == variant);
                if let Some(pos) = planned {
                    self.s.meta[idx].retry = Some(self.s.retries.remove(pos).retry);
                }
            }
        }
    }

    fn refuse_pipeline(&mut self, idx: usize, now: Millis, fx: &mut Vec<Effect>) {
        let error = format!("{}: nested pipelines are not available yet.", self.s.run.nodes[idx].name);
        let view = &mut self.s.run.nodes[idx];
        view.status = NodeStatus::Failed;
        view.started.get_or_insert(now);
        view.finished = Some(now);
        view.summary = "Nested pipelines are not available yet.".to_string();
        self.log(fx, now, Some(idx), LogLevel::Error, error.clone());
        self.halt_run(Halt::Failed, Some(error), now, fx);
    }

    /// Starts pending instances, oldest first, within the budget and `limits.max_parallel`.
    fn start_jobs(&mut self, now: Millis, fx: &mut Vec<Effect>) {
        if self.s.halt.is_some() || self.over_budget() {
            return;
        }
        let limit = self.graph.limits.max_parallel.max(1) as usize;
        // Cancelled jobs that have not returned still hold a worker.
        let mut agents = (0..self.s.meta.len())
            .filter(|&idx| self.s.meta[idx].in_flight && self.is_agent_node(&self.s.run.nodes[idx].node))
            .count();
        let lanes = self.lanes();
        for idx in 0..self.s.meta.len() {
            let ready = self.is_current(idx) && self.s.run.nodes[idx].status == NodeStatus::Pending && !self.s.meta[idx].in_flight;
            if !ready {
                continue;
            }
            let Some(def) = self.def(&self.s.run.nodes[idx].node).cloned() else { continue };
            if matches!(def.kind, NodeKind::Pipeline(_)) {
                self.refuse_pipeline(idx, now, fx);
                return;
            }
            let agent = is_agent(&def.kind);
            if agent && agents >= limit {
                continue;
            }
            let job = self.job(idx, &def, &lanes);
            let meta = &mut self.s.meta[idx];
            meta.in_flight = true;
            meta.job_cost = 0.0;
            meta.job_usage = Default::default();
            let view = &mut self.s.run.nodes[idx];
            view.status = NodeStatus::Running;
            view.started.get_or_insert(now);
            view.finished = None;
            // The job that starts carries the answer.
            view.question = None;
            if agent {
                agents += 1;
            }
            fx.push(Effect::Start(Box::new(job)));
        }
    }

    fn job(&self, idx: usize, def: &NodeDef, lanes: &Lanes) -> Job {
        let view = &self.s.run.nodes[idx];
        let meta = &self.s.meta[idx];
        let upstream = self
            .upstream(&view.node, view.variant, lanes)
            .into_iter()
            .filter(|&up| !matches!(self.def(&self.s.run.nodes[up].node).map(|def| &def.kind), Some(NodeKind::Source(_))))
            .map(|up| Upstream {
                key: self.key_of(up),
                name: self.s.run.nodes[up].name.clone(),
                output: self.s.meta[up].output.clone().unwrap_or_default(),
            })
            .collect();
        Job {
            run: self.s.run.id.clone(),
            key: self.key_of(idx),
            node: def.clone(),
            attached: attached(&self.graph, &def.id),
            item: self.s.item.clone(),
            upstream,
            retry: meta.retry.clone(),
            answer: meta.answer.clone(),
            resume_session: if meta.resume { view.session.clone() } else { None },
            budget_left_usd: self.budget_left(idx),
            test: self.s.run.test,
        }
    }

    /// The finished instances on the item's way to variant `variant` of `node`, in the order
    /// they finished. After a shipped review only the picked variant's instances count.
    pub(super) fn upstream(&self, node: &NodeId, variant: u32, lanes: &Lanes) -> Vec<usize> {
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut stack = vec![(node.clone(), variant)];
        while let Some((at, variant)) = stack.pop() {
            for (pred, port) in self.plan.incoming(&at) {
                let Some(from) = self.feed_variant(pred, variant, lanes) else { continue };
                let Some(idx) = self.current(pred, from) else { continue };
                let view = &self.s.run.nodes[idx];
                if view.status != NodeStatus::Done || view.port.as_deref() != Some(port.as_str()) {
                    continue;
                }
                if seen.insert(idx) {
                    stack.push((pred.clone(), self.s.meta[idx].picked.unwrap_or(from)));
                }
            }
        }
        let mut path: Vec<usize> = seen.into_iter().collect();
        path.sort_by_key(|&idx| self.s.meta[idx].order);
        path
    }
}
