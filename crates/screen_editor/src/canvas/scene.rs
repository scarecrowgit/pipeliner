//! The graph laid out in canvas units: one placed node per graph node with its look and shape,
//! the port positions, and the edges as the canvas primitives draw them.

use gpui::{Bounds, Point, SharedString, point};
use hub::model::{EdgeKind, GraphDef, NodeId, NodeKind};
use ui_rich::canvas::{
    Edge, EdgeKind as Stroke, HEADER_PORT_Y, NODE_WIDTH, NodeShape, Outputs, bounds_of, hit_edge,
};

use super::look::{NodeLook, look};
use crate::Selection;

/// One node where it sits.
#[derive(Clone, Debug)]
pub struct Placed {
    pub id: NodeId,
    pub name: SharedString,
    pub look: NodeLook,
    pub shape: NodeShape,
}

impl Placed {
    /// Where out-port `port` is, in canvas units.
    pub fn out_port(&self, port: &str) -> Option<Point<f32>> {
        let index = self.look.ports.iter().position(|p| p == port)?;
        self.shape.output_ports().get(index).copied()
    }

    /// Where the in-port is.
    pub fn in_port(&self) -> Option<Point<f32>> {
        self.shape.input_port()
    }
}

/// One edge of the graph as drawn.
#[derive(Clone, Debug)]
pub struct SceneEdge {
    /// Index in `GraphDef::edges`.
    pub index: usize,
    pub edge: Edge,
    /// A selected back edge: drawn over in the accent, since the primitives' selected stroke
    /// would straighten its curve.
    pub highlight: bool,
}

/// The whole graph in canvas units.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub nodes: Vec<Placed>,
    pub edges: Vec<SceneEdge>,
}

/// How close (canvas units, along x) a node under another must be for a straight connector:
/// the same column.
const COLUMN_SLACK: f32 = 1.0;
/// The least gap between two nodes for a straight connector.
const COLUMN_GAP: f32 = 8.0;

impl Scene {
    /// Lays out `graph` with `selection` marking the selected path.
    pub fn build(graph: &GraphDef, selection: &Selection) -> Scene {
        let nodes: Vec<Placed> = graph
            .nodes
            .iter()
            .map(|node| {
                let look = look(graph, node);
                let mut shape = NodeShape::new(point(node.pos.x, node.pos.y), look.rows.len()).with_outputs(look.outputs);
                if !look.input {
                    shape = shape.without_input();
                }
                Placed { id: node.id.clone(), name: node.name.clone().into(), look, shape }
            })
            .collect();
        let mut scene = Scene { nodes, edges: Vec::new() };
        let selected_node = match selection {
            Selection::Node(id) => Some(id),
            _ => None,
        };
        for (index, def) in graph.edges.iter().enumerate() {
            let (Some(from), Some(to)) = (scene.index_of(&def.from), scene.index_of(&def.to)) else { continue };
            let (a, b) = (&scene.nodes[from], &scene.nodes[to]);
            let start = a.out_port(&def.port).unwrap_or_else(|| point(a.shape.origin.x + NODE_WIDTH, a.shape.origin.y + HEADER_PORT_Y));
            let end = b.in_port().unwrap_or_else(|| point(b.shape.origin.x, b.shape.origin.y + HEADER_PORT_Y));
            let this_selected = *selection == Selection::Edge(index);
            let on_path = def.kind == EdgeKind::Route && selected_node.is_some_and(|id| *id == def.from || *id == def.to);
            let stroke = match def.kind {
                EdgeKind::Route if this_selected || on_path => Stroke::Selected,
                EdgeKind::Route => Stroke::Route,
                EdgeKind::Context if this_selected => Stroke::Selected,
                EdgeKind::Context => Stroke::Dim,
                EdgeKind::Spawn if this_selected => Stroke::Selected,
                EdgeKind::Spawn => Stroke::Spawn,
                EdgeKind::Back => Stroke::Back,
            };
            let below = a.look.outputs == Outputs::One
                && (a.shape.origin.x - b.shape.origin.x).abs() <= COLUMN_SLACK
                && b.shape.origin.y >= a.shape.bounds().bottom() + COLUMN_GAP;
            let mut edge = if def.kind == EdgeKind::Route && below {
                Edge::vertical(a.shape.bottom_center(), b.shape.top_center())
            } else {
                Edge::new(start, end)
            }
            .kind(stroke);
            if def.kind == EdgeKind::Route
                && let (NodeKind::Router(_), Some(NodeKind::Agent(agent) | NodeKind::Orchestrator(agent))) =
                    (&graph.nodes[from].kind, graph.nodes.get(to).map(|n| &n.kind))
            {
                edge = edge.label(format!("×{}", agent.variants.max(1)));
            }
            scene.edges.push(SceneEdge { index, edge, highlight: this_selected && def.kind == EdgeKind::Back });
        }
        scene
    }

    /// The position of node `id` in [`Scene::nodes`].
    pub fn index_of(&self, id: &NodeId) -> Option<usize> {
        self.nodes.iter().position(|node| &node.id == id)
    }

    /// The node `id`.
    pub fn node(&self, id: &NodeId) -> Option<&Placed> {
        self.nodes.iter().find(|node| &node.id == id)
    }

    /// The shapes, in graph order, for the hit tests.
    pub fn shapes(&self) -> Vec<NodeShape> {
        self.nodes.iter().map(|node| node.shape).collect()
    }

    /// The box around every node.
    pub fn bounds(&self) -> Option<Bounds<f32>> {
        bounds_of(&self.shapes())
    }

    /// The edge (its index in `GraphDef::edges`) within `tolerance` canvas units of `p`.
    pub fn hit_edge(&self, p: Point<f32>, tolerance: f32) -> Option<usize> {
        let edges: Vec<Edge> = self.edges.iter().map(|e| e.edge.clone()).collect();
        hit_edge(&edges, p, tolerance).map(|i| self.edges[i].index)
    }
}
