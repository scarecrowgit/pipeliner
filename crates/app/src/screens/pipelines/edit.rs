//! Pure graph editing and layout for the pipeline editor. No UI here, so it is unit-tested.

use std::collections::{HashMap, HashSet, VecDeque};

use harness_core::{Edge, Id, Node, NodeKind, PipelineGraph, ToolNode};

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// A lowercase id derived from `base` that no node in `g` uses yet.
pub fn unique_node_id(g: &PipelineGraph, base: &str) -> Id {
    let slug = slug(base, "node");
    if g.node(&slug).is_none() {
        return slug;
    }
    (2..)
        .map(|i| format!("{slug}{i}"))
        .find(|id| g.node(id).is_none())
        .unwrap_or(slug)
}

/// A workdir id that no workdir in `g` uses yet.
pub fn unique_workdir_id(g: &PipelineGraph, base: &str) -> Id {
    let slug = slug(base, "work");
    if g.workdir(&slug).is_none() {
        return slug;
    }
    (2..)
        .map(|i| format!("{slug}{i}"))
        .find(|id| g.workdir(id).is_none())
        .unwrap_or(slug)
}

fn slug(base: &str, fallback: &str) -> String {
    let mut s = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

/// Adds a node. With `after`, it goes below that node: a single-port node's outgoing wires
/// move to the new node and `after -> new` is wired; a branching node wires its first free
/// port. Without `after` it is appended after the deepest node that has no outgoing wire.
/// Source nodes are never wired in. Returns the new node's id.
pub fn add_node(g: &mut PipelineGraph, name: &str, kind: NodeKind, after: Option<&str>) -> Id {
    let id = unique_node_id(g, name);
    let is_source = matches!(kind, NodeKind::Source(_));
    let new_has_default = kind.ports().contains(&None);
    g.nodes.push(Node {
        id: id.clone(),
        name: name.to_string(),
        kind,
        x: 0.0,
        y: 0.0,
    });
    if is_source {
        return id;
    }
    let anchor = match after.filter(|a| g.node(a).is_some()) {
        Some(a) => Some(a.to_string()),
        None => last_sink(g, &id),
    };
    let Some(anchor) = anchor else { return id };
    let ports = g.node(&anchor).map(|n| n.kind.ports()).unwrap_or_default();
    if ports == vec![None] {
        if new_has_default && after.is_some() {
            for e in g
                .edges
                .iter_mut()
                .filter(|e| e.from == anchor && e.port.is_none())
            {
                e.from = id.clone();
            }
        }
        g.edges.push(Edge {
            from: anchor,
            to: id.clone(),
            port: None,
        });
    } else {
        let free = ports
            .iter()
            .find(|p| !g.edges.iter().any(|e| e.from == anchor && &e.port == *p))
            .cloned();
        let port = free.or_else(|| ports.first().cloned()).flatten();
        g.edges.push(Edge {
            from: anchor,
            to: id.clone(),
            port,
        });
    }
    id
}

/// The deepest node (other than `except`) without outgoing wires.
fn last_sink(g: &PipelineGraph, except: &str) -> Option<Id> {
    let layout = layout(g);
    g.nodes
        .iter()
        .filter(|n| n.id != except && !g.edges.iter().any(|e| e.from == n.id))
        .max_by_key(|n| {
            layout
                .pos
                .get(&n.id)
                .map(|p| (p.row, usize::MAX - p.lane))
                .unwrap_or((0, 0))
        })
        .map(|n| n.id.clone())
}

/// Removes a node, its wires, and gate send-back targets pointing at it.
pub fn remove_node(g: &mut PipelineGraph, id: &str) {
    g.nodes.retain(|n| n.id != id);
    g.edges.retain(|e| e.from != id && e.to != id);
    for n in &mut g.nodes {
        if let NodeKind::Gate(gate) = &mut n.kind {
            if gate.back_to.as_deref() == Some(id) {
                gate.back_to = None;
            }
        }
    }
}

/// Wires `from.port -> to` unless that exact wire exists (or it would be a self-loop).
pub fn connect(g: &mut PipelineGraph, from: &str, port: Option<String>, to: &str) -> bool {
    if from == to
        || g.node(to).is_none()
        || g.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.port == port)
    {
        return false;
    }
    g.edges.push(Edge {
        from: from.into(),
        to: to.into(),
        port,
    });
    true
}

pub fn disconnect(g: &mut PipelineGraph, from: &str, port: &Option<String>, to: &str) {
    g.edges
        .retain(|e| !(e.from == from && e.to == to && &e.port == port));
}

/// All nodes that can reach `id` through wires.
pub fn upstream(g: &PipelineGraph, id: &str) -> Vec<Id> {
    let mut seen: HashSet<Id> = HashSet::new();
    let mut queue: VecDeque<Id> = VecDeque::from([id.to_string()]);
    while let Some(cur) = queue.pop_front() {
        for e in g.incoming(&cur) {
            if e.from != id && seen.insert(e.from.clone()) {
                queue.push_back(e.from.clone());
            }
        }
    }
    g.nodes
        .iter()
        .filter(|n| seen.contains(&n.id))
        .map(|n| n.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Router rules (ports `rule1..N` follow rule order)
// ---------------------------------------------------------------------------

/// Removes rule `ix` of router `node`: its wires go, later rules' wires shift up.
pub fn remove_rule(g: &mut PipelineGraph, node: &str, ix: usize) {
    let Some(NodeKind::Router(r)) = g.node_mut(node).map(|n| &mut n.kind) else {
        return;
    };
    if ix >= r.rules.len() {
        return;
    }
    r.rules.remove(ix);
    let gone = rule_port(ix);
    g.edges
        .retain(|e| !(e.from == node && e.port.as_deref() == Some(gone.as_str())));
    for e in g.edges.iter_mut().filter(|e| e.from == node) {
        if let Some(n) = e.port.as_deref().and_then(rule_index) {
            if n > ix {
                e.port = Some(rule_port(n - 1));
            }
        }
    }
}

/// Swaps rules `a` and `b` of router `node`, and their wires with them.
pub fn swap_rules(g: &mut PipelineGraph, node: &str, a: usize, b: usize) {
    let Some(NodeKind::Router(r)) = g.node_mut(node).map(|n| &mut n.kind) else {
        return;
    };
    if a >= r.rules.len() || b >= r.rules.len() || a == b {
        return;
    }
    r.rules.swap(a, b);
    for e in g.edges.iter_mut().filter(|e| e.from == node) {
        match e.port.as_deref().and_then(rule_index) {
            Some(n) if n == a => e.port = Some(rule_port(b)),
            Some(n) if n == b => e.port = Some(rule_port(a)),
            _ => {}
        }
    }
}

/// `rule1` for index 0.
pub fn rule_port(ix: usize) -> String {
    format!("rule{}", ix + 1)
}

/// Index of a `ruleN` port.
fn rule_index(port: &str) -> Option<usize> {
    port.strip_prefix("rule")?
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n - 1)
}

/// Human label of a port: `yes`, `rule 1`, `else`; `None` for the default port.
pub fn port_label(port: &Option<String>) -> Option<String> {
    let p = port.as_deref()?;
    Some(match rule_index(p) {
        Some(i) => format!("rule {}", i + 1),
        None => p.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Workdirs
// ---------------------------------------------------------------------------

/// Every node reference to a workdir, mutable.
fn workdir_refs(n: &mut Node) -> Option<&mut Option<Id>> {
    match &mut n.kind {
        NodeKind::Agent(a) => Some(&mut a.workdir),
        NodeKind::Decision(d) => Some(&mut d.workdir),
        NodeKind::Render(r) => Some(&mut r.workdir),
        NodeKind::Tool(ToolNode::Command { workdir, .. })
        | NodeKind::Tool(ToolNode::PullRequest { workdir, .. }) => Some(workdir),
        _ => None,
    }
}

/// Renames a workdir id everywhere nodes use it.
pub fn rename_workdir(g: &mut PipelineGraph, old: &str, new: &str) {
    if old == new {
        return;
    }
    if let Some(w) = g.workdirs.iter_mut().find(|w| w.id == old) {
        w.id = new.to_string();
    }
    for n in &mut g.nodes {
        if let Some(r) = workdir_refs(n) {
            if r.as_deref() == Some(old) {
                *r = Some(new.to_string());
            }
        }
    }
}

/// Removes workdir `ix`; nodes that used it lose their workdir.
pub fn remove_workdir(g: &mut PipelineGraph, ix: usize) {
    if ix >= g.workdirs.len() {
        return;
    }
    let id = g.workdirs.remove(ix).id;
    for n in &mut g.nodes {
        if let Some(r) = workdir_refs(n) {
            if r.as_deref() == Some(id.as_str()) {
                *r = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Draft vs live
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeChange {
    Added,
    Edited,
}

/// What a draft changes compared with the live graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DraftDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub edited: Vec<String>,
    pub wiring: bool,
    pub workdirs: bool,
}

impl DraftDiff {
    pub fn node_count(&self) -> usize {
        self.added.len() + self.removed.len() + self.edited.len()
    }
}

pub fn diff(live: &PipelineGraph, draft: &PipelineGraph) -> DraftDiff {
    let mut d = DraftDiff::default();
    for n in &draft.nodes {
        match live.node(&n.id) {
            None => d.added.push(n.name.clone()),
            Some(o) if o.name != n.name || o.kind != n.kind => d.edited.push(n.name.clone()),
            _ => {}
        }
    }
    d.removed = live
        .nodes
        .iter()
        .filter(|n| draft.node(&n.id).is_none())
        .map(|n| n.name.clone())
        .collect();
    let key = |e: &Edge| (e.from.clone(), e.to.clone(), e.port.clone());
    let a: HashSet<_> = live.edges.iter().map(key).collect();
    let b: HashSet<_> = draft.edges.iter().map(key).collect();
    d.wiring = a != b;
    d.workdirs = live.workdirs != draft.workdirs;
    d
}

/// Per node of the draft: added or edited compared with live.
pub fn node_changes(live: &PipelineGraph, draft: &PipelineGraph) -> HashMap<Id, NodeChange> {
    draft
        .nodes
        .iter()
        .filter_map(|n| match live.node(&n.id) {
            None => Some((n.id.clone(), NodeChange::Added)),
            Some(o) if o.name != n.name || o.kind != n.kind => {
                Some((n.id.clone(), NodeChange::Edited))
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Grid cell of a node: `row` = depth from the source, `lane` = branch column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub lane: usize,
    pub row: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub pos: HashMap<Id, Cell>,
    pub lanes: usize,
    pub rows: usize,
}

/// Outgoing wires of `id` in port order (yes before no, rule1.. before else).
pub fn ordered_out<'a>(g: &'a PipelineGraph, n: &Node) -> Vec<&'a Edge> {
    let mut out = Vec::new();
    for p in n.kind.ports() {
        out.extend(g.edges.iter().filter(|e| e.from == n.id && e.port == p));
    }
    // Wires on ports the node no longer has still show (so they can be removed).
    out.extend(
        g.edges
            .iter()
            .filter(|e| e.from == n.id && !n.kind.ports().contains(&e.port)),
    );
    out
}

/// Top-to-bottom layout: rows by longest path from the roots, branches side by side.
pub fn layout(g: &PipelineGraph) -> Layout {
    let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    let valid = |id: &str| ids.contains(&id);
    let edges: Vec<&Edge> = g
        .edges
        .iter()
        .filter(|e| valid(&e.from) && valid(&e.to) && e.from != e.to)
        .collect();

    // Rows: Kahn's algorithm (longest path); nodes left in cycles go below their known parents.
    let mut indeg: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    for e in &edges {
        *indeg.get_mut(e.to.as_str()).unwrap() += 1;
    }
    let mut row: HashMap<&str, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = ids.iter().copied().filter(|id| indeg[id] == 0).collect();
    for id in &queue {
        row.insert(id, 0);
    }
    while let Some(cur) = queue.pop_front() {
        let r = row[cur];
        for e in edges.iter().filter(|e| e.from == cur) {
            let to = e.to.as_str();
            let entry = row.entry(to).or_insert(0);
            *entry = (*entry).max(r + 1);
            let d = indeg.get_mut(to).unwrap();
            *d -= 1;
            if *d == 0 {
                queue.push_back(to);
            }
        }
    }
    // Nodes only reachable through a cycle go below their placed parents.
    while row.len() < ids.len() {
        let pending: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| !row.contains_key(id))
            .collect();
        let mut placed = false;
        for id in &pending {
            let parent = edges
                .iter()
                .filter(|e| e.to == *id)
                .filter_map(|e| row.get(e.from.as_str()))
                .map(|r| r + 1)
                .max();
            if let Some(r) = parent {
                row.insert(id, r);
                placed = true;
            }
        }
        if !placed {
            row.insert(pending[0], 0);
        }
    }

    // Lanes: depth-first from the roots; the first branch keeps the parent's lane.
    let mut lane: HashMap<&str, usize> = HashMap::new();
    let mut next_lane = 0usize;
    let mut roots: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| !edges.iter().any(|e| e.to == *id))
        .collect();
    // Source first, then anything unreachable (cycles).
    roots.sort_by_key(|id| !matches!(g.node(id).map(|n| &n.kind), Some(NodeKind::Source(_))));
    let mut stack: Vec<(&str, usize)> = Vec::new();
    let all_starts: Vec<&str> = roots.iter().copied().chain(ids.iter().copied()).collect();
    for start in all_starts {
        if lane.contains_key(start) {
            continue;
        }
        stack.push((start, next_lane));
        next_lane += 1;
        while let Some((cur, l)) = stack.pop() {
            if lane.contains_key(cur) {
                continue;
            }
            lane.insert(cur, l);
            let node = g.node(cur).unwrap();
            let children: Vec<&str> = ordered_out(g, node)
                .into_iter()
                .map(|e| e.to.as_str())
                .filter(|to| valid(to) && !lane.contains_key(to))
                .collect();
            let mut seen = HashSet::new();
            let children: Vec<&str> = children.into_iter().filter(|c| seen.insert(*c)).collect();
            // Push in reverse so the first child is visited first.
            let mut assigned: Vec<(&str, usize)> = Vec::new();
            for (i, c) in children.iter().enumerate() {
                let cl = if i == 0 {
                    l
                } else {
                    next_lane += 1;
                    next_lane - 1
                };
                assigned.push((c, cl));
            }
            for a in assigned.into_iter().rev() {
                stack.push(a);
            }
        }
    }

    // Resolve collisions: a cell holds one node.
    let mut order: Vec<&str> = ids.clone();
    order.sort_by_key(|id| {
        (
            row.get(id).copied().unwrap_or(0),
            lane.get(id).copied().unwrap_or(0),
        )
    });
    let mut taken: HashSet<(usize, usize)> = HashSet::new();
    let mut pos = HashMap::new();
    let (mut lanes, mut rows) = (0, 0);
    for id in order {
        let r = row.get(id).copied().unwrap_or(0);
        let mut l = lane.get(id).copied().unwrap_or(0);
        while taken.contains(&(l, r)) {
            l += 1;
        }
        taken.insert((l, r));
        lanes = lanes.max(l + 1);
        rows = rows.max(r + 1);
        pos.insert(id.to_string(), Cell { lane: l, row: r });
    }
    // Drop lanes nobody ended up in (lane numbers can skip after merges).
    let mut used: Vec<usize> = pos.values().map(|c| c.lane).collect();
    used.sort_unstable();
    used.dedup();
    for c in pos.values_mut() {
        c.lane = used.binary_search(&c.lane).unwrap_or(0);
    }
    lanes = used.len().min(lanes.max(1));
    Layout {
        pos,
        lanes: if g.nodes.is_empty() { 0 } else { lanes },
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::{
        AgentNode, DecisionNode, GateNode, RouterNode, RouterRule, RuleOp, SourceNode,
    };

    fn base() -> PipelineGraph {
        let mut g = PipelineGraph::default();
        g.nodes.push(Node {
            id: "source".into(),
            name: "Source".into(),
            kind: NodeKind::Source(SourceNode::default()),
            x: 0.0,
            y: 0.0,
        });
        g
    }

    #[test]
    fn appends_and_inserts() {
        let mut g = base();
        let a = add_node(
            &mut g,
            "Agent step",
            NodeKind::Agent(AgentNode::default()),
            None,
        );
        assert_eq!(a, "agent-step");
        assert!(g.edges.iter().any(|e| e.from == "source" && e.to == a));
        let gate = add_node(&mut g, "Gate", NodeKind::Gate(GateNode::default()), None);
        assert!(g.edges.iter().any(|e| e.from == a && e.to == gate));
        // Insert between source and agent.
        let b = add_node(
            &mut g,
            "Agent step",
            NodeKind::Agent(AgentNode::default()),
            Some("source"),
        );
        assert_eq!(b, "agent-step2");
        assert!(g.edges.iter().any(|e| e.from == "source" && e.to == b));
        assert!(g.edges.iter().any(|e| e.from == b && e.to == a));
        assert!(!g.edges.iter().any(|e| e.from == "source" && e.to == a));
    }

    #[test]
    fn decision_wires_free_ports() {
        let mut g = base();
        let d = add_node(
            &mut g,
            "Decision",
            NodeKind::Decision(DecisionNode::default()),
            None,
        );
        let y = add_node(
            &mut g,
            "Yes",
            NodeKind::Agent(AgentNode::default()),
            Some(&d),
        );
        let n = add_node(
            &mut g,
            "No",
            NodeKind::Agent(AgentNode::default()),
            Some(&d),
        );
        assert!(g
            .edges
            .iter()
            .any(|e| e.from == d && e.to == y && e.port.as_deref() == Some("yes")));
        assert!(g
            .edges
            .iter()
            .any(|e| e.from == d && e.to == n && e.port.as_deref() == Some("no")));
        let l = layout(&g);
        assert_eq!(l.pos[&y].row, 2);
        assert_eq!(l.pos[&n].row, 2);
        assert_ne!(l.pos[&y].lane, l.pos[&n].lane);
        assert_eq!(l.lanes, 2);
    }

    #[test]
    fn remove_node_cleans_up() {
        let mut g = base();
        let a = add_node(&mut g, "A", NodeKind::Agent(AgentNode::default()), None);
        let gate = add_node(
            &mut g,
            "G",
            NodeKind::Gate(GateNode {
                back_to: Some(a.clone()),
                ..Default::default()
            }),
            None,
        );
        remove_node(&mut g, &a);
        assert!(g.edges.is_empty());
        let NodeKind::Gate(gn) = &g.node(&gate).unwrap().kind else {
            panic!()
        };
        assert_eq!(gn.back_to, None);
    }

    #[test]
    fn router_rule_ports_follow_rules() {
        let mut g = base();
        let rule = |v: &str| RouterRule {
            field: "input".into(),
            op: RuleOp::Contains,
            value: v.into(),
        };
        let r = add_node(
            &mut g,
            "Router",
            NodeKind::Router(RouterNode {
                rules: vec![rule("a"), rule("b"), rule("c")],
            }),
            None,
        );
        let x = add_node(&mut g, "X", NodeKind::Agent(AgentNode::default()), Some(&r));
        let y = add_node(&mut g, "Y", NodeKind::Agent(AgentNode::default()), Some(&r));
        let z = add_node(&mut g, "Z", NodeKind::Agent(AgentNode::default()), Some(&r));
        swap_rules(&mut g, &r, 0, 1);
        assert!(g
            .edges
            .iter()
            .any(|e| e.to == x && e.port.as_deref() == Some("rule2")));
        assert!(g
            .edges
            .iter()
            .any(|e| e.to == y && e.port.as_deref() == Some("rule1")));
        remove_rule(&mut g, &r, 0);
        assert!(!g.edges.iter().any(|e| e.to == y));
        assert!(g
            .edges
            .iter()
            .any(|e| e.to == x && e.port.as_deref() == Some("rule1")));
        assert!(g
            .edges
            .iter()
            .any(|e| e.to == z && e.port.as_deref() == Some("rule2")));
    }

    #[test]
    fn merge_layout_has_no_collisions() {
        let mut g = base();
        let d = add_node(
            &mut g,
            "D",
            NodeKind::Decision(DecisionNode::default()),
            None,
        );
        let a = add_node(&mut g, "A", NodeKind::Agent(AgentNode::default()), Some(&d));
        let b = add_node(&mut g, "B", NodeKind::Agent(AgentNode::default()), Some(&a));
        let m = add_node(&mut g, "M", NodeKind::Agent(AgentNode::default()), Some(&b));
        connect(&mut g, &d, Some("no".into()), &m);
        let l = layout(&g);
        assert_eq!(l.pos[&m].row, 4);
        let cells: HashSet<_> = l.pos.values().map(|c| (c.lane, c.row)).collect();
        assert_eq!(cells.len(), g.nodes.len());
        assert_eq!(upstream(&g, &m).len(), 4);
    }

    #[test]
    fn cycles_still_lay_out() {
        let mut g = base();
        let a = add_node(&mut g, "A", NodeKind::Agent(AgentNode::default()), None);
        let b = add_node(&mut g, "B", NodeKind::Agent(AgentNode::default()), None);
        connect(&mut g, &b, None, &a);
        let l = layout(&g);
        assert_eq!(l.pos.len(), 3);
    }

    #[test]
    fn workdir_rename_and_remove() {
        let mut g = base();
        g.workdirs.push(harness_core::Workdir {
            id: "code".into(),
            name: "Code".into(),
            path: String::new(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: String::new(),
            setup: String::new(),
            shell: String::new(),
            env: Default::default(),
        });
        let a = add_node(
            &mut g,
            "A",
            NodeKind::Agent(AgentNode {
                workdir: Some("code".into()),
                ..Default::default()
            }),
            None,
        );
        rename_workdir(&mut g, "code", "repo");
        let NodeKind::Agent(an) = &g.node(&a).unwrap().kind else {
            panic!()
        };
        assert_eq!(an.workdir.as_deref(), Some("repo"));
        remove_workdir(&mut g, 0);
        let NodeKind::Agent(an) = &g.node(&a).unwrap().kind else {
            panic!()
        };
        assert_eq!(an.workdir, None);
    }

    #[test]
    fn diff_counts() {
        let live = base();
        let mut draft = live.clone();
        add_node(&mut draft, "A", NodeKind::Agent(AgentNode::default()), None);
        draft.nodes[0].name = "Issue".into();
        let d = diff(&live, &draft);
        assert_eq!(d.added, vec!["A".to_string()]);
        assert_eq!(d.edited, vec!["Issue".to_string()]);
        assert!(d.wiring);
        assert_eq!(node_changes(&live, &draft).len(), 2);
    }
}
