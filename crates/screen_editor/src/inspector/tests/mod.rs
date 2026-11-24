//! The inspector in a test window over a draft loaded by hand (and one over the fixture
//! world), inside an overlay host, driven by simulated clicks and keys.

mod fields;
mod kinds;
mod long;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px, size};
use hub::fixtures::{FixtureApi, World};
use hub::model::{
    ActionNode, AgentNode, AskRule, EdgeDef, EdgeKind, FileMode, FileNode, GRAPH_SCHEMA, GraphDef, Limits, ModelRef, NodeDef,
    NodeKind, PORT_APPROVED, PORT_ELSE, PORT_OUT, PipelineId, PipelineNode, Pos, Problem, ReviewNode, RouterNode, RouterRule,
    Severity, SourceKind, SourceNode, SubagentNode, WorkdirDef,
};
use hub::Hub;
use ui::OverlayHost;

use crate::{Draft, InspectorView, Selection};

/// The fixed clock of the fixture world.
const NOW: i64 = 1_795_000_000_000;

fn node(id: &str, name: &str, kind: NodeKind) -> NodeDef {
    NodeDef { id: id.into(), name: name.into(), pos: Pos::default(), kind }
}

fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind }
}

/// A graph with one node of every kind.
pub(crate) fn graph() -> GraphDef {
    let fixer = AgentNode {
        role: "Fix it.".into(),
        model: Some(ModelRef::new("anthropic", "opus-5.5")),
        workdir: "code".into(),
        variants: 2,
        files_out: vec!["patch.diff".into()],
        ..Default::default()
    };
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new("demo"),
        name: "Demo".into(),
        description: "A demo.".into(),
        limits: Limits::default(),
        workdirs: vec![
            WorkdirDef { id: "code".into(), name: "Code".into(), path: "/home/me/code/demo".into(), worktree: true, base_branch: "main".into(), ..Default::default() },
            WorkdirDef { id: "spare".into(), name: "Spare".into(), path: "/tmp/spare".into(), ..Default::default() },
        ],
        nodes: vec![
            node("src", "Issues", NodeKind::Source(SourceNode { source: SourceKind::Issue })),
            node(
                "triage",
                "Triage",
                NodeKind::Router(RouterNode {
                    rules: vec![
                        RouterRule { id: "bug".into(), label: "Bug".into(), when: "type == \"bug\"".into(), ask: None },
                        RouterRule { id: "ui".into(), label: "UI".into(), when: String::new(), ask: Some(AskRule { question: "Touches the UI?".into(), ..Default::default() }) },
                    ],
                }),
            ),
            node("fixer", "Fixer", NodeKind::Agent(fixer)),
            node("lead", "Lead", NodeKind::Orchestrator(AgentNode { workdir: "code".into(), variants: 1, ..Default::default() })),
            node("helper", "Helper", NodeKind::Subagent(SubagentNode { role: "Help.".into(), per_run: 2, max_steps: 10, ..Default::default() })),
            node("docs", "Docs", NodeKind::File(FileNode { workdir: "code".into(), path: "docs/**".into(), mode: FileMode::Read })),
            node("review", "Review", NodeKind::Review(ReviewNode { send_back_to: Some("fixer".into()), skip_if_empty: false })),
            node("test", "Test", NodeKind::Action(ActionNode::Command { workdir: "code".into(), command: "cargo test".into(), tests: true, parse: String::new(), timeout_secs: 0 })),
            node("pr", "Open PR", NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: false, push: true })),
            node("nested", "Nested", NodeKind::Pipeline(PipelineNode { pipeline: PipelineId::new("other") })),
        ],
        edges: vec![
            edge("src", PORT_OUT, "triage", EdgeKind::Route),
            edge("triage", "bug", "fixer", EdgeKind::Route),
            edge("triage", PORT_ELSE, "lead", EdgeKind::Route),
            edge("docs", PORT_OUT, "fixer", EdgeKind::Context),
            edge("fixer", PORT_OUT, "helper", EdgeKind::Spawn),
            edge("fixer", PORT_OUT, "review", EdgeKind::Route),
            edge("lead", PORT_OUT, "review", EdgeKind::Route),
            edge("review", PORT_APPROVED, "test", EdgeKind::Route),
            edge("test", PORT_OUT, "pr", EdgeKind::Route),
        ],
    }
}

/// An error about `node`, as the validator would word it.
pub(crate) fn problem(node: &str, message: &str) -> Problem {
    Problem { severity: Severity::Error, node: Some(node.into()), message: message.into() }
}

struct Stage {
    view: Entity<InspectorView>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().flex().child(div().flex_1()).child(div().h_full().child(self.view.clone()))
    }
}

/// An open inspector.
pub(crate) struct Open {
    pub draft: Entity<Draft>,
    pub view: Entity<InspectorView>,
    _hub: Entity<Hub>,
}

/// The inspector over `graph` with `selection`.
pub(crate) fn open(cx: &mut TestAppContext, graph: GraphDef, selection: Selection) -> (Open, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let hub = cx.update(|cx| Hub::new_fixture(api, cx));
    let draft = cx.new(|cx| {
        let mut draft = Draft::new(graph.id.clone());
        draft.load(graph, cx);
        draft.select(selection, cx);
        draft
    });
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let d = draft.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| InspectorView::new(d, window, cx));
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.simulate_resize(size(px(1200.), px(2400.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (Open { draft, view, _hub: hub }, cx)
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// An element with debug selector `selector` was drawn in the last frame.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// Presses keys, space-separated.
pub(crate) fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

/// Types `text` into the focused field.
pub(crate) fn type_text(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_input(text);
    cx.run_until_parked();
}

/// Selects `selection` the way the canvas does.
pub(crate) fn select(t: &Open, cx: &mut VisualTestContext, selection: Selection) {
    t.draft.update(cx, |draft, cx| draft.select(selection, cx));
    cx.run_until_parked();
}

/// The graph as the draft holds it now.
pub(crate) fn graph_now(t: &Open, cx: &mut VisualTestContext) -> GraphDef {
    t.draft.read_with(cx, |draft, _| draft.graph().cloned().unwrap_or_else(graph))
}

/// The node `id` of the graph now.
pub(crate) fn node_now(t: &Open, cx: &mut VisualTestContext, id: &str) -> NodeDef {
    graph_now(t, cx).node(&id.into()).cloned().unwrap_or_else(|| panic!("no node {id}"))
}

/// The text of inspector field `key`.
pub(crate) fn text(t: &Open, cx: &mut VisualTestContext, key: &str) -> String {
    t.view.read_with(cx, |view, cx| view.field_text(key, cx)).unwrap_or_else(|| panic!("no field {key}"))
}

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}
