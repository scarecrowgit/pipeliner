//! Starting points for "New → From a template…".

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use model::graph::{PORT_APPROVED, PORT_OUT};
use model::{
    ActionNode, AgentDef, AgentNode, EdgeDef, EdgeKind, GraphDef, Limits, NodeDef, NodeKind, Origin, PipelineId, Pos,
    ReviewNode, Runnable, RunnableKind, RunnableRef, SourceNode, ToolAccess, ToolGrants, Uses, WorkdirDef,
    GRAPH_SCHEMA,
};

use crate::write::{reference_of, slug};
use crate::{Catalog, SelfEditModels, self_edit_pipeline_with};

/// One entry of the template list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: RunnableKind,
}

/// What a template makes. Nothing is written until the caller saves it.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived: made, shown, saved
pub enum Instance {
    /// An agent, ready for `write::save_agent` (origin `Library`, unique id).
    Agent(Runnable),
    /// A pipeline, ready for `write::save_pipeline` (unique id).
    Pipeline(GraphDef),
}

/// What pipeline templates need to know about the project they are made for.
#[derive(Clone, Debug, Default)]
pub struct TemplateContext {
    /// The repo the pipeline works in; the current folder when absent.
    pub repo: Option<PathBuf>,
    /// The mockups folder of the self-editing pipeline. When absent (and for the code folder
    /// when `repo` is absent) the workdir is left without a folder for the person to set; the
    /// validator flags it, so the pipeline cannot be deployed until then.
    pub mockups_dir: Option<PathBuf>,
    /// A cargo target folder the self-editing pipeline's builds share; each worktree builds in
    /// its own when absent.
    pub cargo_target: Option<PathBuf>,
    pub models: SelfEditModels,
}

/// All templates: agents first, then pipelines.
pub fn templates() -> Vec<Template> {
    let t = |id: &str, name: &str, description: &str, kind| Template {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        kind,
    };
    vec![
        t("reviewer", "Reviewer", "Reads a diff and reports what is wrong with it; edits nothing", RunnableKind::Agent),
        t("test-writer", "Test writer", "Writes one failing test that pins a bug", RunnableKind::Agent),
        t("explorer", "Explorer", "Searches and explains code; edits nothing", RunnableKind::Agent),
        t("agent-pr", "Agent → pull request", "One agent works the issue, then a pull request is opened", RunnableKind::Pipeline),
        t(
            "agent-x3-review-pr",
            "Agent ×3 → review → pull request",
            "Three variants of the work; you pick one, it becomes a pull request",
            RunnableKind::Pipeline,
        ),
        t(
            "self-edit",
            "Mockup → GPUI build",
            "Changes the mockups, waits for your approval, builds the UI and opens a pull request",
            RunnableKind::Pipeline,
        ),
    ]
}

const REVIEWER: &str = "You review a change. Read the diff you are given, then the code around \
it, and report what is wrong or risky: bugs, missed cases, broken conventions, missing tests. \
You do not edit anything.

For each finding give the place (`path:line`), what is wrong, why it matters and what would fix \
it, most serious first. Say which findings you are sure of and which are guesses. Do not pad \
the review with style nits or praise; if the change is sound, say so in a sentence.";

const TEST_WRITER: &str = "You write one failing test that pins a bug. Read the report and the \
code until you understand how the bug happens, find how the project writes tests, and add a \
single focused test in that style that fails now for the reason in the report and would pass \
once it is fixed. Run it and confirm it fails for the right reason. Do not fix the bug and do \
not touch other code. Report the test's name, where it lives and the failure you saw.";

const EXPLORER: &str = "You explore code and answer questions about it. Search the folder, read \
what you find and explain how it works, with references as `path:line`. You change nothing. \
Follow the question to its end: name the files involved, the data that flows between them and \
anything surprising. Say plainly what you could not find.";

fn agent(catalog: &Catalog, name: &str, description: &str, role: &str, access: ToolAccess) -> Instance {
    let id = free_ref(catalog, RunnableKind::Agent, &slug(name));
    Instance::Agent(Runnable {
        reference: reference_of(RunnableKind::Agent, &id),
        kind: RunnableKind::Agent,
        name: name.into(),
        description: description.into(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef {
            role: role.into(),
            runtime: "local".into(),
            tools: ToolGrants { access, ..Default::default() },
            ..Default::default()
        }),
        path: None,
    })
}

fn free_ref(catalog: &Catalog, kind: RunnableKind, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    while catalog.get(&reference_of(kind, &id)).is_some() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Makes the template `id` with names that do not clash with anything in `catalog`.
pub fn instantiate(catalog: &Catalog, id: &str, ctx: &TemplateContext) -> Result<Instance> {
    let repo = ctx.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    match id {
        "reviewer" => Ok(agent(catalog, "Reviewer", "Reviews a diff", REVIEWER, ToolAccess::ReadOnly)),
        "test-writer" => Ok(agent(catalog, "Test writer", "Writes a failing test for a bug", TEST_WRITER, ToolAccess::Full)),
        "explorer" => Ok(agent(catalog, "Explorer", "Searches and explains code", EXPLORER, ToolAccess::ReadOnly)),
        "agent-pr" => Ok(Instance::Pipeline(agent_pr(catalog, &repo, 1))),
        "agent-x3-review-pr" => Ok(Instance::Pipeline(agent_pr(catalog, &repo, 3))),
        "self-edit" => {
            // Nothing is guessed from the environment: missing folders stay empty for the
            // person to set in the editor.
            let code = ctx.repo.clone().unwrap_or_default();
            let mockups = ctx.mockups_dir.clone().unwrap_or_default();
            let mut graph = self_edit_pipeline_with(&code, &mockups, ctx.cargo_target.as_deref(), &ctx.models);
            graph.id = PipelineId::new(free_ref(catalog, RunnableKind::Pipeline, graph.id.as_str()));
            Ok(Instance::Pipeline(graph))
        }
        other => Err(anyhow!("no template called `{other}`")),
    }
}

/// Source → agent → (review →) pull request. With `variants > 1` a review picks one.
fn agent_pr(catalog: &Catalog, repo: &std::path::Path, variants: u32) -> GraphDef {
    let (name, base) = if variants > 1 {
        ("Agent ×3 → review → pull request", "agent-review-pull-request")
    } else {
        ("Agent → pull request", "agent-pull-request")
    };
    let node = |id: &str, name: &str, x: f32, y: f32, kind| NodeDef { id: id.into(), name: name.into(), pos: Pos { x, y }, kind };
    let edge = |from: &str, port: &str, to: &str, kind| EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind };
    let mut nodes = vec![
        node("source", "Issues", 20.0, 100.0, NodeKind::Source(SourceNode::default())),
        node(
            "agent",
            "Agent",
            210.0,
            60.0,
            NodeKind::Agent(AgentNode {
                runnable: Some(RunnableRef::agent(crate::BUILD_ID)),
                workdir: "code".into(),
                variants,
                ..Default::default()
            }),
        ),
    ];
    let mut edges = vec![edge("source", PORT_OUT, "agent", EdgeKind::Route)];
    let pr_x = if variants > 1 { 590.0 } else { 400.0 };
    if variants > 1 {
        nodes.push(node(
            "review",
            "Your review",
            400.0,
            60.0,
            NodeKind::Review(ReviewNode { send_back_to: Some("agent".into()), skip_if_empty: false }),
        ));
        edges.push(edge("agent", PORT_OUT, "review", EdgeKind::Route));
        edges.push(edge("review", PORT_APPROVED, "pull-request", EdgeKind::Route));
        edges.push(edge("review", PORT_OUT, "agent", EdgeKind::Back));
    } else {
        edges.push(edge("agent", PORT_OUT, "pull-request", EdgeKind::Route));
    }
    nodes.push(node(
        "pull-request",
        "Pull request",
        pr_x,
        100.0,
        NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true }),
    ));
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new(free_ref(catalog, RunnableKind::Pipeline, base)),
        name: name.into(),
        description: String::new(),
        limits: Limits::default(),
        workdirs: vec![WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: repo.to_path_buf(),
            worktree: true,
            base_branch: "main".into(),
            ..Default::default()
        }],
        nodes,
        edges,
    }
}
