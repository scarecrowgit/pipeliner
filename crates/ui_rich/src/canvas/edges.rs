//! The edges of the pipeline graph: one painted layer for all of them, and the label pills.

use gpui::{
    App, Hsla, IntoElement, ParentElement, PathBuilder, Point, RenderOnce, SharedString, Window,
    canvas, div, point, prelude::*, px,
};
use ui::theme::*;

use super::geometry::{Curve, back_curve, is_finite, route_curve};
use super::viewport::Viewport;

/// Stroke width of an edge, in canvas units.
const STROKE: f32 = 1.5;
/// Dash and gap of a spawn edge, in canvas units (`stroke-dasharray="5 4"`).
const SPAWN_DASH: [f32; 2] = [5.0, 4.0];
/// Dash and gap of a back edge.
const BACK_DASH: [f32; 2] = [2.0, 3.0];
/// Opacity of a dimmed edge (`opacity=".6"`).
const DIM_OPACITY: f32 = 0.6;
/// Label pill: height, side padding and the width of the box it is centred in (units).
const LABEL_HEIGHT: f32 = 18.0;
const LABEL_PADDING: f32 = 6.0;
const LABEL_BOX_WIDTH: f32 = 160.0;

/// What an edge means; it decides how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EdgeKind {
    /// A route between nodes: `line-strong`.
    #[default]
    Route,
    /// On the selected path: accent.
    Selected,
    /// An agent creating another: violet and dashed.
    Spawn,
    /// Context that is not the main flow: a route at 60 % opacity.
    Dim,
    /// Work sent back (a review that rejects): the curve loops round below the nodes,
    /// drawn dotted.
    Back,
}

impl EdgeKind {
    /// Colour, with the opacity of a dim edge applied.
    pub fn color(self) -> Hsla {
        match self {
            EdgeKind::Route | EdgeKind::Back => LINE_STRONG,
            EdgeKind::Selected => ACCENT,
            EdgeKind::Spawn => SPAWN_LINE,
            EdgeKind::Dim => LINE_STRONG.opacity(DIM_OPACITY),
        }
    }

    /// Dash and gap in canvas units, `None` for a solid line.
    pub fn dash(self) -> Option<[f32; 2]> {
        match self {
            EdgeKind::Spawn => Some(SPAWN_DASH),
            EdgeKind::Back => Some(BACK_DASH),
            _ => None,
        }
    }

    /// Painting order: later kinds are drawn over earlier ones, the selected path last.
    fn layer(self) -> u8 {
        match self {
            EdgeKind::Dim => 0,
            EdgeKind::Route => 1,
            EdgeKind::Back => 2,
            EdgeKind::Spawn => 3,
            EdgeKind::Selected => 4,
        }
    }
}

/// How an edge runs between its ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EdgeShape {
    /// A curve leaving the output to the right and arriving at the input from the left.
    #[default]
    Curve,
    /// A straight connector, for a node under another.
    Vertical,
}

/// One edge: where it starts and ends (canvas units), what it means, an optional label.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// The output port it leaves.
    pub from: Point<f32>,
    /// The input port it arrives at.
    pub to: Point<f32>,
    /// What it means.
    pub kind: EdgeKind,
    /// How it runs.
    pub shape: EdgeShape,
    /// A pill at the middle of the curve, e.g. `"×3"`.
    pub label: Option<SharedString>,
}

impl Edge {
    /// A route from an output to an input.
    pub fn new(from: Point<f32>, to: Point<f32>) -> Self {
        Self { from, to, kind: EdgeKind::Route, shape: EdgeShape::Curve, label: None }
    }

    /// A straight connector from `from` to `to`, e.g. the bottom of one node to the top of
    /// the next.
    pub fn vertical(from: Point<f32>, to: Point<f32>) -> Self {
        Self { shape: EdgeShape::Vertical, ..Self::new(from, to) }
    }

    /// Set what the edge means.
    pub fn kind(mut self, kind: EdgeKind) -> Self {
        self.kind = kind;
        self
    }

    /// A pill at the middle of the edge.
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// What the edge runs along.
    pub fn curve(&self) -> Curve {
        match (self.shape, self.kind) {
            (EdgeShape::Vertical, _) => Curve::Line(self.from, self.to),
            (EdgeShape::Curve, EdgeKind::Back) => Curve::Cubic(back_curve(self.from, self.to)),
            (EdgeShape::Curve, _) => Curve::Cubic(route_curve(self.from, self.to)),
        }
    }

    /// The middle of the edge, in canvas units.
    pub fn midpoint(&self) -> Point<f32> {
        self.curve().midpoint()
    }
}

/// The edge nearest to `p` (canvas units) within `tolerance` units, if any. On a tie the one
/// drawn last (on top) wins.
pub fn hit_edge(edges: &[Edge], p: Point<f32>, tolerance: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, edge) in edges.iter().enumerate() {
        if !is_finite(edge.from) || !is_finite(edge.to) {
            continue;
        }
        let d = edge.curve().distance_to(p);
        if d <= tolerance && best.is_none_or(|(b, _)| d <= b) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// All edges of a graph in one painted layer, with their label pills.
///
/// Fills its parent and sits under the nodes: put it after the [`DotGrid`](super::DotGrid) and
/// before the node frames, in a container that clips. The selected path is drawn on top of
/// the other edges.
#[derive(IntoElement)]
pub struct Edges {
    viewport: Viewport,
    edges: Vec<Edge>,
}

impl Edges {
    /// The edges of a graph seen through `viewport`.
    pub fn new(viewport: Viewport, edges: impl IntoIterator<Item = Edge>) -> Self {
        Self { viewport: viewport.sanitized(), edges: edges.into_iter().collect() }
    }

    /// The edge under `p` (canvas units), allowing `tolerance_px` pixels of slack on screen.
    /// For click selection.
    pub fn hit_test(&self, p: Point<f32>, tolerance_px: f32) -> Option<usize> {
        hit_edge(&self.edges, p, tolerance_px / self.viewport.zoom)
    }
}

/// A pill on an edge: mono 11, `surface-raised` with a `line-strong` border.
#[derive(IntoElement)]
pub struct EdgeLabel {
    viewport: Viewport,
    at: Point<f32>,
    text: SharedString,
}

impl EdgeLabel {
    /// A pill reading `text`, centred on `at` (canvas units).
    pub fn new(viewport: Viewport, at: Point<f32>, text: impl Into<SharedString>) -> Self {
        Self { viewport: viewport.sanitized(), at, text: text.into() }
    }
}

impl RenderOnce for EdgeLabel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let vp = self.viewport;
        let screen = if is_finite(self.at) { vp.to_screen(self.at) } else { point(0.0, 0.0) };
        // A wide box centred on the point; the pill is centred inside it.
        div()
            .absolute()
            .left(px(screen.x) - vp.z(LABEL_BOX_WIDTH / 2.0))
            .top(px(screen.y) - vp.z(LABEL_HEIGHT / 2.0))
            .w(vp.z(LABEL_BOX_WIDTH))
            .h(vp.z(LABEL_HEIGHT))
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(vp.z(LABEL_HEIGHT))
                    .px(vp.z(LABEL_PADDING))
                    .rounded_full()
                    .bg(SURFACE_RAISED)
                    .border(vp.z(1.0))
                    .border_color(LINE_STRONG)
                    .font_family(FONT_MONO)
                    .text_size(vp.z(MONO_SM.size))
                    .line_height(vp.z(MONO_SM.line))
                    .text_color(TEXT_SECONDARY)
                    .whitespace_nowrap()
                    .child(self.text),
            )
    }
}

/// Stroke `curve` through `viewport` into the window, relative to `origin` on screen.
fn paint_edge(
    window: &mut Window,
    viewport: Viewport,
    origin: Point<gpui::Pixels>,
    edge: &Edge,
) {
    if !is_finite(edge.from) || !is_finite(edge.to) {
        return;
    }
    let at = |p: Point<f32>| {
        let s = viewport.to_screen(p);
        point(origin.x + px(s.x), origin.y + px(s.y))
    };
    let mut path = PathBuilder::stroke(viewport.z(STROKE));
    if let Some([dash, gap]) = edge.kind.dash() {
        path = path.dash_array(&[viewport.z(dash), viewport.z(gap)]);
    }
    match edge.curve() {
        Curve::Cubic(c) => {
            path.move_to(at(c.p0));
            path.cubic_bezier_to(at(c.p3), at(c.p1), at(c.p2));
        }
        Curve::Line(a, b) => {
            path.move_to(at(a));
            path.line_to(at(b));
        }
    }
    // A path with no length fails to build; there is nothing to draw then.
    if let Ok(path) = path.build() {
        window.paint_path(path, edge.kind.color());
    }
}

impl RenderOnce for Edges {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let vp = self.viewport;
        let labels: Vec<EdgeLabel> = self
            .edges
            .iter()
            .filter(|e| is_finite(e.from) && is_finite(e.to))
            .filter_map(|e| Some(EdgeLabel::new(vp, e.midpoint(), e.label.clone()?)))
            .collect();
        let mut ordered = self.edges;
        // Stable: edges of one kind keep their order, the selected path ends up on top.
        ordered.sort_by_key(|e| e.kind.layer());
        div()
            .absolute()
            .size_full()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        for edge in &ordered {
                            paint_edge(window, vp, bounds.origin, edge);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(labels)
    }
}
