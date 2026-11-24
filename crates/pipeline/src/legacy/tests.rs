use std::path::Path;

use model::Severity;

use super::*;
use crate::validate::{AnyLibrary, validate};

/// The old app's built-in pipeline as its `legacy_default()` wrote it (instructions shortened).
const OLD_DEFAULT: &str = r#"
# Harness pipeline. Steps run top to bottom; see the editor for what each field does.

name = "Mockup → GPUI build"
description = "Triage decides whether the mockup changes; the designer edits it and you approve the renders; an orchestrator has coders build it, tests it and opens a PR."

[[workdirs]]
id = "mockups"
name = "Mockups"
path = "~/Pictures/designs/pipeliner-mockups"
worktree = true
base_branch = "main"
branch_prefix = "design/"
setup = ""

[workdirs.env]

[[workdirs]]
id = "code"
name = "Code"
path = "/work/pipeliner"
worktree = true
base_branch = "main"
branch_prefix = ""
setup = "nix-shell shell.nix --run 'cargo build' </dev/null"

[workdirs.env]
CARGO_TARGET_DIR = "/home/u/.harness/cargo-target/pipeliner"

[[steps]]
id = "triage"
name = "Triage"
kind = "decision"
workdir = "mockups"
model = "nr/cc/claude-haiku-5-5"
instructions = ""
tools = "read_only"
skills = []
skills_on_demand = []
max_steps = 12
question = "Does this issue need changes to the UI mockups before it is coded?"
on_no = "build"
files = "**/*.html"
back_to = ""
skip_if_nothing_to_review = true
command = ""
timeout_secs = 600
allow_failure = false
draft = true
push = true

[[steps]]
id = "mockup"
name = "Mockup"
kind = "agent"
workdir = "mockups"
model = "nr/cc/claude-opus-5-5"
instructions = "You change the UI mockups for one issue."
tools = "full"
skills = ["ui-ux-pro-max"]
skills_on_demand = []
max_steps = 60

[[steps]]
id = "render"
name = "Render mockups"
kind = "render"
workdir = "mockups"
files = "screens/*.html"

[[steps]]
id = "approval"
name = "Your approval"
kind = "approval"
back_to = "mockup"
skip_if_nothing_to_review = true

[[steps]]
id = "merge-mockup"
name = "Merge mockup"
kind = "merge"
workdir = "mockups"

[[steps]]
id = "build"
name = "Build"
kind = "agent"
workdir = "code"
model = "nr/cc/claude-opus-5-5"
instructions = "You lead coders to implement one issue in a Rust + GPUI codebase."
tools = "read_and_run"
skills = []
skills_on_demand = ["gpui-elements", "gpui-styling"]
max_steps = 80

[steps.delegate]
name = "Coder"
model = "nr/cc/claude-sonnet-5-5"
tools = "full"
skills = []
skills_on_demand = ["gpui-elements", "gpui-styling"]
max_steps = 80
instructions = "You implement one task given by the orchestrator."

[[steps]]
id = "pull-request"
name = "Pull request"
kind = "pull_request"
workdir = "code"
draft = true
push = false
"#;

fn paths() -> Paths {
    Paths::under(Path::new("/root"))
}

fn convert_text(text: &str) -> Result<GraphDef> {
    convert_file(None, text, &paths())
}

fn edges(graph: &GraphDef) -> Vec<(String, String, String, EdgeKind)> {
    graph.edges.iter().map(|e| (e.from.to_string(), e.port.clone(), e.to.to_string(), e.kind)).collect()
}

fn e(from: &str, port: &str, to: &str, kind: EdgeKind) -> (String, String, String, EdgeKind) {
    (from.into(), port.into(), to.into(), kind)
}

#[test]
fn the_old_default_converts_into_a_graph_without_problems() {
    let graph = convert_text(OLD_DEFAULT).unwrap();
    assert_eq!(validate(&graph, &AnyLibrary), []);
    assert_eq!(graph.id.as_str(), "mockup-gpui-build");
    assert_eq!(graph.name, "Mockup → GPUI build");

    let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        ["source", "triage", "mockup", "render", "approval", "merge-mockup", "build", "build-coder", "pull-request"]
    );
    use EdgeKind::{Back, Route, Spawn};
    assert_eq!(
        edges(&graph),
        [
            e("source", "out", "triage", Route),
            e("triage", "else", "build", Route),
            e("triage", "yes", "mockup", Route),
            e("mockup", "out", "render", Route),
            e("render", "out", "approval", Route),
            e("approval", "out", "mockup", Back),
            e("approval", "approved", "merge-mockup", Route),
            e("merge-mockup", "out", "build", Route),
            e("build", "out", "build-coder", Spawn),
            e("build", "out", "pull-request", Route),
        ]
    );

    let xs: Vec<f32> = graph.nodes.iter().filter(|n| n.pos.y == LINE_Y).map(|n| n.pos.x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "laid out left to right: {xs:?}");

    let kind = |id: &str| &graph.node(&id.into()).unwrap().kind;
    let NodeKind::Router(triage) = kind("triage") else { panic!() };
    let ask = triage.rules[0].ask.as_ref().unwrap();
    assert_eq!((ask.workdir.as_str(), ask.max_steps), ("mockups", 12));
    assert_eq!(ask.model, ModelRef::parse("nr/cc/claude-haiku-5-5"));
    let NodeKind::Agent(mockup) = kind("mockup") else { panic!() };
    assert_eq!(mockup.skills.as_ref().unwrap().forced, ["ui-ux-pro-max"]);
    assert_eq!(mockup.tools.as_ref().unwrap().access, ToolAccess::Full);
    let NodeKind::Orchestrator(build) = kind("build") else { panic!() };
    assert_eq!(build.tools.as_ref().unwrap().access, ToolAccess::ReadAndRun);
    assert!(build.spawn.as_ref().unwrap().enabled);
    let NodeKind::Subagent(coder) = kind("build-coder") else { panic!() };
    assert_eq!((coder.max_steps, coder.model.short()), (80, "claude-sonnet-5-5"));
    let NodeKind::Review(review) = kind("approval") else { panic!() };
    assert_eq!(review.send_back_to, Some("mockup".into()));
    let NodeKind::Action(ActionNode::PullRequest { push, draft, .. }) = kind("pull-request") else { panic!() };
    assert_eq!((*draft, *push), (true, false));

    assert_eq!(graph.workdirs[0].path, Path::new("/root/home/Pictures/designs/pipeliner-mockups"));
    assert_eq!(graph.workdirs[1].env["CARGO_TARGET_DIR"], "/home/u/.harness/cargo-target/pipeliner");
}

#[test]
fn the_file_stem_wins_over_the_name() {
    let graph = convert_file(Some("mine"), OLD_DEFAULT, &paths()).unwrap();
    assert_eq!(graph.id.as_str(), "mine");
}

#[test]
fn commands_reports_and_decisions_that_end() {
    let text = r#"
name = "Checks"
[[workdirs]]
id = "code"
name = "Code"
path = "/work/app"

[[steps]]
id = "gate"
kind = "decision"
question = "Is it worth checking?"
on_no = "end"

[[steps]]
id = "tests"
name = "Tests"
kind = "command"
workdir = "code"
command = "cargo test"
timeout_secs = 120
allow_failure = true

[[steps]]
kind = "report"
"#;
    let graph = convert_text(text).unwrap();
    let problems = validate(&graph, &AnyLibrary);
    assert!(problems.iter().all(|p| p.severity == Severity::Warning), "{problems:#?}");
    assert!(problems.iter().any(|p| p.message.contains("no else route")), "{problems:#?}");
    assert_eq!(graph.id.as_str(), "checks");
    assert_eq!(graph.nodes[1].name, "Decision");
    assert_eq!(graph.nodes[3].id.as_str(), "report");
    let NodeKind::Action(ActionNode::Command { timeout_secs, command, .. }) = &graph.nodes[2].kind else { panic!() };
    assert_eq!((command.as_str(), *timeout_secs), ("cargo test", 120));
    assert!(!graph.edges.iter().any(|e| e.port == PORT_ELSE));
}

#[test]
fn ids_stay_unique() {
    let text = r#"
name = "Odd"
[[steps]]
id = "source"
instructions = "Do it."
[[steps]]
instructions = "Again."
[[steps]]
instructions = "Once more."
"#;
    let graph = convert_text(text).unwrap();
    let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["source-2", "source", "agent", "agent-2"]);
    assert!(validate(&graph, &AnyLibrary).iter().all(|p| p.severity == Severity::Warning));
}

#[test]
fn jumps_graphs_cannot_express_are_errors() {
    let cases = [
        (
            "[[steps]]\nid = \"a\"\ninstructions = \"x\"\n[[steps]]\nid = \"d\"\nkind = \"decision\"\non_no = \"a\"\n",
            "jumps back to a",
        ),
        ("[[steps]]\nid = \"d\"\nkind = \"decision\"\non_no = \"ghost\"\n", "missing step “ghost”"),
        ("[[steps]]\nid = \"r\"\nkind = \"approval\"\nback_to = \"ghost\"\n", "sends back to a missing step"),
        ("[[steps]]\nid = \"r\"\nkind = \"approval\"\nback_to = \"r\"\n", "not before it"),
    ];
    for (text, needle) in cases {
        let err = convert_text(text).expect_err(text).to_string();
        assert!(err.contains(needle), "{text}: {err}");
    }
    assert!(convert_text("steps = 3").is_err());
}
