//! "Send back": a review starts a new attempt of an upstream node.
//!
//! Everything from the target on is replaced: its instances become history, in-flight jobs
//! there are cancelled, and the item flows again from the target with the next attempt number.
//!
//! - Target is the scope's opener, or a node before the scope: the opener gets the new
//!   `variants` count and each new variant builds on what `StartFrom` names.
//! - Target is inside the scope: the scope stays as it is. `All` runs the target again in every
//!   variant, `Variant` keeps only that variant and drops the others, `Over` cannot keep the
//!   scope and restarts from its opener instead.

use std::collections::{BTreeMap, BTreeSet};

use model::graph::PORT_OUT;
use model::{LogLevel, Millis, NodeId, NodeKind, NodeStatus, ReviewNote, ReviewState, StartFrom};

use super::plan::{MAX_VARIANTS, variant_set};
use super::review::Place;
use super::state::PlannedRetry;
use super::{Effect, ExecError, Machine};
use crate::api::Retry;

impl Machine {
    /// Handles the "Send back" decision of the open review `pos`, whose instance is `idx`.
    /// Fails without changing anything when the decision cannot be carried out.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_back(
        &mut self,
        pos: usize,
        idx: usize,
        note: String,
        start: StartFrom,
        count: u32,
        now: Millis,
        fx: &mut Vec<Effect>,
    ) -> Result<(), ExecError> {
        let round = self.s.reviews[pos].clone();
        let review_node = round.review.node.clone();
        let send_back_to = match self.def(&review_node).map(|def| &def.kind) {
            Some(NodeKind::Review(review)) => review.send_back_to.clone(),
            _ => None,
        };
        let max = self.graph.limits.max_loops;
        let loops = self
            .s
            .reviews
            .iter()
            .filter(|round| round.review.node == review_node && round.review.state == ReviewState::SentBack)
            .count();
        if loops >= max as usize {
            return Err(ExecError::LoopLimit(max));
        }
        if count > MAX_VARIANTS {
            return Err(ExecError::TooManyVariants(MAX_VARIANTS));
        }
        let mut target = send_back_to.or_else(|| round.agent.clone()).ok_or(ExecError::NoSendBackTarget)?;
        if !self.plan.order.contains(&target) || !self.plan.reaches(&target, &review_node) {
            return Err(ExecError::NoSendBackTarget);
        }
        let keep = match start {
            StartFrom::Variant { index } => Some(round.lane_of(index).ok_or(ExecError::UnknownVariant(index))?),
            _ => None,
        };
        let lanes = self.lanes();
        let variants_of = |node: &NodeId| lanes.get(node).map(|lane| lane.variants.clone()).unwrap_or_else(|| vec![0]);
        // The scope the target sits in without having opened it.
        let mut inside = lanes.get(&target).and_then(|lane| lane.opener.clone()).filter(|opener| opener != &target);
        if start == StartFrom::Over
            && let Some(opener) = inside.take()
        {
            target = opener;
        }

        let mut region = self.plan.descendants(&target);
        region.insert(target.clone());
        // The node whose variants the new round counts, unless the scope stays as it is.
        let fan = match (&inside, &round.opener) {
            (Some(_), _) => None,
            (None, Some(opener)) => Some(opener.clone()).filter(|opener| region.contains(opener)),
            (None, None) if self.is_agent_node(&target) => Some(target.clone()),
            (None, None) => round.agent.clone().filter(|agent| region.contains(agent) && variants_of(agent) == [0]),
        };
        if count >= 2
            && let Some(fan) = &fan
            && self.plan.scope_nodes(fan).iter().any(|node| node != fan && self.asks_for_variants(node))
        {
            return Err(ExecError::NestedVariants);
        }

        // Nothing fails from here on.
        let mut notes: Vec<String> = self.feedback(&review_node).into_iter().map(|note| note.text).collect();
        notes.push(note.clone());

        let mut plans: Vec<PlannedRetry> = Vec::new();
        let mut dropped: Vec<usize> = Vec::new();
        match inside {
            Some(opener) => {
                let variants = keep.map_or_else(|| variants_of(&target), |lane| vec![lane]);
                for variant in variants {
                    plans.push(self.plan_retry(&target, variant, &notes, start, self.current(&target, variant)));
                }
                if let Some(lane) = keep {
                    dropped = (0..self.s.meta.len())
                        .filter(|&at| {
                            let view = &self.s.run.nodes[at];
                            self.is_current(at)
                                && view.variant != lane
                                && lanes.get(&view.node).is_some_and(|l| l.opener.as_ref() == Some(&opener))
                        })
                        .collect();
                    self.s.scopes.insert(opener, vec![lane]);
                }
            }
            None => {
                if let Some(fan) = &fan {
                    let old: BTreeMap<u32, usize> =
                        variants_of(fan).into_iter().filter_map(|variant| Some((variant, self.current(fan, variant)?))).collect();
                    let new = if count == 0 { variants_of(fan) } else { variant_set(count) };
                    for &variant in &new {
                        let from = match start {
                            StartFrom::Over => None,
                            StartFrom::Variant { .. } => keep.and_then(|lane| old.get(&lane)),
                            StartFrom::All => old.get(&variant).or_else(|| {
                                // One old variant feeds every new one; so does the first when
                                // the round shrinks to a single lane.
                                (variant == 0 || old.len() == 1).then(|| old.values().next()).flatten()
                            }),
                        };
                        plans.push(self.plan_retry(fan, variant, &notes, start, from.copied()));
                    }
                    self.s.scopes.insert(fan.clone(), new);
                }
                if fan.as_ref() != Some(&target) {
                    for variant in variants_of(&target) {
                        plans.push(self.plan_retry(&target, variant, &notes, start, self.current(&target, variant)));
                    }
                }
            }
        }

        // The review itself is decided.
        let order = self.next_order();
        self.s.meta[idx].order = order;
        let view = &mut self.s.run.nodes[idx];
        view.status = NodeStatus::Done;
        view.port = Some(PORT_OUT.to_string());
        view.finished = Some(now);
        view.summary = format!("Sent back: {}", base::fmt::first_line(&note));

        // What is replaced, and which places the new attempt or the rest of the path stands on.
        let stale: Vec<usize> = (0..self.s.meta.len())
            .filter(|&at| self.is_current(at) && (region.contains(&self.s.run.nodes[at].node) || dropped.contains(&at)))
            .collect();
        let mut keep_places: BTreeSet<Place> = plans
            .iter()
            .map(|plan| (plan.retry.from_worktree.clone(), plan.retry.from_branch.clone()))
            .filter(|place| place.0.is_some() || place.1.is_some())
            .collect();
        keep_places.extend((0..self.s.meta.len()).filter(|at| self.is_current(*at) && !stale.contains(at)).filter_map(|at| self.place(at)));
        self.discard(&stale, &keep_places, fx);

        let mut stale_keys = Vec::new();
        for &at in &stale {
            stale_keys.push(self.key_of(at));
            if self.s.meta[at].in_flight {
                fx.push(Effect::CancelJob(self.key_of(at)));
            }
            let status = self.s.run.nodes[at].status;
            if matches!(status, NodeStatus::Running | NodeStatus::Pending | NodeStatus::Waiting) {
                if status == NodeStatus::Waiting {
                    // Another review that the new attempt will reach again.
                    self.close_review_of(at, ReviewState::Skipped, fx);
                }
                let view = &mut self.s.run.nodes[at];
                view.status = NodeStatus::Cancelled;
                view.finished = Some(now);
                view.question = None;
            }
            self.s.meta[at].superseded = true;
        }
        for at in 0..self.s.meta.len() {
            let orphan = self.s.meta[at].spawn.as_ref().is_some_and(|link| stale_keys.contains(&link.parent));
            let view = &mut self.s.run.nodes[at];
            if orphan && view.status == NodeStatus::Running {
                view.status = NodeStatus::Cancelled;
                view.finished = Some(now);
            }
        }
        for node in &region {
            let attempt = self.attempt_of(node);
            let used = (0..self.s.meta.len()).any(|at| {
                let view = &self.s.run.nodes[at];
                self.s.meta[at].spawn.is_none() && &view.node == node && view.attempt == attempt
            });
            if used {
                self.s.attempts.insert(node.clone(), attempt + 1);
            }
        }
        self.s.retries.retain(|plan| !region.contains(&plan.node));
        self.s.retries.extend(plans);

        let target_name = self.def(&target).map(|def| def.name.clone()).unwrap_or_default();
        let round = &mut self.s.reviews[pos];
        round.review.state = ReviewState::SentBack;
        round.note = Some(ReviewNote { at: now, text: note.clone() });
        fx.push(Effect::ReviewClosed(round.review.clone()));
        self.log(fx, now, Some(idx), LogLevel::Info, format!("Sent back to {target_name}: {note}"));
        Ok(())
    }

    /// Whether `node` would open a variant scope of its own when the item reaches it.
    fn asks_for_variants(&self, node: &NodeId) -> bool {
        match self.def(node).map(|def| &def.kind) {
            Some(NodeKind::Agent(agent) | NodeKind::Orchestrator(agent)) => {
                self.s.scopes.get(node).map_or(agent.variants >= 2, |variants| variants.as_slice() != [0])
            }
            _ => false,
        }
    }

    /// How variant `variant` of `node` begins its next attempt, building on instance `from`.
    fn plan_retry(&self, node: &NodeId, variant: u32, notes: &[String], start: StartFrom, from: Option<usize>) -> PlannedRetry {
        let from = from.filter(|_| start != StartFrom::Over);
        let place = from.and_then(|idx| self.place(idx));
        let retry = Retry {
            notes: notes.to_vec(),
            start,
            from_branch: place.as_ref().and_then(|place| place.1.clone()),
            from_worktree: place.and_then(|place| place.0),
            from_session: from.and_then(|idx| self.s.run.nodes[idx].session.clone()),
        };
        PlannedRetry { node: node.clone(), variant, retry }
    }
}
