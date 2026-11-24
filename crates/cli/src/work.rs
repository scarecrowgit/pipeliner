//! `issue`, `run`, `review` and `pipeline`: compact line output over the engine. Commands
//! that start or continue a run follow it until it needs a person or ends.

use std::io::Write;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use engine::Engine;
use engine::api::{
    CatalogApi, InboxApi, IssueApi, IssueKey, NewIssue, OpsApi, PipelineId, Project, ProjectId, Review, ReviewDecision,
    ReviewId, Run, RunApi, RunId, RunInput, RunLogLine, RunState, RunnableKind, RunnableRef, Severity, StartFrom,
    StartTriggers, variant_letter,
};

use crate::app::Deps;
use crate::args::{IssueCmd, PipelineCmd, ReviewCmd, RunCmd};
use crate::args_work::From;
use crate::signal;

/// How often a followed run is looked at.
const POLL: Duration = Duration::from_millis(40);

fn state_word(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::WaitingReview => "waiting for review",
        RunState::WaitingAnswer => "waiting for an answer",
        RunState::Done => "done",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
        RunState::Interrupted => "interrupted",
    }
}

fn exit_code(state: RunState) -> i32 {
    if matches!(state, RunState::Failed | RunState::Cancelled) { 1 } else { 0 }
}

fn run_or_fail(engine: &Engine, id: &str) -> Result<Run> {
    engine.run(&RunId::new(id))?.ok_or_else(|| anyhow!("there is no run {id}"))
}

fn log_line(line: &RunLogLine) -> String {
    let who = match &line.node {
        Some(node) if line.variant > 0 => format!("{node}·{}", variant_letter(line.variant)),
        Some(node) => node.to_string(),
        None => "run".to_string(),
    };
    format!("  {who:<14} {}", line.text)
}

/// Prints new log lines until the run needs a person or ends; returns its last state.
fn follow(engine: &Engine, id: &RunId, after: u64, deps: &mut Deps) -> Result<Run> {
    signal::set_in_turn(true);
    let result = follow_inner(engine, id, after, deps);
    signal::set_in_turn(false);
    result
}

fn follow_inner(engine: &Engine, id: &RunId, mut after: u64, deps: &mut Deps) -> Result<Run> {
    loop {
        for line in engine.run_log(id, after, 200)? {
            after = after.max(line.seq);
            writeln!(deps.out, "{}", log_line(&line))?;
        }
        let run = engine.run(id)?.ok_or_else(|| anyhow!("the run {id} is gone"))?;
        if !matches!(run.state, RunState::Queued | RunState::Running) {
            for line in engine.run_log(id, after, 200)? {
                writeln!(deps.out, "{}", log_line(&line))?;
            }
            return Ok(run);
        }
        if signal::take_pressed() {
            writeln!(deps.err, "^C stopped following; the run is interrupted when the CLI exits, `run resume {id}` continues it")?;
            return Ok(run);
        }
        std::thread::sleep(POLL);
    }
}

/// The last log line's number, to follow from.
fn last_seq(engine: &Engine, id: &RunId) -> u64 {
    engine.run_log_tail(id, 1).ok().and_then(|lines| lines.last().map(|l| l.seq)).unwrap_or(0)
}

/// One line about the run, and what it waits for.
fn status(engine: &Engine, run: &Run, deps: &mut Deps) -> Result<i32> {
    writeln!(deps.out, "run {} · {} · {}", run.id, state_word(run.state), base::fmt::cost(run.cost_usd))?;
    if let Some(url) = &run.pr_url {
        writeln!(deps.out, "pull request {url}")?;
    }
    if let Some(error) = &run.error {
        writeln!(deps.out, "error: {error}")?;
    }
    for node in run.nodes.iter().filter(|n| n.question.is_some()) {
        writeln!(deps.out, "{} asks: {}", node.name, node.question.as_deref().unwrap_or_default())?;
    }
    for review in engine.open_reviews()?.iter().filter(|r| r.run == run.id) {
        writeln!(deps.out, "review {} waits: {}", review.id, base::fmt::plural(review.variants.len() as u64, "variant", "variants"))?;
    }
    Ok(exit_code(run.state))
}

/// Follows a run that was just started or continued, then says where it stands.
fn followed(engine: &Engine, id: &RunId, after: u64, deps: &mut Deps) -> Result<i32> {
    let run = follow(engine, id, after, deps)?;
    status(engine, &run, deps)
}

/// The project new issues go to: the one named, the current one, the first, or a new one.
fn project_for(engine: &Engine, named: Option<String>) -> Result<ProjectId> {
    let projects = engine.projects()?;
    if let Some(id) = named {
        return projects.iter().find(|p| p.id.as_str() == id).map(|p| p.id.clone()).ok_or_else(|| anyhow!("there is no project {id}"));
    }
    let current = engine.settings()?.current_project;
    if let Some(found) = current.and_then(|c| projects.iter().find(|p| p.id == c).map(|p| p.id.clone())) {
        return Ok(found);
    }
    if let Some(first) = projects.first() {
        return Ok(first.id.clone());
    }
    let project = Project { id: ProjectId::new("default"), name: "Default".into(), key_prefix: "HAR".into(), repos: Vec::new() };
    engine.save_project(project.clone())?;
    Ok(project.id)
}

pub fn issue(engine: &Engine, cmd: IssueCmd, deps: &mut Deps) -> Result<i32> {
    match cmd {
        IssueCmd::New { title, body, pipeline, project } => {
            let runner = pipeline.map(|p| RunnableRef::pipeline(&p));
            if let Some(runner) = &runner
                && !engine.runnables().iter().any(|r| &r.reference == runner)
            {
                bail!("there is no pipeline {}", runner.id());
            }
            let project = project_for(engine, project)?;
            // Attached, not started: `issue start` starts it.
            let triggers = StartTriggers { on_assign: false, ..Default::default() };
            let issue = engine.create_issue(NewIssue { project, title, body, runner, triggers, ..Default::default() })?;
            writeln!(deps.out, "{} {}", issue.key, issue.title)?;
            Ok(0)
        }
        IssueCmd::List { project } => {
            let project = project_for(engine, project)?;
            for issue in engine.issues(&project)? {
                let runner = issue.runner.as_ref().map(|r| format!("  [{r}]")).unwrap_or_default();
                writeln!(deps.out, "{:<8} {:<12} {}{runner}", issue.key, issue.status.label(), issue.title)?;
            }
            Ok(0)
        }
        IssueCmd::Show { key } => {
            let key = IssueKey::new(key);
            let issue = engine.issue(&key)?.ok_or_else(|| anyhow!("there is no issue {key}"))?;
            writeln!(deps.out, "{} {}\n{} · {} · {}", issue.key, issue.title, issue.status.label(), issue.kind.label(), issue.priority.label())?;
            if let Some(runner) = &issue.runner {
                writeln!(deps.out, "runs with {runner}")?;
            }
            if let Some(run) = &issue.run {
                writeln!(deps.out, "latest run {run} (of {})", issue.run_count)?;
            }
            if let Some(url) = &issue.pr_url {
                writeln!(deps.out, "pull request {url}")?;
            }
            if !issue.body.trim().is_empty() {
                writeln!(deps.out, "\n{}\n", issue.body.trim())?;
            }
            for event in engine.issue_events(&key)? {
                writeln!(deps.out, "  {}", event_line(&event.kind))?;
            }
            Ok(0)
        }
        IssueCmd::Comment { key, text } => {
            let key = IssueKey::new(key);
            let before = engine.issue(&key)?.and_then(|i| i.run);
            let seq = before.as_ref().map_or(0, |id| last_seq(engine, id));
            engine.comment(&key, &text, Vec::new())?;
            writeln!(deps.out, "commented on {key}")?;
            match engine.issue(&key)?.and_then(|i| i.run) {
                Some(run) if engine.run(&run)?.is_some_and(|r| matches!(r.state, RunState::Queued | RunState::Running)) => {
                    let after = if Some(&run) == before.as_ref() { seq } else { 0 };
                    followed(engine, &run, after, deps)
                }
                _ => Ok(0),
            }
        }
        IssueCmd::Start { key } => {
            let key = IssueKey::new(key);
            engine.start_issue(&key)?;
            let issue = engine.issue(&key)?.ok_or_else(|| anyhow!("there is no issue {key}"))?;
            match issue.run {
                Some(run) if issue.runner.as_ref().is_some_and(|r| r.scheme() == "pipeline") => {
                    writeln!(deps.out, "run {run} started on {key}")?;
                    followed(engine, &run, 0, deps)
                }
                _ => {
                    writeln!(deps.out, "started on {key}")?;
                    Ok(0)
                }
            }
        }
    }
}

fn event_line(kind: &engine::api::IssueEventKind) -> String {
    use engine::api::IssueEventKind as K;
    match kind {
        K::Created => "created".into(),
        K::Comment { text, .. } => format!("comment: {}", base::fmt::first_line(text)),
        K::StatusChanged { from, to } => format!("moved {} → {}", from.label(), to.label()),
        K::Attached { runner } => format!("attached {runner}"),
        K::Detached => "detached".into(),
        K::Note { text } => format!("note: {}", base::fmt::first_line(text)),
        K::Woken { name, number, cause, .. } => format!("woke {name} (run {number}, {cause})"),
        K::WakeFailed { name, reason, .. } => format!("could not wake {name}: {reason}"),
        K::PrOpened { url } => format!("pull request {url}"),
    }
}

pub fn run(engine: &Engine, cmd: RunCmd, deps: &mut Deps) -> Result<i32> {
    match cmd {
        RunCmd::Start { pipeline, text, test } => {
            let id = PipelineId::new(pipeline);
            let title = base::fmt::truncate_chars(base::fmt::first_line(text.trim()), 80);
            let input = RunInput { title, text };
            let run = if test { engine.test_run(&id, input)? } else { engine.start_run(&id, input)? };
            writeln!(deps.out, "run {} started{}", run.id, if test { " (test)" } else { "" })?;
            followed(engine, &run.id, 0, deps)
        }
        RunCmd::List => {
            for run in engine.latest_runs()? {
                let issue = run.issue.as_ref().map(|k| format!(" {k}")).unwrap_or_default();
                writeln!(
                    deps.out,
                    "{}  {:<21} {} v{}{issue}  {}  {}",
                    run.id,
                    state_word(run.state),
                    run.pipeline,
                    run.version,
                    run.title,
                    base::fmt::cost(run.cost_usd)
                )?;
            }
            Ok(0)
        }
        RunCmd::Show { id } => {
            let run = run_or_fail(engine, &id)?;
            writeln!(deps.out, "{} · {} v{}{}", run.title, run.pipeline, run.version, if run.test { " · test" } else { "" })?;
            for n in &run.nodes {
                let letter = if n.variant > 0 { format!("·{}", variant_letter(n.variant)) } else { String::new() };
                let attempt = if n.attempt > 1 { format!(" #{}", n.attempt) } else { String::new() };
                let status = format!("{:?}", n.status).to_lowercase();
                writeln!(deps.out, "  {:<18} {status:<11} {}", format!("{}{letter}{attempt}", n.node), base::fmt::first_line(&n.summary))?;
            }
            status(engine, &run, deps)
        }
        RunCmd::Log { id, follow: tail } => {
            let id = RunId::new(id);
            run_or_fail(engine, id.as_str())?;
            if tail {
                return followed(engine, &id, 0, deps);
            }
            for line in engine.run_log(&id, 0, usize::MAX)? {
                writeln!(deps.out, "{}", log_line(&line))?;
            }
            Ok(0)
        }
        RunCmd::Cancel { id } => {
            let id = RunId::new(id);
            engine.cancel_run(&id)?;
            let run = run_or_fail(engine, id.as_str())?;
            status(engine, &run, deps)
        }
        RunCmd::Resume { id } => {
            let id = RunId::new(id);
            let after = last_seq(engine, &id);
            engine.resume_run(&id)?;
            followed(engine, &id, after, deps)
        }
        RunCmd::Answer { id, text, node } => {
            let id = RunId::new(id);
            let after = last_seq(engine, &id);
            let node = node.map(engine::api::NodeId::new);
            engine.answer_run(&id, node.as_ref(), &text)?;
            followed(engine, &id, after, deps)
        }
    }
}

fn review_or_fail(engine: &Engine, id: &str) -> Result<Review> {
    engine.review(&ReviewId::new(id))?.ok_or_else(|| anyhow!("there is no review {id}"))
}

pub fn review(engine: &Engine, cmd: ReviewCmd, deps: &mut Deps) -> Result<i32> {
    let (id, decision) = match cmd {
        ReviewCmd::List => {
            for review in engine.open_reviews()? {
                let issue = review.issue.as_ref().map(|k| format!("{k} ")).unwrap_or_default();
                let count = base::fmt::plural(review.variants.len() as u64, "variant", "variants");
                writeln!(deps.out, "{}  {issue}{}  {count}  run {}", review.id, review.title, review.run)?;
            }
            return Ok(0);
        }
        ReviewCmd::Show { id } => {
            let review = review_or_fail(engine, &id)?;
            writeln!(deps.out, "{} · {} · {:?}", review.title, review.agent, review.state)?;
            for v in &review.variants {
                let tests = match (v.tests, v.tests_ok) {
                    (Some(t), _) => format!(" · tests {}/{}", t.passed, t.total),
                    (None, Some(ok)) => format!(" · tests {}", if ok { "pass" } else { "fail" }),
                    _ => String::new(),
                };
                writeln!(
                    deps.out,
                    "{} {}  +{} −{} in {}{tests} · {}",
                    variant_letter(v.index),
                    v.name,
                    v.added,
                    v.removed,
                    base::fmt::plural(v.files.len() as u64, "file", "files"),
                    base::fmt::cost(v.cost_usd)
                )?;
                if let Some(branch) = &v.branch {
                    writeln!(deps.out, "  branch {branch}")?;
                }
                for file in &v.files {
                    writeln!(deps.out, "  {} +{} −{}", file.path, file.added, file.removed)?;
                }
            }
            for render in &review.renders {
                writeln!(deps.out, "render {} ({}×{})", render.path, render.width, render.height)?;
            }
            for note in &review.feedback {
                writeln!(deps.out, "sent back: {}", note.text)?;
            }
            return Ok(0);
        }
        ReviewCmd::Ship { id, variant } => {
            let review = review_or_fail(engine, &id)?;
            let variant = match (variant, review.variants.as_slice()) {
                (Some(v), _) => v,
                (None, [only]) => only.index,
                (None, _) => bail!("the review has {} variants; name the one to ship", review.variants.len()),
            };
            (review.id, ReviewDecision::Ship { variant })
        }
        ReviewCmd::Skip { id } => (review_or_fail(engine, &id)?.id, ReviewDecision::Skip),
        ReviewCmd::SendBack { id, note, from, variants } => {
            let start = match from {
                From::All => StartFrom::All,
                From::Over => StartFrom::Over,
                From::Variant(index) => StartFrom::Variant { index },
            };
            (review_or_fail(engine, &id)?.id, ReviewDecision::SendBack { note, start, variants })
        }
    };
    let run = engine.review(&id)?.map(|r| r.run).ok_or_else(|| anyhow!("there is no review {id}"))?;
    let after = last_seq(engine, &run);
    engine.decide_review(&id, decision)?;
    followed(engine, &run, after, deps)
}

pub fn pipeline(engine: &Engine, cmd: PipelineCmd, deps: &mut Deps) -> Result<i32> {
    match cmd {
        PipelineCmd::List => {
            for runnable in engine.runnables().into_iter().filter(|r| r.kind == RunnableKind::Pipeline) {
                let id = PipelineId::new(runnable.reference.id());
                let live = engine.pipeline_versions(&id)?.into_iter().find(|v| v.state == engine::api::VersionState::Live);
                let version = live.map_or_else(|| "draft".to_string(), |v| format!("v{} live", v.version));
                writeln!(deps.out, "{:<24} {:<10} {}", id, version, runnable.name)?;
            }
            Ok(0)
        }
        PipelineCmd::Problems { id } => {
            let id = PipelineId::new(id);
            let graph = engine.pipeline(&id).ok_or_else(|| anyhow!("there is no pipeline {id}"))?;
            let problems = engine.pipeline_problems(&graph);
            if problems.is_empty() {
                writeln!(deps.out, "no problems")?;
            }
            for p in &problems {
                let level = if p.severity == Severity::Error { "error" } else { "warning" };
                let node = p.node.as_ref().map(|n| format!(" [{n}]")).unwrap_or_default();
                writeln!(deps.out, "{level}{node}: {}", p.message)?;
            }
            Ok(if problems.iter().any(|p| p.severity == Severity::Error) { 1 } else { 0 })
        }
        PipelineCmd::Deploy { id, note } => {
            let id = PipelineId::new(id);
            let version = engine.deploy_pipeline(&id, &note)?;
            writeln!(deps.out, "{id} v{} is live", version.version)?;
            Ok(0)
        }
    }
}
