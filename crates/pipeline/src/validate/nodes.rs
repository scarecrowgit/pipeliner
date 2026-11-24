//! Each node's own fields: router rules, agents, sub-agents, files, actions, nested pipelines.

use std::collections::HashSet;

use model::graph::PORT_ELSE;
use model::{ActionNode, AgentNode, NodeKind, RouterNode, SourceKind};

use super::{Ctx, MAX_VARIANTS, Routes};
use crate::expr;

pub(super) fn check(ctx: &mut Ctx, routes: &Routes) {
    let from_issues = ctx.graph.nodes.iter().any(|node| {
        matches!(&node.kind, NodeKind::Source(source) if source.source == SourceKind::Issue)
    });
    for i in 0..ctx.graph.nodes.len() {
        match ctx.kind(i) {
            NodeKind::Router(router) => self::router(ctx, routes, i, router),
            NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => self::agent(ctx, routes, i, agent),
            NodeKind::Subagent(sub) => {
                if sub.role.trim().is_empty() {
                    ctx.error(Some(i), format!("{} has no role", ctx.name(i)));
                }
                if !sub.model.is_empty() && !ctx.library.has_model(&sub.model) {
                    ctx.error(Some(i), format!("Unknown model “{}”", sub.model));
                }
                if sub.per_run == 0 {
                    ctx.warn(Some(i), format!("{} may be created 0 times per run", ctx.name(i)));
                }
            }
            NodeKind::File(file) => {
                workdir(ctx, i, &file.workdir, true);
                if file.path.trim().is_empty() {
                    ctx.error(Some(i), format!("{} has no path", ctx.name(i)));
                }
            }
            NodeKind::Action(action) => self::action(ctx, i, action, from_issues),
            NodeKind::Pipeline(_) => ctx.error(Some(i), "Nested pipelines are not available yet"),
            NodeKind::Source(_) | NodeKind::Review(_) => {}
        }
    }
}

/// Checks a workdir reference of node `i`; `required` makes an empty one an error.
fn workdir(ctx: &mut Ctx, i: usize, id: &str, required: bool) {
    if id.is_empty() {
        if required {
            ctx.error(Some(i), format!("{} has no workdir", ctx.name(i)));
        }
    } else if ctx.graph.workdir(id).is_none() {
        ctx.error(Some(i), format!("{} uses an unknown workdir “{id}”", ctx.name(i)));
    }
}

fn router(ctx: &mut Ctx, routes: &Routes, i: usize, router: &RouterNode) {
    if router.rules.is_empty() {
        ctx.warn(Some(i), format!("{} has no rules, so everything goes to else", ctx.name(i)));
    }
    let mut ids = HashSet::new();
    for rule in &router.rules {
        let label = if rule.label.trim().is_empty() { rule.id.as_str() } else { rule.label.as_str() };
        if rule.id.trim().is_empty() {
            ctx.error(Some(i), format!("Rule “{label}” has no id"));
        } else if rule.id == PORT_ELSE {
            ctx.error(Some(i), "A rule cannot be called “else”");
        } else if !ids.insert(rule.id.as_str()) {
            ctx.error(Some(i), format!("Two rules share the id “{}”", rule.id));
        }
        let when = rule.when.trim();
        match (&rule.ask, when.is_empty()) {
            (None, true) => ctx.error(Some(i), format!("Rule “{label}” has no condition")),
            (Some(_), false) => ctx.error(Some(i), format!("Rule “{label}” has both a condition and a question; keep one")),
            (None, false) => {
                if let Err(err) = expr::parse(when) {
                    ctx.error(Some(i), format!("Rule “{label}”: {err}"));
                }
            }
            (Some(ask), true) => {
                if ask.question.trim().is_empty() {
                    ctx.error(Some(i), format!("Rule “{label}” asks an empty question"));
                }
                if !ask.model.is_empty() && !ctx.library.has_model(&ask.model) {
                    ctx.error(Some(i), format!("Rule “{label}” uses an unknown model “{}”", ask.model));
                }
                if !ask.workdir.is_empty() && ctx.graph.workdir(&ask.workdir).is_none() {
                    ctx.error(Some(i), format!("Rule “{label}” reads an unknown workdir “{}”", ask.workdir));
                }
            }
        }
        if !rule.id.is_empty() && !routes.ports[i].contains(&rule.id) {
            ctx.error(Some(i), format!("Rule “{label}” leads nowhere; connect its port"));
        }
    }
    if !routes.ports[i].iter().any(|port| port == PORT_ELSE) {
        ctx.warn(Some(i), format!("{} has no else route: items no rule matches end the run", ctx.name(i)));
    }
}

fn agent(ctx: &mut Ctx, routes: &Routes, i: usize, agent: &AgentNode) {
    let name = ctx.name(i);
    match &agent.runnable {
        None if agent.role.trim().is_empty() => {
            ctx.error(Some(i), format!("{name} needs an agent from your library or a role"));
        }
        None => {}
        Some(reference) => {
            let kind_fits = matches!(reference.scheme(), "agent" | "orchestrator");
            if !kind_fits || !ctx.library.has_runnable(reference) {
                ctx.error(Some(i), format!("Unknown agent “{reference}”"));
            }
        }
    }
    if let Some(model) = &agent.model
        && !model.is_empty()
        && !ctx.library.has_model(model)
    {
        ctx.error(Some(i), format!("Unknown model “{model}”"));
    }
    workdir(ctx, i, &agent.workdir, false);
    if agent.variants == 0 {
        ctx.error(Some(i), format!("{name} has 0 variants"));
    } else if agent.variants > MAX_VARIANTS {
        ctx.error(Some(i), format!("{name} has {} variants; at most {MAX_VARIANTS}", agent.variants));
    } else if agent.variants > 1 && !ctx.graph.workdir(&agent.workdir).is_some_and(|w| w.worktree) {
        ctx.error(Some(i), format!("Variants of {name} need a workdir with worktrees"));
    }
    if let Some(budget) = agent.budget_usd
        && !(budget.is_finite() && budget > 0.0)
    {
        ctx.error(Some(i), format!("The budget of {name} must be more than $0.00"));
    }
    if agent.max_steps == Some(0) {
        ctx.error(Some(i), format!("{name} may take 0 steps"));
    }
    if routes.spawns[i] && !agent.spawn.as_ref().is_some_and(|spawn| spawn.enabled) {
        ctx.warn(Some(i), format!("{name} has sub-agents but may not spawn them"));
    }
}

fn action(ctx: &mut Ctx, i: usize, action: &ActionNode, from_issues: bool) {
    let name = ctx.name(i);
    match action {
        ActionNode::Command { workdir: id, command, .. } => {
            workdir(ctx, i, id, true);
            if command.trim().is_empty() {
                ctx.error(Some(i), format!("{name} has no command"));
            }
        }
        ActionNode::Render { workdir: id, files } => {
            workdir(ctx, i, id, true);
            if files.trim().is_empty() {
                ctx.error(Some(i), format!("{name} has no files to render"));
            }
        }
        ActionNode::Merge { workdir: id } | ActionNode::PullRequest { workdir: id, .. } => {
            workdir(ctx, i, id, true);
            if let Some(dir) = ctx.graph.workdir(id)
                && !dir.worktree
            {
                let what = if matches!(action, ActionNode::Merge { .. }) { "A merge" } else { "A pull request" };
                ctx.error(Some(i), format!("{what} needs a workdir with worktrees; {} is used as is", dir.name));
            }
        }
        ActionNode::Report => {}
        ActionNode::UpdateIssue { status, comment } => {
            if !from_issues {
                ctx.warn(Some(i), format!("{name} updates an issue, but the source does not start from issues"));
            }
            if status.is_none() && comment.trim().is_empty() {
                ctx.warn(Some(i), format!("{name} changes nothing: set a status or a comment"));
            }
        }
    }
}
