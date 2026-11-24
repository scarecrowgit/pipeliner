//! How an item moves: reachability, loops made of routes, back edges and Send back targets,
//! variant scopes, reviews with nothing to look at, dead ends.

use std::collections::{BTreeSet, VecDeque};

use model::{ActionNode, NodeKind};

use super::{Ctx, Routes};

pub(super) fn check(ctx: &mut Ctx, routes: &Routes) {
    let reach: Vec<BTreeSet<usize>> = (0..ctx.graph.nodes.len()).map(|i| downstream(routes, i)).collect();
    let source = (0..ctx.graph.nodes.len()).find(|&i| matches!(ctx.kind(i), NodeKind::Source(_)));
    let live = |i: usize| source.is_none_or(|s| s == i || reach[s].contains(&i));
    reachability(ctx, routes, &live);
    loops(ctx, routes);
    send_back(ctx, routes, &reach);
    reviews(ctx, &reach, &live);
    variants(ctx, routes);
    dead_ends(ctx, routes);
}

/// Nodes reachable from `start` along routes, `start` itself only when a loop returns to it.
fn downstream(routes: &Routes, start: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<usize> = routes.succ[start].iter().copied().collect();
    while let Some(i) = queue.pop_front() {
        if seen.insert(i) {
            queue.extend(routes.succ[i].iter().copied());
        }
    }
    seen
}

fn is_attached_kind(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Subagent(_) | NodeKind::File(_))
}

/// `live` says whether the source reaches a node (every node counts when there is no source).
fn reachability(ctx: &mut Ctx, routes: &Routes, live: &dyn Fn(usize) -> bool) {
    let n = ctx.graph.nodes.len();
    for i in 0..n {
        let kind = ctx.kind(i);
        if !is_attached_kind(kind) && !matches!(kind, NodeKind::Source(_)) && !live(i) {
            ctx.warn(Some(i), format!("Nothing routes to {}, so it never runs", ctx.name(i)));
        }
    }
    for i in 0..n {
        if is_attached_kind(ctx.kind(i)) && !routes.attached[i] {
            ctx.warn(Some(i), format!("{} is not attached to any agent", ctx.name(i)));
        }
    }
}

/// Strongly connected components of the route graph (Tarjan), each sorted by file order.
fn components(routes: &Routes) -> Vec<Vec<usize>> {
    struct State<'r> {
        routes: &'r Routes,
        next: usize,
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        out: Vec<Vec<usize>>,
    }
    fn visit(s: &mut State, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on_stack[v] = true;
        for &w in &s.routes.succ[v] {
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(iw) if s.on_stack[w] => s.low[v] = s.low[v].min(iw),
                Some(_) => {}
            }
        }
        if Some(s.low[v]) == s.index[v] {
            let mut component = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            component.sort_unstable();
            s.out.push(component);
        }
    }
    let n = routes.succ.len();
    let mut state = State {
        routes,
        next: 0,
        index: vec![None; n],
        low: vec![0; n],
        stack: Vec::new(),
        on_stack: vec![false; n],
        out: Vec::new(),
    };
    for v in 0..n {
        if state.index[v].is_none() {
            visit(&mut state, v);
        }
    }
    state.out
}

fn loops(ctx: &mut Ctx, routes: &Routes) {
    for component in components(routes) {
        let first = component[0];
        let closed = component.len() > 1 || routes.succ[first].contains(&first);
        if closed {
            let names: Vec<&str> = component.iter().map(|&i| ctx.name(i)).collect();
            ctx.error(
                Some(first),
                format!("Routes loop through {}; only a review’s back edge may go back", names.join(", ")),
            );
        }
    }
}

fn send_back(ctx: &mut Ctx, routes: &Routes, reach: &[BTreeSet<usize>]) {
    for &(review, target) in &routes.back {
        if !reach[target].contains(&review) {
            ctx.error(
                Some(review),
                format!("The back edge of {} must point upstream; {} is not before it", ctx.name(review), ctx.name(target)),
            );
        }
    }
    for review in 0..ctx.graph.nodes.len() {
        let NodeKind::Review(node) = ctx.kind(review) else { continue };
        let Some(id) = &node.send_back_to else { continue };
        let Some(target) = ctx.find(id) else {
            ctx.error(Some(review), format!("Send back goes to a missing node “{id}”"));
            continue;
        };
        if !reach[target].contains(&review) {
            ctx.error(
                Some(review),
                format!("Send back must restart a node before the review; {} is not upstream", ctx.name(target)),
            );
            continue;
        }
        let drawn: Vec<usize> = routes.back.iter().filter(|(r, _)| *r == review).map(|(_, t)| *t).collect();
        if !drawn.is_empty() && !drawn.contains(&target) {
            ctx.warn(
                Some(review),
                format!("Send back restarts {}, but the back edge points elsewhere", ctx.name(target)),
            );
        }
    }
}

fn reviewable(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_) | NodeKind::Action(ActionNode::Render { .. }))
}

fn reviews(ctx: &mut Ctx, reach: &[BTreeSet<usize>], live: &dyn Fn(usize) -> bool) {
    for review in 0..ctx.graph.nodes.len() {
        if !matches!(ctx.kind(review), NodeKind::Review(_)) {
            continue;
        }
        let something = (0..ctx.graph.nodes.len()).any(|u| live(u) && reach[u].contains(&review) && reviewable(ctx.kind(u)));
        if !something {
            ctx.warn(
                Some(review),
                format!("{} has nothing to review: put it after an agent or a render", ctx.name(review)),
            );
        }
    }
}

/// An agent with variants opens a scope that every path must close with a review before the
/// graph ends or meets a branch without variants.
fn variants(ctx: &mut Ctx, routes: &Routes) {
    for agent in 0..ctx.graph.nodes.len() {
        let (NodeKind::Agent(node) | NodeKind::Orchestrator(node)) = ctx.kind(agent) else { continue };
        if node.variants <= 1 {
            continue;
        }
        let mut scope = BTreeSet::from([agent]);
        let mut queue = VecDeque::from([agent]);
        while let Some(i) = queue.pop_front() {
            for &next in &routes.succ[i] {
                if !matches!(ctx.kind(next), NodeKind::Review(_)) && scope.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        if let Some(&end) = scope.iter().find(|&&i| routes.succ[i].is_empty()) {
            ctx.error(
                Some(agent),
                format!(
                    "The variants of {} reach the end at {} without a review; add a review after it",
                    ctx.name(agent),
                    ctx.name(end)
                ),
            );
        }
        for &i in scope.iter().filter(|&&i| i != agent) {
            if routes.pred[i].iter().any(|p| !scope.contains(p)) {
                ctx.error(
                    Some(i),
                    format!(
                        "{} joins the variants of {} with another branch; put a review before it",
                        ctx.name(i),
                        ctx.name(agent)
                    ),
                );
            }
        }
    }
}

fn dead_ends(ctx: &mut Ctx, routes: &Routes) {
    for i in 0..ctx.graph.nodes.len() {
        let open = matches!(
            ctx.kind(i),
            NodeKind::Router(_) | NodeKind::Agent(_) | NodeKind::Orchestrator(_) | NodeKind::Pipeline(_)
        );
        if open && routes.succ[i].is_empty() {
            ctx.warn(Some(i), format!("Nothing comes after {}; the run ends there", ctx.name(i)));
        }
    }
}
