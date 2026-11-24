//! The static shape of a graph as the executor reads it: which nodes the item can reach, in
//! which order, along which `route` edges.

use std::collections::{BTreeMap, BTreeSet};

use model::{EdgeKind, GraphDef, NodeDef, NodeId, NodeKind};

/// Whether the item itself passes through a node of this type.
pub(super) fn is_flow(kind: &NodeKind) -> bool {
    !matches!(kind, NodeKind::File(_) | NodeKind::Subagent(_))
}

/// Agents and orchestrators: the jobs `limits.max_parallel` counts.
pub(super) fn is_agent(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))
}

/// Variant indices of a node that asks for `count` variants: `[0]` when it opens no scope.
pub(super) fn variant_set(count: u32) -> Vec<u32> {
    if count >= 2 { (1..=count).collect() } else { vec![0] }
}

/// Route edges between the nodes an item can reach from a source.
pub(super) struct Plan {
    /// Reachable flow nodes, every node after all nodes with a route edge into it.
    pub order: Vec<NodeId>,
    /// Source nodes in graph order.
    pub sources: Vec<NodeId>,
    /// Why the graph cannot run at all.
    pub problem: Option<String>,
    incoming: BTreeMap<NodeId, Vec<(NodeId, String)>>,
    outgoing: BTreeMap<NodeId, Vec<NodeId>>,
}

impl Plan {
    pub fn new(graph: &GraphDef) -> Plan {
        let flow: BTreeSet<&NodeId> = graph.nodes.iter().filter(|node| is_flow(&node.kind)).map(|node| &node.id).collect();
        let sources: Vec<NodeId> =
            graph.nodes.iter().filter(|node| matches!(node.kind, NodeKind::Source(_))).map(|node| node.id.clone()).collect();
        let routes: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Route && flow.contains(&edge.from) && flow.contains(&edge.to))
            .collect();

        // What a source reaches.
        let mut reached: BTreeSet<NodeId> = sources.iter().cloned().collect();
        let mut stack: Vec<NodeId> = sources.clone();
        while let Some(node) = stack.pop() {
            for edge in routes.iter().filter(|edge| edge.from == node) {
                if reached.insert(edge.to.clone()) {
                    stack.push(edge.to.clone());
                }
            }
        }

        let mut incoming: BTreeMap<NodeId, Vec<(NodeId, String)>> = BTreeMap::new();
        let mut outgoing: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for edge in &routes {
            if !reached.contains(&edge.from) {
                continue;
            }
            let into = incoming.entry(edge.to.clone()).or_default();
            if !into.iter().any(|(from, port)| from == &edge.from && port == &edge.port) {
                into.push((edge.from.clone(), edge.port.clone()));
            }
            let out = outgoing.entry(edge.from.clone()).or_default();
            if !out.contains(&edge.to) {
                out.push(edge.to.clone());
            }
        }

        // Kahn's sort in graph order, so the result is the same on every machine.
        let mut waiting: BTreeMap<&NodeId, usize> =
            reached.iter().map(|id| (id, incoming.get(id).map_or(0, |edges| distinct_sources(edges)))).collect();
        let mut order: Vec<NodeId> = Vec::new();
        loop {
            let next = graph.nodes.iter().map(|node| &node.id).find(|id| waiting.get(id) == Some(&0));
            let Some(next) = next else { break };
            waiting.remove(next);
            for to in outgoing.get(next).map(Vec::as_slice).unwrap_or_default() {
                if let Some(count) = waiting.get_mut(to) {
                    *count = count.saturating_sub(1);
                }
            }
            order.push(next.clone());
        }
        let mut problem = None;
        if sources.is_empty() {
            problem = Some("The pipeline has no source node.".to_string());
        } else if !waiting.is_empty() {
            let names: Vec<String> =
                graph.nodes.iter().filter(|node| waiting.contains_key(&node.id)).map(|node| node.name.clone()).collect();
            problem = Some(format!("The pipeline has a loop of route edges through {}.", names.join(", ")));
        }
        Plan { order, sources, problem, incoming, outgoing }
    }

    /// Route edges into `node` as (from, port).
    pub fn incoming(&self, node: &NodeId) -> &[(NodeId, String)] {
        self.incoming.get(node).map(Vec::as_slice).unwrap_or_default()
    }

    /// Every node the item can reach after `node`, without `node` itself.
    pub fn descendants(&self, node: &NodeId) -> BTreeSet<NodeId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![node.clone()];
        while let Some(at) = stack.pop() {
            for to in self.outgoing.get(&at).map(Vec::as_slice).unwrap_or_default() {
                if seen.insert(to.clone()) {
                    stack.push(to.clone());
                }
            }
        }
        seen
    }

    /// Whether the item can get from `from` to `to` along route edges.
    pub fn reaches(&self, from: &NodeId, to: &NodeId) -> bool {
        self.descendants(from).contains(to)
    }
}

fn distinct_sources(edges: &[(NodeId, String)]) -> usize {
    edges.iter().map(|(from, _)| from).collect::<BTreeSet<_>>().len()
}

/// File nodes and sub-agent templates handed to the job of `node`: what its `context` and
/// `spawn` edges touch, and the files attached to those templates.
pub(super) fn attached(graph: &GraphDef, node: &NodeId) -> Vec<NodeDef> {
    let mut ids: Vec<&NodeId> = Vec::new();
    let mut add = |id| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };
    let neighbours = |of: &NodeId, kind: EdgeKind| {
        graph
            .edges
            .iter()
            .filter(move |edge| edge.kind == kind && (&edge.from == of || &edge.to == of))
            .map(move |edge| if &edge.from == of { &edge.to } else { &edge.from })
            .collect::<Vec<_>>()
    };
    let is_file = |id: &NodeId| graph.node(id).is_some_and(|def| matches!(def.kind, NodeKind::File(_)));
    let is_template = |id: &NodeId| graph.node(id).is_some_and(|def| matches!(def.kind, NodeKind::Subagent(_)));
    for id in neighbours(node, EdgeKind::Context) {
        if is_file(id) {
            add(id);
        }
    }
    for template in neighbours(node, EdgeKind::Spawn) {
        if !is_template(template) {
            continue;
        }
        add(template);
        for id in neighbours(template, EdgeKind::Context) {
            if is_file(id) {
                add(id);
            }
        }
    }
    ids.into_iter().filter_map(|id| graph.node(id).cloned()).collect()
}
