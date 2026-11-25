//! Review queue data: everything the screen shows, derived from the Store on a background
//! thread (`ReviewData::query`) so `render()` only reads plain structs.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use harness_core::{
    Card, Config, Decision, DecisionKind, GateKind, Id, NodeKind, PipelineGraph, Run, Status,
    Store, Task, TaskKind, ToolNode, Variant,
};

use crate::ui::ParsedDiff;

/// What a waiting item asks of you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// Pick one of the variants (or approve the only one).
    Pick,
    /// Approve (merge) the run's pull request.
    ApprovePr,
    /// An orchestrator plan waiting for approval.
    Plan,
    /// Any other waiting task: it can only be answered with text.
    Other,
}

/// One row of the input summary: an upstream step's output.
#[derive(Clone, Debug)]
pub struct StepRow {
    pub name: String,
    pub summary: String,
    pub cost: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillState {
    Done,
    Current,
    Pending,
}

/// A node in the step chain under the input summary.
#[derive(Clone, Debug)]
pub struct Pill {
    pub name: String,
    pub detail: String,
    pub state: PillState,
}

/// A node that runs after the gate.
#[derive(Clone, Debug)]
pub struct NextRow {
    pub name: String,
    pub kind: &'static str,
    pub model: String,
    pub detail: String,
}

/// Run context of an item (absent for chat plans).
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub number: u64,
    pub pipeline: String,
    pub version: u32,
    pub input: String,
    pub external_ref: Option<String>,
    pub chat_id: Option<Id>,
    pub cost: f64,
    /// Names of the nodes done so far (breadcrumb).
    pub crumbs: Vec<String>,
    pub rows: Vec<StepRow>,
    pub chain: Vec<Pill>,
    /// Upstream Agent nodes a send-back can target (closest first): (node id, name).
    pub back_options: Vec<(Id, String)>,
    pub next: Vec<NextRow>,
}

/// One variant of a Pick gate, with its diff parsed once.
#[derive(Clone, Debug)]
pub struct VariantView {
    pub v: Variant,
    pub title: String,
    /// Summary without its first line (the title).
    pub body: String,
    pub diff: Arc<ParsedDiff>,
    /// One single-file diff per file, for per-file "Viewed" rendering.
    pub files: Vec<Arc<ParsedDiff>>,
    pub added: usize,
    pub removed: usize,
}

impl VariantView {
    fn new(v: Variant) -> Self {
        let diff = ParsedDiff::parse(&v.diff);
        let files = diff
            .files
            .iter()
            .map(|f| {
                Arc::new(ParsedDiff {
                    files: vec![f.clone()],
                })
            })
            .collect();
        let first = first_line(&v.summary);
        let title = if v.title.trim().is_empty() {
            first.clone()
        } else {
            v.title.clone()
        };
        let body = rest_after_first_line(&v.summary);
        VariantView {
            title,
            body,
            added: diff.added(),
            removed: diff.removed(),
            diff,
            files,
            v,
        }
    }
}

/// A waiting item in the queue.
#[derive(Clone, Debug)]
pub struct Item {
    pub task: Task,
    pub kind: ItemKind,
    pub run: Option<RunInfo>,
    /// "Pick a design · 2 variants", "Approve PR", "Plan approval".
    pub kind_line: String,
    pub variants: Vec<VariantView>,
    pub plan_steps: Vec<String>,
    /// The chat a plan belongs to.
    pub chat_id: Option<Id>,
    /// Default target of a send-back (the gate's `back_to`).
    pub back_to: Option<Id>,
    pub pr_url: Option<String>,
    pub has_images: bool,
}

/// A failed task from today (Retry / Open).
#[derive(Clone, Debug)]
pub struct AlsoRow {
    pub task_id: Id,
    pub title: String,
    pub run_ref: String,
    pub error: String,
    pub retryable: bool,
}

/// A decision made today.
#[derive(Clone, Debug)]
pub struct DecidedRow {
    pub task_id: Id,
    pub time: String,
    pub what: String,
    pub run_ref: String,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewData {
    pub items: Vec<Item>,
    pub also: Vec<AlsoRow>,
    pub decided: Vec<DecidedRow>,
    pub median_secs: Option<i64>,
    /// Real past send-back notes, most frequent first.
    pub reasons: Vec<(String, usize)>,
}

impl ReviewData {
    pub fn query(store: &Arc<Store>) -> ReviewData {
        let tasks = store.tasks();
        let runs: HashMap<Id, Run> = store
            .runs()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let pipelines: HashMap<Id, String> = store
            .pipelines()
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect();
        let decisions = store.decisions();
        let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        let mut waiting: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.status == Status::Waiting)
            .collect();
        waiting.sort_by_key(|t| t.created_at);
        let items = waiting
            .into_iter()
            .map(|t| build_item(store, t, &runs, &pipelines, &tasks))
            .collect();

        let today = Local::now().date_naive();
        let is_today = |d: DateTime<Utc>| d.with_timezone(&Local).date_naive() == today;
        let run_ref = |t: &Task| -> String {
            t.run_id
                .as_deref()
                .and_then(|r| runs.get(r))
                .map(|r| format!("#{}", r.number))
                .unwrap_or_default()
        };

        let mut failed: Vec<&Task> = tasks
            .iter()
            .filter(|t| {
                t.status == Status::Failed && is_today(t.finished_at.unwrap_or(t.created_at))
            })
            .collect();
        failed.sort_by_key(|t| std::cmp::Reverse(t.finished_at.unwrap_or(t.created_at)));
        let also = failed
            .into_iter()
            .take(8)
            .map(|t| AlsoRow {
                task_id: t.id.clone(),
                title: t.title.clone(),
                run_ref: run_ref(t),
                error: t.error.as_deref().map(first_line).unwrap_or_default(),
                retryable: t.run_id.is_some() && t.node_id.is_some(),
            })
            .collect();

        let mut today_decisions: Vec<&Decision> = decisions
            .iter()
            .filter(|d| is_today(d.created_at))
            .collect();
        today_decisions.sort_by_key(|d| std::cmp::Reverse(d.created_at));
        let waits: Vec<i64> = today_decisions
            .iter()
            .filter_map(|d| {
                by_id
                    .get(d.task_id.as_str())
                    .map(|t| (d.created_at - t.created_at).num_seconds())
            })
            .collect();
        let decided = today_decisions
            .iter()
            .take(8)
            .map(|d| {
                let task = by_id.get(d.task_id.as_str());
                let title = task.map(|t| t.title.clone()).unwrap_or_default();
                DecidedRow {
                    task_id: d.task_id.clone(),
                    time: d
                        .created_at
                        .with_timezone(&Local)
                        .format("%H:%M")
                        .to_string(),
                    what: if title.is_empty() {
                        decision_label(&d.kind)
                    } else {
                        format!("{} · {title}", decision_label(&d.kind))
                    },
                    run_ref: task.map(|t| run_ref(t)).unwrap_or_default(),
                }
            })
            .collect();

        ReviewData {
            items,
            also,
            decided,
            median_secs: median(waits),
            reasons: send_back_reasons(&decisions),
        }
    }

    /// Age in seconds of the oldest waiting item.
    pub fn oldest_secs(&self) -> Option<i64> {
        self.items
            .iter()
            .map(|i| (Utc::now() - i.task.created_at).num_seconds())
            .max()
    }
}

fn build_item(
    store: &Arc<Store>,
    t: &Task,
    runs: &HashMap<Id, Run>,
    pipelines: &HashMap<Id, String>,
    tasks: &[Task],
) -> Item {
    let run = t.run_id.as_deref().and_then(|r| runs.get(r));
    let gate = run
        .zip(t.node_id.as_deref())
        .and_then(|(r, n)| match &r.graph.node(n)?.kind {
            NodeKind::Gate(g) => Some(g.clone()),
            _ => None,
        });
    let kind = match (&gate, t.kind) {
        (Some(g), _) if g.kind == GateKind::ApprovePr => ItemKind::ApprovePr,
        (Some(_), _) => ItemKind::Pick,
        (None, TaskKind::Chat) => ItemKind::Plan,
        _ => ItemKind::Other,
    };
    let mut variants: Vec<Variant> = if kind == ItemKind::Pick || kind == ItemKind::ApprovePr {
        store.variants(&t.id)
    } else {
        vec![]
    };
    variants.sort_by_key(|v| v.index);
    let has_images = variants.iter().any(|v| !v.images.is_empty());
    let variants: Vec<VariantView> = variants.into_iter().map(VariantView::new).collect();

    let (plan_steps, chat_id) = if kind == ItemKind::Plan {
        let chat = t.chat_id.clone();
        let steps = chat
            .as_deref()
            .map(|c| {
                store
                    .messages(c)
                    .into_iter()
                    .rev()
                    .find_map(|m| match m.card {
                        Some(Card::Plan { task_id, steps }) if task_id == t.id => Some(steps),
                        _ => None,
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        (steps, chat)
    } else {
        (vec![], None)
    };

    let kind_line = match kind {
        ItemKind::Pick if variants.len() == 1 => "Approve output".to_string(),
        ItemKind::Pick => format!(
            "{} · {} variants",
            if has_images {
                "Pick a design"
            } else {
                "Pick a variant"
            },
            variants.len()
        ),
        ItemKind::ApprovePr => "Approve PR".to_string(),
        ItemKind::Plan => "Plan approval".to_string(),
        ItemKind::Other => format!("{} needs you", t.kind.label()),
    };
    let pr_url = run.and_then(|r| r.pr_url.clone()).or_else(|| {
        (kind == ItemKind::ApprovePr)
            .then(|| variants.first().map(|v| v.v.title.clone()))
            .flatten()
            .filter(|u| u.starts_with("http"))
    });
    let run_info = run.map(|r| run_info(store, r, t, pipelines, tasks));
    Item {
        task: t.clone(),
        kind,
        run: run_info,
        kind_line,
        variants,
        plan_steps,
        chat_id,
        back_to: gate.and_then(|g| g.back_to),
        pr_url,
        has_images,
    }
}

fn run_info(
    store: &Arc<Store>,
    run: &Run,
    gate_task: &Task,
    pipelines: &HashMap<Id, String>,
    tasks: &[Task],
) -> RunInfo {
    let g = &run.graph;
    let gate_node = gate_task.node_id.clone().unwrap_or_default();
    let upstream = upstream_order(g, &gate_node);
    let downstream = downstream_order(g, &gate_node);

    // Cost and fan-out count per node.
    let mut per_node: HashMap<&str, (f64, usize)> = HashMap::new();
    for t in tasks
        .iter()
        .filter(|t| t.run_id.as_deref() == Some(run.id.as_str()))
    {
        if let Some(n) = t.node_id.as_deref() {
            let e = per_node.entry(n).or_default();
            e.0 += t.cost_usd;
            if t.variant.is_some() && t.depth == 0 && t.kind != TaskKind::Gate {
                e.1 += 1;
            }
        }
    }
    let cost_of = |id: &str| per_node.get(id).map(|c| c.0).filter(|c| *c > 0.0);

    let mut crumbs = vec![];
    let mut rows = vec![];
    let mut chain = vec![];
    for id in &upstream {
        let (Some(n), Some(o)) = (g.node(id), run.outputs.get(id)) else {
            continue;
        };
        crumbs.push(n.name.clone());
        let summary = first_line(&o.summary);
        if !summary.is_empty() && !matches!(n.kind, NodeKind::Source(_)) {
            rows.push(StepRow {
                name: n.name.clone(),
                summary,
                cost: cost_of(id),
            });
        }
        let fan = per_node.get(id.as_str()).map(|c| c.1).filter(|n| *n > 1);
        let mut detail = String::new();
        if let Some(n) = fan {
            detail.push_str(&format!("×{n}"));
        }
        if let Some(c) = cost_of(id) {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&crate::ui::fmt_usd(c));
        }
        chain.push(Pill {
            name: n.name.clone(),
            detail,
            state: PillState::Done,
        });
    }
    chain.push(Pill {
        name: g
            .node(&gate_node)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "Gate".into()),
        detail: "you".into(),
        state: PillState::Current,
    });
    let mut next = vec![];
    for id in &downstream {
        let Some(n) = g.node(id) else { continue };
        chain.push(Pill {
            name: n.name.clone(),
            detail: String::new(),
            state: PillState::Pending,
        });
        next.push(next_row(store, n));
    }

    let back_options = upstream
        .iter()
        .rev()
        .filter_map(|id| g.node(id))
        .filter(|n| matches!(n.kind, NodeKind::Agent(_)))
        .map(|n| (n.id.clone(), n.name.clone()))
        .collect();

    RunInfo {
        number: run.number,
        pipeline: pipelines.get(&run.pipeline_id).cloned().unwrap_or_default(),
        version: run.pipeline_version,
        input: run.input.clone(),
        external_ref: run.external_ref.clone().filter(|r| !r.is_empty()),
        chat_id: run.chat_id.clone(),
        cost: run.cost_usd,
        crumbs,
        rows,
        chain,
        back_options,
        next,
    }
}

fn next_row(store: &Arc<Store>, n: &harness_core::Node) -> NextRow {
    let mut model = String::new();
    let detail = match &n.kind {
        NodeKind::Agent(a) => {
            let m = if !a.model.is_empty() {
                a.model.clone()
            } else {
                a.agent
                    .as_deref()
                    .and_then(|id| store.agent(id))
                    .map(|d| d.model)
                    .unwrap_or_default()
            };
            if !m.is_empty() {
                model = Config::short_model(&m);
            }
            first_line(&a.instructions)
        }
        NodeKind::Tool(ToolNode::Command { command, .. }) => command.clone(),
        NodeKind::Tool(ToolNode::PullRequest { draft, .. }) => {
            if *draft {
                "Opens a draft pull request".into()
            } else {
                "Opens a pull request".into()
            }
        }
        NodeKind::Tool(ToolNode::MergePr { method }) => {
            format!("Merges the pull request ({method})")
        }
        NodeKind::Render(r) => format!("Renders {}", r.files),
        NodeKind::Decision(d) => d.question.clone(),
        NodeKind::Router(r) => format!("{} rules", r.rules.len()),
        NodeKind::Gate(_) => "Waits for you".into(),
        NodeKind::Source(_) => String::new(),
    };
    NextRow {
        name: n.name.clone(),
        kind: n.kind.label(),
        model,
        detail: truncate(&detail, 90),
    }
}

/// Nodes upstream of `id` (transitive), earliest first.
pub fn upstream_order(g: &PipelineGraph, id: &str) -> Vec<Id> {
    let mut seen: Vec<Id> = vec![];
    let mut q: VecDeque<Id> = g.incoming(id).iter().map(|e| e.from.clone()).collect();
    while let Some(n) = q.pop_front() {
        if seen.contains(&n) {
            continue;
        }
        seen.push(n.clone());
        q.extend(g.incoming(&n).iter().map(|e| e.from.clone()));
    }
    seen.reverse();
    seen
}

/// Nodes downstream of `id` (transitive, breadth first), at most 8.
pub fn downstream_order(g: &PipelineGraph, id: &str) -> Vec<Id> {
    let mut seen: Vec<Id> = vec![];
    let mut q: VecDeque<Id> = g.next(id, None).iter().map(|e| e.to.clone()).collect();
    while let Some(n) = q.pop_front() {
        if seen.contains(&n) || n == id {
            continue;
        }
        seen.push(n.clone());
        q.extend(g.next(&n, None).iter().map(|e| e.to.clone()));
    }
    seen.truncate(8);
    seen
}

pub fn decision_label(k: &DecisionKind) -> String {
    match k {
        DecisionKind::Pick { index } => format!("Picked {}", harness_core::variant_letter(*index)),
        DecisionKind::Approve => "Approved".into(),
        DecisionKind::SendBack { .. } => "Sent back".into(),
        DecisionKind::Reject { .. } => "Rejected".into(),
        DecisionKind::Reply { .. } => "Replied".into(),
    }
}

/// Median of the values (mean of the middle two for an even count).
pub fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    })
}

/// Send-back notes you wrote before, most frequent first (ties: most recent first).
/// Only short notes qualify: a chip must be readable.
pub fn send_back_reasons(decisions: &[Decision]) -> Vec<(String, usize)> {
    let mut sorted: Vec<&Decision> = decisions.iter().collect();
    sorted.sort_by_key(|d| std::cmp::Reverse(d.created_at));
    let mut order: Vec<String> = vec![];
    let mut counts: HashMap<String, usize> = HashMap::new();
    for d in sorted {
        if let DecisionKind::SendBack { note, .. } = &d.kind {
            let note = note.trim();
            if note.is_empty() || note.chars().count() > 100 || note.contains('\n') {
                continue;
            }
            if !counts.contains_key(note) {
                order.push(note.to_string());
            }
            *counts.entry(note.to_string()).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = order
        .into_iter()
        .map(|n| (counts[&n], n))
        .map(|(c, n)| (n, c))
        .collect();
    out.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    out.truncate(6);
    out
}

/// First non-empty line, markdown heading marks removed.
pub fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches('#')
        .trim()
        .to_string()
}

fn rest_after_first_line(s: &str) -> String {
    let mut lines = s.lines().skip_while(|l| l.trim().is_empty());
    lines.next();
    lines.collect::<Vec<_>>().join("\n").trim().to_string()
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

/// Distinct image file names (without extension) across variants, in first-seen order.
pub fn image_names(variants: &[VariantView]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for v in variants {
        for p in &v.v.images {
            let n = image_name(p);
            if seen.insert(n.clone()) {
                out.push(n);
            }
        }
    }
    out
}

/// `/x/y/pricing.png` -> `pricing`.
pub fn image_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::{Edge, Node, SourceNode};

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            kind: NodeKind::Source(SourceNode::default()),
            x: 0.0,
            y: 0.0,
        }
    }

    fn graph() -> PipelineGraph {
        let e = |a: &str, b: &str| Edge {
            from: a.into(),
            to: b.into(),
            port: None,
        };
        PipelineGraph {
            workdirs: vec![],
            nodes: ["a", "b", "g", "c", "d"].iter().map(|n| node(n)).collect(),
            edges: vec![e("a", "b"), e("b", "g"), e("g", "c"), e("c", "d")],
        }
    }

    #[test]
    fn orders_around_a_gate() {
        let g = graph();
        assert_eq!(upstream_order(&g, "g"), vec!["a", "b"]);
        assert_eq!(downstream_order(&g, "g"), vec!["c", "d"]);
    }

    #[test]
    fn medians() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![5, 1, 9]), Some(5));
        assert_eq!(median(vec![2, 4]), Some(3));
    }

    #[test]
    fn reasons_are_real_and_sorted() {
        let d = |note: &str| Decision {
            id: harness_core::new_id(),
            task_id: "t".into(),
            run_id: None,
            kind: DecisionKind::SendBack {
                note: note.into(),
                target: None,
            },
            created_at: Utc::now(),
            consumed: true,
        };
        let r = send_back_reasons(&[
            d("no tests"),
            d("too big"),
            d("no tests"),
            d("  "),
            d("line\nbreak"),
        ]);
        assert_eq!(
            r,
            vec![("no tests".to_string(), 2), ("too big".to_string(), 1)]
        );
        assert!(send_back_reasons(&[]).is_empty());
    }

    #[test]
    fn summary_lines() {
        assert_eq!(first_line("\n## Title\nbody"), "Title");
        assert_eq!(rest_after_first_line("Title\n\nbody\nmore"), "body\nmore");
        assert_eq!(image_name("/a/b/pricing.png"), "pricing");
    }
}
