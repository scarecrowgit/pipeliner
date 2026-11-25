//! Store queries for the Pipelines screen. They run on a background thread (`screens::load`).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use harness_core::{AgentDef, Id, Pipeline, Run, Status, Store, Task, TaskKind};

/// How many recent runs feed node stats and the Runs tab.
const RUN_LIMIT: usize = 200;

/// One row of the pipeline list.
pub struct ListRow {
    pub pipeline: Pipeline,
    pub runs: usize,
    /// Status, time and number of the newest run.
    pub last: Option<(Status, DateTime<Utc>, u64)>,
    pub active: usize,
    pub waiting: usize,
}

pub fn list(store: &Store) -> Vec<ListRow> {
    let runs = store.runs();
    let waiting = store.waiting();
    let run_pipeline: HashMap<&str, &str> = runs
        .iter()
        .map(|r| (r.id.as_str(), r.pipeline_id.as_str()))
        .collect();
    let mut rows: Vec<ListRow> = store
        .pipelines()
        .into_iter()
        .map(|p| {
            let mine: Vec<&Run> = runs.iter().filter(|r| r.pipeline_id == p.id).collect();
            let last = mine
                .iter()
                .max_by_key(|r| r.created_at)
                .map(|r| (r.status, r.finished_at.unwrap_or(r.created_at), r.number));
            let active = mine.iter().filter(|r| r.status.is_active()).count();
            let waiting = waiting
                .iter()
                .filter(|t| {
                    t.run_id
                        .as_deref()
                        .and_then(|r| run_pipeline.get(r))
                        .copied()
                        == Some(p.id.as_str())
                })
                .count();
            ListRow {
                runs: mine.len(),
                last,
                active,
                waiting,
                pipeline: p,
            }
        })
        .collect();
    rows.sort_by_key(|r| r.pipeline.name.to_lowercase());
    rows
}

/// A run of this pipeline with its tasks.
pub struct RunRow {
    pub run: Run,
    /// Pipeline steps, oldest first.
    pub tasks: Vec<Task>,
    /// The node working now (active runs) or the last one that ran.
    pub current: Option<String>,
    /// The newest failed step (Retry re-runs it).
    pub failed_task: Option<Id>,
    /// A gate waiting for you.
    pub waiting_gate: Option<Id>,
}

/// Live numbers for one node, from real tasks of recent runs.
#[derive(Clone, Debug, Default)]
pub struct NodeStats {
    pub running: usize,
    pub queued: usize,
    pub waiting: usize,
    /// Newest run number with an active task here.
    pub active_run: Option<u64>,
    pub waiting_since: Option<DateTime<Utc>>,
    pub done: usize,
    pub failed: usize,
    cost_sum: f64,
    secs_sum: f64,
    secs_n: usize,
    /// Variants produced here that a gate offered / you picked.
    pub offered: usize,
    pub picked: usize,
    /// Ports taken (decisions, routers).
    pub ports: BTreeMap<String, usize>,
}

impl NodeStats {
    pub fn avg_cost(&self) -> Option<f64> {
        (self.done > 0 && self.cost_sum > 0.0).then(|| self.cost_sum / self.done as f64)
    }
    pub fn avg_secs(&self) -> Option<i64> {
        (self.secs_n > 0).then(|| (self.secs_sum / self.secs_n as f64).round() as i64)
    }
    pub fn picked_pct(&self) -> Option<u32> {
        (self.offered > 0)
            .then(|| (self.picked as f64 * 100.0 / self.offered as f64).round() as u32)
    }
}

pub struct Detail {
    pub pipeline: Pipeline,
    pub agents: Vec<AgentDef>,
    /// Newest first (at most [`RUN_LIMIT`]).
    pub runs: Vec<RunRow>,
    pub total_runs: usize,
    pub stats: HashMap<Id, NodeStats>,
}

pub fn detail(store: &Store, id: &str) -> Option<Detail> {
    let pipeline = store.pipeline(id)?;
    let mut agents = store.agents();
    agents.sort_by_key(|a| a.name.to_lowercase());
    let mut runs = store.runs_for_pipeline(id);
    runs.sort_by_key(|r| std::cmp::Reverse(r.number));
    let total_runs = runs.len();
    runs.truncate(RUN_LIMIT);

    let mut stats: HashMap<Id, NodeStats> = HashMap::new();
    let mut rows = Vec::with_capacity(runs.len());
    for run in runs {
        let mut tasks: Vec<Task> = store
            .tasks_for_run(&run.id)
            .into_iter()
            .filter(|t| {
                t.node_id.is_some() && !matches!(t.kind, TaskKind::Spawned | TaskKind::Chat)
            })
            .collect();
        tasks.sort_by_key(|t| t.created_at);
        let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        for t in &tasks {
            let s = stats
                .entry(t.node_id.clone().unwrap_or_default())
                .or_default();
            match t.status {
                Status::Running => s.running += 1,
                Status::Queued => s.queued += 1,
                Status::Waiting => {
                    s.waiting += 1;
                    let since = t.started_at.unwrap_or(t.created_at);
                    s.waiting_since = Some(s.waiting_since.map_or(since, |w| w.min(since)));
                }
                Status::Done => {
                    s.done += 1;
                    s.cost_sum += t.cost_usd;
                    if let (Some(a), Some(b)) = (t.started_at, t.finished_at) {
                        s.secs_sum += (b - a).num_milliseconds() as f64 / 1000.0;
                        s.secs_n += 1;
                    }
                }
                Status::Failed => s.failed += 1,
                Status::Cancelled => {}
            }
            if t.status.is_active() {
                s.active_run = Some(s.active_run.map_or(run.number, |n| n.max(run.number)));
            }
            // Picked rate: variants this node produced that a gate showed you.
            if t.kind == TaskKind::Gate {
                let variants = store.variants(&t.id);
                if variants.iter().any(|v| v.picked) {
                    for v in &variants {
                        let Some(node) = v
                            .source_task_id
                            .as_deref()
                            .and_then(|s| by_id.get(s))
                            .and_then(|t| t.node_id.clone())
                        else {
                            continue;
                        };
                        let ns = stats.entry(node).or_default();
                        ns.offered += 1;
                        ns.picked += v.picked as usize;
                    }
                }
            }
        }
        for (node, out) in &run.outputs {
            if let Some(p) = &out.port {
                *stats
                    .entry(node.clone())
                    .or_default()
                    .ports
                    .entry(p.clone())
                    .or_default() += 1;
            }
        }

        let node_name = |t: &Task| {
            t.node_id
                .as_deref()
                .and_then(|n| run.graph.node(n))
                .map(|n| n.name.clone())
                .unwrap_or_else(|| t.title.clone())
        };
        let current = if run.status.is_active() {
            tasks
                .iter()
                .rev()
                .find(|t| t.status.is_active())
                .or(tasks.last())
                .map(node_name)
        } else {
            tasks.last().map(node_name)
        };
        let failed_task = tasks
            .iter()
            .rev()
            .find(|t| t.status == Status::Failed)
            .map(|t| t.id.clone());
        let waiting_gate = tasks
            .iter()
            .find(|t| t.status == Status::Waiting && t.kind == TaskKind::Gate)
            .map(|t| t.id.clone());
        rows.push(RunRow {
            run,
            tasks,
            current,
            failed_task,
            waiting_gate,
        });
    }
    Some(Detail {
        pipeline,
        agents,
        runs: rows,
        total_runs,
        stats,
    })
}
