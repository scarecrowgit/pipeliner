//! The agents of an issue's latest run, as a comment can mention them, and what one is told
//! when a mention wakes it in its own session.

use std::collections::HashSet;

use model::{GraphDef, Issue, NodeKind, NodeStatus, Run, RunAgent, RunState, Runnable, RunnableKind, RunnableRef, SpanKind};

#[cfg(test)]
mod tests;

/// The agents and orchestrators of `run` that a comment can wake: one per node and variant
/// that had a session (its latest attempt), in the run's order. Steps, reviews and spawned
/// sub-agents are left out. `graph` is the graph the run executed, when it is known: it tells
/// orchestrators apart and names the library runnable a node uses.
pub fn run_agents(run: &Run, graph: Option<&GraphDef>, runnables: &[Runnable]) -> Vec<RunAgent> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for record in &run.nodes {
        if !seen.insert((record.node.clone(), record.variant)) {
            continue;
        }
        let Some(latest) = run.instance(&record.node, record.variant) else { continue };
        let Some(session) = latest.session.clone() else { continue };
        if latest.spawned_by.is_some() || matches!(latest.kind, SpanKind::Step | SpanKind::Waiting | SpanKind::Spawned) {
            continue;
        }
        let def = graph.and_then(|g| g.nodes.iter().find(|n| n.id == latest.node));
        let (kind, uses) = match def.map(|n| &n.kind) {
            Some(NodeKind::Agent(a)) => (RunnableKind::Agent, a.runnable.as_ref()),
            Some(NodeKind::Orchestrator(a)) => (RunnableKind::Orchestrator, a.runnable.as_ref()),
            Some(_) => continue,
            None => (RunnableKind::Agent, None),
        };
        let letter = latest.letter();
        let name = if letter.is_empty() { latest.name.trim().to_string() } else { format!("{} {letter}", latest.name.trim()) };
        let runnable_name = uses
            .and_then(|r| runnables.iter().find(|known| &known.reference == r))
            .map(|r| r.name.trim().to_string())
            .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&name) && variant_free(latest.variant));
        out.push(RunAgent {
            reference: RunnableRef::session(&session),
            name,
            runnable_name,
            kind,
            session,
            run: run.id.clone(),
            run_number: run.number,
            node: latest.node.clone(),
            variant: latest.variant,
            worktree: latest.worktree.clone(),
            branch: latest.branch.clone(),
            busy: latest.status == NodeStatus::Running,
        });
    }
    out
}

/// Variants of one node share its runnable: its name would not tell them apart.
fn variant_free(variant: u32) -> bool {
    variant == 0
}

/// What a woken agent of the run is told before the comment: the issue, which step of which
/// run it was, how the run stands after its step (the failed step and its error, so it can be
/// fixed), and where it works now.
pub fn wake_context(issue: &Issue, run: &Run, agent: &RunAgent) -> String {
    let mut lines = vec![format!(
        "You are back on issue {}: {}. You worked on it as the {} step of run {}.",
        issue.key,
        issue.title.trim(),
        agent.name,
        run.number
    )];
    lines.push(standing(run, agent));
    let place = match (&agent.worktree, &agent.branch) {
        (Some(path), Some(branch)) => format!("You continue in the same worktree ({}, branch `{branch}`)", path.display()),
        (Some(path), None) => format!("You continue in the same folder ({})", path.display()),
        _ => "You continue in the same conversation".to_string(),
    };
    lines.push(format!("{place}, under the same tools and permissions as in the run."));
    if agent.worktree.is_some() {
        lines.push(
            "File tools work inside that worktree only; shell commands start in it. When something you need is out of \
             your reach, say so in your reply instead of working around it."
                .to_string(),
        );
    }
    lines.push(
        "The run itself does not go on until a person resumes it. Your reply is posted on the issue as your comment."
            .to_string(),
    );
    lines.join("\n")
}

/// The full message of a woken agent: the context, then the comment.
pub fn wake_message(issue: &Issue, run: &Run, agent: &RunAgent, comment: &str) -> String {
    format!("{}\n\n## New comment\n\n{}", wake_context(issue, run, agent), comment.trim())
}

/// How the run stands, as seen from the agent's step.
fn standing(run: &Run, agent: &RunAgent) -> String {
    let n = run.number;
    let mine = run.instance(&agent.node, agent.variant);
    match run.state {
        RunState::Failed => {
            let failed = run
                .nodes
                .iter()
                .rev()
                .filter(|r| r.status == NodeStatus::Failed)
                .find(|r| run.instance(&r.node, r.variant).is_some_and(|latest| latest.attempt == r.attempt));
            // The run's error is the step's full error ("Merge mockup failed: …"); the step's
            // summary may be cut. Either names what the agent has to look at.
            let error = |name: &str, summary: &str| {
                let full = run.error.as_deref().and_then(|e| e.strip_prefix(&format!("{name} failed: "))).map(str::trim);
                let summary = summary.trim();
                match full {
                    Some(full) if full.len() >= summary.len() => full.to_string(),
                    _ if !summary.is_empty() => summary.to_string(),
                    _ => run.error.clone().unwrap_or_default(),
                }
            };
            match failed {
                Some(step) if step.node == agent.node && step.variant == agent.variant => {
                    format!("Run {n} failed at your step: {}", error(&step.name, &step.summary))
                }
                Some(step) => format!("Run {n} failed after your step, at the {} step: {}", step.name, error(&step.name, &step.summary)),
                None => format!("Run {n} failed: {}", run.error.clone().unwrap_or_else(|| "no reason was kept".into())),
            }
        }
        RunState::Cancelled => format!("Run {n} was cancelled."),
        RunState::Done => format!("Run {n} is done."),
        RunState::WaitingReview => format!("Run {n} waits for a person's review."),
        RunState::WaitingAnswer => match mine.and_then(|m| m.question.as_deref()) {
            Some(question) => format!("Run {n} waits for the answer to your question: {}", question.trim()),
            None => format!("Run {n} waits for a person's answer."),
        },
        RunState::Interrupted => format!("Run {n} was interrupted when the app closed."),
        RunState::Queued | RunState::Running => format!("Run {n} is still working on other steps."),
    }
}
