//! Inbox reports (`inbox.json`).

use anyhow::{Context as _, Result};
use model::{InboxId, InboxItem, InboxKind, Link, Outcome};
use serde::Deserialize;

use super::report::Kind;
use super::{Ctx, read_json, secs_to_ms};

#[derive(Deserialize)]
struct OldFile {
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct OldItem {
    id: String,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    chat_id: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    links: Vec<OldLink>,
    #[serde(default)]
    read: bool,
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
struct OldLink {
    #[serde(default)]
    url: String,
    #[serde(default)]
    label: Option<String>,
}

fn outcome_of(text: Option<&str>) -> Outcome {
    match text {
        Some("needs_review" | "review") => Outcome::NeedsReview,
        Some("blocked") => Outcome::Blocked,
        Some("failed") => Outcome::Failed,
        _ => Outcome::Done,
    }
}

/// `owner/repo#42` for a pull request URL, as the old Inbox always labelled them.
fn pull_request_label(url: &str) -> Option<String> {
    let rest = url.trim().trim_start_matches("https://").trim_start_matches("http://");
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let at = parts
        .iter()
        .position(|p| matches!(*p, "pull" | "merge_requests" | "pull-requests"))?;
    let number = parts.get(at + 1)?;
    let repo: Vec<&str> = parts.get(1..at)?.iter().copied().filter(|p| *p != "-").collect();
    (!repo.is_empty()).then(|| format!("{}#{number}", repo.join("/")))
}

fn link_of(old: OldLink) -> Option<Link> {
    let url = old.url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let label = pull_request_label(&url)
        .or_else(|| old.label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()))
        .unwrap_or_else(|| url.clone());
    Some(Link { label, url })
}

pub(super) fn import(cx: &mut Ctx) {
    let path = cx.data.join("inbox.json");
    if !path.exists() {
        return;
    }
    let file = match read_json(&path).and_then(|v| serde_json::from_value::<OldFile>(v).context("not an inbox file")) {
        Ok(file) => file,
        Err(err) => {
            cx.report.fail(Kind::Inbox, "inbox.json", format!("{err:#}"));
            return;
        }
    };
    for (index, value) in file.items.into_iter().enumerate() {
        let label = value["id"]
            .as_str()
            .map_or_else(|| format!("inbox.json item {}", index + 1), str::to_string);
        match import_one(cx, value) {
            Ok(true) => cx.report.imported(Kind::Inbox, 1),
            Ok(false) => cx.report.skipped(Kind::Inbox, 1),
            Err(err) => cx.report.fail(Kind::Inbox, label, format!("{err:#}")),
        }
    }
}

/// Writes one item; `false` when it is already in the store.
fn import_one(cx: &Ctx, value: serde_json::Value) -> Result<bool> {
    let old: OldItem = serde_json::from_value(value)?;
    let id = InboxId::new(old.id.trim());
    if id.as_str().is_empty() {
        anyhow::bail!("the item has no id");
    }
    if cx.store.inbox_item(&id)?.is_some() {
        return Ok(false);
    }
    let agent = old.agent.trim();
    let item = InboxItem {
        id,
        kind: InboxKind::Report,
        title: old.title.trim().to_string(),
        summary: old.summary,
        outcome: Some(outcome_of(old.outcome.as_deref())),
        source: if agent.is_empty() {
            "Agent".to_string()
        } else {
            agent.to_string()
        },
        issue: None,
        session: cx.sessions.get(&old.chat_id).cloned(),
        run: None,
        links: old.links.into_iter().filter_map(link_of).collect(),
        created: secs_to_ms(old.created),
        read: old.read,
        archived: old.archived,
    };
    if !cx.dry {
        cx.store.upsert_inbox_item(&item)?;
    }
    Ok(true)
}
