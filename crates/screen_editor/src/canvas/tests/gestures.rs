//! The canvas in a test window: selecting, moving, connecting, deleting, zooming, keys and
//! problem marks, driven by simulated pointer and keys.

use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
use hub::model::{EdgeKind, NodeKind, Problem, Severity, SubagentNode};

use super::super::interaction::Gesture;
use super::{drag, drawn, graph, header_of, node, open};
use crate::Selection;

#[gpui::test]
fn a_click_selects_a_node_an_edge_or_nothing(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    let at = header_of(cx, "node-fixer");
    cx.simulate_click(at, Modifiers::none());
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::Node("fixer".into()));

    // The middle of the review → PR edge.
    let mid = canvas.read_with(cx, |c, cx| {
        let scene = c.scene(cx).expect("loaded");
        let edge = scene.edges.iter().find(|e| e.index == 5).expect("drawn");
        c.viewport().to_screen(edge.edge.midpoint())
    });
    cx.simulate_click(point(px(mid.x), px(mid.y)), Modifiers::none());
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::Edge(5));

    cx.simulate_click(point(px(900.0), px(40.0)), Modifiers::none());
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::None);
}

#[gpui::test]
fn dragging_a_node_moves_it_on_the_grid_in_one_undo_step(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    let zoom = canvas.read_with(cx, |c, _| c.zoom());
    let from = header_of(cx, "node-fixer");
    drag(cx, from, point(from.x + px(100.0 * zoom), from.y + px(60.0 * zoom)));

    let pos = draft.read_with(cx, |d, _| d.graph().and_then(|g| g.node(&"fixer".into())).map(|n| n.pos)).expect("there");
    assert_eq!((pos.x, pos.y), (504.0, 100.0), "moved by the drag, snapped to 20-unit steps");
    assert_eq!(draft.read_with(cx, |d, _| d.can_undo()), Some("Move node"), "the drag is one step");
    draft.update(cx, |d, cx| d.undo(cx));
    let pos = draft.read_with(cx, |d, _| d.graph().and_then(|g| g.node(&"fixer".into())).map(|n| n.pos)).expect("there");
    assert_eq!((pos.x, pos.y), (404.0, 40.0), "one undo puts it back");
    assert_eq!(draft.read_with(cx, |d, _| d.can_undo()), None);
}

#[gpui::test]
fn a_drag_from_a_port_to_a_node_draws_the_edge_that_fits(cx: &mut gpui::TestAppContext) {
    let mut g = graph();
    g.nodes.push(node("helper", "Helper", 600.0, 400.0, NodeKind::Subagent(SubagentNode::default())));
    let (_canvas, draft, cx) = open(cx, g);
    let edges = |cx: &mut gpui::VisualTestContext| draft.read_with(cx, |d, _| d.graph().map(|g| g.edges.clone()).unwrap_or_default());
    let before = edges(cx).len();

    let port = cx.debug_bounds("port-router-story-out").expect("drawn").center();
    let to = header_of(cx, "node-review");
    drag(cx, port, to);
    let after = edges(cx);
    assert_eq!(after.len(), before + 1);
    let added = after.last().expect("added");
    assert_eq!((added.from.as_str(), added.port.as_str(), added.to.as_str(), added.kind), ("router", "story", "review", EdgeKind::Route));

    let port = cx.debug_bounds("port-fixer-out-out").expect("drawn").center();
    let to = header_of(cx, "node-helper");
    drag(cx, port, to);
    assert_eq!(edges(cx).last().map(|e| e.kind), Some(EdgeKind::Spawn), "an agent onto a sub-agent spawns it");

    // A read file onto a review, and anything onto a source, are refused.
    let count = edges(cx).len();
    let port = cx.debug_bounds("port-docs-out-out").expect("drawn").center();
    let to = header_of(cx, "node-review");
    drag(cx, port, to);
    let port = cx.debug_bounds("port-fixer-out-out").expect("drawn").center();
    let to = header_of(cx, "node-src");
    drag(cx, port, to);
    assert_eq!(edges(cx).len(), count, "nothing is added where the kind does not fit");
}

#[gpui::test]
fn a_pending_edge_follows_the_pointer_and_escape_drops_it(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    canvas.update(cx, |c, cx| assert!(c.start_connect(&"router".into(), "story", point(300.0, 500.0), cx)));
    let pending = |cx: &mut gpui::VisualTestContext| {
        canvas.read_with(cx, |c, _| match &c.gesture {
            Some(Gesture::Connect(connect)) => Some((connect.pointer, connect.target.clone())),
            _ => None,
        })
    };
    assert_eq!(pending(cx).map(|(p, t)| (p, t.is_none())), Some((point(300.0, 500.0), true)));
    // Over the review it would land there as a route.
    canvas.update(cx, |c, cx| {
        c.start_connect(&"router".into(), "story", point(800.0, 240.0), cx);
    });
    assert_eq!(pending(cx).and_then(|(_, t)| t), Some(("review".into(), EdgeKind::Route)));

    draft.update(cx, |d, cx| d.select(Selection::Node("fixer".into()), cx));
    cx.update(|window, cx| window.focus(&canvas.read(cx).focus_handle().clone()));
    cx.simulate_keystrokes("escape");
    assert!(pending(cx).is_none(), "escape drops the edge first");
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::Node("fixer".into()));
    cx.simulate_keystrokes("escape");
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::None, "then the selection");
}

#[gpui::test]
fn delete_removes_the_selected_node_with_its_edges_or_the_edge(cx: &mut gpui::TestAppContext) {
    let (_canvas, draft, cx) = open(cx, graph());
    let at = header_of(cx, "node-fixer");
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_keystrokes("delete");
    let (nodes, edges) = draft.read_with(cx, |d, _| {
        let g = d.graph().expect("loaded");
        (g.nodes.len(), g.edges.iter().map(|e| (e.from.to_string(), e.to.to_string())).collect::<Vec<_>>())
    });
    assert_eq!(nodes, 6);
    assert_eq!(edges, [("src".to_string(), "router".to_string()), ("review".into(), "pr".into())]);
    assert_eq!(draft.read_with(cx, |d, _| d.can_undo()), Some("Delete node"));

    draft.update(cx, |d, cx| d.select(Selection::Edge(0), cx));
    cx.simulate_keystrokes("backspace");
    assert_eq!(draft.read_with(cx, |d, _| d.graph().map(|g| g.edges.len())), Some(1));
}

#[gpui::test]
fn arrows_nudge_the_selected_node_in_one_undo_step(cx: &mut gpui::TestAppContext) {
    let (_canvas, draft, cx) = open(cx, graph());
    let at = header_of(cx, "node-pr");
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_keystrokes("right right shift-down");
    let pos = |cx: &mut gpui::VisualTestContext| {
        draft.read_with(cx, |d, _| d.graph().and_then(|g| g.node(&"pr".into())).map(|n| (n.pos.x, n.pos.y)))
    };
    assert_eq!(pos(cx), Some((1020.0, 171.0)));
    draft.update(cx, |d, cx| d.undo(cx));
    assert_eq!(pos(cx), Some((980.0, 170.0)));
}

#[gpui::test]
fn ctrl_wheel_zooms_around_the_pointer(cx: &mut gpui::TestAppContext) {
    let (canvas, _draft, cx) = open(cx, graph());
    let at = point(px(420.0), px(300.0));
    let (zoom, under) = canvas.read_with(cx, |c, _| (c.zoom(), c.viewport().to_canvas(point(420.0, 300.0))));
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(120.0))),
        modifiers: Modifiers { control: true, ..Modifiers::default() },
        touch_phase: TouchPhase::Moved,
    });
    let (after, still) = canvas.read_with(cx, |c, _| (c.zoom(), c.viewport().to_canvas(point(420.0, 300.0))));
    assert!(after > zoom, "{after} > {zoom}");
    assert!((still.x - under.x).abs() < 0.01 && (still.y - under.y).abs() < 0.01, "{still:?} stays {under:?}");

    // Without a modifier the wheel pans.
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-50.0))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    let moved = canvas.read_with(cx, |c, _| c.viewport().to_canvas(point(420.0, 300.0)));
    assert!((moved.y - still.y - 50.0 / after).abs() < 0.01, "panned by 50px");
}

#[gpui::test]
fn zoom_to_fit_shows_every_node_and_the_rest_is_not_laid_out(cx: &mut gpui::TestAppContext) {
    let (canvas, _draft, cx) = open(cx, graph());
    canvas.update(cx, |c, cx| {
        c.set_zoom(2.0, cx);
        c.center_on(&"src".into(), cx);
    });
    cx.run_until_parked();
    assert_eq!(canvas.read_with(cx, |c, _| c.zoom()), 2.0);
    assert!(drawn(&canvas, cx, "node-src"));
    assert!(!drawn(&canvas, cx, "node-pr"), "a node far out of view is not laid out");

    canvas.update(cx, |c, cx| c.zoom_to_fit(cx));
    cx.run_until_parked();
    for id in ["node-src", "node-router", "node-fixer", "node-testw", "node-docs", "node-review", "node-pr"] {
        let b = cx.debug_bounds(id).unwrap_or_else(|| panic!("{id} is drawn"));
        assert!(b.origin.x >= px(0.0) && b.origin.y >= px(0.0), "{id}: {b:?}");
        assert!(b.origin.x + b.size.width <= px(1200.0) && b.origin.y + b.size.height <= px(800.0), "{id}: {b:?}");
    }
}

#[gpui::test]
fn problems_mark_their_nodes_by_severity(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    draft.update(cx, |d, cx| {
        let problems = vec![
            Problem { severity: Severity::Error, node: Some("fixer".into()), message: "No model".into() },
            Problem { severity: Severity::Warning, node: Some("pr".into()), message: "Draft".into() },
        ];
        d.set_problems(d.revision(), problems, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("problem-fixer-error").is_some());
    assert!(cx.debug_bounds("problem-pr-warning").is_some());
    assert!(drawn(&canvas, cx, "problem-fixer-error"));
    assert!(!drawn(&canvas, cx, "problem-src-error") && !drawn(&canvas, cx, "problem-src-warning"));
}

#[gpui::test]
fn double_click_opens_the_node_and_on_the_background_does_not_select(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = opened.clone();
    cx.update(|_, cx| {
        cx.subscribe(&canvas, move |_, event: &super::super::CanvasEvent, _| seen.borrow_mut().push(event.clone())).detach();
    });
    let at = header_of(cx, "node-review");
    for click_count in [1, 2] {
        cx.simulate_event(gpui::MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent { position: at, modifiers: Modifiers::none(), button: MouseButton::Left, click_count });
    }
    assert_eq!(*opened.borrow(), [super::super::CanvasEvent::Open("review".into())]);
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::Node("review".into()));
}

#[gpui::test]
fn adding_a_node_selects_it_in_the_middle_of_the_view(cx: &mut gpui::TestAppContext) {
    let (canvas, draft, cx) = open(cx, graph());
    let id = canvas.update(cx, |c, cx| c.add_node(super::super::NewNode::Review, None, cx)).expect("added");
    cx.run_until_parked();
    assert_eq!(id.as_str(), "review-2");
    assert_eq!(draft.read_with(cx, |d, _| d.selection().clone()), Selection::Node(id));
    assert_eq!(draft.read_with(cx, |d, _| d.can_undo()), Some("Add node"));
    let b = cx.debug_bounds("node-review-2").expect("drawn");
    assert!(b.origin.x > px(0.0) && b.origin.x < px(1200.0));
    assert!(canvas.update(cx, |c, cx| c.add_node(super::super::NewNode::Pipeline, None, cx)).is_none());
}
