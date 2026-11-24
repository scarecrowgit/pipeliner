//! The shape of the file: the source, ids, workdirs, limits and every edge.

use std::collections::HashSet;

use model::graph::{PORT_APPROVED, PORT_ELSE, PORT_OUT};
use model::{EdgeKind, NodeDef, NodeKind, SourceKind};

use super::{Ctx, Routes};

/// The source, node ids, workdirs and limits.
pub(super) fn check(ctx: &mut Ctx) {
    sources(ctx);
    ids(ctx);
    workdirs(ctx);
    limits(ctx);
}

fn sources(ctx: &mut Ctx) {
    let sources: Vec<usize> =
        (0..ctx.graph.nodes.len()).filter(|&i| matches!(ctx.kind(i), NodeKind::Source(_))).collect();
    if sources.is_empty() {
        ctx.error(None, "Add a source: nothing starts this pipeline");
    }
    for (n, &i) in sources.iter().enumerate() {
        if n > 0 {
            ctx.error(Some(i), "Only one source is allowed");
        }
        if let NodeKind::Source(source) = ctx.kind(i) {
            match source.source {
                SourceKind::Jira => ctx.error(Some(i), "Jira sources are coming soon"),
                SourceKind::Sentry => ctx.error(Some(i), "Sentry sources are coming soon"),
                SourceKind::Issue | SourceKind::Manual | SourceKind::Schedule => {}
            }
        }
    }
}

fn ids(ctx: &mut Ctx) {
    let mut seen = HashSet::new();
    for (i, node) in ctx.graph.nodes.iter().enumerate() {
        if node.id.as_str().trim().is_empty() {
            ctx.error(Some(i), format!("{} has no id", ctx.name(i)));
        } else if !seen.insert(&node.id) {
            ctx.error(Some(i), format!("Another node already has the id “{}”", node.id));
        }
    }
}

fn workdirs(ctx: &mut Ctx) {
    let mut seen = HashSet::new();
    for workdir in &ctx.graph.workdirs {
        let name = if workdir.name.is_empty() { workdir.id.as_str() } else { workdir.name.as_str() };
        if workdir.id.trim().is_empty() {
            ctx.error(None, format!("Workdir {name} has no id"));
        } else if !seen.insert(&workdir.id) {
            ctx.error(None, format!("Two workdirs share the id “{}”", workdir.id));
        }
        if workdir.path.as_os_str().is_empty() {
            ctx.error(None, format!("Workdir {name} has no folder"));
        }
    }
}

fn limits(ctx: &mut Ctx) {
    let limits = &ctx.graph.limits;
    if limits.max_parallel == 0 {
        ctx.error(None, "Max parallel is 0, so no agent would ever start");
    }
    if limits.max_loops == 0 {
        ctx.warn(None, "Max loops is 0: the first Send back stops the run");
    }
    if let Some(budget) = limits.budget_usd
        && !(budget.is_finite() && budget > 0.0)
    {
        ctx.error(None, "The run budget must be more than $0.00");
    }
}

/// Whether `node` has a `route` out-port named `port`.
pub(super) fn has_route_port(node: &NodeDef, port: &str) -> bool {
    match &node.kind {
        NodeKind::Router(router) => port == PORT_ELSE || router.rules.iter().any(|rule| rule.id == port),
        NodeKind::Review(_) => port == PORT_APPROVED,
        NodeKind::Subagent(_) | NodeKind::File(_) => false,
        NodeKind::Source(_)
        | NodeKind::Agent(_)
        | NodeKind::Orchestrator(_)
        | NodeKind::Action(_)
        | NodeKind::Pipeline(_) => port == PORT_OUT,
    }
}

fn is_agent(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))
}

/// Checks every edge and returns the ones the flow checks may rely on.
pub(super) fn edges(ctx: &mut Ctx) -> Routes {
    let n = ctx.graph.nodes.len();
    let mut routes = Routes {
        succ: vec![Vec::new(); n],
        pred: vec![Vec::new(); n],
        ports: vec![Vec::new(); n],
        back: Vec::new(),
        attached: vec![false; n],
        spawns: vec![false; n],
    };
    let mut seen = HashSet::new();
    for edge in &ctx.graph.edges {
        let (from, to) = match (ctx.find(&edge.from), ctx.find(&edge.to)) {
            (Some(from), Some(to)) => (from, to),
            (None, to) => {
                ctx.error(to, format!("An edge starts at a missing node “{}”", edge.from));
                continue;
            }
            (Some(from), None) => {
                ctx.error(Some(from), format!("{} connects to a missing node “{}”", ctx.name(from), edge.to));
                continue;
            }
        };
        if !seen.insert((&edge.from, &edge.port, &edge.to, edge.kind)) {
            ctx.warn(Some(from), format!("The edge {} → {} is drawn twice", ctx.name(from), ctx.name(to)));
            continue;
        }
        let (from_kind, to_kind) = (ctx.kind(from), ctx.kind(to));
        match edge.kind {
            EdgeKind::Route => {
                if let Some(message) = route_problem(ctx, from, to, &edge.port) {
                    ctx.error(Some(from), message);
                    continue;
                }
                if !routes.succ[from].contains(&to) {
                    routes.succ[from].push(to);
                    routes.pred[to].push(from);
                }
                if !routes.ports[from].contains(&edge.port) {
                    routes.ports[from].push(edge.port.clone());
                }
            }
            EdgeKind::Back => {
                if !matches!(from_kind, NodeKind::Review(_)) {
                    ctx.error(Some(from), "Only a review can send work back");
                    continue;
                }
                routes.back.push((from, to));
            }
            EdgeKind::Context => {
                let fits = matches!(from_kind, NodeKind::File(_))
                    && (is_agent(to_kind) || matches!(to_kind, NodeKind::Subagent(_)));
                if !fits {
                    ctx.error(Some(from), "Context edges go from a file to an agent");
                    continue;
                }
                routes.attached[from] = true;
            }
            EdgeKind::Spawn => {
                if !(is_agent(from_kind) && matches!(to_kind, NodeKind::Subagent(_))) {
                    ctx.error(Some(from), "Spawn edges go from an agent or orchestrator to a sub-agent");
                    continue;
                }
                routes.attached[to] = true;
                routes.spawns[from] = true;
            }
        }
    }
    if let Some(source) = (0..n).find(|&i| matches!(ctx.kind(i), NodeKind::Source(_)))
        && routes.succ[source].is_empty()
    {
        ctx.error(Some(source), "Connect the source to the first step");
    }
    routes
}

fn route_problem(ctx: &Ctx, from: usize, to: usize, port: &str) -> Option<String> {
    let name = ctx.name(from);
    match ctx.kind(from) {
        NodeKind::Subagent(_) => return Some(format!("{name} is a sub-agent: attach it with a spawn edge, not a route")),
        NodeKind::File(_) => return Some(format!("{name} is a file: attach it with a context edge, not a route")),
        _ => {}
    }
    match ctx.kind(to) {
        NodeKind::Subagent(_) => {
            return Some(format!("{} is a sub-agent: attach it with a spawn edge, not a route", ctx.name(to)));
        }
        NodeKind::File(_) => {
            return Some(format!("{} is a file: attach it with a context edge, not a route", ctx.name(to)));
        }
        NodeKind::Source(_) => return Some("Nothing can route into the source".into()),
        _ => {}
    }
    if has_route_port(ctx.node(from), port) {
        return None;
    }
    Some(match ctx.kind(from) {
        NodeKind::Review(_) => format!("A review routes on through “approved”, not “{port}”"),
        _ => format!("{name} has no port “{port}”"),
    })
}
