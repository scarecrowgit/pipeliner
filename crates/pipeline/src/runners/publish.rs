//! Action jobs whose effect leaves the worktree: merges, pull requests, Inbox reports and
//! issue updates. In a test run each of them only logs what it would have done.

use anyhow::{Context as _, Result, anyhow, bail};
use model::{IssueStatus, Link, LogLevel, Outcome};
use vcs::MergeOutcome;

use super::Ctx;
use super::git::{dirty, git};
use super::services::InboxPost;
use crate::api::{JobResult, NodeOutput};

/// Longest Markdown body of a pull request.
const BODY_MAX: usize = 60_000;

fn done(output: NodeOutput) -> Result<JobResult> {
    Ok(JobResult::Done { port: model::graph::PORT_OUT.into(), output })
}

fn simulated(ctx: &Ctx, what: String) -> Result<JobResult> {
    ctx.log(LogLevel::Info, format!("Test run, not done: {what}"));
    done(NodeOutput { summary: format!("Test run: {what}"), ..Default::default() })
}

pub(crate) fn merge(ctx: &Ctx, workdir: &str) -> Result<JobResult> {
    let wd = ctx.workdir(workdir)?;
    if !wd.worktree {
        return done(NodeOutput { summary: format!("{} is used in place; nothing to merge.", wd.name), ..Default::default() });
    }
    let place = ctx
        .found_place(wd)
        .ok_or_else(|| anyhow!("Nothing to merge: no node of this run worked in a worktree of {}", wd.name))?;
    let branch = place.branch.clone().ok_or_else(|| anyhow!("{} has no branch to merge", place.path.display()))?;
    let mut output = NodeOutput { worktree: Some(place.path.clone()), branch: Some(branch.clone()), ..Default::default() };
    if ctx.job.test {
        let mut result = simulated(ctx, format!("merge {branch} into {}", place.base))?;
        if let JobResult::Done { output: out, .. } = &mut result {
            out.worktree = output.worktree;
            out.branch = output.branch;
        }
        return Ok(result);
    }
    if dirty(&place.path, ctx.cancel)? {
        vcs::commit_all(&place.path, &ctx.title(), ctx.cancel).context("committing before the merge")?;
        ctx.log(LogLevel::Info, format!("Committed the changes on {branch}."));
    }
    match vcs::merge_into_base(&wd.path, &branch, &place.base, ctx.cancel)? {
        MergeOutcome::Merged { head } => {
            ctx.log(LogLevel::Info, format!("Merged {branch} into {} ({}).", place.base, &head[..head.len().min(9)]));
            output.summary = format!("Merged {branch} into {}.", place.base);
            done(output)
        }
        MergeOutcome::UpToDate => {
            output.summary = format!("{} already has {branch}.", place.base);
            done(output)
        }
        MergeOutcome::Conflict { files } => {
            let error = format!(
                "Merging {branch} into {} conflicts in {}. Nothing was changed; resolve it in {} or send the work back",
                place.base,
                files.join(", "),
                place.path.display()
            );
            output.summary = "Merge conflict.".to_string();
            Ok(JobResult::Failed { error, output })
        }
    }
}

/// What the run did so far, as Markdown lines.
fn run_lines(ctx: &Ctx) -> String {
    let mut text = String::new();
    for up in &ctx.job.upstream {
        let summary = up.output.summary.trim();
        if !summary.is_empty() {
            text.push_str(&format!("- **{}**: {}\n", up.name, summary.replace('\n', " ")));
        }
    }
    if let Some(tests) = ctx.job.upstream.iter().rev().find_map(|up| up.output.tests) {
        text.push_str(&format!("\nTests: {} of {} passed.\n", tests.passed, tests.total));
    } else if let Some(ok) = ctx.job.upstream.iter().rev().find_map(|up| up.output.tests_ok) {
        text.push_str(if ok { "\nTests passed.\n" } else { "\n**Tests failed.**\n" });
    }
    text
}

fn pr_body(ctx: &Ctx) -> String {
    let item = &ctx.job.item;
    let issue = match &item.issue {
        Some(issue) => format!(" for **{}**", issue.key),
        None => String::new(),
    };
    let text = if item.text.trim().is_empty() { "(no description)" } else { item.text.trim() };
    let body = format!(
        "Opened by the Harness pipeline “{}”{issue}: {}\n\n## Issue\n\n{text}\n\n## Run\n\n{}",
        ctx.runner.pipeline,
        item.title,
        run_lines(ctx)
    );
    redact(&base::fmt::truncate_chars(&body, BODY_MAX))
}

pub(crate) fn pull_request(ctx: &Ctx, workdir: &str, draft: bool, push: bool) -> Result<JobResult> {
    let wd = ctx.workdir(workdir)?;
    if !wd.worktree {
        bail!("A pull request needs a worktree workdir; {} is used in place", wd.name);
    }
    let place = ctx
        .found_place(wd)
        .ok_or_else(|| anyhow!("Nothing to open a pull request for: no node of this run worked in {}", wd.name))?;
    let branch = place.branch.clone().ok_or_else(|| anyhow!("{} has no branch", place.path.display()))?;
    let title = ctx.title();
    let body = pr_body(ctx);
    let changed = dirty(&place.path, ctx.cancel)?;
    let ahead = git(&place.path, &["rev-list", "--count", &format!("{}..HEAD", place.base)], ctx.cancel)
        .map(|n| n.trim() != "0")
        .unwrap_or(false);
    if !changed && !ahead {
        bail!("Nothing to open a pull request for: {branch} has no changes against {}", place.base);
    }
    let mut output = NodeOutput { worktree: Some(place.path.clone()), branch: Some(branch.clone()), ..Default::default() };
    let kind = if draft { "draft pull request" } else { "pull request" };
    if ctx.job.test {
        if changed {
            ctx.log(LogLevel::Info, format!("Test run, not done: commit the changes on {branch}"));
        }
        if push {
            ctx.log(LogLevel::Info, format!("Test run, not done: push {branch}"));
        }
        let mut result = simulated(ctx, format!("open a {kind} “{title}” from {branch} into {}", place.base))?;
        if let JobResult::Done { output: out, .. } = &mut result {
            out.worktree = output.worktree;
            out.branch = output.branch;
        }
        return Ok(result);
    }
    if changed {
        let message = format!("{title}\n\n{}", run_lines(ctx));
        vcs::commit_all(&place.path, &redact(&message), ctx.cancel).context("committing the changes")?;
        ctx.log(LogLevel::Info, format!("Committed the changes on {branch}."));
    }
    let services = ctx.services();
    let issue = ctx.job.item.issue.as_ref().map(|i| i.key.clone());
    if !push {
        output.summary = format!("Committed on {branch}; push is off, so no pull request was opened.");
        services.records.post(&InboxPost {
            run: ctx.job.run.clone(),
            title,
            summary: body,
            outcome: Outcome::NeedsReview,
            source: ctx.source(),
            issue,
            session: None,
            links: Vec::new(),
        })?;
        return done(output);
    }
    vcs::push(&place.path, &branch, ctx.cancel).with_context(|| format!("pushing {branch}"))?;
    ctx.log(LogLevel::Info, format!("Pushed {branch}."));
    let new = vcs::pr::NewPr { title: &title, body: &body, base: &place.base, draft };
    let url = vcs::pr::create_with(&*services.gh, &place.path, &new, ctx.cancel)?;
    ctx.log(LogLevel::Info, format!("Opened {url}"));
    services.records.post(&InboxPost {
        run: ctx.job.run.clone(),
        title,
        summary: body,
        outcome: Outcome::NeedsReview,
        source: ctx.source(),
        issue: issue.clone(),
        session: None,
        links: vec![Link { label: "Pull request".into(), url: url.clone() }],
    })?;
    if let Some(key) = &issue
        && let Err(error) = services.tracker.pr_opened(key, &url)
    {
        ctx.log(LogLevel::Warn, format!("Could not note the pull request on {key}: {error:#}"));
    }
    output.summary = format!("Opened {url}");
    output.pr_url = Some(url);
    done(output)
}

pub(crate) fn report(ctx: &Ctx) -> Result<JobResult> {
    let title = ctx.title();
    if ctx.job.test {
        return simulated(ctx, format!("post “{title}” to the Inbox"));
    }
    let links: Vec<Link> = ctx
        .job
        .upstream
        .iter()
        .filter_map(|up| up.output.pr_url.clone())
        .map(|url| Link { label: "Pull request".into(), url })
        .collect();
    let lines = run_lines(ctx);
    ctx.services().records.post(&InboxPost {
        run: ctx.job.run.clone(),
        title,
        summary: redact(if lines.is_empty() { "The run finished." } else { &lines }),
        outcome: Outcome::Done,
        source: ctx.source(),
        issue: ctx.job.item.issue.as_ref().map(|i| i.key.clone()),
        session: ctx.job.upstream.iter().rev().find_map(|up| up.output.session.clone()),
        links,
    })?;
    done(NodeOutput { summary: "Posted to the Inbox.".into(), ..Default::default() })
}

pub(crate) fn update_issue(ctx: &Ctx, status: Option<IssueStatus>, comment: &str) -> Result<JobResult> {
    let Some(issue) = &ctx.job.item.issue else {
        return done(NodeOutput { summary: "No issue to update.".into(), ..Default::default() });
    };
    let comment = ctx.expand(comment.trim());
    let what = match status {
        Some(status) => format!("move {} to {}", issue.key, status.label()),
        None => format!("comment on {}", issue.key),
    };
    if ctx.job.test {
        return simulated(ctx, what);
    }
    ctx.services().tracker.update_issue(&issue.key, status, &comment)?;
    let summary = match status {
        Some(status) => format!("Moved {} to {}.", issue.key, status.label()),
        None => format!("Commented on {}.", issue.key),
    };
    done(NodeOutput { summary, ..Default::default() })
}

/// Hides words that look like provider keys or tokens.
pub(crate) fn redact(text: &str) -> String {
    const PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza"];
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.len() >= 20 && PREFIXES.iter().any(|p| word.starts_with(p)) {
            out.push_str("[redacted]");
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}
