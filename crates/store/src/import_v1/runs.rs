//! Pipeline runs of the old app (`runs/<id>.json`), imported as history.
//!
//! The old run had one stage per pipeline step, a flat log and a list of what each agent used.
//! A run that was still running or waiting when the old app last wrote it becomes
//! `Interrupted`: nothing in the new app owns it, and it is not resumed. The old file only
//! recorded one token total per agent, which is kept as the input count.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context as _, Result};
use model::{
    IssueKey, LogLevel, Millis, ModelRef, NodeId, NodeInstance, NodeStatus, PipelineId, Run, RunId, RunLogLine,
    RunState, RunnableRef, SpanKind, Usage, UsageEntry,
};
use serde::Deserialize;

use super::renders::{self, OldRender};
use super::report::Kind;
use super::{Ctx, json_files, read_json, rel, secs_to_ms};

/// The pipeline of runs saved before runs recorded theirs.
const LEGACY_PIPELINE: &str = "mockup-gpui-build";

/// Marks a run whose import finished, so a half-written one is completed and a finished one
/// left alone.
fn done_marker(run: &str) -> String {
    format!("v1_run:{run}")
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OldRunStatus {
    Running,
    WaitingApproval,
    Done,
    Failed,
    Cancelled,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OldStageStatus {
    Pending,
    Running,
    Waiting,
    Done,
    Skipped,
    Failed,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OldLogKind {
    Step,
    Text,
    Tool,
    Error,
}

#[derive(Deserialize)]
struct OldStage {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    kind: String,
    status: OldStageStatus,
    #[serde(default)]
    started: u64,
    #[serde(default)]
    finished: u64,
    #[serde(default)]
    summary: String,
}

#[derive(Deserialize)]
struct OldLine {
    at: u64,
    #[serde(default)]
    who: String,
    kind: OldLogKind,
    text: String,
}

#[derive(Deserialize)]
struct OldAgentUsage {
    who: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    cost_usd: f64,
}

#[derive(Deserialize)]
pub(super) struct OldRun {
    pub(super) id: String,
    #[serde(default)]
    pub(super) issue: String,
    pub(super) created: u64,
    status: OldRunStatus,
    #[serde(default)]
    pipeline: String,
    stages: Vec<OldStage>,
    #[serde(default)]
    notes: BTreeMap<String, String>,
    #[serde(default)]
    renders: Vec<OldRender>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    cost_usd: f64,
    #[serde(default)]
    usage: Vec<OldAgentUsage>,
    #[serde(default)]
    log: Vec<OldLine>,
}

/// A run file that parsed, with its name for reports.
pub(super) struct ParsedRun {
    pub(super) file: String,
    pub(super) old: OldRun,
}

impl OldRun {
    pub(super) fn state(&self) -> RunState {
        match self.status {
            OldRunStatus::Done => RunState::Done,
            OldRunStatus::Failed => RunState::Failed,
            OldRunStatus::Cancelled => RunState::Cancelled,
            OldRunStatus::Running | OldRunStatus::WaitingApproval => RunState::Interrupted,
        }
    }

    /// When the run last did anything, for a finished run's end and its issue's last change.
    pub(super) fn last_activity(&self) -> Millis {
        let stages = self.stages.iter().map(|s| s.finished.max(s.started));
        let lines = self.log.iter().map(|l| l.at);
        let last = stages.chain(lines).max().unwrap_or(0).max(self.created);
        secs_to_ms(last)
    }

    pub(super) fn pr_url(&self) -> Option<&str> {
        self.pr_url.as_deref().filter(|u| !u.is_empty())
    }
}

/// Reads every `runs/*.json`; the ones that do not parse are reported and left out.
pub(super) fn load_all(cx: &mut Ctx) -> Vec<ParsedRun> {
    let dir = cx.data.join("runs");
    let files = match json_files(&dir) {
        Ok(files) => files,
        Err(err) => {
            cx.report.fail(Kind::Run, "runs", format!("{err:#}"));
            return Vec::new();
        }
    };
    let mut runs = Vec::new();
    for file in files {
        let name = rel(&cx.data, &file);
        let parsed =
            read_json(&file).and_then(|value| serde_json::from_value::<OldRun>(value).context("not a saved run"));
        match parsed {
            Ok(old) if old.id.trim().is_empty() || old.id.contains(['/', '\\']) => {
                cx.report.fail(Kind::Run, name, "the run has no usable id");
            }
            Ok(old) => runs.push(ParsedRun { file: name, old }),
            Err(err) => cx.report.fail(Kind::Run, name, format!("{err:#}")),
        }
    }
    runs
}

/// What the issue a run belongs to gives it.
pub(super) struct IssueInfo {
    pub(super) key: IssueKey,
    pub(super) title: String,
    pub(super) body: String,
    /// "Run 2 on this issue".
    pub(super) number: u32,
}

/// Imports one run: skipped when an earlier import finished it, otherwise written (or, in a
/// dry run, only counted).
pub(super) fn import_one(cx: &mut Ctx, parsed: &ParsedRun, info: Option<&IssueInfo>) -> Result<()> {
    let old = &parsed.old;
    let id = RunId::new(&old.id);
    let marker = done_marker(&old.id);
    let existing = cx.store.run(&id)?;
    if existing.is_some() && (cx.dry || cx.store.marker(&marker)?.is_some()) {
        cx.report.skipped(Kind::Run, 1);
        return Ok(());
    }
    let converted = convert(old, info);
    // Pictures first: a missing one is reported even in a dry run.
    let (renders, images) = renders::copy(cx, &old.id, &old.renders);
    cx.report.imported(Kind::Image, images);
    if cx.dry {
        cx.report.imported(Kind::Run, 1);
        return Ok(());
    }
    let store = cx.store;
    store.upsert_run(&converted.run)?;
    if store.run_log_tail(&id, 1)?.is_empty() {
        store.append_run_log(&id, &converted.log)?;
    }
    if store.renders(&id)?.is_empty() {
        for render in &renders {
            store.add_render(&id, render)?;
        }
    }
    if existing.is_none() {
        for entry in &converted.usage {
            store.record_usage(entry)?;
        }
    }
    store.set_marker(&marker, "1")?;
    cx.report.imported(Kind::Run, 1);
    Ok(())
}

struct Converted {
    run: Run,
    log: Vec<RunLogLine>,
    usage: Vec<UsageEntry>,
}

/// A stage's id and name; runs of the first version had neither, only a kind.
fn identity(stage: &OldStage) -> (String, String) {
    let id = if stage.id.is_empty() {
        stage.kind.clone()
    } else {
        stage.id.clone()
    };
    let name = if stage.name.is_empty() {
        let spaced = stage.kind.replace('_', " ");
        let mut chars = spaced.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or(spaced)
    } else {
        stage.name.clone()
    };
    (id, name)
}

fn node_status(status: OldStageStatus) -> NodeStatus {
    match status {
        OldStageStatus::Pending => NodeStatus::Pending,
        OldStageStatus::Running | OldStageStatus::Waiting => NodeStatus::Interrupted,
        OldStageStatus::Done => NodeStatus::Done,
        OldStageStatus::Skipped => NodeStatus::Skipped,
        OldStageStatus::Failed => NodeStatus::Failed,
    }
}

fn span_kind(kind: &str) -> SpanKind {
    match kind {
        "agent" => SpanKind::Agent,
        "approval" => SpanKind::Waiting,
        _ => SpanKind::Step,
    }
}

fn some_ms(secs: u64) -> Option<Millis> {
    (secs > 0).then(|| secs_to_ms(secs))
}

/// `Coder 2` is the agent `coder`: the trailing number only tells instances apart.
fn agent_slug(who: &str) -> String {
    let name = who.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace());
    base::id::slug(if name.is_empty() { who } else { name }, 40)
}

fn convert(old: &OldRun, info: Option<&IssueInfo>) -> Converted {
    let state = old.state();
    let started = secs_to_ms(old.created);
    let finished = (!state.is_open()).then(|| old.last_activity());

    let mut attempts: HashMap<String, u32> = HashMap::new();
    let mut nodes = Vec::with_capacity(old.stages.len());
    for stage in &old.stages {
        let (id, name) = identity(stage);
        let attempt = attempts.entry(id.clone()).and_modify(|n| *n += 1).or_insert(1);
        let note = old.notes.get(&id).map(|n| n.trim()).filter(|n| !n.is_empty());
        let cost_usd = old
            .usage
            .iter()
            .filter(|u| u.who.eq_ignore_ascii_case(&name))
            .map(|u| u.cost_usd)
            .sum();
        nodes.push(NodeInstance {
            node: NodeId(id),
            name,
            variant: 0,
            attempt: *attempt,
            status: node_status(stage.status),
            kind: span_kind(&stage.kind),
            port: None,
            session: None,
            worktree: None,
            branch: None,
            started: some_ms(stage.started),
            finished: some_ms(stage.finished),
            cost_usd,
            summary: note.unwrap_or(stage.summary.trim()).to_string(),
            spawned_by: None,
        });
    }
    let node_of = |who: &str| -> Option<NodeId> {
        nodes
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(who) || n.node.as_str().eq_ignore_ascii_case(who))
            .map(|n| n.node.clone())
    };

    let log = old
        .log
        .iter()
        .map(|line| {
            let node = node_of(&line.who);
            // Lines of agents that are not a stage (`Coder 2`) keep their name in the text.
            let named = node.is_none() && !line.who.is_empty() && line.who != "pipeline";
            RunLogLine {
                seq: 0,
                at: secs_to_ms(line.at),
                variant: 0,
                level: match line.kind {
                    OldLogKind::Step | OldLogKind::Text => LogLevel::Info,
                    OldLogKind::Tool => LogLevel::Tool,
                    OldLogKind::Error => LogLevel::Error,
                },
                text: if named {
                    format!("{}: {}", line.who, line.text)
                } else {
                    line.text.clone()
                },
                node,
            }
        })
        .collect();

    let at = finished.unwrap_or(started);
    let usage: Vec<UsageEntry> = old
        .usage
        .iter()
        .map(|u| UsageEntry {
            at,
            session: None,
            run: Some(RunId::new(&old.id)),
            node: node_of(&u.who),
            runnable: RunnableRef::agent(&agent_slug(&u.who)),
            name: u.who.clone(),
            created_by: None,
            model: ModelRef::parse(&u.model),
            usage: Usage {
                input: u.tokens,
                ..Usage::default()
            },
            cost_usd: u.cost_usd,
            imported: true,
        })
        .collect();
    let cost_usd = if old.cost_usd > 0.0 {
        old.cost_usd
    } else {
        old.usage.iter().map(|u| u.cost_usd).sum()
    };
    let tokens = if old.tokens > 0 {
        old.tokens
    } else {
        old.usage.iter().map(|u| u.tokens).sum()
    };

    let pipeline = if old.pipeline.is_empty() {
        LEGACY_PIPELINE
    } else {
        &old.pipeline
    };
    let run = Run {
        id: RunId::new(&old.id),
        pipeline: PipelineId::new(pipeline),
        version: 0,
        issue: info.map(|i| i.key.clone()),
        number: info.map_or(1, |i| i.number),
        title: info.map_or_else(|| old.id.clone(), |i| i.title.clone()),
        input: info.map(|i| i.body.clone()).unwrap_or_default(),
        state,
        test: false,
        started,
        finished,
        cost_usd,
        usage: Usage {
            input: tokens,
            ..Usage::default()
        },
        error: old.error.clone().filter(|e| !e.is_empty()),
        pr_url: old.pr_url().map(str::to_string),
        nodes,
    };
    Converted { run, log, usage }
}
