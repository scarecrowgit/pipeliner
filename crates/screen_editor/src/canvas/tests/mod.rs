//! Tests of the canvas: what nodes show and which edges fit (plain functions), and the view in a
//! test window driven by simulated pointer and keys, over a hand-made draft and over the fixture
//! backend.

mod fixture;
mod gestures;
mod looks;

use std::collections::BTreeMap;

use gpui::{
    AppContext as _, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point,
    px, size,
};
use hub::model::{
    ActionNode, AgentNode, EdgeDef, EdgeKind, FileMode, FileNode, GRAPH_SCHEMA, GraphDef, Limits, ModelRef, NodeDef,
    NodeKind, PORT_APPROVED, PORT_OUT, PipelineId, Pos, ReviewNode, RouterNode, RouterRule, SourceNode, SpawnPolicy,
    SubagentNode, ToolAccess, ToolGrants, WorkdirDef,
};

use super::CanvasView;
use crate::Draft;

pub(super) fn node(id: &str, name: &str, x: f32, y: f32, kind: NodeKind) -> NodeDef {
    NodeDef { id: id.into(), name: name.into(), pos: Pos { x, y }, kind }
}

pub(super) fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
    EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind }
}

fn rule(id: &str, label: &str) -> RouterRule {
    RouterRule { id: id.into(), label: label.into(), when: format!("type == \"{id}\""), ask: None }
}

/// A small pipeline with one node of most kinds, laid out like the mockup's left half.
pub(super) fn graph() -> GraphDef {
    let fixer = AgentNode {
        model: Some(ModelRef::new("anthropic", "opus-5.5")),
        workdir: "code".into(),
        variants: 3,
        tools: Some(ToolGrants { access: ToolAccess::ReadOnly, allow: vec!["git.bisect".into()], deny: Vec::new() }),
        spawn: Some(SpawnPolicy { enabled: true, max_depth: 2, ..SpawnPolicy::default() }),
        ..AgentNode::default()
    };
    GraphDef {
        schema: GRAPH_SCHEMA,
        id: PipelineId::new("demo"),
        name: "Demo".into(),
        description: String::new(),
        limits: Limits::default(),
        workdirs: vec![WorkdirDef {
            id: "code".into(),
            name: "payments-api".into(),
            path: "/repo".into(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: "fix/".into(),
            setup: String::new(),
            env: BTreeMap::new(),
        }],
        nodes: vec![
            node("src", "Issues", 20.0, 170.0, NodeKind::Source(SourceNode::default())),
            node("router", "Router", 212.0, 120.0, NodeKind::Router(RouterNode { rules: vec![rule("bug", "Bug"), rule("story", "Story")] })),
            node("fixer", "Fixer", 404.0, 40.0, NodeKind::Agent(fixer)),
            node("testw", "Test writer", 596.0, 0.0, NodeKind::Subagent(SubagentNode { per_run: 3, ..SubagentNode::default() })),
            node("docs", "Repo context", 20.0, 400.0, NodeKind::File(FileNode { workdir: "code".into(), path: "docs/**".into(), mode: FileMode::Read })),
            node("review", "Review", 788.0, 230.0, NodeKind::Review(ReviewNode::default())),
            node("pr", "Open PR", 980.0, 170.0, NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: false, push: true })),
        ],
        edges: vec![
            edge("src", PORT_OUT, "router", EdgeKind::Route),
            edge("router", "bug", "fixer", EdgeKind::Route),
            edge("docs", PORT_OUT, "fixer", EdgeKind::Context),
            edge("fixer", PORT_OUT, "testw", EdgeKind::Spawn),
            edge("fixer", PORT_OUT, "review", EdgeKind::Route),
            edge("review", PORT_APPROVED, "pr", EdgeKind::Route),
        ],
    }
}

/// A canvas over a draft of `graph` in a 1200 × 800 test window.
pub(super) fn open(cx: &mut TestAppContext, graph: GraphDef) -> (Entity<CanvasView>, Entity<Draft>, &mut VisualTestContext) {
    cx.update(ui::init);
    ui::motion::set_reduced(true);
    let draft = cx.new(|_| Draft::new(PipelineId::new("demo")));
    draft.update(cx, |draft, cx| draft.load(graph, cx));
    let for_view = draft.clone();
    let (canvas, cx) = cx.add_window_view(move |window, cx| CanvasView::new(for_view, window, cx));
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    cx.run_until_parked();
    // The first frame measures the canvas; the next one fits the graph into it.
    canvas.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    (canvas, draft, cx)
}

/// The middle of a node's header, on screen.
pub(super) fn header_of(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
    let b = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"));
    point(b.origin.x + b.size.width / 2.0, b.origin.y + px(12.0))
}

/// Press at `from`, move to `to` in steps, release there.
pub(super) fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    let none = Modifiers::none();
    cx.simulate_mouse_down(from, MouseButton::Left, none);
    for step in 1..=4 {
        let t = step as f32 / 4.0;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_mouse_move(at, MouseButton::Left, none);
    }
    cx.simulate_mouse_up(to, MouseButton::Left, none);
    cx.run_until_parked();
}

/// Whether the last frame laid out `selector` (a node, a problem badge or a status pill). Use
/// it to check that something is *not* drawn: the window's debug bounds keep old entries.
pub(super) fn drawn(canvas: &Entity<CanvasView>, cx: &mut VisualTestContext, selector: &str) -> bool {
    canvas.read_with(cx, |c, _| c.drawn.iter().any(|s| s == selector))
}
