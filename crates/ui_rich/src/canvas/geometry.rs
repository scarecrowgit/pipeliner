//! The geometry of the pipeline canvas, in canvas units: node boxes, port positions, edge
//! curves, hit tests. Pure functions on plain numbers; nothing here draws or needs a window.
//!
//! The numbers are the mockup's: a node is 168 units wide with a 49 unit header and 24 unit
//! rows; an edge leaves a port horizontally and arrives horizontally, its control points
//! `max(40, Δx / 2)` away.

use gpui::{Bounds, Point, Size, point, size};

/// Width of a node.
pub const NODE_WIDTH: f32 = 168.0;
/// Height of a node's header (icon tile, title and sub-line).
pub const HEADER_HEIGHT: f32 = 49.0;
/// Height of one key/value row of a node's body.
pub const ROW_HEIGHT: f32 = 24.0;
/// Padding between a body's top rule and its first row (and its last row and the bottom).
pub const BODY_PADDING: f32 = 4.0;
/// Where the header's ports sit, from the node's top.
pub const HEADER_PORT_Y: f32 = 24.0;
/// Diameter of a port.
pub const PORT_SIZE: f32 = 10.0;
/// Distance, in canvas units, within which a pointer hits a port.
pub const PORT_HIT_RADIUS: f32 = 9.0;
/// The grid the dots are drawn on and nodes snap to.
pub const GRID_UNIT: f32 = 20.0;
/// Segments a curve is flattened to for distance tests.
const FLATTEN_SEGMENTS: usize = 48;
/// How far below its lowest end a [`back_curve`] loops.
const BACK_DROP: f32 = 72.0;

/// Whether `p` has two finite coordinates.
pub fn is_finite(p: Point<f32>) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// Distance between two points.
pub fn distance(a: Point<f32>, b: Point<f32>) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// The point `t` of the way from `a` to `b`.
pub fn lerp(a: Point<f32>, b: Point<f32>, t: f32) -> Point<f32> {
    point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Distance from `p` to the segment `a`-`b`.
pub fn distance_to_segment(p: Point<f32>, a: Point<f32>, b: Point<f32>) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return distance(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    distance(p, point(a.x + dx * t, a.y + dy * t))
}

// ---------------------------------------------------------------------------------- nodes

/// Which outputs a node has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Outputs {
    /// A sink: nothing leaves it.
    None,
    /// One output at the right of the header.
    #[default]
    One,
    /// One output at the right of each body row (a router).
    PerRow,
}

/// The height of a node with `rows` body rows: header, rule, rows and padding. Without rows
/// the node is just its header.
pub fn node_height(rows: usize) -> f32 {
    if rows == 0 {
        HEADER_HEIGHT
    } else {
        HEADER_HEIGHT + 1.0 + rows as f32 * ROW_HEIGHT + 2.0 * BODY_PADDING
    }
}

/// Distance from a node's top to the centre of its `row`-th body row.
pub fn row_center_y(row: usize) -> f32 {
    HEADER_HEIGHT + 1.0 + BODY_PADDING + (row as f32 + 0.5) * ROW_HEIGHT
}

/// The ports of a node, in canvas units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodePorts {
    /// The input, at the left of the header.
    pub input: Option<Point<f32>>,
    /// The outputs: one at the right of the header, or one per body row.
    pub outputs: Vec<Point<f32>>,
}

/// The box of a node and which ports it has: all that is needed to place and hit-test it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeShape {
    /// Top-left corner, in canvas units.
    pub origin: Point<f32>,
    /// Number of body rows.
    pub rows: usize,
    /// Has an input at the left of the header.
    pub input: bool,
    /// Which outputs it has.
    pub outputs: Outputs,
}

impl NodeShape {
    /// A node at `origin` with `rows` body rows, an input and one output.
    pub fn new(origin: Point<f32>, rows: usize) -> Self {
        Self { origin, rows, input: true, outputs: Outputs::One }
    }

    /// Without an input port.
    pub fn without_input(mut self) -> Self {
        self.input = false;
        self
    }

    /// With the given outputs.
    pub fn with_outputs(mut self, outputs: Outputs) -> Self {
        self.outputs = outputs;
        self
    }

    /// The size of the node.
    pub fn size(&self) -> Size<f32> {
        size(NODE_WIDTH, node_height(self.rows))
    }

    /// The whole node.
    pub fn bounds(&self) -> Bounds<f32> {
        Bounds::new(self.origin, self.size())
    }

    /// The header: the part a node is dragged by.
    pub fn header_bounds(&self) -> Bounds<f32> {
        Bounds::new(self.origin, size(NODE_WIDTH, HEADER_HEIGHT))
    }

    /// Where the input is, if there is one.
    pub fn input_port(&self) -> Option<Point<f32>> {
        self.input.then(|| point(self.origin.x, self.origin.y + HEADER_PORT_Y))
    }

    /// Where the outputs are: one at the header, one per row, or none.
    pub fn output_ports(&self) -> Vec<Point<f32>> {
        let x = self.origin.x + NODE_WIDTH;
        match self.outputs {
            Outputs::None => Vec::new(),
            Outputs::One => vec![point(x, self.origin.y + HEADER_PORT_Y)],
            Outputs::PerRow => {
                (0..self.rows).map(|i| point(x, self.origin.y + row_center_y(i))).collect()
            }
        }
    }

    /// All ports.
    pub fn ports(&self) -> NodePorts {
        NodePorts { input: self.input_port(), outputs: self.output_ports() }
    }

    /// The middle of the top edge (where a vertical connector arrives).
    pub fn top_center(&self) -> Point<f32> {
        point(self.origin.x + NODE_WIDTH / 2.0, self.origin.y)
    }

    /// The middle of the bottom edge (where a vertical connector leaves).
    pub fn bottom_center(&self) -> Point<f32> {
        point(self.origin.x + NODE_WIDTH / 2.0, self.origin.y + node_height(self.rows))
    }
}

/// The smallest box that holds every node, or `None` for no nodes (or only non-finite ones).
pub fn bounds_of(nodes: &[NodeShape]) -> Option<Bounds<f32>> {
    nodes
        .iter()
        .filter(|n| is_finite(n.origin))
        .map(NodeShape::bounds)
        .reduce(|a, b| a.union(&b))
}

/// Whether `p` is inside `bounds` (edges included).
fn inside(bounds: &Bounds<f32>, p: Point<f32>) -> bool {
    bounds.contains(&p)
}

/// The topmost node (the last one in `nodes`) whose header contains `p`.
pub fn hit_node_header(nodes: &[NodeShape], p: Point<f32>) -> Option<usize> {
    nodes.iter().rposition(|n| inside(&n.header_bounds(), p))
}

/// The topmost node that contains `p` anywhere.
pub fn hit_node(nodes: &[NodeShape], p: Point<f32>) -> Option<usize> {
    nodes.iter().rposition(|n| inside(&n.bounds(), p))
}

/// Which side of a node a port is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSide {
    /// The left input.
    Input,
    /// An output at the right.
    Output,
}

/// A port under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortHit {
    /// Index of the node in the slice given to [`hit_port`].
    pub node: usize,
    /// Input or output.
    pub side: PortSide,
    /// Which output (the row for a router); always `0` for an input.
    pub index: usize,
}

/// The port nearest to `p` within `radius` canvas units, on the topmost node that has one.
pub fn hit_port(nodes: &[NodeShape], p: Point<f32>, radius: f32) -> Option<PortHit> {
    for (node, shape) in nodes.iter().enumerate().rev() {
        let mut best: Option<(f32, PortHit)> = None;
        let mut offer = |d: f32, hit: PortHit| {
            if d <= radius && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, hit));
            }
        };
        if let Some(port) = shape.input_port() {
            offer(distance(p, port), PortHit { node, side: PortSide::Input, index: 0 });
        }
        for (index, port) in shape.output_ports().into_iter().enumerate() {
            offer(distance(p, port), PortHit { node, side: PortSide::Output, index });
        }
        if let Some((_, hit)) = best {
            return Some(hit);
        }
    }
    None
}

// ---------------------------------------------------------------------------------- curves

/// A cubic Bézier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    /// Start.
    pub p0: Point<f32>,
    /// First control point.
    pub p1: Point<f32>,
    /// Second control point.
    pub p2: Point<f32>,
    /// End.
    pub p3: Point<f32>,
}

impl Cubic {
    /// The point at `t` (`0` start, `1` end).
    pub fn point_at(&self, t: f32) -> Point<f32> {
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        point(
            a * self.p0.x + b * self.p1.x + c * self.p2.x + d * self.p3.x,
            a * self.p0.y + b * self.p1.y + c * self.p2.y + d * self.p3.y,
        )
    }

    /// The point halfway along the parameter: where an edge label goes.
    pub fn midpoint(&self) -> Point<f32> {
        self.point_at(0.5)
    }

    /// The curve as `segments` straight pieces: `segments + 1` points.
    pub fn flatten(&self, segments: usize) -> Vec<Point<f32>> {
        let n = segments.max(1);
        (0..=n).map(|i| self.point_at(i as f32 / n as f32)).collect()
    }

    /// Distance from `p` to the curve (measured to a 48 piece polyline).
    pub fn distance_to(&self, p: Point<f32>) -> f32 {
        polyline_distance(&self.flatten(FLATTEN_SEGMENTS), p)
    }
}

fn polyline_distance(points: &[Point<f32>], p: Point<f32>) -> f32 {
    points
        .windows(2)
        .map(|w| distance_to_segment(p, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

/// How far the control points of a curve from `from` to `to` stand off the ports:
/// `max(40, Δx / 2)`.
pub fn control_offset(from: Point<f32>, to: Point<f32>) -> f32 {
    ((to.x - from.x) * 0.5).max(40.0)
}

/// The edge from an output at `from` to an input at `to`: leaves to the right, arrives from
/// the left, with control points `max(40, Δx / 2)` away.
pub fn route_curve(from: Point<f32>, to: Point<f32>) -> Cubic {
    let d = control_offset(from, to);
    Cubic {
        p0: from,
        p1: point(from.x + d, from.y),
        p2: point(to.x - d, to.y),
        p3: to,
    }
}

/// The edge of work sent back: from an output at `from` to an input at `to` that is usually
/// further left. It leaves to the right, drops below both ports and comes round to arrive
/// from the left.
pub fn back_curve(from: Point<f32>, to: Point<f32>) -> Cubic {
    let d = (0.5 * (to.x - from.x).abs()).max(40.0);
    let lane = from.y.max(to.y) + BACK_DROP;
    Cubic {
        p0: from,
        p1: point(from.x + d, lane),
        p2: point(to.x - d, lane),
        p3: to,
    }
}

/// What an edge is drawn along.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// A Bézier.
    Cubic(Cubic),
    /// A straight line, for the vertical connector.
    Line(Point<f32>, Point<f32>),
}

impl Curve {
    /// The point at `t` (`0` start, `1` end).
    pub fn point_at(&self, t: f32) -> Point<f32> {
        match self {
            Curve::Cubic(c) => c.point_at(t),
            Curve::Line(a, b) => lerp(*a, *b, t),
        }
    }

    /// The middle of the curve.
    pub fn midpoint(&self) -> Point<f32> {
        self.point_at(0.5)
    }

    /// Distance from `p` to the curve.
    pub fn distance_to(&self, p: Point<f32>) -> f32 {
        match self {
            Curve::Cubic(c) => c.distance_to(p),
            Curve::Line(a, b) => distance_to_segment(p, *a, *b),
        }
    }

    /// The box around the curve's end points and control points.
    pub fn bounds(&self) -> Bounds<f32> {
        let pts: Vec<Point<f32>> = match self {
            Curve::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
            Curve::Line(a, b) => vec![*a, *b],
        };
        let (mut lo, mut hi) = (pts[0], pts[0]);
        for p in &pts[1..] {
            lo = point(lo.x.min(p.x), lo.y.min(p.y));
            hi = point(hi.x.max(p.x), hi.y.max(p.y));
        }
        Bounds::from_corners(lo, hi)
    }
}

/// Checks for tests: the flattening used by [`Cubic::distance_to`] against a finer one.
#[cfg(test)]
pub(super) const FLATTEN_CHECK: usize = 2000;

/// Distance from `probe` to `curve` measured on a polyline of `segments` pieces.
#[cfg(test)]
pub(super) fn polyline_for_tests(curve: &Cubic, segments: usize, probe: Point<f32>) -> f32 {
    polyline_distance(&curve.flatten(segments), probe)
}
