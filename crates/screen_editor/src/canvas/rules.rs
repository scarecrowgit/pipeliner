//! Which edge a drag from an out-port draws, and onto which nodes. Only whether the kind fits
//! its two ends is decided here; whether the graph makes sense is the validator's business.

use std::collections::{HashSet, VecDeque};

use hub::model::{EdgeDef, EdgeKind, FileMode, GraphDef, NodeDef, NodeId, NodeKind};

use super::look::PORT_SEND_BACK;

/// Whether `node` can receive the item along a route edge.
fn takes_route(node: &NodeDef) -> bool {
    !matches!(
        node.kind,
        NodeKind::Source(_) | NodeKind::Subagent(_) | NodeKind::File(hub::model::FileNode { mode: FileMode::Read, .. })
    )
}

fn is_agent(node: &NodeDef) -> bool {
    matches!(node.kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))
}

/// Whether `review` can be reached from `start` along route edges: `start` is upstream of it.
fn upstream(graph: &GraphDef, start: &NodeId, review: &NodeId) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start.clone()]);
    while let Some(id) = queue.pop_front() {
        if &id == review {
            return true;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        queue.extend(graph.edges_from(&id).filter(|e| e.kind == EdgeKind::Route).map(|e| e.to.clone()));
    }
    false
}

/// The kind of edge a drag from out-port `port` of `from` onto `to` makes, or `None` when the
/// two ends do not fit (or the same edge is drawn already):
///
/// - a read file feeds an agent: context;
/// - an agent attaches a sub-agent: spawn;
/// - a review's send-back port goes to a node upstream of it: back;
/// - anything else that has a route port goes to a node that takes the item: route.
pub fn edge_kind(graph: &GraphDef, from: &NodeId, port: &str, to: &NodeId) -> Option<EdgeKind> {
    if from == to {
        return None;
    }
    let (a, b) = (graph.node(from)?, graph.node(to)?);
    let kind = match &a.kind {
        NodeKind::Subagent(_) => return None,
        NodeKind::File(file) if file.mode == FileMode::Read => is_agent(b).then_some(EdgeKind::Context)?,
        NodeKind::Review(_) if port == PORT_SEND_BACK => {
            (takes_route(b) && upstream(graph, to, from)).then_some(EdgeKind::Back)?
        }
        NodeKind::Agent(_) | NodeKind::Orchestrator(_) if matches!(b.kind, NodeKind::Subagent(_)) => EdgeKind::Spawn,
        _ => takes_route(b).then_some(EdgeKind::Route)?,
    };
    let drawn = graph.edges.iter().any(|e| &e.from == from && e.port == port && &e.to == to && e.kind == kind);
    (!drawn).then_some(kind)
}

/// Adds the edge to `graph`. A back edge also becomes the review's "send back" target, so the
/// drawing and the setting agree.
pub fn add_edge(graph: &mut GraphDef, from: &NodeId, port: &str, to: &NodeId, kind: EdgeKind) {
    graph.edges.push(EdgeDef { from: from.clone(), port: port.to_string(), to: to.clone(), kind });
    if kind == EdgeKind::Back
        && let Some(NodeKind::Review(review)) = graph.nodes.iter_mut().find(|n| &n.id == from).map(|n| &mut n.kind)
    {
        review.send_back_to = Some(to.clone());
    }
}

/// Removes node `id` with every edge that touches it; a review that sent work back to it no
/// longer does.
pub fn remove_node(graph: &mut GraphDef, id: &NodeId) {
    graph.nodes.retain(|node| &node.id != id);
    graph.edges.retain(|edge| &edge.from != id && &edge.to != id);
    for node in &mut graph.nodes {
        if let NodeKind::Review(review) = &mut node.kind
            && review.send_back_to.as_ref() == Some(id)
        {
            review.send_back_to = None;
        }
    }
}

/// Removes edge `index`; removing a review's back edge also clears its "send back" target when
/// that pointed at the same node.
pub fn remove_edge(graph: &mut GraphDef, index: usize) {
    if index >= graph.edges.len() {
        return;
    }
    let edge = graph.edges.remove(index);
    if edge.kind == EdgeKind::Back
        && let Some(NodeKind::Review(review)) = graph.nodes.iter_mut().find(|n| n.id == edge.from).map(|n| &mut n.kind)
        && review.send_back_to.as_ref() == Some(&edge.to)
    {
        review.send_back_to = None;
    }
}
