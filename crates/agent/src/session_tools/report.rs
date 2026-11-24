//! `report_to_inbox`: an agent's summary of finished work, posted to the Inbox.

use model::{Link, Outcome, ToolSpec};
use serde_json::{Value, json};

use super::names::REPORT_TO_INBOX;
use super::{object, optional_str, required_str};
use crate::host::Report;

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: REPORT_TO_INBOX.into(),
        description: "Post the result of a finished task to the person's Inbox. Call it once, at the end of work \
                      that produced something (code changes, a pull request, a branch, a plan or an \
                      investigation they asked for), not for quick answers. Include every pull request, issue, \
                      branch or commit URL you created."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "What was done, as a short headline (under 80 characters)" },
                "summary": { "type": "string", "description": "Markdown: what changed, how it was verified, anything left for the person" },
                "outcome": {
                    "type": "string",
                    "enum": ["done", "needs_review", "blocked", "failed"],
                    "description": "needs_review when the person should review or approve (an open pull request); blocked when you need a decision or access"
                },
                "links": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string" },
                            "label": { "type": "string", "description": "Optional, e.g. \"PR #42\"" }
                        },
                        "required": ["url"]
                    }
                }
            },
            "required": ["title", "summary"]
        }),
    }
}

/// The report in the input. `status` is accepted for `outcome`; a link without a label is
/// labelled by its last path segment.
pub(crate) fn parse(input: &Value) -> Result<Report, String> {
    let map = object(REPORT_TO_INBOX, input)?;
    let title = required_str(REPORT_TO_INBOX, &map, "title")?.to_string();
    let summary = required_str(REPORT_TO_INBOX, &map, "summary")?.to_string();
    let outcome = match optional_str(&map, "outcome").or_else(|| optional_str(&map, "status")) {
        None | Some("done") => Outcome::Done,
        Some("needs_review") => Outcome::NeedsReview,
        Some("blocked") => Outcome::Blocked,
        Some("failed") => Outcome::Failed,
        Some(other) => {
            return Err(format!(
                "report_to_inbox: `outcome` must be done, needs_review, blocked or failed (got `{other}`)."
            ));
        }
    };
    let mut links = Vec::new();
    for item in map.get("links").and_then(Value::as_array).into_iter().flatten() {
        let url = match item {
            Value::String(url) => url.trim(),
            other => other.get("url").and_then(Value::as_str).map(str::trim).unwrap_or(""),
        };
        if url.is_empty() {
            continue;
        }
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| url.trim_end_matches('/').rsplit('/').next().unwrap_or(url).to_string());
        links.push(Link { label, url: url.to_string() });
    }
    Ok(Report { title, summary, outcome, links })
}

/// What the model reads back.
pub(crate) fn posted(report: &Report) -> String {
    let links = match report.links.len() {
        0 => String::new(),
        n => format!(" with {}", base::fmt::plural(n as u64, "link", "links")),
    };
    format!("Posted to the Inbox{links}: {}", report.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_parses_with_defaults() {
        let report = parse(&json!({
            "title": " Fixed retry ",
            "summary": "Changed `retry.ts`.",
            "status": "needs_review",
            "links": [{"url": "https://github.com/a/b/pull/42"}, {"url": "https://x.dev/y", "label": "Docs"}, "https://z.dev/q/", {"label": "no url"}]
        }))
        .expect("valid");
        assert_eq!(report.title, "Fixed retry");
        assert_eq!(report.outcome, Outcome::NeedsReview);
        let labels: Vec<&str> = report.links.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["42", "Docs", "q"]);
        assert_eq!(posted(&report), "Posted to the Inbox with 3 links: Fixed retry");
        let plain = parse(&json!({"title": "T", "summary": "S"})).expect("valid");
        assert_eq!(plain.outcome, Outcome::Done);
        assert_eq!(posted(&plain), "Posted to the Inbox: T");
    }

    #[test]
    fn missing_fields_and_unknown_outcomes_are_errors() {
        assert!(parse(&json!({"summary": "S"})).unwrap_err().contains("`title`"));
        assert!(parse(&json!({"title": "T", "summary": " "})).unwrap_err().contains("`summary`"));
        assert!(parse(&json!({"title": "T", "summary": "S", "outcome": "great"})).unwrap_err().contains("great"));
    }
}
