//! Router jobs: rules in order, the first that matches names the port.

use model::graph::PORT_ELSE;
use model::{LogLevel, RouterNode, RouterRule};

use super::services::Question;
use super::Ctx;
use crate::api::{JobNote, JobResult, NodeOutput};
use crate::expr;

pub(crate) fn run(ctx: &Ctx, node: &RouterNode) -> JobResult {
    let mut output = NodeOutput::default();
    for rule in &node.rules {
        if ctx.cancel.is_cancelled() {
            return JobResult::Cancelled { output };
        }
        match decide(ctx, rule, &mut output) {
            Ok(Some(reason)) => {
                output.summary = format!("{}: {reason}", label(rule));
                ctx.log(LogLevel::Info, format!("Rule “{}” matched.", label(rule)));
                return JobResult::Done { port: rule.id.clone(), output };
            }
            Ok(None) => {}
            Err(error) => return JobResult::Failed { error, output },
        }
    }
    if output.summary.is_empty() {
        output.summary = "No rule matched.".to_string();
    }
    JobResult::Done { port: PORT_ELSE.into(), output }
}

fn label(rule: &RouterRule) -> &str {
    if rule.label.trim().is_empty() { &rule.id } else { rule.label.trim() }
}

/// `Some(reason)` when the rule matches. A condition that does not parse, a question that
/// cannot be asked and an answer that is neither yes nor no are errors.
fn decide(ctx: &Ctx, rule: &RouterRule, output: &mut NodeOutput) -> Result<Option<String>, String> {
    if let Some(ask) = &rule.ask {
        let workdir = if ask.workdir.is_empty() {
            None
        } else {
            let wd = ctx.workdir(&ask.workdir).map_err(|e| format!("Rule “{}”: {e:#}", label(rule)))?;
            Some(ctx.read_place(wd).map_err(|e| format!("Rule “{}”: {e:#}", label(rule)))?)
        };
        let question = Question {
            run: ctx.job.run.clone(),
            node: ctx.job.key.node.clone(),
            issue: ctx.job.item.issue.as_ref().map(|i| i.key.clone()),
            model: ask.model.clone(),
            question: ask.question.clone(),
            item: item_text(ctx),
            workdir,
            max_steps: ask.max_steps,
        };
        let answer = ctx
            .services()
            .questions
            .ask(&question, ctx.cancel)
            .map_err(|e| format!("Rule “{}” could not be asked: {e:#}", label(rule)))?;
        ctx.sink.note(JobNote::Spent { cost_usd: answer.cost_usd, usage: answer.usage });
        output.cost_usd += answer.cost_usd;
        output.usage.add(answer.usage);
        if output.session.is_none() {
            output.session = answer.session.clone();
        }
        let reason = answer.reason.trim().to_string();
        return match yes_no(&answer.answer) {
            Some(true) => Ok(Some(if reason.is_empty() { "yes".into() } else { reason })),
            Some(false) => {
                ctx.log(LogLevel::Info, format!("Rule “{}”: no. {reason}", label(rule)));
                if !reason.is_empty() {
                    output.summary = format!("{}: no — {reason}", label(rule));
                }
                Ok(None)
            }
            None => Err(format!(
                "Rule “{}” got “{}” where only yes or no counts",
                label(rule),
                base::fmt::truncate_chars(answer.answer.trim(), 80)
            )),
        };
    }
    if rule.when.trim().is_empty() {
        return Ok(None);
    }
    let parsed = expr::parse(&rule.when).map_err(|e| format!("Rule “{}” has a bad condition: {e}", label(rule)))?;
    Ok(parsed.eval(&ctx.job.item).then(|| rule.when.trim().to_string()))
}

/// The answer read strictly: `yes` or `no`, any case, with at most a full stop after it.
pub(crate) fn yes_no(text: &str) -> Option<bool> {
    let word = text.trim().trim_end_matches(['.', '!']).trim().to_lowercase();
    match word.as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

fn item_text(ctx: &Ctx) -> String {
    let item = &ctx.job.item;
    let mut text = format!("Title: {}\n", ctx.title());
    if let Some(issue) = &item.issue {
        text.push_str(&format!("Type: {} · Priority: {} · Status: {}\n", issue.kind.label(), issue.priority.label(), issue.status.label()));
    }
    text.push('\n');
    text.push_str(item.text.trim());
    text
}
