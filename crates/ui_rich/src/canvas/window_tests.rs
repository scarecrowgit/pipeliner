//! Every canvas element in a test window: sizes at several zooms, and no panic on bad input.

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Point, Render, TestAppContext,
    VisualTestContext, Window, div, point, prelude::*, px,
};
use ui::assets::IconName;
use ui::components::TileTone;

use super::*;

fn p(x: f32, y: f32) -> Point<f32> {
    point(x, y)
}


struct Probe(Box<dyn Fn() -> AnyElement>);

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(
            div().relative().w(px(1200.0)).h(px(800.0)).overflow_hidden().child((self.0)()),
        )
    }
}

fn show(
    cx: &mut TestAppContext,
    element: impl Fn() -> AnyElement + 'static,
) -> &mut VisualTestContext {
    ui::motion::set_reduced(true);
    let (_, cx) = cx.add_window_view(move |_, _| Probe(Box::new(element)));
    cx.run_until_parked();
    cx
}

fn node(id: &'static str, vp: Viewport, at: Point<f32>) -> NodeFrame {
    NodeFrame::new(id, vp, at)
        .icon(IconName::Router)
        .tone(TileTone::Info)
        .title("Router")
        .subtitle("first match wins")
        .row("rule 1", "Bug ≥High")
        .row("rule 2", "Bug")
        .row("else", "skip")
}

#[gpui::test]
fn a_node_is_168_units_wide_at_any_zoom(cx: &mut TestAppContext) {
    for zoom in [0.25, 0.86, 1.0, 2.0] {
        let cx = show(cx, move || {
            let vp = Viewport::new(zoom, p(16.0, 20.0));
            node("n", vp, p(100.0, 50.0)).build().debug_selector(|| "node".into()).into_any_element()
        });
        let b = cx.debug_bounds("node").expect("the node was laid out");
        assert!((f32::from(b.size.width) - 168.0 * zoom).abs() < 0.5, "zoom {zoom}: {:?}", b.size);
        assert!((f32::from(b.size.height) - node_height(3) * zoom).abs() < 0.5, "zoom {zoom}: {:?}", b.size);
        assert!((f32::from(b.origin.x) - (16.0 + 100.0 * zoom)).abs() < 0.5);
        assert!((f32::from(b.origin.y) - (20.0 + 50.0 * zoom)).abs() < 0.5);
    }
}

#[gpui::test]
fn every_node_state_renders(cx: &mut TestAppContext) {
    let vp = Viewport::new(0.86, p(16.0, 20.0));
    for state in [NodeState::Default, NodeState::Selected, NodeState::Spawned, NodeState::Soon] {
        let _ = show(cx, move || node("n", vp, p(10.0, 10.0)).state(state).into_any_element());
    }
    // A router, a sink, a source, a node without rows, empty texts, a huge title.
    let _ = show(cx, move || node("r", vp, p(0.0, 0.0)).outputs(Outputs::PerRow).into_any_element());
    let _ = show(cx, move || node("s", vp, p(0.0, 0.0)).outputs(Outputs::None).input(false).into_any_element());
    let _ = show(cx, move || NodeFrame::new("bare", vp, p(0.0, 0.0)).into_any_element());
    let _ = show(cx, move || {
        NodeFrame::new("long", vp, p(0.0, 0.0))
            .title("An extremely long node title that cannot possibly fit in a card")
            .subtitle("and a sub-line that is just as long, with out/{key}/spec.md in it")
            .row("a key that is long", "and a value that is far, far too long for the row")
            .into_any_element()
    });
}

#[gpui::test]
fn a_node_survives_a_broken_viewport(cx: &mut TestAppContext) {
    for vp in [
        Viewport { zoom: f32::NAN, origin: p(f32::NAN, f32::INFINITY) },
        Viewport { zoom: 0.0, origin: p(0.0, 0.0) },
        Viewport { zoom: -3.0, origin: p(1e30, -1e30) },
    ] {
        let _ = show(cx, move || {
            div()
                .child(node("n", vp, p(f32::NAN, 0.0)).state(NodeState::Selected))
                .child(DotGrid::new(vp))
                .child(Edges::new(vp, [Edge::new(p(0.0, 0.0), p(100.0, 100.0)).label("×3")]))
                .into_any_element()
        });
    }
}

#[gpui::test]
fn ports_have_three_tones(cx: &mut TestAppContext) {
    let vp = Viewport::new(1.0, p(0.0, 0.0));
    let cx = show(cx, move || {
        div()
            .flex()
            .gap(px(8.0))
            .child(Port::new(vp, PortTone::Default))
            .child(div().debug_selector(|| "port".into()).child(Port::new(vp, PortTone::Selected)))
            .child(Port::new(vp, PortTone::Spawn))
            .into_any_element()
    });
    let b = cx.debug_bounds("port").expect("laid out");
    assert_eq!((f32::from(b.size.width), f32::from(b.size.height)), (10.0, 10.0));
}

#[gpui::test]
fn the_edge_layer_renders_every_kind(cx: &mut TestAppContext) {
    let vp = Viewport::new(0.86, p(16.0, 20.0));
    let _ = show(cx, move || {
        Edges::new(
            vp,
            [
                Edge::new(p(188.0, 144.0), p(404.0, 64.0)).kind(EdgeKind::Selected).label("×3"),
                Edge::new(p(188.0, 168.0), p(404.0, 304.0)).label("×1"),
                Edge::new(p(188.0, 192.0), p(404.0, 534.0)).kind(EdgeKind::Dim),
                Edge::new(p(572.0, 64.0), p(596.0, 24.0)).kind(EdgeKind::Spawn),
                Edge::new(p(956.0, 254.0), p(404.0, 64.0)).kind(EdgeKind::Back).label("back"),
                Edge::vertical(p(1064.0, 252.0), p(1064.0, 380.0)),
                // Degenerate: no length, backwards, off the canvas, not a number.
                Edge::new(p(5.0, 5.0), p(5.0, 5.0)),
                Edge::new(p(500.0, 500.0), p(0.0, 0.0)),
                Edge::new(p(-1e9, -1e9), p(1e9, 1e9)),
                Edge::new(p(f32::NAN, 0.0), p(1.0, 1.0)).label("NaN"),
                Edge::new(p(0.0, f32::INFINITY), p(1.0, 1.0)),
            ],
        )
        .into_any_element()
    });
    let _ = show(cx, move || Edges::new(vp, Vec::new()).into_any_element());
}

#[gpui::test]
fn the_dot_grid_renders_at_every_zoom(cx: &mut TestAppContext) {
    for zoom in [0.25, 0.3, 0.86, 2.0] {
        let _ = show(cx, move || DotGrid::new(Viewport::new(zoom, p(-35.0, 17.0))).into_any_element());
    }
    let _ = show(cx, || {
        div().size_0().overflow_hidden().child(DotGrid::new(Viewport::default())).into_any_element()
    });
}

#[gpui::test]
fn the_legend_renders(cx: &mut TestAppContext) {
    let cx = show(cx, || div().debug_selector(|| "legend".into()).child(Legend::new()).into_any_element());
    let b = cx.debug_bounds("legend").expect("laid out");
    assert!(f32::from(b.size.width) > 100.0);
    assert_eq!(f32::from(b.size.height), 18.0);
}

#[gpui::test]
fn the_whole_canvas_composes(cx: &mut TestAppContext) {
    let vp = Viewport::new(0.86, p(16.0, 20.0));
    let _ = show(cx, move || {
        div()
            .relative()
            .size_full()
            .child(DotGrid::new(vp))
            .child(Edges::new(vp, [Edge::new(p(188.0, 144.0), p(404.0, 64.0)).label("×3")]))
            .child(node("a", vp, p(212.0, 120.0)).outputs(Outputs::PerRow))
            .child(node("b", vp, p(404.0, 40.0)).state(NodeState::Selected))
            .child(div().absolute().left(px(16.0)).bottom(px(12.0)).child(Legend::new()))
            .into_any_element()
    });
}

