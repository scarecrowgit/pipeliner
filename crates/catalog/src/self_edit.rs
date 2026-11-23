//! The app's own "Mockup → GPUI build" pipeline: an issue is triaged, the mockups change when
//! the UI does, a person approves the renders, an orchestrator has coders build it, and a pull
//! request is opened.

use std::collections::BTreeMap;
use std::path::Path;

use base::paths::Paths;
use model::graph::{PORT_APPROVED, PORT_ELSE, PORT_OUT};
use model::{
    ActionNode, AgentNode, AskRule, EdgeDef, EdgeKind, FileMode, FileNode, GraphDef, Limits, ModelRef, NodeDef,
    NodeKind, PipelineId, Pos, ReviewNode, RouterNode, RouterRule, SkillPolicy, SourceNode, SpawnPolicy,
    SubagentNode, ToolAccess, ToolGrants, WorkdirDef, GRAPH_SCHEMA,
};

/// Models of the roles in the self-editing pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfEditModels {
    /// Answers the triage question; cheap.
    pub triage: ModelRef,
    pub designer: ModelRef,
    pub orchestrator: ModelRef,
    pub coder: ModelRef,
}

impl Default for SelfEditModels {
    fn default() -> Self {
        Self {
            triage: ModelRef::parse("nr/cc/claude-haiku-5-5"),
            designer: ModelRef::parse("nr/cc/claude-opus-5-5"),
            orchestrator: ModelRef::parse("nr/cc/claude-opus-5-5"),
            coder: ModelRef::parse("nr/cc/claude-sonnet-5-5"),
        }
    }
}

const TRIAGE_QUESTION: &str = "Does this issue need changes to the UI mockups before it is coded? \
Changes to what the user sees (new or changed controls, layout, copy, states, screens) need a \
mockup change; pure logic, data, performance, build or refactoring work does not. The mockups \
are generated HTML screens in `screens-v2/` of your workspace; read one only when the issue is \
ambiguous.";

const DESIGNER: &str = "You change the UI mockups for one issue. The mockups are generated: the \
screens live in `screens-v2/*.html` and are produced by `python3 screens-v2/_build/build.py` from \
the generator sources `screens-v2/_build/*.py` (`lib.py` holds the shared shell, CSS and icons; \
the `screens_*.py` files hold the screens). Never edit the generated HTML by hand: change the \
generator sources, re-run the build, and check that the screens you meant to change did change. \
`README.md` in the mockups folder explains the format and `GRAPHITE.md` the design system. Keep \
the screens static (no scripts, no network assets), keep the existing structure, change only \
what the issue needs and stay consistent with the surrounding screens. Do not touch unrelated \
screens. The user reviews renders of the screens you change before anything is coded.";

const ORCHESTRATOR: &str = "You lead coders to implement one issue in a Rust + GPUI codebase. \
You do not edit files yourself: plan the work, hand it to coders (one focused task at a time, \
with exact files and acceptance criteria), review what they did (read the code, `git diff`), and \
send follow-up tasks until it is right. Then verify: build, run the tests and the linter as the \
project instructions say, and hand fixes to coders until everything passes. Do not commit or \
push. If earlier steps changed mockups, they are approved: read them from the mockups folder \
(offered as context) and implement them faithfully. Load the GPUI skills for guidance.";

const CODER: &str = "You implement one task given by the orchestrator in a Rust + GPUI \
codebase. Read the relevant code first, make focused changes in the existing style, and check \
your work (at least compile it as the project instructions say). Do not commit. Load the GPUI \
skills for guidance and read the approved mockups when the task refers to them.";

fn gpui_skills() -> Vec<String> {
    ["gpui-elements", "gpui-styling", "gpui-entities", "gpui-fundamentals", "gpui-patterns", "gpui-troubleshooting"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn node(id: &str, name: &str, x: f32, y: f32, kind: NodeKind) -> NodeDef {
    NodeDef { id: id.into(), name: name.into(), pos: Pos { x, y }, kind }
}

fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind }
}

/// The "Mockup → GPUI build" graph for `code_repo` and its `mockups_dir` (a git folder of
/// generated mockups). Both work in fresh worktrees; the build folder warms up with a cargo
/// build and shares a target directory under `~/.harness/cargo-target/<repo>`.
pub fn self_edit_pipeline(code_repo: &Path, mockups_dir: &Path, models: &SelfEditModels) -> GraphDef {
    let repo_name = code_repo.file_name().map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
    let target = Paths::from_env().home.join(".harness/cargo-target").join(&repo_name);
    let workdirs = vec![
        WorkdirDef {
            id: "mockups".into(),
            name: "Mockups".into(),
            path: mockups_dir.to_path_buf(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: "design/".into(),
            ..Default::default()
        },
        WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: code_repo.to_path_buf(),
            worktree: true,
            base_branch: "main".into(),
            setup: "nix-shell shell.nix --run 'cargo build' </dev/null".into(),
            env: BTreeMap::from([("CARGO_TARGET_DIR".to_string(), target.display().to_string())]),
            ..Default::default()
        },
    ];
    let full = ToolGrants { access: ToolAccess::Full, ..Default::default() };
    let nodes = vec![
        node("source", "Issues", 20.0, 170.0, NodeKind::Source(SourceNode::default())),
        node(
            "triage",
            "Triage",
            210.0,
            120.0,
            NodeKind::Router(RouterNode {
                rules: vec![RouterRule {
                    id: "mockup".into(),
                    label: "Needs a mockup".into(),
                    when: String::new(),
                    ask: Some(AskRule {
                        question: TRIAGE_QUESTION.into(),
                        model: models.triage.clone(),
                        workdir: "mockups".into(),
                        max_steps: 12,
                    }),
                }],
            }),
        ),
        node(
            "designer",
            "Mockup designer",
            400.0,
            40.0,
            NodeKind::Agent(AgentNode {
                role: DESIGNER.into(),
                model: Some(models.designer.clone()),
                workdir: "mockups".into(),
                variants: 1,
                tools: Some(full.clone()),
                skills: Some(SkillPolicy { forced: vec!["ui-ux-pro-max".into()], auto: Vec::new() }),
                max_steps: Some(60),
                ..Default::default()
            }),
        ),
        node(
            "render",
            "Render mockups",
            590.0,
            40.0,
            NodeKind::Action(ActionNode::Render { workdir: "mockups".into(), files: "screens-v2/*.html".into() }),
        ),
        node(
            "approval",
            "Your approval",
            780.0,
            40.0,
            NodeKind::Review(ReviewNode { send_back_to: Some("designer".into()), skip_if_empty: true }),
        ),
        node("merge-mockup", "Merge mockup", 970.0, 40.0, NodeKind::Action(ActionNode::Merge { workdir: "mockups".into() })),
        node(
            "mockups-file",
            "Mockups",
            970.0,
            330.0,
            NodeKind::File(FileNode { workdir: "mockups".into(), path: "screens-v2/**".into(), mode: FileMode::Read }),
        ),
        node(
            "build",
            "Build",
            1160.0,
            170.0,
            NodeKind::Orchestrator(AgentNode {
                role: ORCHESTRATOR.into(),
                model: Some(models.orchestrator.clone()),
                workdir: "code".into(),
                variants: 1,
                tools: Some(ToolGrants { access: ToolAccess::ReadAndRun, ..Default::default() }),
                skills: Some(SkillPolicy { forced: Vec::new(), auto: gpui_skills() }),
                spawn: Some(SpawnPolicy { enabled: true, ..Default::default() }),
                max_steps: Some(80),
                ..Default::default()
            }),
        ),
        node(
            "coder",
            "Coder",
            1350.0,
            330.0,
            NodeKind::Subagent(SubagentNode {
                role: CODER.into(),
                model: models.coder.clone(),
                tools: full,
                skills: SkillPolicy { forced: Vec::new(), auto: gpui_skills() },
                max_steps: 80,
                per_run: SpawnPolicy::default().per_run,
            }),
        ),
        node(
            "pull-request",
            "Pull request",
            1350.0,
            40.0,
            NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true }),
        ),
    ];
    let route = EdgeKind::Route;
    let edges = vec![
        edge("source", PORT_OUT, "triage", route),
        edge("triage", "mockup", "designer", route),
        edge("designer", PORT_OUT, "render", route),
        edge("render", PORT_OUT, "approval", route),
        edge("approval", PORT_APPROVED, "merge-mockup", route),
        edge("merge-mockup", PORT_OUT, "build", route),
        edge("triage", PORT_ELSE, "build", route),
        edge("build", PORT_OUT, "pull-request", route),
        edge("build", PORT_OUT, "coder", EdgeKind::Spawn),
        edge("mockups-file", PORT_OUT, "build", EdgeKind::Context),
        edge("mockups-file", PORT_OUT, "coder", EdgeKind::Context),
        // A back edge leaves the review through its plain out-port; the review decides when.
        edge("approval", PORT_OUT, "designer", EdgeKind::Back),
    ];
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new("mockup-gpui-build"),
        name: "Mockup → GPUI build".into(),
        description: "Triage decides whether the mockup changes; the designer edits it and you approve \
            the renders; an orchestrator has coders build it, tests it and opens a pull request."
            .into(),
        limits: Limits::default(),
        workdirs,
        nodes,
        edges,
    }
}
