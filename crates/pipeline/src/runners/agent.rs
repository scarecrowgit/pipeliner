//! Agent and orchestrator jobs: a session in the instance's worktree that finishes with
//! `step_done`.

use std::collections::HashMap;

use agent::{AgentEnv, AgentOutcome, AgentSpec, Finish, FinishTool, Input, StopCause, SubagentTemplate};
use anyhow::{Result, anyhow, bail};
use model::{
    AgentDef, AgentEvent, AgentNode, LogLevel, ModelRef, NodeKind, Runnable, RunnableRef, SessionId, StartFrom,
    ToolDisplay,
};

use super::places::Place;
use super::services::SessionOpen;
use super::task::feedback;
use super::{Ctx, failed};
use crate::api::{JobNote, JobResult, NodeOutput, SpawnedState};

/// Longest description of a sub-agent template, from the start of its role.
const TEMPLATE_DESCRIPTION: usize = 160;

pub(crate) fn run(ctx: &Ctx, node: &AgentNode) -> JobResult {
    match run_inner(ctx, node) {
        Ok(result) => result,
        Err(error) => failed(format!("{error:#}")),
    }
}

/// The library agent with the node's overrides: its display name, who spends, and the definition.
fn resolve(ctx: &Ctx, node: &AgentNode) -> Result<(String, RunnableRef, AgentDef)> {
    let sessions = &ctx.services().sessions;
    let found = match &node.runnable {
        Some(reference) => Some(sessions.runnable(reference).ok_or_else(|| anyhow!("{reference} is no longer in the library"))?),
        None => None,
    };
    node_agent(&ctx.job.node.name, node, found.as_ref(), &ctx.runner.pipeline_id, sessions.default_model())
}

/// What an agent or orchestrator node of the pipeline with id `pipeline` runs as: the library agent it uses
/// (`library`, looked up from `node.runnable`) with the node's overrides, or the node's own
/// definition. Gives its display name, the runnable that spends, and the definition; fails
/// with a reason a person can act on when there is no model or no role.
///
/// The engine also uses it to continue a node's session after its run, under the same rules.
pub fn node_agent(
    node_name: &str,
    node: &AgentNode,
    library: Option<&Runnable>,
    pipeline: &str,
    default_model: ModelRef,
) -> Result<(String, RunnableRef, AgentDef)> {
    let (name, reference, mut def) = match (&node.runnable, library) {
        (Some(reference), Some(found)) => {
            let def = found.agent.clone().ok_or_else(|| anyhow!("{reference} is not an agent or orchestrator"))?;
            (found.name.clone(), reference.clone(), def)
        }
        (Some(reference), None) => bail!("{reference} is no longer in the library"),
        (None, _) => (node_name.to_string(), RunnableRef::pipeline(pipeline), AgentDef::default()),
    };
    if !node.role.trim().is_empty() {
        def.role = node.role.clone();
    }
    if let Some(model) = node.model.as_ref().filter(|m| !m.is_empty()) {
        def.model = model.clone();
    }
    if let Some(tools) = &node.tools {
        def.tools = tools.clone();
    }
    if let Some(skills) = &node.skills {
        def.skills = skills.clone();
    }
    if let Some(spawn) = &node.spawn {
        def.spawn = spawn.clone();
    }
    if let Some(steps) = node.max_steps {
        def.max_steps = steps;
    }
    if def.model.is_empty() {
        def.model = default_model;
    }
    if def.model.is_empty() {
        bail!("{node_name} has no model and none is configured: pick one on the node or add a provider");
    }
    if def.role.trim().is_empty() {
        bail!("{node_name} has no role: write one on the node or pick a library agent");
    }
    Ok((name, reference, def))
}

/// Sub-agent nodes attached by `spawn` edges, as templates the agent may create.
fn templates(ctx: &Ctx) -> Vec<SubagentTemplate> {
    ctx.job
        .attached
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Subagent(sub) => Some(SubagentTemplate {
                name: node.name.clone(),
                description: base::fmt::truncate_chars(base::fmt::first_line(sub.role.trim()), TEMPLATE_DESCRIPTION),
                role: sub.role.clone(),
                model: (!sub.model.is_empty()).then(|| sub.model.clone()),
                tools: sub.tools.clone(),
                skills: sub.skills.clone(),
                max_steps: sub.max_steps,
                per_run: sub.per_run,
            }),
            _ => None,
        })
        .collect()
}

fn run_inner(ctx: &Ctx, node: &AgentNode) -> Result<JobResult> {
    let job = ctx.job;
    let sessions = ctx.services().sessions.clone();
    let (name, reference, def) = resolve(ctx, node)?;
    let wd = if node.workdir.is_empty() { None } else { Some(ctx.workdir(&node.workdir)?) };
    let place = match wd {
        Some(wd) => Some(ctx.work_place(wd)?),
        None => None,
    };
    let task = ctx.task(place.as_ref(), &node.workdir, &node.files_out)?;

    // A sent-back attempt keeps its conversation only when it also keeps its folder.
    let kept = job.retry.as_ref().filter(|retry| {
        retry.start != StartFrom::Over
            && retry.from_session.is_some()
            && place.as_ref().map(|p| Some(&p.path)) == Some(retry.from_worktree.as_ref())
    });
    let (session, input) = match (&job.resume_session, kept) {
        (Some(session), _) => {
            let input = match &job.answer {
                Some(answer) => Input::User(agent::UserInput::text(answer.clone())),
                // Stopped before the task was even stored: hand it over again.
                None if !sessions.has_conversation(session) => Input::Item(task.text.clone()),
                None => Input::Continue,
            };
            (session.clone(), input)
        }
        (None, Some(retry)) => {
            let text = format!(
                "{}Carry on in the same folder, then call `step_done` again with a summary of what changed.",
                feedback(&retry.notes)
            );
            (retry.from_session.clone().unwrap_or_default(), Input::User(agent::UserInput::text(text)))
        }
        (None, None) => {
            let open = SessionOpen {
                title: format!("{} · {}", job.node.name, ctx.title()),
                runnable: reference.clone(),
                model: def.model.clone(),
                run: job.run.clone(),
                node: job.key.node.clone(),
                variant: job.key.variant,
                issue: job.item.issue.as_ref().map(|issue| issue.key.clone()),
                workdir: place.as_ref().map(|p| p.path.clone()),
                source: wd.map(|w| w.path.clone()),
                branch: place.as_ref().and_then(|p| p.branch.clone()),
            };
            (sessions.open(&open)?, Input::Item(task.text.clone()))
        }
    };
    ctx.sink.note(JobNote::Place {
        session: Some(session.clone()),
        worktree: place.as_ref().map(|p| p.path.clone()),
        branch: place.as_ref().and_then(|p| p.branch.clone()),
    });

    let mut spec = AgentSpec::from_def(name.clone(), reference, &def);
    spec.finish = Finish::Tool(FinishTool::summary());
    spec.templates = templates(ctx);
    if !spec.templates.is_empty() {
        spec.spawn.enabled = true;
    }
    let spent = sessions.spent(&session);
    let left = job.budget_left_usd.map(|left| spent + left.max(0.0));
    // The agent's own cap counts the step's spend only, not answers given in between.
    let own = def.budget_usd.map(|cap| cap + sessions.spent_outside_runs(&session));
    spec.budget_usd = match (left, own) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let env = AgentEnv {
        session: session.clone(),
        depth: 0,
        workdir: place.as_ref().map(|p| p.path.clone()),
        repo: wd.map(|w| w.path.clone()),
        env: wd.map(|w| w.env.clone()).unwrap_or_default(),
        read_only: false,
        mode: Default::default(),
        run: Some(job.run.clone()),
        node: Some(job.key.node.clone()),
        created_by: None,
    };
    let mut names: HashMap<SessionId, String> = HashMap::new();
    let mut on_event = |sid: &SessionId, event: &AgentEvent| note_event(ctx, &session, &mut names, sid, event);
    let outcome = sessions.run(&spec, &env, input, &mut on_event, ctx.cancel);
    Ok(finish(ctx, &name, &spec, outcome, session, place, &task.required))
}

/// Turns what the agent reports into the run's notes.
fn note_event(ctx: &Ctx, top: &SessionId, names: &mut HashMap<SessionId, String>, sid: &SessionId, event: &AgentEvent) {
    match event {
        AgentEvent::Usage { usage, cost_usd, .. } => ctx.sink.note(JobNote::Spent { cost_usd: *cost_usd, usage: *usage }),
        AgentEvent::ToolCallReady { display, name, .. } if sid == top => ctx.log(LogLevel::Tool, tool_line(name, display)),
        AgentEvent::SubagentStarted { session, name, task, .. } => {
            names.insert(session.clone(), name.clone());
            ctx.sink.note(JobNote::Spawned {
                id: session.to_string(),
                name: name.clone(),
                session: Some(session.clone()),
                state: SpawnedState::Running,
                summary: base::fmt::truncate_chars(base::fmt::first_line(task), 120),
            });
        }
        AgentEvent::SubagentFinished { session, summary, ok, .. } => ctx.sink.note(JobNote::Spawned {
            id: session.to_string(),
            name: names.get(session).cloned().unwrap_or_else(|| "Sub-agent".to_string()),
            session: Some(session.clone()),
            state: if *ok { SpawnedState::Done } else { SpawnedState::Failed },
            summary: base::fmt::truncate_chars(base::fmt::first_line(summary), 200),
        }),
        AgentEvent::Retrying { attempt, reason } if sid == top => {
            ctx.log(LogLevel::Warn, format!("Retrying the model call ({attempt}): {reason}"));
        }
        AgentEvent::Notice { text } if sid == top => ctx.log(LogLevel::Info, text.clone()),
        AgentEvent::Failed { error } if sid == top => ctx.log(LogLevel::Error, error.clone()),
        _ => {}
    }
}

/// "read src/lib.rs", "$ cargo test".
fn tool_line(name: &str, display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::Line { verb, target, .. } => format!("{verb} {target}").trim().to_string(),
        ToolDisplay::Command { command, .. } => format!("$ {}", base::fmt::truncate_chars(base::fmt::first_line(command), 160)),
        ToolDisplay::Diff(diff) => format!("edit {}", diff.path),
        ToolDisplay::Text { title, .. } if !title.is_empty() => title.clone(),
        ToolDisplay::Text { .. } => name.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    ctx: &Ctx,
    name: &str,
    spec: &AgentSpec,
    outcome: AgentOutcome,
    session: SessionId,
    place: Option<Place>,
    required: &[super::task::Required],
) -> JobResult {
    let files = match place.as_ref() {
        Some(place) if place.branch.is_some() => vcs::changed_files(&place.path, &place.base, ctx.cancel).unwrap_or_else(|error| {
            ctx.log(LogLevel::Warn, format!("Could not list the changed files: {error:#}"));
            outcome.files.clone()
        }),
        _ => outcome.files.clone(),
    };
    let mut output = NodeOutput {
        summary: String::new(),
        session: Some(session),
        worktree: place.as_ref().map(|p| p.path.clone()),
        branch: place.as_ref().and_then(|p| p.branch.clone()),
        files,
        cost_usd: outcome.totals.cost_usd,
        usage: outcome.totals.usage,
        ..Default::default()
    };
    match outcome.stop {
        StopCause::Finished => {
            let Some(payload) = outcome.payload else {
                output.summary = outcome.text.trim().to_string();
                return JobResult::Failed { error: format!("{name} stopped without calling step_done"), output };
            };
            output.summary = payload.get("summary").and_then(|s| s.as_str()).unwrap_or_default().trim().to_string();
            let mut missing: Vec<&str> = Vec::new();
            for file in required {
                match file.check() {
                    Ok(true) => {}
                    Ok(false) => missing.push(file.shown.as_str()),
                    Err(error) => return JobResult::Failed { error: format!("{name}: {error:#}"), output },
                }
            }
            if !missing.is_empty() {
                let error = format!("{name} finished without writing {}", missing.join(", "));
                return JobResult::Failed { error, output };
            }
            JobResult::Done { port: model::graph::PORT_OUT.into(), output }
        }
        StopCause::Waiting { question } => {
            output.summary = format!("Asked: {}", base::fmt::first_line(&question));
            JobResult::Ask { question, output }
        }
        StopCause::Cancelled => JobResult::Cancelled { output },
        StopCause::Budget => JobResult::OverBudget { output },
        StopCause::StepLimit => JobResult::Failed {
            error: format!("{name} used all {} steps without finishing; raise its max steps, or resume to let it go on", spec.step_limit()),
            output,
        },
        StopCause::Failed { error } => JobResult::Failed { error, output },
    }
}
