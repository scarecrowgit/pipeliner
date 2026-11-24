//! Action jobs that work on the machine: shell commands and renders. Merges, pull requests,
//! reports and issue updates are in `publish`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use base::process::Cmd;
use model::{ActionNode, LogLevel, Render};

use super::places::Place;
use super::{Ctx, failed, publish};
use crate::api::{JobResult, NodeOutput};

/// Lines of a command's output kept in the run's log.
const LOG_LINES: usize = 20;

pub(crate) fn run(ctx: &Ctx, action: &ActionNode) -> JobResult {
    let result = match action {
        ActionNode::Command { workdir, command, tests, parse, timeout_secs } => {
            self::command(ctx, workdir, command, *tests, parse, *timeout_secs)
        }
        ActionNode::Render { workdir, files } => render(ctx, workdir, files),
        ActionNode::Merge { workdir } => publish::merge(ctx, workdir),
        ActionNode::PullRequest { workdir, draft, push } => publish::pull_request(ctx, workdir, *draft, *push),
        ActionNode::Report => publish::report(ctx),
        ActionNode::UpdateIssue { status, comment } => publish::update_issue(ctx, *status, comment),
    };
    result.unwrap_or_else(|error| failed(format!("{error:#}")))
}

/// Where a command or render works: the upstream worktree, else a new one.
fn place(ctx: &Ctx, workdir: &str) -> Result<Place> {
    let wd = ctx.workdir(workdir)?;
    match ctx.found_place(wd) {
        Some(place) => Ok(place),
        None => ctx.work_place(wd),
    }
}

fn output_at(place: &Place) -> NodeOutput {
    NodeOutput { worktree: Some(place.path.clone()), branch: place.branch.clone(), ..Default::default() }
}

fn command(ctx: &Ctx, workdir: &str, script: &str, tests: bool, parse: &str, timeout_secs: u64) -> Result<JobResult> {
    if script.trim().is_empty() {
        bail!("The command is empty");
    }
    let wd = ctx.workdir(workdir)?;
    let place = place(ctx, workdir)?;
    let timeout = if timeout_secs == 0 { ctx.runner.config.command_timeout } else { Duration::from_secs(timeout_secs) };
    ctx.log(LogLevel::Info, format!("$ {}", base::fmt::first_line(script.trim())));
    let mut cmd = Cmd::shell(script).cwd(&place.path).timeout(timeout);
    for (k, v) in &wd.env {
        cmd = cmd.env(k, v);
    }
    let out = cmd.run(ctx.cancel).with_context(|| format!("starting the command in {}", place.path.display()))?;
    let mut output = output_at(&place);
    if out.cancelled {
        return Ok(JobResult::Cancelled { output });
    }
    let text = out.combined();
    let lines: Vec<&str> = text.lines().collect();
    for line in &lines[lines.len().saturating_sub(LOG_LINES)..] {
        ctx.log(LogLevel::Info, line.to_string());
    }
    let secs = out.elapsed.as_secs();
    if out.timed_out {
        output.summary = format!("Timed out after {}.", base::fmt::duration(timeout.as_secs()));
        return Ok(JobResult::Failed { error: format!("The command timed out after {}", base::fmt::duration(timeout.as_secs())), output });
    }
    let ok = out.success();
    if tests {
        output.tests = super::pattern::parse_tests(parse, &text).map_err(anyhow::Error::msg)?;
        output.tests_ok = Some(ok);
        output.summary = match output.tests {
            Some(t) => format!("{} of {} tests passed in {}.", t.passed, t.total, base::fmt::duration(secs)),
            None if ok => format!("Tests passed in {}.", base::fmt::duration(secs)),
            None => format!("Tests failed (exit {}).", out.code.unwrap_or(-1)),
        };
        // A failing tests command is a result for the review, not a failure of the node.
        return Ok(JobResult::Done { port: model::graph::PORT_OUT.into(), output });
    }
    if !ok {
        output.summary = format!("Exit {}.", out.code.unwrap_or(-1));
        let error = format!("The command exited with {}: {}", out.code.unwrap_or(-1), super::services::tail(&text, 600));
        return Ok(JobResult::Failed { error, output });
    }
    output.summary = format!("Done in {}.", base::fmt::duration(secs));
    Ok(JobResult::Done { port: model::graph::PORT_OUT.into(), output })
}

fn render(ctx: &Ctx, workdir: &str, files: &str) -> Result<JobResult> {
    let place = place(ctx, workdir)?;
    let glob = if files.trim().is_empty() { "**/*.html" } else { files.trim() };
    let changed: Vec<String> = super::git::changed_paths(&place.path, &place.base, ctx.cancel)?
        .into_iter()
        .filter(|path| super::task::glob_matches(glob, path))
        .collect();
    let mut output = output_at(&place);
    if changed.is_empty() {
        output.summary = "Nothing to render: no changed file matches.".to_string();
        return Ok(JobResult::Done { port: model::graph::PORT_OUT.into(), output });
    }
    for path in &changed {
        if ctx.cancel.is_cancelled() {
            return Ok(JobResult::Cancelled { output });
        }
        ctx.log(LogLevel::Info, format!("Rendering {path}"));
        let render = render_one(ctx, &place, path).with_context(|| format!("rendering {path}"))?;
        output.renders.push(render);
    }
    output.summary = base::fmt::plural(output.renders.len() as u64, "file rendered", "files rendered");
    Ok(JobResult::Done { port: model::graph::PORT_OUT.into(), output })
}

/// The file now, and as it is on the base branch (rendered from a copy beside it so relative
/// assets still resolve).
fn render_one(ctx: &Ctx, place: &Place, path: &str) -> Result<Render> {
    let services = ctx.services();
    let after = services.renderer.render(&place.path.join(path), ctx.cancel)?;
    let after_blob = services.records.put_blob(&after.png)?;
    let before = match super::git::git(&place.path, &["show", &format!("{}:{path}", place.base)], ctx.cancel) {
        Ok(text) => {
            let file = place.path.join(path);
            let stem = Path::new(path).file_stem().map_or_else(|| "file".into(), |s| s.to_string_lossy().into_owned());
            let copy = file.with_file_name(format!(".harness-before-{stem}.html"));
            std::fs::write(&copy, text).with_context(|| format!("writing {}", copy.display()))?;
            let picture = services.renderer.render(&copy, ctx.cancel);
            let _ = std::fs::remove_file(&copy);
            Some(services.records.put_blob(&picture?.png)?)
        }
        // A new file has no before.
        Err(_) => None,
    };
    Ok(Render { path: path.to_string(), before, after: after_blob, width: after.width, height: after.height })
}
