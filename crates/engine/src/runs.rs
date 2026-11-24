//! [`RunApi`] and [`InboxApi`]: what the store holds about runs, reviews and the Inbox is
//! read and flagged here. Starting, cancelling and deciding runs waits for the pipeline
//! executor and fails with [`NOT_WIRED`].

use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use model::{
    Event, FileDiff, InboxId, InboxItem, IssueKey, NodeStatus, PipelineId, PullRequest, Review, ReviewDecision, ReviewId,
    Run, RunId, RunLogLine, RunStats, StartFrom,
};

use crate::api::{Estimate, InboxApi, RunApi, RunInput};
use crate::engine::{Engine, NOT_WIRED};

impl RunApi for Engine {
    fn run(&self, id: &RunId) -> Result<Option<Run>> {
        self.inner.store.run(id)
    }

    fn latest_runs(&self) -> Result<Vec<Run>> {
        self.inner.store.latest_runs()
    }

    fn runs_for_issue(&self, key: &IssueKey) -> Result<Vec<Run>> {
        self.inner.store.runs_for_issue(key)
    }

    fn run_log(&self, id: &RunId, after_seq: u64, limit: usize) -> Result<Vec<RunLogLine>> {
        self.inner.store.run_log_after(id, after_seq, limit)
    }

    fn run_log_tail(&self, id: &RunId, count: usize) -> Result<Vec<RunLogLine>> {
        self.inner.store.run_log_tail(id, count)
    }

    fn run_stats(&self, id: &RunId) -> Result<RunStats> {
        let run = self.inner.store.run(id)?.ok_or_else(|| anyhow!("There is no run {id}"))?;
        let reviews = self.inner.store.reviews_for_run(id)?;
        Ok(RunStats {
            agents_running: run.nodes.iter().filter(|n| n.status == NodeStatus::Running).count() as u32,
            agents_max: self.inner.store.settings()?.slots,
            spawned: run.nodes.iter().filter(|n| n.spawned_by.is_some()).count() as u32,
            waiting_variants: reviews.iter().filter(|r| r.picked.is_none()).map(|r| r.variants.len() as u32).sum(),
            cost_usd: self.inner.store.spend_for_run(id)?.cost_usd,
            budget_usd: None,
        })
    }

    fn start_run(&self, _pipeline: &PipelineId, _input: RunInput) -> Result<Run> {
        bail!("{NOT_WIRED}")
    }

    fn test_run(&self, _pipeline: &PipelineId, _input: RunInput) -> Result<Run> {
        bail!("{NOT_WIRED}")
    }

    fn cancel_run(&self, _id: &RunId) -> Result<()> {
        bail!("{NOT_WIRED}")
    }

    fn resume_run(&self, _id: &RunId) -> Result<()> {
        bail!("{NOT_WIRED}")
    }
}

impl Engine {
    fn inbox_changed(&self, id: &InboxId) -> Result<()> {
        let item: InboxItem = self.inner.store.inbox_item(id)?.ok_or_else(|| anyhow!("There is no Inbox item {id}"))?;
        self.inner.publish(Event::InboxChanged { item });
        Ok(())
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

    fn variant_diff(&self, _id: &ReviewId, _variant: u32, _path: &str) -> Result<FileDiff> {
        bail!("{NOT_WIRED}")
    }

    /// The mean time and cost of the review's variants, times the variants asked for.
    fn send_back_estimate(&self, id: &ReviewId, _start: StartFrom, variants: u32) -> Result<Estimate> {
        let review = self.inner.store.review(id)?.ok_or_else(|| anyhow!("There is no review {id}"))?;
        let n = review.variants.len().max(1) as f64;
        let secs = review.variants.iter().map(|v| v.secs as f64).sum::<f64>() / n;
        let cost = review.variants.iter().map(|v| v.cost_usd).sum::<f64>() / n;
        Ok(Estimate { secs: secs.round() as u64, cost_usd: cost * f64::from(variants.max(1)) })
    }

    fn decide_review(&self, _id: &ReviewId, _decision: ReviewDecision) -> Result<()> {
        bail!("{NOT_WIRED}")
    }

    fn pull_request(&self, url: &str) -> Result<PullRequest> {
        vcs::pr::view(url, &CancelToken::new())
    }
}
