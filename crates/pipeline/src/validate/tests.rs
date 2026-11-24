use std::path::Path;

use model::graph::{PORT_APPROVED, PORT_ELSE, PORT_OUT};
use model::{
    ActionNode, AgentNode, AskRule, EdgeDef, EdgeKind, FileNode, GRAPH_SCHEMA, GraphDef, Limits, ModelRef, NodeDef,
    NodeKind, PipelineId, PipelineNode, Pos, ReviewNode, RouterNode, RouterRule, RunnableRef, Severity, SourceKind,
    SourceNode, SubagentNode, WorkdirDef,
};

use super::*;

use Severity::{Error, Warning};

fn node(id: &str, kind: NodeKind) -> NodeDef {
    let name = id[..1].to_uppercase() + &id[1..];
    NodeDef { id: id.into(), name, pos: Pos::default(), kind }
}

fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind }
}

fn route(from: &str, to: &str) -> EdgeDef {
    edge(from, PORT_OUT, to, EdgeKind::Route)
}

fn agent(role: &str) -> NodeKind {
    NodeKind::Agent(AgentNode { role: role.into(), workdir: "code".into(), variants: 1, ..Default::default() })
}

fn review(back_to: &str) -> NodeKind {
    NodeKind::Review(ReviewNode { send_back_to: Some(back_to.into()), skip_if_empty: false })
}

fn command(cmd: &str) -> NodeKind {
    NodeKind::Action(ActionNode::Command {
        workdir: "code".into(),
        command: cmd.into(),
        tests: true,
        parse: String::new(),
        timeout_secs: 0,
    })
}

fn pull_request() -> NodeKind {
    NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true })
}

fn rule(id: &str, when: &str) -> RouterRule {
    RouterRule { id: id.into(), label: id.into(), when: when.into(), ask: None }
}

/// Source → Fix → Check (sends back to Fix) → Pr. Has no problems.
fn base() -> GraphDef {
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new("t"),
        name: "Test".into(),
        description: String::new(),
        limits: Limits::default(),
        workdirs: vec![WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: "/repo".into(),
            worktree: true,
            ..Default::default()
        }],
        nodes: vec![
            node("src", NodeKind::Source(SourceNode::default())),
            node("fix", agent("Fix the bug.")),
            node("check", review("fix")),
            node("pr", pull_request()),
        ],
        edges: vec![
            route("src", "fix"),
            route("fix", "check"),
            edge("check", PORT_APPROVED, "pr", EdgeKind::Route),
            edge("check", PORT_OUT, "fix", EdgeKind::Back),
        ],
    }
}

fn kind_mut<'g>(graph: &'g mut GraphDef, id: &str) -> &'g mut NodeKind {
    let node = graph.nodes.iter_mut().find(|n| n.id.as_str() == id);
    &mut node.unwrap_or_else(|| panic!("no node {id}")).kind
}

fn agent_mut<'g>(graph: &'g mut GraphDef, id: &str) -> &'g mut AgentNode {
    match kind_mut(graph, id) {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => agent,
        other => panic!("{id} is {other:?}"),
    }
}

#[track_caller]
fn expect(graph: &GraphDef, severity: Severity, node: Option<&str>, needle: &str) {
    expect_in(&validate(graph, &AnyLibrary), severity, node, needle);
}

#[track_caller]
fn expect_in(problems: &[Problem], severity: Severity, node: Option<&str>, needle: &str) {
    let found = problems
        .iter()
        .any(|p| p.severity == severity && p.node.as_ref().map(|n| n.as_str()) == node && p.message.contains(needle));
    assert!(found, "expected {severity:?} at {node:?} containing {needle:?}, got {problems:#?}");
}

#[test]
fn the_base_graph_is_clean() {
    assert_eq!(validate(&base(), &AnyLibrary), []);
}

#[test]
fn the_built_in_graph_has_no_problems() {
    let graph = catalog::self_edit_pipeline(Path::new("/work/pipeliner"), Path::new("/work/mockups"), &Default::default());
    assert_eq!(validate(&graph, &AnyLibrary), []);
}

#[test]
fn sources() {
    let mut g = base();
    g.nodes.remove(0);
    g.edges.remove(0);
    expect(&g, Error, None, "Add a source");

    let mut g = base();
    g.nodes.push(node("src2", NodeKind::Source(SourceNode { source: SourceKind::Manual })));
    g.edges.push(route("src2", "fix"));
    expect(&g, Error, Some("src2"), "Only one source");

    let mut g = base();
    *kind_mut(&mut g, "src") = NodeKind::Source(SourceNode { source: SourceKind::Jira });
    expect(&g, Error, Some("src"), "Jira sources are coming soon");
    *kind_mut(&mut g, "src") = NodeKind::Source(SourceNode { source: SourceKind::Sentry });
    expect(&g, Error, Some("src"), "Sentry sources are coming soon");

    let mut g = base();
    g.edges.remove(0);
    expect(&g, Error, Some("src"), "Connect the source");

    let mut g = base();
    g.edges.push(route("fix", "src"));
    expect(&g, Error, Some("fix"), "Nothing can route into the source");
}

#[test]
fn node_ids() {
    let mut g = base();
    g.nodes.push(node("fix", command("make")));
    expect(&g, Error, Some("fix"), "Another node already has the id “fix”");
    let mut g = base();
    g.nodes.push(NodeDef { id: "".into(), name: "Lint".into(), pos: Pos::default(), kind: command("make") });
    expect(&g, Error, Some(""), "Lint has no id");
}

#[test]
fn edges_and_ports() {
    let mut g = base();
    g.edges.push(route("fix", "ghost"));
    expect(&g, Error, Some("fix"), "Fix connects to a missing node “ghost”");

    let mut g = base();
    g.edges.push(route("ghost", "fix"));
    expect(&g, Error, Some("fix"), "An edge starts at a missing node “ghost”");

    let mut g = base();
    g.edges.push(edge("fix", PORT_APPROVED, "pr", EdgeKind::Route));
    expect(&g, Error, Some("fix"), "Fix has no port “approved”");

    let mut g = base();
    g.edges[2].port = PORT_OUT.into();
    expect(&g, Error, Some("check"), "A review routes on through “approved”");

    let mut g = base();
    g.edges.push(route("fix", "check"));
    expect(&g, Warning, Some("fix"), "drawn twice");
}

#[test]
fn router_rules() {
    let mut g = base();
    g.nodes.push(node(
        "triage",
        NodeKind::Router(RouterNode {
            rules: vec![
                rule("bug", r#"type == "bug""#),
                rule("lost", r#"type == "bug""#),
                rule("empty", ""),
                RouterRule {
                    ask: Some(AskRule { question: "UI?".into(), ..Default::default() }),
                    ..rule("both", r#"type == "bug""#)
                },
                rule("broken", r#"type == bug"#),
                rule("bug", r#"priority >= "high""#),
                rule("else", r#"type == "docs""#),
                RouterRule {
                    ask: Some(AskRule { question: " ".into(), workdir: "nowhere".into(), ..Default::default() }),
                    ..rule("ask", "")
                },
            ],
        }),
    ));
    g.edges[0] = route("src", "triage");
    for port in ["bug", "empty", "both", "broken", "else", "ask"] {
        g.edges.push(edge("triage", port, "fix", EdgeKind::Route));
    }
    let problems = validate(&g, &AnyLibrary);
    let at = Some("triage");
    expect_in(&problems, Error, at, "Rule “lost” leads nowhere");
    expect_in(&problems, Error, at, "Rule “empty” has no condition");
    expect_in(&problems, Error, at, "Rule “both” has both a condition and a question");
    expect_in(&problems, Error, at, "Rule “broken”: Put the value in quotes");
    expect_in(&problems, Error, at, "Two rules share the id “bug”");
    expect_in(&problems, Error, at, "A rule cannot be called “else”");
    expect_in(&problems, Error, at, "Rule “ask” asks an empty question");
    expect_in(&problems, Error, at, "Rule “ask” reads an unknown workdir “nowhere”");

    let mut g = base();
    g.nodes.push(node("triage", NodeKind::Router(RouterNode { rules: vec![rule("bug", r#"type == "bug""#)] })));
    g.edges[0] = route("src", "triage");
    g.edges.push(edge("triage", "bug", "fix", EdgeKind::Route));
    expect(&g, Warning, Some("triage"), "Triage has no else route");

    g.nodes.push(node("other", NodeKind::Router(RouterNode::default())));
    g.edges.push(edge("triage", PORT_ELSE, "other", EdgeKind::Route));
    g.edges.push(edge("other", PORT_ELSE, "fix", EdgeKind::Route));
    expect(&g, Warning, Some("other"), "Other has no rules");
}

#[test]
fn unreachable_nodes_and_dead_ends() {
    let mut g = base();
    g.nodes.push(node("lone", agent("Idle.")));
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Warning, Some("lone"), "Nothing routes to Lone");
    expect_in(&problems, Warning, Some("lone"), "Nothing comes after Lone");
}

#[test]
fn only_back_edges_close_a_loop() {
    let mut g = base();
    g.edges.push(route("pr", "fix"));
    expect(&g, Error, Some("fix"), "Routes loop through Fix, Check, Pr");

    let mut g = base();
    g.edges.push(route("fix", "fix"));
    expect(&g, Error, Some("fix"), "Routes loop through Fix");
}

#[test]
fn back_edges_and_send_back() {
    let mut g = base();
    g.edges.push(edge("fix", PORT_OUT, "src", EdgeKind::Back));
    expect(&g, Error, Some("fix"), "Only a review can send work back");

    let mut g = base();
    g.edges[3] = edge("check", PORT_OUT, "pr", EdgeKind::Back);
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Error, Some("check"), "The back edge of Check must point upstream");
    expect_in(&problems, Warning, Some("check"), "Send back restarts Fix, but the back edge points elsewhere");

    let mut g = base();
    *kind_mut(&mut g, "check") = review("pr");
    expect(&g, Error, Some("check"), "Send back must restart a node before the review; Pr is not upstream");
    *kind_mut(&mut g, "check") = review("ghost");
    expect(&g, Error, Some("check"), "Send back goes to a missing node “ghost”");
}

#[test]
fn attached_nodes() {
    let file = NodeKind::File(FileNode { workdir: "code".into(), path: "docs/**".into(), ..Default::default() });
    let sub = NodeKind::Subagent(SubagentNode { role: "Write tests.".into(), ..Default::default() });

    let mut g = base();
    g.nodes.push(node("docs", file.clone()));
    g.nodes.push(node("tester", sub.clone()));
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Warning, Some("docs"), "Docs is not attached to any agent");
    expect_in(&problems, Warning, Some("tester"), "Tester is not attached to any agent");

    g.edges.push(edge("docs", PORT_OUT, "check", EdgeKind::Context));
    g.edges.push(edge("check", PORT_OUT, "tester", EdgeKind::Spawn));
    g.edges.push(route("fix", "tester"));
    g.edges.push(route("docs", "fix"));
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Error, Some("docs"), "Context edges go from a file to an agent");
    expect_in(&problems, Error, Some("check"), "Spawn edges go from an agent or orchestrator to a sub-agent");
    expect_in(&problems, Error, Some("fix"), "Tester is a sub-agent: attach it with a spawn edge");
    expect_in(&problems, Error, Some("docs"), "Docs is a file: attach it with a context edge");

    let mut g = base();
    g.nodes.push(node("docs", file));
    g.nodes.push(node("tester", NodeKind::Subagent(SubagentNode { per_run: 0, ..Default::default() })));
    g.edges.push(edge("docs", PORT_OUT, "fix", EdgeKind::Context));
    g.edges.push(edge("fix", PORT_OUT, "tester", EdgeKind::Spawn));
    let problems = validate(&g, &AnyLibrary);
    assert!(!problems.iter().any(|p| p.message.contains("not attached")), "{problems:#?}");
    expect_in(&problems, Error, Some("tester"), "Tester has no role");
    expect_in(&problems, Warning, Some("tester"), "0 times per run");
    expect_in(&problems, Warning, Some("fix"), "Fix has sub-agents but may not spawn them");
}

#[test]
fn variant_scopes_close_with_a_review() {
    let mut g = base();
    agent_mut(&mut g, "fix").variants = 3;
    assert_eq!(validate(&g, &AnyLibrary), []);

    // Fix → Pr directly: the variants reach the end.
    g.edges[1] = route("fix", "pr");
    g.edges.truncate(2);
    *kind_mut(&mut g, "check") = review("fix");
    expect(&g, Error, Some("fix"), "The variants of Fix reach the end at Pr without a review");

    // A branch without variants joins inside the scope.
    let mut g = base();
    agent_mut(&mut g, "fix").variants = 3;
    g.nodes.push(node("tests", command("cargo test")));
    g.edges[1] = route("fix", "tests");
    g.edges.push(route("tests", "check"));
    g.edges.push(route("src", "tests"));
    expect(&g, Error, Some("tests"), "Tests joins the variants of Fix with another branch");
}

#[test]
fn agent_fields() {
    let mut g = base();
    agent_mut(&mut g, "fix").role.clear();
    expect(&g, Error, Some("fix"), "Fix needs an agent from your library or a role");

    let mut g = base();
    agent_mut(&mut g, "fix").variants = 0;
    expect(&g, Error, Some("fix"), "Fix has 0 variants");
    agent_mut(&mut g, "fix").variants = MAX_VARIANTS + 1;
    expect(&g, Error, Some("fix"), "at most 8");

    let mut g = base();
    agent_mut(&mut g, "fix").variants = 2;
    g.workdirs[0].worktree = false;
    expect(&g, Error, Some("fix"), "Variants of Fix need a workdir with worktrees");

    let mut g = base();
    agent_mut(&mut g, "fix").workdir = "nowhere".into();
    agent_mut(&mut g, "fix").budget_usd = Some(0.0);
    agent_mut(&mut g, "fix").max_steps = Some(0);
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Error, Some("fix"), "Fix uses an unknown workdir “nowhere”");
    expect_in(&problems, Error, Some("fix"), "The budget of Fix must be more than $0.00");
    expect_in(&problems, Error, Some("fix"), "Fix may take 0 steps");
}

/// Knows one agent and one model.
struct Strict;

impl Library for Strict {
    fn has_runnable(&self, reference: &RunnableRef) -> bool {
        reference.as_str() == "agent:fixer"
    }

    fn has_model(&self, model: &ModelRef) -> bool {
        model.model == "known"
    }

    fn has_pipeline(&self, _: &PipelineId) -> bool {
        false
    }
}

#[test]
fn library_lookups() {
    let mut g = base();
    agent_mut(&mut g, "fix").runnable = Some(RunnableRef::agent("fixer"));
    agent_mut(&mut g, "fix").model = Some(ModelRef::new("p", "known"));
    assert_eq!(validate(&g, &Strict), []);

    agent_mut(&mut g, "fix").runnable = Some(RunnableRef::agent("ghost"));
    agent_mut(&mut g, "fix").model = Some(ModelRef::new("p", "other"));
    let problems = validate(&g, &Strict);
    expect_in(&problems, Error, Some("fix"), "Unknown agent “agent:ghost”");
    expect_in(&problems, Error, Some("fix"), "Unknown model “p/other”");

    agent_mut(&mut g, "fix").runnable = Some(RunnableRef::pipeline("fixer"));
    expect(&g, Error, Some("fix"), "Unknown agent “pipeline:fixer”");
}

#[test]
fn actions_and_workdirs() {
    let mut g = base();
    g.workdirs[0].worktree = false;
    expect(&g, Error, Some("pr"), "A pull request needs a workdir with worktrees; Code is used as is");

    let mut g = base();
    g.nodes.push(node("tests", command("  ")));
    g.nodes.push(node("merge", NodeKind::Action(ActionNode::Merge { workdir: "nowhere".into() })));
    g.edges[2] = edge("check", PORT_APPROVED, "tests", EdgeKind::Route);
    g.edges.push(route("tests", "merge"));
    g.edges.push(route("merge", "pr"));
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Error, Some("tests"), "Tests has no command");
    expect_in(&problems, Error, Some("merge"), "Merge uses an unknown workdir “nowhere”");

    let mut g = base();
    g.workdirs[0].path = "".into();
    expect(&g, Error, None, "Workdir Code has no folder");
    g.workdirs.push(g.workdirs[0].clone());
    expect(&g, Error, None, "Two workdirs share the id “code”");
}

#[test]
fn nested_pipelines_are_not_available() {
    let mut g = base();
    g.nodes.push(node("sub", NodeKind::Pipeline(PipelineNode { pipeline: "other".into() })));
    g.edges.push(route("pr", "sub"));
    expect(&g, Error, Some("sub"), "Nested pipelines are not available yet");
}

#[test]
fn reviews_need_something_to_review() {
    let mut g = base();
    g.edges[0] = route("src", "check");
    *kind_mut(&mut g, "check") = NodeKind::Review(ReviewNode::default());
    expect(&g, Warning, Some("check"), "Check has nothing to review");
}

#[test]
fn limits() {
    let mut g = base();
    g.limits = Limits { max_parallel: 0, budget_usd: Some(0.0), max_loops: 0 };
    let problems = validate(&g, &AnyLibrary);
    expect_in(&problems, Error, None, "Max parallel is 0");
    expect_in(&problems, Error, None, "The run budget must be more than $0.00");
    expect_in(&problems, Warning, None, "Max loops is 0");
}

#[test]
fn errors_come_first_then_file_order() {
    let mut g = base();
    g.limits.max_parallel = 0;
    g.nodes.insert(1, node("lone", agent("Idle.")));
    g.nodes.push(node("sub", NodeKind::Pipeline(PipelineNode { pipeline: "other".into() })));
    g.edges.push(route("pr", "sub"));
    agent_mut(&mut g, "fix").variants = 0;
    let problems = validate(&g, &AnyLibrary);
    let order: Vec<(Severity, Option<&str>)> =
        problems.iter().map(|p| (p.severity, p.node.as_ref().map(|n| n.as_str()))).collect();
    let mut sorted = order.clone();
    sorted.sort_by_key(|(s, n)| {
        let pos = n.map_or(0, |n| 1 + g.nodes.iter().position(|x| x.id.as_str() == n).unwrap_or(0));
        (*s == Warning, pos)
    });
    assert_eq!(order, sorted);
    assert_eq!(order.first(), Some(&(Error, None)));
    assert!(order.contains(&(Error, Some("fix"))) && order.contains(&(Error, Some("sub"))));
    assert_eq!(order.last().map(|(s, _)| *s), Some(Warning));
    assert!(deployable(&validate(&base(), &AnyLibrary)));
    assert!(!deployable(&validate(&g, &AnyLibrary)));
}
