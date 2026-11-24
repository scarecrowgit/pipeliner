//! [`InboxApi`]: the Inbox from the store, reviews decided through their run's driver.

use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use model::{
    Event, FileDiff, GraphDef, InboxId, InboxItem, NodeKind, PullRequest, Review, ReviewDecision, ReviewId, StartFrom,
};

use crate::api::{Estimate, InboxApi};
use crate::engine::Engine;
use crate::pipelines;

impl Engine {
    fn inbox_changed(&self, id: &InboxId) -> Result<()> {
        let item: InboxItem = self.inner.store.inbox_item(id)?.ok_or_else(|| anyhow!("There is no Inbox item {id}"))?;
        self.inner.publish(Event::InboxChanged { item });
        Ok(())
    }

    fn review_or_fail(&self, id: &ReviewId) -> Result<Review> {
        self.inner.store.review(id)?.ok_or_else(|| anyhow!("There is no review {id}"))
    }

    /// The base branch the reviewed agent's workdir measures changes against.
    fn review_base(&self, review: &Review) -> String {
        let inner = &self.inner;
        let graph: Option<GraphDef> = pipelines::run_graph(inner, &review.run);
        let snapshot = pipelines::run_snapshot(inner, &review.run);
        let agent = snapshot.and_then(|s| s.reviews.into_iter().find(|r| r.review.id == review.id)).and_then(|r| r.agent);
        let base = graph.as_ref().and_then(|graph| {
            let node = graph.node(agent.as_ref()?)?;
            let workdir = match &node.kind {
                NodeKind::Agent(a) | NodeKind::Orchestrator(a) => &a.workdir,
                _ => return None,
            };
            Some(graph.workdir(workdir)?.base_branch.clone())
        });
        base.filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "main".to_string())
    }
}

impl Engine {
    /// Whether an agent or orchestrator follows the review's node along route edges.
    fn agents_after(&self, review: &Review) -> bool {
        let Some(graph) = pipelines::run_graph(&self.inner, &review.run) else {
            return true;
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = vec![review.node.clone()];
        while let Some(node) = stack.pop() {
            for edge in graph.edges_from(&node).filter(|e| e.kind == model::EdgeKind::Route) {
                if seen.insert(edge.to.clone()) {
                    if graph.node(&edge.to).is_some_and(|d| matches!(d.kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))) {
                        return true;
                    }
                    stack.push(edge.to.clone());
                }
            }
        }
        false
    }
}

impl InboxApi for Engine {
    fn inbox(&self, archived: bool) -> Result<Vec<InboxItem>> {
        self.inner.store.inbox(archived)
    }

    fn set_inbox_read(&self, id: &InboxId, read: bool) -> Result<()> {
        if !self.inner.store.set_inbox_read(id, read)? {
            bail!("There is no Inbox item {id}");
        }
        self.inbox_changed(id)
    }

    fn archive_inbox(&self, id: &InboxId, archived: bool) -> Result<()> {
        if !self.inner.store.set_inbox_archived(id, archived)? {
            bail!("There is no Inbox item {id}");
        }
        self.inbox_changed(id)
    }

    fn open_reviews(&self) -> Result<Vec<Review>> {
        self.inner.store.open_reviews()
    }

    fn review(&self, id: &ReviewId) -> Result<Option<Review>> {
        self.inner.store.review(id)
    }

    fn variant_diff(&self, id: &ReviewId, variant: u32, path: &str) -> Result<FileDiff> {
        let review = self.review_or_fail(id)?;
        let found = review.variants.iter().find(|v| v.index == variant).ok_or_else(|| anyhow!("The review has no variant {variant}"))?;
        let worktree = found.worktree.as_ref().ok_or_else(|| anyhow!("Variant {variant} has no worktree to diff"))?;
        if !worktree.is_dir() {
            bail!("The worktree of variant {variant} is gone ({})", worktree.display());
        }
        vcs::file_diff(worktree, &self.review_base(&review), path, &CancelToken::new())
    }

    /// From the variants' own time and cost: the one kept, or their mean for each variant asked for.
    fn send_back_estimate(&self, id: &ReviewId, start: StartFrom, variants: u32) -> Result<Estimate> {
        let review = self.review_or_fail(id)?;
        let pool: Vec<_> = match start {
            StartFrom::Variant { index } => review.variants.iter().filter(|v| v.index == index).collect(),
            _ => review.variants.iter().collect(),
        };
        let n = pool.len().max(1) as f64;
        let secs = pool.iter().map(|v| v.secs as f64).sum::<f64>() / n;
        let cost = pool.iter().map(|v| v.cost_usd).sum::<f64>() / n;
        let count = if variants == 0 { review.variants.len().max(1) as u32 } else { variants };
        Ok(Estimate { secs: secs.round() as u64, cost_usd: cost * f64::from(count) })
    }

    fn decide_review(&self, id: &ReviewId, decision: ReviewDecision) -> Result<()> {
        let review = self.review_or_fail(id)?;
        if review.state != model::ReviewState::Open {
            bail!("The review was already decided");
        }
        // Sending back starts agent work, and so does shipping into agents after the review.
        let starts_agents = match decision {
            ReviewDecision::SendBack { .. } => true,
            ReviewDecision::Ship { .. } => self.agents_after(&review),
            ReviewDecision::Skip => false,
        };
        if starts_agents {
            pipelines::check_limit(&self.inner)?;
        }
        pipelines::command(&self.inner, &review.run, |h| h.decide(id.clone(), decision.clone()))
    }

    fn pull_request(&self, url: &str) -> Result<PullRequest> {
        vcs::pr::view_with(&*self.inner.gh, url, &CancelToken::new())
    }
}
