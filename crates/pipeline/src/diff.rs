//! "Changes since v14": what differs between two versions of a graph, one line per node and
//! one per piece of wiring, worded for the deploy dialog ("variants 2 → 3, model
//! sonnet-5.5 → opus-5.5").
//!
//! Positions on the canvas are not changes. Edges that start or end at an added or removed node
//! are part of that node's line, not separate wiring lines.

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use base::fmt::cost;
use model::{
    ActionNode, AgentNode, EdgeDef, EdgeKind, FileMode, GraphChange, GraphDef, Limits, ModelRef, NodeDef, NodeKind,
    RouterNode, SourceKind, SubagentNode, WorkdirDef,
};

/// Every change from `old` to `new`: nodes in the order of `new` (removed ones after), then
/// wiring. Empty when the graphs are equivalent.
pub fn diff(old: &GraphDef, new: &GraphDef) -> Vec<GraphChange> {
    let mut out = Vec::new();
    for node in &new.nodes {
        match old.node(&node.id) {
            None => out.push(GraphChange::Added { node: node.id.clone(), name: node.name.clone(), what: added(node) }),
            Some(before) => {
                let what = node_changes(before, node);
                if !what.is_empty() {
                    out.push(GraphChange::Changed { node: node.id.clone(), name: node.name.clone(), what: what.join(", ") });
                }
            }
        }
    }
    for node in &old.nodes {
        if new.node(&node.id).is_none() {
            out.push(GraphChange::Removed { node: node.id.clone(), name: node.name.clone() });
        }
    }
    let mut wiring = Vec::new();
    graph_lines(old, new, &mut wiring);
    edges(old, new, &mut wiring);
    workdirs(&old.workdirs, &new.workdirs, &mut wiring);
    limits(&old.limits, &new.limits, &mut wiring);
    out.extend(wiring.into_iter().map(|what| GraphChange::Wiring { what }));
    out
}

fn kind_word(kind: &NodeKind) -> String {
    kind.label().to_lowercase()
}

fn model_word(model: &ModelRef) -> String {
    if model.is_empty() { "default".into() } else { model.short().to_string() }
}

fn opt_model(model: &Option<ModelRef>) -> String {
    model.as_ref().map_or_else(|| "default".into(), model_word)
}

fn added(node: &NodeDef) -> String {
    let word = kind_word(&node.kind);
    match &node.kind {
        NodeKind::Router(router) => format!("new {word}: {}", plural(router.rules.len(), "rule")),
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => match (&agent.model, agent.variants) {
            (Some(model), v) if v > 1 => format!("new {word}: {}, {v} variants", model_word(model)),
            (Some(model), _) => format!("new {word}: {}", model_word(model)),
            (None, v) if v > 1 => format!("new {word}: {v} variants"),
            (None, _) => format!("new {word}"),
        },
        NodeKind::Action(action) => format!("new {word}: {}", action_word(action)),
        _ => format!("new {word}"),
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

fn action_word(action: &ActionNode) -> &'static str {
    match action {
        ActionNode::Command { .. } => "command",
        ActionNode::Render { .. } => "render",
        ActionNode::Merge { .. } => "merge",
        ActionNode::PullRequest { .. } => "pull request",
        ActionNode::Report => "report",
        ActionNode::UpdateIssue { .. } => "update issue",
    }
}

fn source_word(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Issue => "issues",
        SourceKind::Manual => "manual",
        SourceKind::Schedule => "schedule",
        SourceKind::Jira => "Jira",
        SourceKind::Sentry => "Sentry",
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// Collects "field a → b" and "field edited" lines.
struct Lines(Vec<String>);

impl Lines {
    fn arrow(&mut self, field: &str, old: impl ToString, new: impl ToString) {
        let (old, new) = (old.to_string(), new.to_string());
        if old != new {
            self.0.push(format!("{field} {old} → {new}"));
        }
    }

    fn edited<T: PartialEq>(&mut self, field: &str, old: &T, new: &T) {
        if old != new {
            self.0.push(format!("{field} edited"));
        }
    }
}

fn node_changes(old: &NodeDef, new: &NodeDef) -> Vec<String> {
    let mut lines = Lines(Vec::new());
    lines.arrow("renamed", &old.name, &new.name);
    match (&old.kind, &new.kind) {
        (NodeKind::Source(a), NodeKind::Source(b)) => lines.arrow("source", source_word(a.source), source_word(b.source)),
        (NodeKind::Router(a), NodeKind::Router(b)) => router(&mut lines, a, b),
        (NodeKind::Agent(a), NodeKind::Agent(b)) | (NodeKind::Orchestrator(a), NodeKind::Orchestrator(b)) => {
            agent(&mut lines, a, b)
        }
        (NodeKind::Subagent(a), NodeKind::Subagent(b)) => subagent(&mut lines, a, b),
        (NodeKind::File(a), NodeKind::File(b)) => {
            lines.arrow("workdir", &a.workdir, &b.workdir);
            lines.arrow("path", &a.path, &b.path);
            let mode = |m: FileMode| if m == FileMode::Read { "read" } else { "write" };
            lines.arrow("mode", mode(a.mode), mode(b.mode));
        }
        (NodeKind::Review(a), NodeKind::Review(b)) => {
            let target = |t: &Option<model::NodeId>| t.as_ref().map_or_else(|| "none".to_string(), |t| t.to_string());
            lines.arrow("send back", target(&a.send_back_to), target(&b.send_back_to));
            lines.arrow("skip when empty", on_off(a.skip_if_empty), on_off(b.skip_if_empty));
        }
        (NodeKind::Action(a), NodeKind::Action(b)) => action(&mut lines, a, b),
        (NodeKind::Pipeline(a), NodeKind::Pipeline(b)) => lines.arrow("pipeline", &a.pipeline, &b.pipeline),
        (a, b) => lines.arrow("type", kind_word(a), kind_word(b)),
    }
    lines.0
}

fn router(lines: &mut Lines, old: &RouterNode, new: &RouterNode) {
    let label = |rule: &model::RouterRule| if rule.label.is_empty() { rule.id.clone() } else { rule.label.clone() };
    for rule in &new.rules {
        match old.rules.iter().find(|r| r.id == rule.id) {
            None => lines.0.push(format!("rule added: {}", label(rule))),
            Some(before) if before != rule => lines.0.push(format!("rule edited: {}", label(rule))),
            Some(_) => {}
        }
    }
    for rule in &old.rules {
        if !new.rules.iter().any(|r| r.id == rule.id) {
            lines.0.push(format!("rule removed: {}", label(rule)));
        }
    }
    let order = |rules: &[model::RouterRule], other: &[model::RouterRule]| -> Vec<String> {
        rules.iter().filter(|r| other.iter().any(|o| o.id == r.id)).map(|r| r.id.clone()).collect()
    };
    if order(&old.rules, &new.rules) != order(&new.rules, &old.rules) {
        lines.0.push("rules reordered".into());
    }
}

fn agent(lines: &mut Lines, old: &AgentNode, new: &AgentNode) {
    let runnable = |r: &Option<model::RunnableRef>| r.as_ref().map_or_else(|| "none".to_string(), |r| r.to_string());
    lines.arrow("agent", runnable(&old.runnable), runnable(&new.runnable));
    lines.edited("role", &old.role, &new.role);
    lines.arrow("model", opt_model(&old.model), opt_model(&new.model));
    lines.arrow("workdir", &old.workdir, &new.workdir);
    lines.arrow("variants", old.variants, new.variants);
    lines.edited("tools", &old.tools, &new.tools);
    lines.edited("skills", &old.skills, &new.skills);
    let spawning = |a: &AgentNode| a.spawn.as_ref().is_some_and(|s| s.enabled);
    if spawning(old) != spawning(new) {
        lines.arrow("spawning", on_off(spawning(old)), on_off(spawning(new)));
    } else {
        lines.edited("spawn limits", &old.spawn, &new.spawn);
    }
    let money = |b: Option<f64>| b.map_or_else(|| "none".to_string(), cost);
    lines.arrow("budget", money(old.budget_usd), money(new.budget_usd));
    let steps = |s: Option<u32>| s.map_or_else(|| "default".to_string(), |s| s.to_string());
    lines.arrow("max steps", steps(old.max_steps), steps(new.max_steps));
    lines.edited("files out", &old.files_out, &new.files_out);
}

fn subagent(lines: &mut Lines, old: &SubagentNode, new: &SubagentNode) {
    lines.edited("role", &old.role, &new.role);
    lines.arrow("model", model_word(&old.model), model_word(&new.model));
    lines.edited("tools", &old.tools, &new.tools);
    lines.edited("skills", &old.skills, &new.skills);
    lines.arrow("max steps", old.max_steps, new.max_steps);
    lines.arrow("per run", old.per_run, new.per_run);
}

fn action(lines: &mut Lines, old: &ActionNode, new: &ActionNode) {
    use ActionNode as A;
    match (old, new) {
        (
            A::Command { workdir: w1, command: c1, tests: t1, parse: p1, timeout_secs: s1 },
            A::Command { workdir: w2, command: c2, tests: t2, parse: p2, timeout_secs: s2 },
        ) => {
            lines.arrow("workdir", w1, w2);
            lines.edited("command", c1, c2);
            lines.arrow("tests", on_off(*t1), on_off(*t2));
            lines.edited("parse", p1, p2);
            lines.arrow("timeout", base::fmt::duration(*s1), base::fmt::duration(*s2));
        }
        (A::Render { workdir: w1, files: f1 }, A::Render { workdir: w2, files: f2 }) => {
            lines.arrow("workdir", w1, w2);
            lines.arrow("files", f1, f2);
        }
        (A::Merge { workdir: w1 }, A::Merge { workdir: w2 }) => lines.arrow("workdir", w1, w2),
        (
            A::PullRequest { workdir: w1, draft: d1, push: p1 },
            A::PullRequest { workdir: w2, draft: d2, push: p2 },
        ) => {
            lines.arrow("workdir", w1, w2);
            lines.arrow("draft", on_off(*d1), on_off(*d2));
            lines.arrow("push", on_off(*p1), on_off(*p2));
        }
        (A::Report, A::Report) => {}
        (A::UpdateIssue { status: s1, comment: c1 }, A::UpdateIssue { status: s2, comment: c2 }) => {
            let status = |s: &Option<model::IssueStatus>| s.map_or("unchanged", |s| s.label());
            lines.arrow("status", status(s1), status(s2));
            lines.edited("comment", c1, c2);
        }
        (a, b) => lines.arrow("action", action_word(a), action_word(b)),
    }
}

fn graph_lines(old: &GraphDef, new: &GraphDef, out: &mut Vec<String>) {
    if old.name != new.name {
        out.push(format!("renamed {} → {}", old.name, new.name));
    }
    if old.description != new.description {
        out.push("description edited".into());
    }
}

fn edge_line(graph: &GraphDef, edge: &EdgeDef) -> String {
    let name = |id: &model::NodeId| graph.node(id).map_or_else(|| id.to_string(), |n| n.name.clone());
    let kind = match edge.kind {
        EdgeKind::Route => "",
        EdgeKind::Context => "context ",
        EdgeKind::Spawn => "spawn ",
        EdgeKind::Back => "back ",
    };
    let router = graph.node(&edge.from).is_some_and(|n| matches!(n.kind, NodeKind::Router(_)));
    let port = if router { format!(" ({})", edge.port) } else { String::new() };
    format!("{kind}{}{port} → {}", name(&edge.from), name(&edge.to))
}

fn edges(old: &GraphDef, new: &GraphDef, out: &mut Vec<String>) {
    let both = |id: &model::NodeId| old.node(id).is_some() && new.node(id).is_some();
    let key = |e: &EdgeDef| (e.from.clone(), e.port.clone(), e.to.clone(), e.kind);
    let old_set: HashSet<_> = old.edges.iter().map(key).collect();
    let new_set: HashSet<_> = new.edges.iter().map(key).collect();
    let mut seen = HashSet::new();
    for edge in &new.edges {
        if both(&edge.from) && both(&edge.to) && !old_set.contains(&key(edge)) && seen.insert(key(edge)) {
            out.push(format!("edge added: {}", edge_line(new, edge)));
        }
    }
    for edge in &old.edges {
        if both(&edge.from) && both(&edge.to) && !new_set.contains(&key(edge)) && seen.insert(key(edge)) {
            out.push(format!("edge removed: {}", edge_line(old, edge)));
        }
    }
}

fn workdirs(old: &[WorkdirDef], new: &[WorkdirDef], out: &mut Vec<String>) {
    let name = |w: &WorkdirDef| if w.name.is_empty() { w.id.clone() } else { w.name.clone() };
    for dir in new {
        let Some(before) = old.iter().find(|w| w.id == dir.id) else {
            out.push(format!("workdir added: {}", name(dir)));
            continue;
        };
        let mut lines = Lines(Vec::new());
        lines.arrow("renamed", &before.name, &dir.name);
        lines.arrow("folder", before.path.display(), dir.path.display());
        lines.arrow("worktrees", on_off(before.worktree), on_off(dir.worktree));
        lines.arrow("base branch", &before.base_branch, &dir.base_branch);
        lines.arrow("branch prefix", &before.branch_prefix, &dir.branch_prefix);
        lines.edited("setup", &before.setup, &dir.setup);
        lines.edited("environment", &before.env, &dir.env);
        if !lines.0.is_empty() {
            out.push(format!("workdir {}: {}", name(dir), lines.0.join(", ")));
        }
    }
    for dir in old {
        if !new.iter().any(|w| w.id == dir.id) {
            out.push(format!("workdir removed: {}", name(dir)));
        }
    }
}

fn limits(old: &Limits, new: &Limits, out: &mut Vec<String>) {
    let mut lines = Lines(Vec::new());
    lines.arrow("max parallel", old.max_parallel, new.max_parallel);
    let money = |b: Option<f64>| b.map_or_else(|| "none".to_string(), cost);
    lines.arrow("run budget", money(old.budget_usd), money(new.budget_usd));
    lines.arrow("max loops", old.max_loops, new.max_loops);
    out.extend(lines.0);
}
