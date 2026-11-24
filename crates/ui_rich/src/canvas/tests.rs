//! Canvas geometry, viewport maths, edges and node shapes: pure tests, no window.

use gpui::{Bounds, Point, point, px, size};

use super::geometry::{FLATTEN_CHECK, polyline_for_tests};
use super::*;

fn p(x: f32, y: f32) -> Point<f32> {
    point(x, y)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn near_pt(a: Point<f32>, b: Point<f32>) -> bool {
    near(a.x, b.x) && near(a.y, b.y)
}

// ---------------------------------------------------------------------------- viewport

#[test]
fn canvas_and_screen_convert_both_ways() {
    let vp = Viewport::new(0.86, p(16.0, 20.0));
    assert!(near_pt(vp.to_screen(p(0.0, 0.0)), p(16.0, 20.0)));
    assert!(near_pt(vp.to_screen(p(100.0, 50.0)), p(16.0 + 86.0, 20.0 + 43.0)));
    for c in [p(0.0, 0.0), p(-300.0, 25.0), p(1160.0, 760.0)] {
        assert!(near_pt(vp.to_canvas(vp.to_screen(c)), c));
    }
}

#[test]
fn zooming_at_a_point_keeps_it_under_the_cursor() {
    let cursor = p(300.0, 200.0);
    for start in [Viewport::default(), Viewport::new(0.86, p(16.0, 20.0)), Viewport::new(1.7, p(-400.0, 90.0))] {
        for factor in [1.1, 0.9, 1.5, 0.5, 3.0, 0.1] {
            let mut vp = start;
            let under = vp.to_canvas(cursor);
            vp.zoom_at(cursor, factor);
            assert!(near_pt(vp.to_screen(under), cursor), "{start:?} x{factor}: {vp:?}");
        }
    }
}

#[test]
fn zoom_is_clamped_to_a_quarter_and_double() {
    let mut vp = Viewport::default();
    vp.zoom_at(p(0.0, 0.0), 100.0);
    assert_eq!(vp.zoom, MAX_ZOOM);
    vp.zoom_at(p(0.0, 0.0), 0.0001);
    assert_eq!(vp.zoom, MIN_ZOOM);
    assert_eq!(Viewport::new(9.0, p(0.0, 0.0)).zoom, 2.0);
    assert_eq!(Viewport::new(0.0, p(0.0, 0.0)).zoom, 0.25);
    assert_eq!(Viewport::new(f32::NAN, p(0.0, 0.0)).zoom, 1.0);
}

#[test]
fn zooming_past_the_limit_still_keeps_the_point() {
    let mut vp = Viewport::new(1.9, p(10.0, 10.0));
    let cursor = p(120.0, 80.0);
    let under = vp.to_canvas(cursor);
    vp.zoom_at(cursor, 4.0);
    assert_eq!(vp.zoom, MAX_ZOOM);
    assert!(near_pt(vp.to_screen(under), cursor));
}

#[test]
fn a_bad_factor_does_nothing() {
    let start = Viewport::new(0.86, p(16.0, 20.0));
    for factor in [f32::NAN, 0.0, -2.0, f32::INFINITY] {
        let mut vp = start;
        vp.zoom_at(p(50.0, 50.0), factor);
        assert_eq!(vp, start);
    }
}

#[test]
fn percent_is_the_zoom_as_the_toolbar_shows_it() {
    assert_eq!(Viewport::new(0.86, p(0.0, 0.0)).percent(), 86);
    assert_eq!(Viewport::new(0.76, p(0.0, 0.0)).percent(), 76);
    assert_eq!(Viewport::default().percent(), 100);
    assert_eq!(Viewport { zoom: 50.0, origin: p(0.0, 0.0) }.percent(), 200);
}

#[test]
fn pan_moves_the_origin() {
    let mut vp = Viewport::default();
    vp.pan(p(30.0, -10.0));
    vp.pan(p(f32::NAN, 5.0));
    assert_eq!(vp.origin, p(30.0, -10.0));
}

#[test]
fn fit_centres_the_content_with_padding() {
    let content = Bounds::new(p(20.0, 0.0), size(1000.0, 500.0));
    let view = size(800.0, 600.0);
    let vp = Viewport::fit(content, view, 20.0);
    assert!(near(vp.zoom, 0.76), "{}", vp.zoom);
    let centre = vp.to_screen(p(20.0 + 500.0, 250.0));
    assert!(near_pt(centre, p(400.0, 300.0)));
    // All of the content is inside the padding.
    let tl = vp.to_screen(p(20.0, 0.0));
    let br = vp.to_screen(p(1020.0, 500.0));
    assert!(tl.x >= 20.0 - 1e-3 && br.x <= 780.0 + 1e-3 && tl.y >= 20.0 - 1e-3 && br.y <= 580.0 + 1e-3);
}

#[test]
fn fit_does_not_blow_small_content_up_past_the_range() {
    let vp = Viewport::fit(Bounds::new(p(0.0, 0.0), size(100.0, 50.0)), size(1000.0, 800.0), 24.0);
    assert_eq!(vp.zoom, MAX_ZOOM);
    let vp = Viewport::fit(Bounds::new(p(0.0, 0.0), size(100_000.0, 50.0)), size(1000.0, 800.0), 24.0);
    assert_eq!(vp.zoom, MIN_ZOOM);
}

#[test]
fn fit_of_nothing_or_into_nothing_is_finite() {
    let empty = Viewport::fit(Bounds::new(p(40.0, 40.0), size(0.0, 0.0)), size(800.0, 600.0), 24.0);
    assert_eq!(empty.zoom, 1.0);
    assert!(near_pt(empty.to_screen(p(40.0, 40.0)), p(400.0, 300.0)));
    let tiny = Viewport::fit(Bounds::new(p(0.0, 0.0), size(500.0, 500.0)), size(0.0, 0.0), 24.0);
    assert_eq!(tiny.zoom, MIN_ZOOM);
    assert!(tiny.origin.x.is_finite() && tiny.origin.y.is_finite());
    let nan = Viewport::fit(Bounds::new(p(f32::NAN, 0.0), size(f32::NAN, 10.0)), size(100.0, 100.0), f32::NAN);
    assert!(nan.zoom.is_finite() && nan.origin.x.is_finite());
}

#[test]
fn pointer_positions_become_canvas_units() {
    let vp = Viewport::new(0.5, p(10.0, 20.0));
    // The canvas element sits at (200, 100) in the window; the pointer is at (260, 160).
    let c = vp.pointer_to_canvas(point(px(260.0), px(160.0)), point(px(200.0), px(100.0)));
    assert!(near_pt(c, p((60.0 - 10.0) / 0.5, (60.0 - 20.0) / 0.5)));
}

#[test]
fn the_visible_part_of_the_canvas() {
    let vp = Viewport::new(0.5, p(-100.0, 50.0));
    let v = vp.visible(size(400.0, 300.0));
    assert!(near_pt(v.origin, p(200.0, -100.0)));
    assert!(near(v.size.width, 800.0) && near(v.size.height, 600.0));
}

#[test]
fn z_scales_units_and_never_returns_nan() {
    assert_eq!(f32::from(z(0.5, 10.0)), 5.0);
    assert_eq!(f32::from(z(f32::NAN, 10.0)), 0.0);
    assert_eq!(f32::from(z(1.0, f32::INFINITY)), 0.0);
    assert_eq!(f32::from(Viewport::new(2.0, p(0.0, 0.0)).z(12.0)), 24.0);
}

#[test]
fn sanitizing_repairs_a_broken_viewport() {
    let bad = Viewport { zoom: f32::NAN, origin: p(f32::INFINITY, f32::NAN) }.sanitized();
    assert_eq!(bad, Viewport::default());
    assert_eq!(Viewport { zoom: 7.0, origin: p(1.0, 2.0) }.sanitized().zoom, MAX_ZOOM);
}

// ---------------------------------------------------------------------------- dots

#[test]
fn dots_hide_when_denser_than_a_few_pixels() {
    assert_eq!(dot_spacing(1.0), Some(20.0));
    assert_eq!(dot_spacing(0.5), Some(10.0));
    assert_eq!(dot_spacing(0.3), Some(6.0));
    assert_eq!(dot_spacing(0.25), None);
    assert_eq!(dot_spacing(f32::NAN), None);
}

#[test]
fn dot_range_covers_exactly_the_visible_dots() {
    // Dots at 5, 25, 45, ... ; the window shows 0..=60.
    assert_eq!(dot_range(5.0, 20.0, 0.0, 60.0), 0..3);
    // Dots at -15, 5, 25, 45, 65: from 0 to 60 the same three.
    assert_eq!(dot_range(-15.0, 20.0, 0.0, 60.0), 1..4);
    assert_eq!(dot_range(5.0, 20.0, 100.0, 100.0), 0..0, "no dot at exactly 100: 5 + 20k = 100 has no integer k");
    assert_eq!(dot_range(0.0, 20.0, 40.0, 40.0), 2..3);
    assert_eq!(dot_range(0.0, 0.0, 0.0, 10.0), 0..0);
    assert_eq!(dot_range(f32::NAN, 20.0, 0.0, 10.0), 0..0);
}

// ---------------------------------------------------------------------------- nodes

#[test]
fn node_heights_follow_the_mockup() {
    assert_eq!(node_height(2), 49.0 + 1.0 + 48.0 + 8.0);
    assert_eq!(node_height(4), 154.0, "the router");
    assert_eq!(node_height(1), 82.0);
    assert_eq!(node_height(0), 49.0, "no body, just the header");
}

#[test]
fn a_plain_node_has_an_input_and_an_output_at_the_header() {
    let n = NodeShape::new(p(404.0, 40.0), 2);
    assert_eq!(n.input_port(), Some(p(404.0, 64.0)));
    assert_eq!(n.output_ports(), vec![p(404.0 + 168.0, 64.0)]);
    assert_eq!(n.ports().input, Some(p(404.0, 64.0)));
    let source = n.without_input();
    assert_eq!(source.input_port(), None);
}

#[test]
fn a_router_has_an_output_per_row() {
    let n = NodeShape::new(p(212.0, 120.0), 4).with_outputs(Outputs::PerRow);
    let outs = n.output_ports();
    assert_eq!(outs.len(), 4);
    for (i, o) in outs.iter().enumerate() {
        // The mockup's `port("router", "out", i)`: y + HEAD_H + 1 + 4 + i × ROW_H + ROW_H / 2.
        assert_eq!(*o, p(212.0 + 168.0, 120.0 + 49.0 + 1.0 + 4.0 + i as f32 * 24.0 + 12.0));
    }
    assert_eq!(n.input_port(), Some(p(212.0, 144.0)));
}

#[test]
fn a_sink_has_no_outputs() {
    let n = NodeShape::new(p(0.0, 0.0), 1).with_outputs(Outputs::None);
    assert!(n.output_ports().is_empty());
}

#[test]
fn vertical_connector_ends_are_the_middle_of_the_edges() {
    let n = NodeShape::new(p(980.0, 170.0), 1);
    assert_eq!(n.bottom_center(), p(980.0 + 84.0, 170.0 + 82.0));
    assert_eq!(n.top_center(), p(980.0 + 84.0, 170.0));
}

#[test]
fn the_bounding_box_of_the_mockup_nodes() {
    let nodes = [
        NodeShape::new(p(20.0, 170.0), 2),
        NodeShape::new(p(596.0, 0.0), 2),
        NodeShape::new(p(980.0, 380.0), 1),
        NodeShape::new(p(404.0, 510.0), 2),
    ];
    let b = bounds_of(&nodes).expect("there are nodes");
    assert_eq!(b.origin, p(20.0, 0.0));
    assert_eq!(b.size.width, 980.0 + 168.0 - 20.0);
    assert_eq!(b.size.height, 510.0 + node_height(2));
}

#[test]
fn the_bounding_box_of_nothing_is_none() {
    assert_eq!(bounds_of(&[]), None);
    assert_eq!(bounds_of(&[NodeShape::new(p(f32::NAN, 0.0), 1)]), None);
    let one = bounds_of(&[NodeShape::new(p(f32::NAN, 0.0), 1), NodeShape::new(p(5.0, 5.0), 1)]);
    assert_eq!(one.map(|b| b.origin), Some(p(5.0, 5.0)));
}

#[test]
fn the_header_is_hit_but_not_the_body() {
    let nodes = [NodeShape::new(p(100.0, 100.0), 2)];
    assert_eq!(hit_node_header(&nodes, p(150.0, 120.0)), Some(0));
    assert_eq!(hit_node_header(&nodes, p(150.0, 100.0 + 60.0)), None, "the body is not the handle");
    assert_eq!(hit_node(&nodes, p(150.0, 160.0)), Some(0));
    assert_eq!(hit_node(&nodes, p(99.0, 160.0)), None);
    assert_eq!(hit_node(&nodes, p(150.0, 100.0 + node_height(2) + 1.0)), None);
}

#[test]
fn the_topmost_node_is_hit() {
    let nodes = [NodeShape::new(p(0.0, 0.0), 2), NodeShape::new(p(50.0, 10.0), 2)];
    assert_eq!(hit_node_header(&nodes, p(60.0, 20.0)), Some(1));
    assert_eq!(hit_node_header(&nodes, p(10.0, 20.0)), Some(0));
}

#[test]
fn a_port_is_hit_within_its_radius() {
    let nodes = [NodeShape::new(p(0.0, 0.0), 4).with_outputs(Outputs::PerRow)];
    let hit = hit_port(&nodes, p(168.0 + 3.0, 49.0 + 1.0 + 4.0 + 24.0 + 12.0 + 2.0), PORT_HIT_RADIUS);
    assert_eq!(hit, Some(PortHit { node: 0, side: PortSide::Output, index: 1 }));
    let input = hit_port(&nodes, p(-4.0, 24.0), PORT_HIT_RADIUS);
    assert_eq!(input, Some(PortHit { node: 0, side: PortSide::Input, index: 0 }));
    assert_eq!(hit_port(&nodes, p(80.0, 80.0), PORT_HIT_RADIUS), None);
    assert_eq!(hit_port(&nodes, p(168.0 + 20.0, 24.0 + 49.0 + 100.0), PORT_HIT_RADIUS), None);
}

#[test]
fn a_port_in_front_wins_over_one_behind() {
    // The second node's input sits on the first node's output.
    let nodes = [NodeShape::new(p(0.0, 0.0), 1), NodeShape::new(p(168.0, 0.0), 1)];
    let hit = hit_port(&nodes, p(168.0, 24.0), PORT_HIT_RADIUS);
    assert_eq!(hit, Some(PortHit { node: 1, side: PortSide::Input, index: 0 }));
}

// ---------------------------------------------------------------------------- curves

#[test]
fn a_route_leaves_and_arrives_horizontally() {
    let c = route_curve(p(0.0, 0.0), p(200.0, 100.0));
    assert_eq!(c.p0, p(0.0, 0.0));
    assert_eq!(c.p1, p(100.0, 0.0));
    assert_eq!(c.p2, p(100.0, 100.0));
    assert_eq!(c.p3, p(200.0, 100.0));
}

#[test]
fn the_control_offset_is_at_least_forty() {
    assert_eq!(control_offset(p(0.0, 0.0), p(60.0, 0.0)), 40.0);
    assert_eq!(control_offset(p(0.0, 0.0), p(300.0, 0.0)), 150.0);
    assert_eq!(control_offset(p(300.0, 0.0), p(0.0, 0.0)), 40.0, "backwards still stands off");
    let c = route_curve(p(100.0, 0.0), p(20.0, 50.0));
    assert_eq!((c.p1.x, c.p2.x), (140.0, -20.0));
}

#[test]
fn the_midpoint_of_a_route_is_between_its_ends() {
    for (a, b) in [(p(0.0, 0.0), p(200.0, 100.0)), (p(188.0, 144.0), p(404.0, 64.0)), (p(10.0, 10.0), p(30.0, 400.0))] {
        let m = route_curve(a, b).midpoint();
        assert!(near_pt(m, p((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)), "{m:?}");
    }
}

#[test]
fn curve_points_run_from_start_to_end() {
    let c = route_curve(p(0.0, 0.0), p(200.0, 100.0));
    assert!(near_pt(c.point_at(0.0), c.p0) && near_pt(c.point_at(1.0), c.p3));
    let pts = c.flatten(10);
    assert_eq!(pts.len(), 11);
    assert!(pts.windows(2).all(|w| w[1].x >= w[0].x - 1e-3), "a left-to-right route never doubles back");
    assert_eq!(c.flatten(0).len(), 2);
}

#[test]
fn distance_to_a_curve() {
    // Same height: the curve is a straight line along y = 0.
    let flat = route_curve(p(0.0, 0.0), p(200.0, 0.0));
    assert!(near(flat.distance_to(p(100.0, 10.0)), 10.0));
    assert!(near(flat.distance_to(p(100.0, -6.0)), 6.0));
    assert!(flat.distance_to(p(50.0, 0.0)) < 1e-3);
    // Past the end the distance is to the end point.
    assert!(near(flat.distance_to(p(230.0, 40.0)), 50.0));
    // Points on a bent curve are at distance ~0, points far away are far.
    let s = route_curve(p(0.0, 0.0), p(200.0, 100.0));
    for t in [0.1, 0.3, 0.5, 0.8] {
        assert!(s.distance_to(s.point_at(t)) < 0.5, "t = {t}");
    }
    assert!(s.distance_to(p(100.0, 300.0)) > 150.0);
    // Checked against a much finer polyline.
    let probe = p(120.0, 30.0);
    let fine = polyline_for_tests(&s, FLATTEN_CHECK, probe);
    assert!((s.distance_to(probe) - fine).abs() < 0.5);
}

#[test]
fn distance_to_a_segment() {
    assert!(near(distance_to_segment(p(5.0, 5.0), p(0.0, 0.0), p(10.0, 0.0)), 5.0));
    assert!(near(distance_to_segment(p(-3.0, 4.0), p(0.0, 0.0), p(10.0, 0.0)), 5.0));
    assert!(near(distance_to_segment(p(3.0, 4.0), p(1.0, 1.0), p(1.0, 1.0)), (4.0f32 + 9.0).sqrt()));
}

#[test]
fn a_back_edge_loops_below_both_ports() {
    let (from, to) = (p(788.0 + 168.0, 254.0), p(404.0, 64.0));
    let c = back_curve(from, to);
    assert_eq!((c.p0, c.p3), (from, to));
    assert!(c.p1.x > from.x, "it leaves to the right");
    assert!(c.p2.x < to.x, "it arrives from the left");
    let lowest = c.flatten(40).iter().map(|q| q.y).fold(f32::MIN, f32::max);
    assert!(lowest > from.y.max(to.y) + 30.0, "{lowest}");
    assert!(c.midpoint().y > from.y.max(to.y));
}

#[test]
fn the_curve_of_an_edge_follows_its_kind_and_shape() {
    let (a, b) = (p(0.0, 0.0), p(200.0, 80.0));
    assert!(matches!(Edge::new(a, b).curve(), Curve::Cubic(c) if c == route_curve(a, b)));
    assert!(matches!(Edge::new(a, b).kind(EdgeKind::Back).curve(), Curve::Cubic(c) if c == back_curve(a, b)));
    assert!(matches!(Edge::vertical(a, b).curve(), Curve::Line(x, y) if x == a && y == b));
    assert!(near_pt(Edge::vertical(a, b).midpoint(), p(100.0, 40.0)));
    assert!(near_pt(Edge::new(a, b).midpoint(), p(100.0, 40.0)));
}

#[test]
fn a_line_curve_measures_and_bounds_like_a_segment() {
    let line = Curve::Line(p(10.0, 0.0), p(10.0, 100.0));
    assert!(near(line.distance_to(p(14.0, 50.0)), 4.0));
    assert!(near_pt(line.midpoint(), p(10.0, 50.0)));
    let b = line.bounds();
    assert_eq!((b.origin, b.size.height), (p(10.0, 0.0), 100.0));
    let b = Curve::Cubic(route_curve(p(0.0, 0.0), p(200.0, 100.0))).bounds();
    assert_eq!((b.size.width, b.size.height), (200.0, 100.0));
}

// ---------------------------------------------------------------------------- edges

#[test]
fn edge_kinds_look_as_the_mockup_says() {
    use ui::theme::*;
    assert_eq!(EdgeKind::Route.color(), LINE_STRONG);
    assert_eq!(EdgeKind::Selected.color(), ACCENT);
    assert_eq!(EdgeKind::Spawn.color(), SPAWN_LINE);
    assert_eq!(EdgeKind::Dim.color().a, 0.6);
    assert_eq!(EdgeKind::Spawn.dash(), Some([5.0, 4.0]));
    assert_eq!(EdgeKind::Route.dash(), None);
    assert_eq!(EdgeKind::Selected.dash(), None);
    assert_eq!(EdgeKind::Dim.dash(), None);
    assert!(EdgeKind::Back.dash().is_some());
    assert_eq!(EdgeKind::Back.dash(), Some(BACK_DASH));
    assert_eq!(EdgeKind::Spawn.dash(), Some(SPAWN_DASH));
}

#[test]
fn a_selected_back_edge_keeps_its_loop_and_dots_in_accent() {
    use ui::theme::ACCENT;
    let (a, b) = (p(300.0, 40.0), p(20.0, 40.0));
    let edge = Edge::new(a, b).kind(EdgeKind::SelectedBack);
    assert!(matches!(edge.curve(), Curve::Cubic(c) if c == back_curve(a, b)));
    assert_eq!(edge.kind.color(), ACCENT);
    assert_eq!(edge.kind.dash(), Some(BACK_DASH));
    // Drawn over the other edges, with the selected path.
    let mid = edge.midpoint();
    assert_eq!(hit_edge(&[Edge::new(a, b).kind(EdgeKind::Back), edge], mid, 2.0), Some(1));
}

#[test]
fn an_edge_is_hit_near_its_curve() {
    let edges = [Edge::new(p(0.0, 0.0), p(200.0, 0.0)), Edge::new(p(0.0, 100.0), p(200.0, 100.0))];
    assert_eq!(hit_edge(&edges, p(100.0, 4.0), 6.0), Some(0));
    assert_eq!(hit_edge(&edges, p(100.0, 96.0), 6.0), Some(1));
    assert_eq!(hit_edge(&edges, p(100.0, 50.0), 6.0), None);
    assert_eq!(hit_edge(&[], p(0.0, 0.0), 6.0), None);
}

#[test]
fn the_nearer_edge_is_hit_and_ties_go_to_the_top() {
    let edges = [Edge::new(p(0.0, 0.0), p(200.0, 0.0)), Edge::new(p(0.0, 10.0), p(200.0, 10.0))];
    assert_eq!(hit_edge(&edges, p(100.0, 6.0), 8.0), Some(1));
    assert_eq!(hit_edge(&edges, p(100.0, 3.0), 8.0), Some(0));
    let same = [Edge::new(p(0.0, 0.0), p(200.0, 0.0)), Edge::new(p(0.0, 0.0), p(200.0, 0.0))];
    assert_eq!(hit_edge(&same, p(100.0, 1.0), 4.0), Some(1));
}

#[test]
fn a_vertical_connector_is_hit_along_its_line() {
    let edges = [Edge::vertical(p(50.0, 0.0), p(50.0, 100.0))];
    assert_eq!(hit_edge(&edges, p(53.0, 60.0), 5.0), Some(0));
    assert_eq!(hit_edge(&edges, p(60.0, 60.0), 5.0), None);
}

#[test]
fn edges_with_broken_ends_are_never_hit() {
    let edges = [Edge::new(p(f32::NAN, 0.0), p(10.0, 10.0))];
    assert_eq!(hit_edge(&edges, p(5.0, 5.0), 1000.0), None);
}

#[test]
fn the_edge_layer_converts_its_pixel_tolerance_to_units() {
    let edge = Edge::new(p(0.0, 0.0), p(200.0, 0.0));
    let zoomed = Edges::new(Viewport::new(2.0, p(0.0, 0.0)), [edge.clone()]);
    // 8 px at 200 % is 4 units.
    assert_eq!(zoomed.hit_test(p(100.0, 3.5), 8.0), Some(0));
    assert_eq!(zoomed.hit_test(p(100.0, 5.0), 8.0), None);
    let far = Edges::new(Viewport::new(0.5, p(0.0, 0.0)), [edge]);
    assert_eq!(far.hit_test(p(100.0, 12.0), 8.0), Some(0), "8 px at 50 % is 16 units");
}

#[test]
fn frame_ports_are_the_shape_ports() {
    let vp = Viewport::default();
    let f = NodeFrame::new("r", vp, p(212.0, 120.0))
        .row("rule 1", "a")
        .row("rule 2", "b")
        .row("rule 3", "c")
        .row("else", "d")
        .outputs(Outputs::PerRow);
    assert_eq!(f.ports(), NodeShape::new(p(212.0, 120.0), 4).with_outputs(Outputs::PerRow).ports());
    assert_eq!(f.ports().outputs.len(), 4);
    assert_eq!(f.shape().size().height, 154.0);
}
