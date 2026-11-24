//! Adding nodes: the kinds the add menu offers, the defaults a new node starts with, a fresh id
//! and a free spot.

use gpui::{App, Bounds, ElementId, Point, WeakEntity, Window, point, size};
use hub::model::{
    ActionNode, AgentNode, FileMode, FileNode, GraphDef, NodeDef, NodeId, NodeKind, Pos, ReviewNode, RouterNode,
    RouterRule, SourceNode, SubagentNode,
};
use ui::assets::IconName;
use ui::components::{MenuItem, Tile, TileTone};
use ui::menu::Menu;
use ui_rich::canvas::{GRID_UNIT, HEADER_HEIGHT, NODE_WIDTH, node_height};

use super::CanvasView;
use super::look::look;

/// A kind of node the add menu offers, in the mockup's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NewNode {
    Source,
    Router,
    Agent,
    Orchestrator,
    Pipeline,
    Subagent,
    File,
    Review,
    Action,
}

impl NewNode {
    /// Every kind, as the menu lists them.
    pub const ALL: [NewNode; 9] = [
        NewNode::Source,
        NewNode::Router,
        NewNode::Agent,
        NewNode::Orchestrator,
        NewNode::Pipeline,
        NewNode::Subagent,
        NewNode::File,
        NewNode::Review,
        NewNode::Action,
    ];

    /// The word for it, as on a node.
    pub fn label(self) -> &'static str {
        match self {
            NewNode::Source => "Source",
            NewNode::Router => "Router",
            NewNode::Agent => "Agent",
            NewNode::Orchestrator => "Orchestrator",
            NewNode::Pipeline => "Pipeline",
            NewNode::Subagent => "Sub-agent",
            NewNode::File => "File",
            NewNode::Review => "Review",
            NewNode::Action => "Action",
        }
    }

    /// The key in probe names and new ids: `canvas-add-subagent`, `subagent-2`.
    pub fn key(self) -> &'static str {
        match self {
            NewNode::Source => "source",
            NewNode::Router => "router",
            NewNode::Agent => "agent",
            NewNode::Orchestrator => "orchestrator",
            NewNode::Pipeline => "pipeline",
            NewNode::Subagent => "subagent",
            NewNode::File => "file",
            NewNode::Review => "review",
            NewNode::Action => "action",
        }
    }

    /// Its icon.
    pub fn icon(self) -> IconName {
        match self {
            NewNode::Source => IconName::Source,
            NewNode::Router => IconName::Router,
            NewNode::Agent => IconName::Agent,
            NewNode::Orchestrator => IconName::Orchestrator,
            NewNode::Pipeline => IconName::Pipelines,
            NewNode::Subagent => IconName::Spawn,
            NewNode::File => IconName::File,
            NewNode::Review => IconName::Inbox,
            NewNode::Action => IconName::Zap,
        }
    }

    fn tone(self) -> TileTone {
        match self {
            NewNode::Source => TileTone::Info,
            NewNode::Subagent => TileTone::Spawn,
            _ => TileTone::Neutral,
        }
    }

    /// One line on what it does.
    pub fn description(self) -> &'static str {
        match self {
            NewNode::Source => "Where the items come from",
            NewNode::Router => "Sends each item one way, first match wins",
            NewNode::Agent => "One model with tools, skills and limits",
            NewNode::Orchestrator => "Splits work across a team, can create agents",
            NewNode::Pipeline => "Another pipeline as one step",
            NewNode::Subagent => "A helper an agent may create while it works",
            NewNode::File => "Files an agent reads, or one it writes",
            NewNode::Review => "A person picks a variant or sends it back",
            NewNode::Action => "A command, a merge or a pull request",
        }
    }

    /// Whether it can be added yet.
    pub fn available(self) -> bool {
        self != NewNode::Pipeline
    }

    /// The kind a new node starts as. `workdir` is the pipeline's first folder.
    fn kind(self, workdir: &str) -> Option<NodeKind> {
        let workdir = workdir.to_string();
        let agent = || AgentNode { workdir: workdir.clone(), variants: 1, ..AgentNode::default() };
        Some(match self {
            NewNode::Source => NodeKind::Source(SourceNode::default()),
            NewNode::Router => NodeKind::Router(RouterNode {
                rules: vec![RouterRule { id: "rule1".into(), label: "New rule".into(), when: String::new(), ask: None }],
            }),
            NewNode::Agent => NodeKind::Agent(agent()),
            NewNode::Orchestrator => NodeKind::Orchestrator(agent()),
            NewNode::Pipeline => return None,
            NewNode::Subagent => NodeKind::Subagent(SubagentNode { per_run: 1, max_steps: 20, ..SubagentNode::default() }),
            NewNode::File => NodeKind::File(FileNode { workdir: workdir.clone(), path: "docs/**".into(), mode: FileMode::Read }),
            NewNode::Review => NodeKind::Review(ReviewNode::default()),
            NewNode::Action => NodeKind::Action(ActionNode::Command {
                workdir: workdir.clone(),
                command: String::new(),
                tests: false,
                parse: String::new(),
                timeout_secs: 0,
            }),
        })
    }
}

/// `base`, or `base-2`, `base-3`… whichever no node has.
fn fresh_id(graph: &GraphDef, base: &str) -> NodeId {
    let taken = |id: &str| graph.nodes.iter().any(|n| n.id.as_str() == id);
    if !taken(base) {
        return NodeId::new(base);
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).map(NodeId::new).unwrap_or_else(|| NodeId::new(base))
}

/// `label`, or `label 2`… whichever no node is called.
fn fresh_name(graph: &GraphDef, label: &str) -> String {
    let taken = |name: &str| graph.nodes.iter().any(|n| n.name == name);
    if !taken(label) {
        return label.to_string();
    }
    (2..).map(|n| format!("{label} {n}")).find(|name| !taken(name)).unwrap_or_else(|| label.to_string())
}

fn snap(v: f32) -> f32 {
    (v / GRID_UNIT).round() * GRID_UNIT
}

/// The box of every node, canvas units.
fn boxes(graph: &GraphDef) -> Vec<Bounds<f32>> {
    graph
        .nodes
        .iter()
        .map(|n| Bounds::new(point(n.pos.x, n.pos.y), size(NODE_WIDTH, node_height(look(graph, n).rows.len()))))
        .collect()
}

/// Rings of grid steps searched for a free spot.
const SEARCH_RINGS: i32 = 12;

/// The top-left corner for a node of `rows` rows centred on `at`, on the grid and clear of
/// the nodes already there (the nearest free spot, ring by ring).
fn free_spot(graph: &GraphDef, at: Point<f32>, rows: usize) -> Pos {
    let start = point(snap(at.x - NODE_WIDTH / 2.0), snap(at.y - HEADER_HEIGHT / 2.0));
    let taken = boxes(graph);
    let room = size(NODE_WIDTH + GRID_UNIT, node_height(rows) + GRID_UNIT);
    let free = |p: Point<f32>| {
        let mine = Bounds::new(point(p.x - GRID_UNIT / 2.0, p.y - GRID_UNIT / 2.0), room);
        !taken.iter().any(|b| b.intersects(&mine))
    };
    for ring in 0..=SEARCH_RINGS {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs() != ring && dy.abs() != ring {
                    continue;
                }
                let p = point(start.x + dx as f32 * GRID_UNIT, start.y + dy as f32 * GRID_UNIT);
                if free(p) {
                    return Pos { x: p.x, y: p.y };
                }
            }
        }
    }
    Pos { x: start.x, y: start.y }
}

/// A new node of `what` centred on `at` (canvas units), with a fresh id and name; `None` for a
/// kind that cannot be added yet.
pub fn new_node(graph: &GraphDef, what: NewNode, at: Point<f32>) -> Option<NodeDef> {
    let workdir = graph.workdirs.first().map(|w| w.id.clone()).unwrap_or_default();
    let mut node = NodeDef {
        id: fresh_id(graph, what.key()),
        name: fresh_name(graph, what.label()),
        pos: Pos::default(),
        kind: what.kind(&workdir)?,
    };
    node.pos = free_spot(graph, at, look(graph, &node).rows.len());
    Some(node)
}

/// The add menu: every kind with its icon and line; "Pipeline" is not there yet. Choosing one
/// adds it at `at` (canvas units), or at the centre of the view.
pub fn menu(canvas: WeakEntity<CanvasView>, at: Option<Point<f32>>) -> Menu {
    let items = NewNode::ALL.into_iter().map(|what| {
        let item = MenuItem::new(ElementId::Name(format!("canvas-add-{}", what.key()).into()), what.label())
            .tile(Tile::new(what.icon()).tone(what.tone()).medium())
            .subline(what.description());
        if !what.available() {
            return item.coming_soon();
        }
        let canvas = canvas.clone();
        item.on_click(move |_, _: &mut Window, cx: &mut App| {
            canvas.update(cx, |canvas, cx| canvas.add_node(what, at, cx)).ok();
        })
    });
    Menu::new("canvas-add-menu").width(340.0).items(items)
}
