//! Reviews: what a person is shown, and what Ship and Skip do. Send-back lives in `sendback`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use model::graph::PORT_APPROVED;
use model::{
    LogLevel, Millis, NodeDef, NodeId, NodeKind, NodeStatus, Render, Review, ReviewDecision, ReviewId, ReviewNode,
    ReviewNote, ReviewState, Variant, variant_letter,
};

use super::flow::Lanes;
use super::state::ReviewRound;
use super::{Effect, ExecError, Machine, PORT_SKIPPED};
use crate::api::NodeOutput;

/// Longest variant name taken from an agent's summary.
const NAME_CHARS: usize = 60;

/// A worktree and a branch: what `discard` removes.
pub(super) type Place = (Option<PathBuf>, Option<String>);

impl Machine {
    /// Every variant reached review node `def`: show them to a person, or pass straight
    /// through when there is nothing to look at.
    pub(super) fn open_review(
        &mut self,
        def: &NodeDef,
        review: &ReviewNode,
        arrived: &[u32],
        lanes: &Lanes,
        now: Millis,
        fx: &mut Vec<Effect>,
    ) {
        let scope = self.review_scope(&def.id, lanes);
        let paths: Vec<Vec<usize>> = arrived.iter().map(|&variant| self.upstream(&def.id, variant, lanes)).collect();
        let first = paths.first().map(Vec::as_slice).unwrap_or_default();
        let agent = self.reviewed_agent(review, scope.opener.as_ref(), first);

        let mut variants = Vec::new();
        let mut renders: Vec<Render> = Vec::new();
        for (path, &lane) in paths.iter().zip(arrived) {
            let span = self.span(path, agent.as_ref(), lane);
            let index = if scope.opener.is_some() { lane } else { 1 };
            variants.push(self.variant(index, &span, agent.as_ref()));
            renders.extend(span.iter().filter_map(|&idx| self.s.meta[idx].output.as_ref()).flat_map(|out| out.renders.clone()));
        }

        let idx = self.push_instance(def, 0, NodeStatus::Waiting);
        self.s.run.nodes[idx].started = Some(now);
        let attempt = self.s.run.nodes[idx].attempt;
        let nothing = renders.is_empty() && variants.iter().all(|variant| variant.files.is_empty());
        let round = ReviewRound {
            review: Review {
                id: ReviewId::new(format!("{}-{}-{}", self.s.run.id, def.id, attempt)),
                run: self.s.run.id.clone(),
                node: def.id.clone(),
                issue: self.s.run.issue.clone(),
                title: self.s.run.title.clone(),
                state: ReviewState::Open,
                created: now,
                provenance: self.provenance(first, scope.opener.as_ref(), lanes),
                agent: agent.as_ref().and_then(|id| self.def(id)).map(|def| def.name.clone()).unwrap_or_default(),
                variants,
                renders,
                feedback: self.feedback(&def.id),
                picked: None,
            },
            attempt,
            lanes: arrived.to_vec(),
            agent,
            opener: scope.opener,
            note: None,
        };
        if review.skip_if_empty && nothing {
            let (lane, index) = (round.lanes[0], round.review.variants[0].index);
            self.log(fx, now, Some(idx), LogLevel::Info, "Nothing to review; passed through.".to_string());
            self.ship(idx, &round, lane, index, now, fx);
            self.s.run.nodes[idx].summary = "Nothing to review.".to_string();
            return;
        }
        let count = round.review.variants.len() as u64;
        self.log(fx, now, Some(idx), LogLevel::Info, format!("Waiting for you: {}.", base::fmt::plural(count, "variant", "variants")));
        fx.push(Effect::ReviewOpened(round.review.clone()));
        // The same review can open again after the run was stopped and resumed.
        self.s.reviews.retain(|old| old.review.id != round.review.id);
        self.s.reviews.push(round);
    }

    /// The node whose work a review shows: the opener of the scope it closes, else the node it
    /// sends back to, else the last agent before it.
    fn reviewed_agent(&self, review: &ReviewNode, opener: Option<&NodeId>, path: &[usize]) -> Option<NodeId> {
        let on_path = |id: &NodeId| path.iter().any(|&idx| &self.s.run.nodes[idx].node == id);
        if let Some(opener) = opener {
            return Some(opener.clone());
        }
        if let Some(target) = review.send_back_to.as_ref().filter(|id| self.is_agent_node(id) && on_path(id)) {
            return Some(target.clone());
        }
        path.iter().rev().map(|&idx| &self.s.run.nodes[idx].node).find(|id| self.is_agent_node(id)).cloned()
    }

    /// The part of a variant's path that is its own work: the reviewed agent and what ran
    /// after it in the same lane.
    fn span(&self, path: &[usize], agent: Option<&NodeId>, lane: u32) -> Vec<usize> {
        let from = agent.and_then(|agent| path.iter().rposition(|&idx| &self.s.run.nodes[idx].node == agent)).unwrap_or(0);
        path[from..]
            .iter()
            .enumerate()
            .filter(|&(pos, &idx)| pos == 0 || self.s.run.nodes[idx].variant == lane)
            .map(|(_, &idx)| idx)
            .collect()
    }

    fn variant(&self, index: u32, span: &[usize], agent: Option<&NodeId>) -> Variant {
        let outputs = || span.iter().filter_map(|&idx| self.s.meta[idx].output.as_ref());
        let lead = span.first().copied().filter(|&idx| Some(&self.s.run.nodes[idx].node) == agent);
        let lead_out = lead.and_then(|idx| self.s.meta[idx].output.as_ref());
        let approach = lead_out.map(|out| out.summary.clone()).unwrap_or_default();
        let headline = base::fmt::first_line(&approach);
        let headline = headline.split_once(". ").map_or(headline, |(first, _)| first).trim_end_matches('.');
        let name = if headline.is_empty() {
            format!("Variant {}", variant_letter(index))
        } else {
            base::fmt::truncate_chars(headline, NAME_CHARS)
        };
        let files = outputs().rev().map(|out| &out.files).find(|files| !files.is_empty()).cloned().unwrap_or_default();
        let tested = outputs().rev().find(|out| out.tests.is_some() || out.tests_ok.is_some());
        let tests = tested.and_then(|out| out.tests);
        let secs = span
            .iter()
            .map(|&idx| &self.s.run.nodes[idx])
            .filter_map(|view| Some((view.finished? - view.started?).max(0) as u64))
            .sum::<u64>()
            / 1000;
        Variant {
            index,
            name,
            approach,
            branch: lead_out.and_then(|out| out.branch.clone()).or_else(|| outputs().rev().find_map(|out| out.branch.clone())),
            worktree: lead_out.and_then(|out| out.worktree.clone()).or_else(|| outputs().rev().find_map(|out| out.worktree.clone())),
            session: lead_out.and_then(|out| out.session.clone()).or_else(|| outputs().rev().find_map(|out| out.session.clone())),
            added: files.iter().map(|file| file.added).sum(),
            removed: files.iter().map(|file| file.removed).sum(),
            files,
            tests,
            tests_ok: tested.and_then(|out| out.tests_ok).or_else(|| tests.map(|tests| tests.failed() == 0)),
            secs,
            cost_usd: span.iter().map(|&idx| self.s.run.nodes[idx].cost_usd).sum(),
        }
    }

    /// The breadcrumb of a review: sources, routers with the rule they took, agents, then "You".
    fn provenance(&self, path: &[usize], opener: Option<&NodeId>, lanes: &Lanes) -> Vec<String> {
        let mut crumbs = Vec::new();
        let mut seen: BTreeSet<&NodeId> = BTreeSet::new();
        for &idx in path {
            let view = &self.s.run.nodes[idx];
            let Some(def) = self.def(&view.node) else { continue };
            if !seen.insert(&view.node) {
                continue;
            }
            match &def.kind {
                NodeKind::Router(router) => {
                    let port = view.port.as_deref().unwrap_or_default();
                    let rule = router.rules.iter().position(|rule| rule.id == port);
                    crumbs.push(match rule {
                        Some(pos) => format!("{} · rule {}", view.name, pos + 1),
                        None => format!("{} · else", view.name),
                    });
                }
                NodeKind::Agent(_) | NodeKind::Orchestrator(_) if opener == Some(&view.node) => {
                    let count = lanes.get(&view.node).map_or(1, |lane| lane.variants.len());
                    crumbs.push(format!("{} ×{count}", view.name));
                }
                // Actions are plumbing; the breadcrumb names who decided and who worked.
                NodeKind::Action(_) => {}
                _ => crumbs.push(view.name.clone()),
            }
        }
        crumbs.push("You".to_string());
        crumbs
    }

    /// Notes of the earlier send-backs of a review node, oldest first.
    pub(super) fn feedback(&self, node: &NodeId) -> Vec<ReviewNote> {
        self.s.reviews.iter().filter(|round| &round.review.node == node).filter_map(|round| round.note.clone()).collect()
    }

    pub(super) fn decide_review(
        &mut self,
        id: &ReviewId,
        decision: ReviewDecision,
        now: Millis,
        fx: &mut Vec<Effect>,
    ) -> Result<(), ExecError> {
        let closed = || ExecError::ReviewNotOpen(id.clone());
        let pos = self.s.reviews.iter().position(|round| &round.review.id == id && round.review.state == ReviewState::Open);
        let pos = pos.ok_or_else(closed)?;
        let round = self.s.reviews[pos].clone();
        let idx = self.current(&round.review.node, 0).ok_or_else(closed)?;
        let view = &self.s.run.nodes[idx];
        if view.status != NodeStatus::Waiting || view.attempt != round.attempt {
            return Err(closed());
        }
        match decision {
            ReviewDecision::Ship { variant } => {
                let lane = round.lane_of(variant).ok_or(ExecError::UnknownVariant(variant))?;
                let review = &mut self.s.reviews[pos].review;
                review.state = ReviewState::Shipped;
                review.picked = Some(variant);
                fx.push(Effect::ReviewClosed(review.clone()));
                self.ship(idx, &round, lane, variant, now, fx);
                let text = format!("Shipped variant {}.", variant_letter(variant));
                self.log(fx, now, Some(idx), LogLevel::Info, text);
            }
            ReviewDecision::Skip => {
                let review = &mut self.s.reviews[pos].review;
                review.state = ReviewState::Skipped;
                fx.push(Effect::ReviewClosed(review.clone()));
                let order = self.next_order();
                self.s.meta[idx].order = order;
                let view = &mut self.s.run.nodes[idx];
                view.status = NodeStatus::Done;
                view.port = Some(PORT_SKIPPED.to_string());
                view.finished = Some(now);
                view.summary = "Skipped.".to_string();
                self.log(fx, now, Some(idx), LogLevel::Info, "Skipped; nothing after the review runs.".to_string());
            }
            ReviewDecision::SendBack { note, start, variants } => self.send_back(pos, idx, note, start, variants, now, fx)?,
        }
        Ok(())
    }

    /// The review instance `idx` lets lane `lane` through; everything else that was made for
    /// this review is thrown away.
    fn ship(&mut self, idx: usize, round: &ReviewRound, lane: u32, index: u32, now: Millis, fx: &mut Vec<Effect>) {
        let lanes = self.lanes();
        let node = round.review.node.clone();
        let path = self.upstream(&node, lane, &lanes);
        let span = self.span(&path, round.agent.as_ref(), lane);
        let picked = round.review.variants.iter().find(|variant| variant.index == index);
        let output = NodeOutput {
            summary: format!("Shipped variant {}.", variant_letter(index)),
            session: picked.and_then(|variant| variant.session.clone()),
            worktree: picked.and_then(|variant| variant.worktree.clone()),
            branch: picked.and_then(|variant| variant.branch.clone()),
            files: picked.map(|variant| variant.files.clone()).unwrap_or_default(),
            ..NodeOutput::default()
        };
        let order = self.next_order();
        let meta = &mut self.s.meta[idx];
        meta.order = order;
        meta.picked = Some(lane);
        let view = &mut self.s.run.nodes[idx];
        view.status = NodeStatus::Done;
        view.port = Some(PORT_APPROVED.to_string());
        view.finished = Some(now);
        view.summary = output.summary.clone();
        self.s.meta[idx].output = Some(output);

        // Everything between the reviewed agent and the review, in any round and variant,
        // except what the picked variant stands on.
        let Some(agent) = round.agent.clone() else { return };
        let keep: BTreeSet<Place> = span.iter().filter_map(|&at| self.place(at)).collect();
        let between = |id: &NodeId| id == &agent || (self.plan.reaches(&agent, id) && self.plan.reaches(id, &node));
        let candidates: Vec<usize> = (0..self.s.meta.len())
            .filter(|&at| self.s.meta[at].spawn.is_none() && between(&self.s.run.nodes[at].node))
            .collect();
        self.discard(&candidates, &keep, fx);
    }

    /// The worktree and branch an instance worked in, when it has either.
    pub(super) fn place(&self, idx: usize) -> Option<Place> {
        let view = &self.s.run.nodes[idx];
        let place = match &self.s.meta[idx].output {
            Some(out) => (out.worktree.clone().or_else(|| view.worktree.clone()), out.branch.clone().or_else(|| view.branch.clone())),
            None => (view.worktree.clone(), view.branch.clone()),
        };
        (place.0.is_some() || place.1.is_some()).then_some(place)
    }

    /// Hands the places of `candidates` to `discard`, once each. A place that shares its
    /// worktree or branch with one in `keep` stays. A job that is still winding down is
    /// discarded when it returns.
    pub(super) fn discard(&mut self, candidates: &[usize], keep: &BTreeSet<Place>, fx: &mut Vec<Effect>) {
        let kept = |place: &Place| {
            keep.iter().any(|other| (place.0.is_some() && place.0 == other.0) || (place.1.is_some() && place.1 == other.1))
        };
        let mut leaving: Vec<(usize, Place)> = Vec::new();
        for &idx in candidates {
            if self.s.meta[idx].discarded {
                continue;
            }
            let Some(place) = self.place(idx).filter(|place| !kept(place)) else { continue };
            if self.s.meta[idx].in_flight {
                self.s.meta[idx].discard_late = true;
                continue;
            }
            self.s.meta[idx].discarded = true;
            leaving.push((idx, place));
        }
        let mut gone: BTreeSet<&Place> = BTreeSet::new();
        for (idx, place) in &leaving {
            // An action reports the worktree it ran in; the agent's output names the branch too.
            let covered = place.1.is_none() && leaving.iter().any(|(_, other)| other.0 == place.0 && other.1.is_some());
            if covered || !gone.insert(place) {
                continue;
            }
            let mut output = self.s.meta[*idx].output.clone().unwrap_or_default();
            output.worktree = place.0.clone();
            output.branch = place.1.clone();
            output.session = output.session.or_else(|| self.s.run.nodes[*idx].session.clone());
            fx.push(Effect::Discard(Box::new(output)));
        }
    }
}
