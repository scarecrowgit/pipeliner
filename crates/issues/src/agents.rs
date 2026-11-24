//! The agents of an issue's latest run, as a comment can mention them, and what one is told
//! when a mention wakes it in its own session.

use std::collections::HashSet;

use model::{
    GraphDef, Issue, NodeKind, NodeStatus, Run, RunAgent, RunState, Runnable, RunnableKind, RunnableRef, SessionId, SpanKind,
};

#[cfg(test)]
mod tests;

/// The agents of a run a comment can mention, and the names they cannot be told apart by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunAgents {
    pub agents: Vec<RunAgent>,
    /// Library runnable names several variants of one node answer to, none of them shipped.
    pub shared: Vec<SharedName>,
}

/// A library runnable's name that several variants of a run's node used: a mention of it is
/// refused with the names to use instead, never sent to one of them by guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedName {
    pub name: String,
    /// "Mockup A", "Mockup B".
    pub agents: Vec<String>,
    pub run_number: u32,
}

impl SharedName {
    /// What a mention that names it [`RunnableRef`]s: `shared:Designer`.
    pub fn reference(&self) -> RunnableRef {
        RunnableRef(format!("shared:{}", self.name))
    }

    /// Why a mention of it wakes nobody.
    pub fn reason(&self) -> String {
        format!(
            "{} worked as {} in run {}; mention one of them by that name",
            self.name,
            join_names(&self.agents),
            self.run_number
        )
    }
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl RunAgents {
    /// Everything a mention can name, the agents first: the shared names come as entries whose
    /// reference is [`SharedName::reference`], so a mention of them is found (and refused)
    /// instead of reaching a library runnable of that name.
    pub fn mentionable(&self) -> Vec<RunAgent> {
        let mut out = self.agents.clone();
        for shared in &self.shared {
            if let Some(first) = self.agents.iter().find(|a| shared.agents.contains(&a.name)) {
                out.push(RunAgent { reference: shared.reference(), name: shared.name.clone(), runnable_name: None, ..first.clone() });
            }
        }
        out
    }

    /// The shared name a mention's reference stands for.
    pub fn shared(&self, reference: &RunnableRef) -> Option<&SharedName> {
        self.shared.iter().find(|s| &s.reference() == reference)
    }
}

/// The agents and orchestrators of `run` that a comment can wake: one per node and variant
/// that had a session, in the run's order, at its latest attempt (the session of the latest
/// attempt that had one, while a new attempt has not started its own). Steps, reviews and
/// spawned sub-agents are left out. `graph` is the graph the run executed, when it is known:
/// it tells orchestrators apart and names the library runnable a node uses. `shipped` are the
/// sessions of variants a review shipped: of several variants, the shipped one answers to the
/// runnable's name.
pub fn run_agents(run: &Run, graph: Option<&GraphDef>, runnables: &[Runnable]) -> Vec<RunAgent> {
    run_agents_shipped(run, graph, runnables, &[]).agents
}

/// [`run_agents`] with the shipped variants, and the runnable names variants share.
pub fn run_agents_shipped(run: &Run, graph: Option<&GraphDef>, runnables: &[Runnable], shipped: &[SessionId]) -> RunAgents {
    let mut seen = HashSet::new();
    // Each agent with the library name it may answer to.
    let mut found: Vec<(RunAgent, Option<String>)> = Vec::new();
    for record in &run.nodes {
        if !seen.insert((record.node.clone(), record.variant)) {
            continue;
        }
        let Some(latest) = run.instance(&record.node, record.variant) else { continue };
        let with_session = run
            .nodes
            .iter()
            .filter(|n| n.node == record.node && n.variant == record.variant && n.session.is_some())
            .max_by_key(|n| n.attempt);
        let Some(worked) = with_session else { continue };
        let Some(session) = worked.session.clone() else { continue };
        if worked.spawned_by.is_some() || matches!(worked.kind, SpanKind::Step | SpanKind::Waiting | SpanKind::Spawned) {
            continue;
        }
        let def = graph.and_then(|g| g.nodes.iter().find(|n| n.id == worked.node));
        let (kind, uses) = match def.map(|n| &n.kind) {
            Some(NodeKind::Agent(a)) => (RunnableKind::Agent, a.runnable.as_ref()),
            Some(NodeKind::Orchestrator(a)) => (RunnableKind::Orchestrator, a.runnable.as_ref()),
            Some(_) => continue,
            None => (RunnableKind::Agent, None),
        };
        let letter = worked.letter();
        let name = if letter.is_empty() { worked.name.trim().to_string() } else { format!("{} {letter}", worked.name.trim()) };
        let library = uses
            .and_then(|r| runnables.iter().find(|known| &known.reference == r))
            .map(|r| r.name.trim().to_string())
            .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&name));
        let agent = RunAgent {
            reference: RunnableRef::session(&session),
            name,
            runnable_name: None,
            kind,
            session,
            run: run.id.clone(),
            run_number: run.number,
            node: worked.node.clone(),
            variant: worked.variant,
            worktree: worked.worktree.clone(),
            branch: worked.branch.clone(),
            busy: latest.status == NodeStatus::Running,
        };
        found.push((agent, library));
    }
    // A library name goes to the only agent that answers to it, else to the shipped one of
    // them; otherwise nobody gets it and a mention of it is refused.
    let mut shared: Vec<SharedName> = Vec::new();
    let names: Vec<Option<String>> = found.iter().map(|(_, library)| library.clone()).collect();
    for (ix, library) in names.iter().enumerate() {
        let Some(library) = library else { continue };
        let owners: Vec<usize> =
            names.iter().enumerate().filter(|(_, n)| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(library))).map(|(i, _)| i).collect();
        let winner = match owners.as_slice() {
            [one] => Some(*one),
            many => {
                let picked: Vec<usize> = many.iter().copied().filter(|i| shipped.contains(&found[*i].0.session)).collect();
                if let [one] = picked.as_slice() { Some(*one) } else { None }
            }
        };
        match winner {
            Some(winner) if winner == ix => found[ix].0.runnable_name = Some(library.clone()),
            Some(_) => {}
            None if !shared.iter().any(|s| s.name.eq_ignore_ascii_case(library)) => shared.push(SharedName {
                name: library.clone(),
                agents: owners.iter().map(|i| found[*i].0.name.clone()).collect(),
                run_number: run.number,
            }),
            None => {}
        }
    }
    RunAgents { agents: found.into_iter().map(|(agent, _)| agent).collect(), shared }
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
