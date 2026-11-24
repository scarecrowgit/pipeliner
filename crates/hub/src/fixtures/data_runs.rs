//! Pipelines and runs: the PAY triage graph with its twelve nodes, two smaller pipelines, their
//! versions, the runs on the board and a log.

use std::collections::BTreeMap;

use crate::api::{
    ActionNode, AgentNode, EdgeDef, EdgeKind, FileMode, FileNode, GraphDef, IssueKey, IssueStatus, Limits, LogLevel,
    NodeDef, NodeId, NodeInstance, NodeKind, NodeStatus, PORT_APPROVED, PORT_OUT, PipelineId, PipelineVersion, Pos,
    ReviewNode, RouterNode, RouterRule, Run, RunId, RunLogLine, RunState, RunnableRef, SkillPolicy, SourceKind,
    SourceNode, SpanKind, SpawnPolicy, SubagentNode, ToolAccess, ToolGrants, Usage, VersionState, WorkdirDef,
};

use super::data_sessions::{model, path};
use super::{HOUR, MINUTE, World};

pub const PAY_TRIAGE: &str = "pay-triage";

fn node(id: &str, name: &str, x: f32, y: f32, kind: NodeKind) -> NodeDef {
    NodeDef { id: NodeId::new(id), name: name.to_string(), pos: Pos { x, y }, kind }
}

fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
    EdgeDef { from: NodeId::new(from), port: port.to_string(), to: NodeId::new(to), kind }
}

fn agent(runnable: &str, model_name: &str, variants: u32, spawn_depth: Option<u32>) -> AgentNode {
    AgentNode {
        runnable: Some(RunnableRef::agent(runnable)),
        model: Some(model(model_name)),
        workdir: "payments".to_string(),
        variants,
        spawn: Some(match spawn_depth {
            Some(depth) => SpawnPolicy { enabled: true, max_depth: depth, ..SpawnPolicy::default() },
            None => SpawnPolicy::default(),
        }),
        ..AgentNode::default()
    }
}

fn rule(id: &str, label: &str, when: &str) -> RouterRule {
    RouterRule { id: id.to_string(), label: label.to_string(), when: when.to_string(), ask: None }
}

/// PAY triage as drawn in the editor mockup.
pub fn pay_triage() -> GraphDef {
    use EdgeKind::*;
    let fixer = AgentNode {
        role: "Reproduce the bug from the issue, write a failing test, then fix it. Keep the diff under 200 lines.".to_string(),
        budget_usd: Some(0.6),
        files_out: vec!["patch.diff".to_string(), "summary.md".to_string()],
        tools: Some(ToolGrants { access: ToolAccess::Full, allow: vec!["git.bisect".to_string()], deny: Vec::new() }),
        spawn: Some(SpawnPolicy { enabled: true, max_depth: 2, per_run: 3, ..SpawnPolicy::default() }),
        ..agent("fixer", "opus-5.5", 3, Some(2))
    };
    let subagent = |role: &str, per_run: u32| SubagentNode {
        role: role.to_string(),
        model: model("sonnet-5.5"),
        tools: ToolGrants { access: ToolAccess::ReadAndRun, ..ToolGrants::default() },
        skills: SkillPolicy::default(),
        max_steps: 20,
        per_run,
    };
    GraphDef {
        schema: 1,
        id: PipelineId::new(PAY_TRIAGE),
        name: "PAY triage".to_string(),
        description: "Router › Fixer · Reproducer · Analyst › Review. Returns variants for a person to pick.".to_string(),
        limits: Limits { max_parallel: 4, budget_usd: Some(3.0), max_loops: 5 },
        workdirs: vec![WorkdirDef {
            id: "payments".to_string(),
            name: "payments-api".to_string(),
            path: path("code/acme/payments-api"),
            worktree: true,
            base_branch: "main".to_string(),
            branch_prefix: "fix/".to_string(),
            setup: "bun install".to_string(),
            env: BTreeMap::new(),
        }],
        nodes: vec![
            node("jira", "Jira puller", 20.0, 170.0, NodeKind::Source(SourceNode { source: SourceKind::Jira })),
            node("repo", "Repo context", 20.0, 400.0, NodeKind::File(FileNode { workdir: "payments".into(), path: "docs/**".into(), mode: FileMode::Read })),
            node(
                "router",
                "Router",
                212.0,
                120.0,
                NodeKind::Router(RouterNode {
                    rules: vec![
                        rule("rule1", "Bug ≥High", r#"type == "bug" && priority >= "high""#),
                        rule("rule2", "Bug", r#"type == "bug""#),
                        rule("rule3", "Story", r#"type == "story""#),
                    ],
                }),
            ),
            node("fixer", "Fixer", 404.0, 40.0, NodeKind::Agent(fixer)),
            node("repro", "Reproducer", 404.0, 280.0, NodeKind::Agent(agent("reproducer", "sonnet-5.5", 1, Some(1)))),
            node("analyst", "Analyst", 404.0, 510.0, NodeKind::Agent(agent("analyst", "sonnet-5.5", 2, None))),
            node("testw", "Test writer", 596.0, 0.0, NodeKind::Subagent(subagent("Writes one failing test that pins a bug.", 3))),
            node("logd", "Log digger", 596.0, 380.0, NodeKind::Subagent(subagent("Searches the last 24h of logs for the failing request.", 2))),
            node("spec", "spec.md", 596.0, 560.0, NodeKind::File(FileNode { workdir: "payments".into(), path: "out/{key}/spec.md".into(), mode: FileMode::Write })),
            node("review", "Review", 788.0, 230.0, NodeKind::Review(ReviewNode { send_back_to: Some(NodeId::new("fixer")), skip_if_empty: false })),
            node("pr", "Open PR", 980.0, 170.0, NodeKind::Action(ActionNode::PullRequest { workdir: "payments".into(), draft: false, push: true })),
            node(
                "jiraupd",
                "Update Jira",
                980.0,
                380.0,
                NodeKind::Action(ActionNode::UpdateIssue { status: Some(IssueStatus::InReview), comment: "PR link".into() }),
            ),
        ],
        edges: vec![
            edge("jira", PORT_OUT, "router", Route),
            edge("router", "rule1", "fixer", Route),
            edge("router", "rule2", "repro", Route),
            edge("router", "rule3", "analyst", Route),
            edge("repo", PORT_OUT, "fixer", Context),
            edge("repo", PORT_OUT, "repro", Context),
            edge("repo", PORT_OUT, "analyst", Context),
            edge("fixer", PORT_OUT, "testw", Spawn),
            edge("repro", PORT_OUT, "logd", Spawn),
            edge("fixer", PORT_OUT, "review", Route),
            edge("repro", PORT_OUT, "review", Route),
            edge("analyst", PORT_OUT, "spec", Route),
            edge("spec", PORT_OUT, "review", Route),
            edge("review", PORT_APPROVED, "pr", Route),
            edge("pr", PORT_OUT, "jiraupd", Route),
            edge("review", "send_back", "fixer", Back),
        ],
    }
}

fn small_pipeline(id: &str, name: &str, description: &str, source: SourceKind, worker: &str, variants: u32, last: NodeKind) -> GraphDef {
    GraphDef {
        schema: 1,
        id: PipelineId::new(id),
        name: name.to_string(),
        description: description.to_string(),
        limits: Limits::default(),
        workdirs: pay_triage().workdirs,
        nodes: vec![
            node("source", "Source", 20.0, 120.0, NodeKind::Source(SourceNode { source })),
            node(worker, &capitalise(worker), 220.0, 100.0, NodeKind::Agent(agent(worker, "opus-5.5", variants, None))),
            node("last", "Finish", 420.0, 120.0, last),
        ],
        edges: vec![edge("source", PORT_OUT, worker, EdgeKind::Route), edge(worker, PORT_OUT, "last", EdgeKind::Route)],
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[expect(clippy::too_many_arguments, reason = "one row of the mockup's node table")]
fn instance(node: &str, name: &str, variant: u32, status: NodeStatus, kind: SpanKind, started: i64, cost: f64, summary: &str) -> NodeInstance {
    NodeInstance { question: None,
        node: NodeId::new(node),
        name: name.to_string(),
        variant,
        attempt: 1,
        status,
        kind,
        port: None,
        session: None,
        worktree: None,
        branch: (variant > 0).then(|| format!("fix/{node}-{variant}")),
        started: Some(started),
        finished: (status == NodeStatus::Done).then_some(started + 2 * MINUTE),
        cost_usd: cost,
        summary: summary.to_string(),
        spawned_by: (kind == SpanKind::Spawned).then(|| NodeId::new(if node == "logd" { "repro" } else { "fixer" })),
    }
}

fn run(id: &str, issue: Option<&str>, title: &str, state: RunState, started: i64, cost: f64, nodes: Vec<NodeInstance>) -> Run {
    Run {
        id: RunId::new(id),
        pipeline: PipelineId::new(PAY_TRIAGE),
        version: 14,
        issue: issue.map(IssueKey::new),
        number: 1,
        title: title.to_string(),
        input: String::new(),
        state,
        test: false,
        started,
        finished: (!state.is_open()).then_some(started + 2 * MINUTE),
        cost_usd: cost,
        usage: Usage::default(),
        error: None,
        pr_url: None,
        nodes,
    }
}

pub fn fill(world: &mut World) {
    let now = world.now;
    world.pipelines = vec![
        pay_triage(),
        small_pipeline(
            "nightly-audit",
            "Nightly audit",
            "Auditor ×2 › Report. Checks log redaction and dependency licences.",
            SourceKind::Schedule,
            "auditor",
            2,
            NodeKind::Action(ActionNode::Report),
        ),
        small_pipeline(
            "release-notes",
            "Release notes",
            "Collects merged PRs and drafts notes for review.",
            SourceKind::Manual,
            "analyst",
            1,
            NodeKind::Review(ReviewNode::default()),
        ),
    ];
    let version = |pipeline: &str, number: u32, state: VersionState, note: &str, ago: i64| PipelineVersion {
        pipeline: PipelineId::new(pipeline),
        version: number,
        state,
        def_text: String::new(),
        hash: format!("{pipeline}-{number:03}"),
        note: note.to_string(),
        deployed_at: now - ago,
    };
    world.versions.insert(
        PipelineId::new(PAY_TRIAGE),
        vec![
            version(PAY_TRIAGE, 13, VersionState::Retired, "Reproducer may spawn Log digger", 9 * 24 * HOUR),
            version(PAY_TRIAGE, 14, VersionState::Live, "Fixer runs 3 variants", 2 * 24 * HOUR),
        ],
    );
    world.versions.insert(PipelineId::new("nightly-audit"), vec![version("nightly-audit", 3, VersionState::Live, "Licence check", 20 * 24 * HOUR)]);

    use NodeStatus::*;
    use SpanKind::{Agent, Spawned, Step};
    let t = now - HOUR;
    let mut r1423 = run(
        "run-1423",
        Some("PAY-1423"),
        "Double charge on retry",
        RunState::WaitingReview,
        t,
        0.84,
        vec![
            instance("jira", "Jira puller", 0, Done, Step, t, 0.0, "PAY-1423 pulled"),
            instance("router", "Router", 0, Done, Step, t + MINUTE, 0.0, "rule 1: Bug ≥High"),
            instance("fixer", "Fixer", 1, Done, Agent, t + 2 * MINUTE, 0.21, "Idempotency key on retry"),
            instance("fixer", "Fixer", 2, Done, Agent, t + 2 * MINUTE, 0.24, "Lock the row before charging"),
            instance("fixer", "Fixer", 3, Done, Agent, t + 2 * MINUTE, 0.18, "Dedupe in the webhook handler"),
            instance("testw", "Test writer", 1, Done, Spawned, t + 5 * MINUTE, 0.02, "1 failing test"),
            instance("testw", "Test writer", 2, Done, Spawned, t + 6 * MINUTE, 0.02, "1 failing test"),
            instance("review", "Review", 0, Waiting, SpanKind::Waiting, now - 40 * MINUTE, 0.0, "3 variants to review"),
        ],
    );
    r1423.nodes[1].port = Some("rule1".to_string());
    let t = now - 20 * MINUTE;
    let mut r1427 = run(
        "run-1427",
        Some("PAY-1427"),
        "Refund webhook times out",
        RunState::Running,
        t,
        0.12,
        vec![
            instance("jira", "Jira puller", 0, Done, Step, t, 0.0, "PAY-1427 pulled"),
            instance("router", "Router", 0, Done, Step, t + MINUTE, 0.0, "rule 2: Bug"),
            instance("repro", "Reproducer", 0, Running, Agent, t + 2 * MINUTE, 0.10, ""),
            instance("logd", "Log digger", 0, Running, Spawned, t + 11 * MINUTE, 0.02, ""),
        ],
    );
    r1427.number = 2;
    let t = now - 3 * HOUR - 20 * MINUTE;
    let r1431 = run(
        "run-1431",
        Some("PAY-1431"),
        "Saved cards for guest checkout",
        RunState::WaitingReview,
        t,
        0.31,
        vec![
            instance("jira", "Jira puller", 0, Done, Step, t, 0.0, "PAY-1431 pulled"),
            instance("router", "Router", 0, Done, Step, t + MINUTE, 0.0, "rule 3: Story"),
            instance("analyst", "Analyst", 1, Done, Agent, t + 2 * MINUTE, 0.15, "Tokenised guest cards"),
            instance("analyst", "Analyst", 2, Done, Agent, t + 2 * MINUTE, 0.16, "Vault-only cards"),
            instance("review", "Review", 0, Waiting, SpanKind::Waiting, now - 3 * HOUR, 0.0, "2 specs to review"),
        ],
    );
    let t = now - 26 * HOUR;
    let mut test = run(
        "run-test-42",
        None,
        "Test run #42",
        RunState::Done,
        t,
        0.84,
        vec![
            instance("jira", "Jira puller", 0, Done, Step, t, 0.0, "3 issues"),
            instance("router", "Router", 0, Done, Step, t + MINUTE, 0.0, "3 routed"),
            instance("fixer", "Fixer", 1, Done, Agent, t + 2 * MINUTE, 0.30, "PAY-1423"),
            instance("repro", "Reproducer", 0, Done, Agent, t + 2 * MINUTE, 0.20, "PAY-1427"),
            instance("analyst", "Analyst", 1, Done, Agent, t + 2 * MINUTE, 0.34, "PAY-1431"),
        ],
    );
    test.test = true;
    test.version = 0;
    let t = now - 3 * HOUR;
    let mut dat = run(
        "run-dat-58",
        Some("DAT-58"),
        "Nightly export misses refunds",
        RunState::Failed,
        t,
        0.09,
        vec![
            instance("jira", "Jira puller", 0, Done, Step, t, 0.0, "DAT-58 pulled"),
            instance("repro", "Reproducer", 0, Failed, Agent, t + MINUTE, 0.09, "The warehouse was unreachable"),
        ],
    );
    dat.error = Some("Reproducer failed: the warehouse was unreachable".into());
    world.runs = vec![r1423, r1427, r1431, test, dat];
    world.logs.insert(RunId::new("run-1427"), log_1427(now - 20 * MINUTE));
}

fn log_1427(start: i64) -> Vec<RunLogLine> {
    let lines: [(&str, LogLevel, &str); 16] = [
        ("jira", LogLevel::Info, "pulled PAY-1427 · Refund webhook times out"),
        ("router", LogLevel::Info, "rule 2 matched: Bug"),
        ("repro", LogLevel::Info, "worktree ready · fix/PAY-1427-repro"),
        ("repro", LogLevel::Tool, "read src/refunds/webhook.ts"),
        ("repro", LogLevel::Tool, "grep RefundBatcher"),
        ("repro", LogLevel::Tool, "read src/refunds/batcher.ts"),
        ("repro", LogLevel::Tool, "bash bun test src/refunds"),
        ("repro", LogLevel::Info, "refund.spec passes locally with 10 line items"),
        ("repro", LogLevel::Tool, "bash bun run seed:refund --items 420"),
        ("repro", LogLevel::Warn, "gateway call took 11.8s (timeout 10s)"),
        ("repro", LogLevel::Info, "spawned Log digger · depth 1 of 1"),
        ("logd", LogLevel::Tool, "search logs: refund.timeout last 24h"),
        ("logd", LogLevel::Info, "41 timeouts, all with more than 400 line items"),
        ("repro", LogLevel::Tool, "write out/PAY-1427/repro.md"),
        ("repro", LogLevel::Info, "minimal reproduction: 420 line items, one refund"),
        ("logd", LogLevel::Tool, "read logs/gateway-2026-11-22.jsonl"),
    ];
    lines
        .iter()
        .enumerate()
        .map(|(i, (node, level, text))| RunLogLine {
            seq: i as u64 + 1,
            at: start + i as i64 * MINUTE,
            node: Some(NodeId::new(*node)),
            variant: 0,
            level: *level,
            text: (*text).to_string(),
        })
        .collect()
}
