//! [`RunApi`]: runs from the store, started and steered through their drivers. Answering a
//! question and raising a budget are not in the contract yet and live on [`Engine`].

use anyhow::{Result, anyhow, bail};
use model::{
    Event, GraphDef, Issue, IssueKey, NodeStatus, PipelineId, Run, RunId, RunLogLine, RunState, RunStats, SourceKind,
};
use pipeline::Item;

use crate::api::{CatalogApi, RunApi, RunInput};
use crate::engine::{Engine, Inner};
use crate::pipelines;

/// A new run's shell: what the executor fills in.
fn shell(inner: &Inner, pipeline: &PipelineId, version: u32, title: &str, input: &str, test: bool) -> Run {
    Run {
        id: RunId::new(base::id::new_id()),
        pipeline: pipeline.clone(),
        version,
        issue: None,
        number: 0,
        title: title.to_string(),
        input: input.to_string(),
        state: RunState::Queued,
        test,
        started: inner.now(),
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    }
}

fn manual_item(input: &RunInput) -> Result<Item> {
    let title = input.title.trim();
    let title = if title.is_empty() { base::fmt::first_line(input.text.trim()) } else { title };
    if title.is_empty() {
        bail!("A run needs a title or some text");
    }
    Ok(Item {
        title: base::fmt::truncate_chars(title, 120),
        text: input.text.trim().to_string(),
        issue: None,
        source: SourceKind::Manual,
    })
}

impl Inner {
    fn run_or_fail(&self, id: &RunId) -> Result<Run> {
        self.store.run(id)?.ok_or_else(|| anyhow!("There is no run {id}"))
    }
}

impl Engine {
    /// Starts the live version of `issue`'s pipeline with the issue as the item and `text` as
    /// what it is told. Returns the run; the issue records it.
    pub(crate) fn start_issue_run(&self, issue: &Issue, pipeline: &PipelineId, text: &str) -> Result<Run> {
        let inner = &self.inner;
        let (graph, version) = pipelines::live_graph(inner, pipeline)?;
        let mut run = shell(inner, pipeline, version, &issue.title, text, false);
        let number = inner.store.runs_for_issue(&issue.key)?.len() as u32 + 1;
        run.issue = Some(issue.key.clone());
        run.number = number;
        let item = Item { title: issue.title.clone(), text: text.to_string(), issue: Some(issue.clone()), source: SourceKind::Issue };
        let run = pipelines::launch(inner, graph, run, item)?;
        if let Some(mut stored) = inner.store.issue(&issue.key)? {
            stored.run = Some(run.id.clone());
            stored.run_count = stored.run_count.max(number);
            stored.updated = inner.now();
            inner.save_issue(&stored)?;
        }
        Ok(run)
    }

    fn start_graph(&self, graph: GraphDef, version: u32, input: RunInput, test: bool) -> Result<Run> {
        let item = manual_item(&input)?;
        let run = shell(&self.inner, &graph.id, version, &item.title, &item.text, test);
        pipelines::launch(&self.inner, graph, run, item)
    }

    /// Answers the question an agent of the run waits on: the instance of `node` when given,
    /// else the only one that asks.
    pub fn answer_run(&self, id: &RunId, node: Option<&model::NodeId>, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            bail!("The answer is empty");
        }
        let run = self.inner.run_or_fail(id)?;
        let asking: Vec<_> = run.nodes.iter().filter(|n| n.question.is_some() && node.is_none_or(|id| &n.node == id)).collect();
        let instance = match asking.as_slice() {
            [one] => *one,
            [] => bail!("No agent of run {id} waits for an answer"),
            _ => bail!("Several agents of run {id} ask something; name the node to answer"),
        };
        let key = pipeline::InstanceKey { node: instance.node.clone(), variant: instance.variant, attempt: instance.attempt };
        let handle = pipelines::handle(&self.inner, id)?;
        handle.answer(key.clone(), text.trim().to_string()).map_err(|e| anyhow!("{e}"))?;
        pipelines::resolve(&self.inner, &pipelines::question_inbox_id(id, &key))
    }

    /// Allows the run to spend `usd` more, and continues what stopped at its budget.
    pub fn raise_run_budget(&self, id: &RunId, usd: f64) -> Result<()> {
        self.inner.run_or_fail(id)?;
        pipelines::handle(&self.inner, id)?.raise_budget(usd).map_err(|e| anyhow!("{e}"))
    }
}

impl RunApi for Engine {
    fn run(&self, id: &RunId) -> Result<Option<Run>> {
        self.inner.store.run(id)
    }

    fn latest_runs(&self) -> Result<Vec<Run>> {
        let mut runs = self.inner.store.latest_runs()?;
        let open = self.inner.store.open_runs()?;
        runs.extend(open.into_iter().filter(|r| r.issue.is_none()));
        Ok(runs)
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
        let inner = &self.inner;
        let run = inner.run_or_fail(id)?;
        let reviews = inner.store.reviews_for_run(id)?;
        let snapshot = inner
            .store
            .marker(&pipelines::snapshot_marker(id))?
            .and_then(|text| serde_json::from_str::<pipeline::exec::Snapshot>(&text).ok());
        let graph = inner.store.marker(&pipelines::graph_marker(id))?.and_then(|text| pipelines::parse_graph(&text).ok());
        Ok(RunStats {
            agents_running: run
                .nodes
                .iter()
                .filter(|n| n.status == NodeStatus::Running && n.kind != model::SpanKind::Step)
                .count() as u32,
            agents_max: graph.map_or_else(|| inner.store.settings().map_or(1, |s| s.slots), |g| g.limits.max_parallel),
            spawned: run.nodes.iter().filter(|n| n.spawned_by.is_some()).count() as u32,
            waiting_variants: reviews
                .iter()
                .filter(|r| r.state == model::ReviewState::Open)
                .map(|r| r.variants.len() as u32)
                .sum(),
            cost_usd: run.cost_usd.max(inner.store.spend_for_run(id)?.cost_usd),
            budget_usd: snapshot.and_then(|s| s.budget_usd),
        })
    }

    fn start_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run> {
        let (graph, version) = pipelines::live_graph(&self.inner, pipeline)?;
        self.start_graph(graph, version, input, false)
    }

    fn test_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run> {
        let graph = self.pipeline(pipeline).ok_or_else(|| anyhow!("There is no pipeline {pipeline}"))?;
        self.start_graph(graph, 0, input, true)
    }

    fn cancel_run(&self, id: &RunId) -> Result<()> {
        let run = self.inner.run_or_fail(id)?;
        if !run.state.is_open() {
            bail!("The run is already {}", format!("{:?}", run.state).to_lowercase());
        }
        pipelines::handle(&self.inner, id)?.cancel().map_err(|e| anyhow!("{e}"))
    }

    fn resume_run(&self, id: &RunId) -> Result<()> {
        self.inner.run_or_fail(id)?;
        pipelines::check_limit(&self.inner)?;
        pipelines::handle(&self.inner, id)?.resume().map_err(|e| anyhow!("{e}"))?;
        let failed = model::InboxId::new(format!("failed:{id}"));
        if self.inner.store.inbox_item(&failed)?.is_some() {
            pipelines::resolve(&self.inner, &failed)?;
        }
        if let Some(run) = self.inner.store.run(id)? {
            self.inner.publish(Event::RunChanged { run });
        }
        Ok(())
    }
}
