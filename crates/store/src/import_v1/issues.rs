//! Issues (`issues.json`) with their comments, the project that holds them, and the old
//! runs that belong to them.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context as _, Result};
use model::{
    Actor, Issue, IssueEvent, IssueEventKind, IssueKey, IssueStatus, IssueType, Millis, Priority, Project, ProjectId,
    RunId, RunnableRef, StartTriggers,
};
use serde::Deserialize;

use super::report::Kind;
use crate::issues;
use super::runs::{self, IssueInfo, ParsedRun};
use super::{Ctx, read_json, secs_to_ms};

/// The one project every old issue goes into.
const PROJECT_ID: &str = "pipeliner";
const PROJECT_NAME: &str = "Pipeliner";
/// The prefix the old app used, for a project whose keys give none.
const DEFAULT_PREFIX: &str = "HAR";

#[derive(Deserialize)]
struct OldIssueFile {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OldStatus {
    Backlog,
    Triage,
    Design,
    Approval,
    Build,
    Review,
    Done,
    Failed,
}

#[derive(Deserialize)]
struct OldIssue {
    key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    created: u64,
    status: OldStatus,
    #[serde(default)]
    pipeline: String,
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    comments: Vec<OldComment>,
}

#[derive(Deserialize)]
struct OldComment {
    #[serde(default)]
    created: u64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    mentions: Vec<String>,
}

fn status_of(status: OldStatus) -> IssueStatus {
    match status {
        OldStatus::Backlog => IssueStatus::Backlog,
        OldStatus::Triage | OldStatus::Design | OldStatus::Approval | OldStatus::Build | OldStatus::Failed => {
            IssueStatus::InProgress
        }
        OldStatus::Review => IssueStatus::InReview,
        OldStatus::Done => IssueStatus::Done,
    }
}

fn kind_of(kind: &str) -> IssueType {
    match kind.trim().to_ascii_lowercase().as_str() {
        "bug" => IssueType::Bug,
        "chore" => IssueType::Chore,
        "docs" => IssueType::Docs,
        _ => IssueType::Story,
    }
}

/// `HAR-12` as `("HAR", 12)`.
fn split_key(key: &str) -> Option<(&str, u64)> {
    let (prefix, number) = key.rsplit_once('-')?;
    let number = number.parse::<u64>().ok().filter(|n| *n > 0)?;
    (!prefix.is_empty()).then_some((prefix, number))
}

pub(super) fn import(cx: &mut Ctx) {
    let path = cx.data.join("issues.json");
    let file = if path.exists() {
        match read_json(&path)
            .and_then(|value| serde_json::from_value::<OldIssueFile>(value).context("not an issues file"))
        {
            Ok(file) => file,
            Err(err) => {
                cx.report.fail(Kind::Issue, "issues.json", format!("{err:#}"));
                OldIssueFile {
                    next: 0,
                    items: Vec::new(),
                }
            }
        }
    } else {
        OldIssueFile {
            next: 0,
            items: Vec::new(),
        }
    };

    let mut issues = Vec::new();
    for (index, value) in file.items.into_iter().enumerate() {
        let label = value["key"]
            .as_str()
            .map_or_else(|| format!("issues.json item {}", index + 1), str::to_string);
        match serde_json::from_value::<OldIssue>(value) {
            Ok(issue) if split_key(&issue.key).is_none() => {
                cx.report
                    .fail(Kind::Issue, label, "the key is not of the form PREFIX-NUMBER");
            }
            Ok(issue) => issues.push(issue),
            Err(err) => cx.report.fail(Kind::Issue, label, format!("{err}")),
        }
    }

    let runs = runs::load_all(cx);
    let infos = run_infos(&issues, &runs);
    let prefix = project_prefix(&issues);
    if !issues.is_empty()
        && let Err(err) = ensure_project(cx, &prefix)
    {
        cx.report.fail(Kind::Issue, "project", format!("{err:#}"));
        return;
    }

    let by_id: HashMap<&str, &ParsedRun> = runs.iter().map(|run| (run.old.id.as_str(), run)).collect();
    let mut imported_numbers = Vec::new();
    for issue in &issues {
        match import_issue(cx, issue, &runs, &by_id, &infos) {
            Ok(true) => cx.report.imported(Kind::Issue, 1),
            Ok(false) => cx.report.skipped(Kind::Issue, 1),
            Err(err) => {
                cx.report.fail(Kind::Issue, issue.key.clone(), format!("{err:#}"));
                continue;
            }
        }
        imported_numbers.extend(split_key(&issue.key).map(|(_, n)| n));
    }

    for parsed in &runs {
        let info = infos.get(parsed.old.id.as_str());
        if let Err(err) = runs::import_one(cx, parsed, info) {
            cx.report.fail(Kind::Run, parsed.file.clone(), format!("{err:#}"));
        }
    }

    if !cx.dry
        && !issues.is_empty()
        && let Err(err) = raise_counter(
            cx,
            &prefix,
            file.next,
            imported_numbers.iter().copied().max().unwrap_or(0),
        )
    {
        cx.report.fail(Kind::Issue, "key counter", format!("{err:#}"));
    }
}

/// For each run its issue's details and its place among the issue's runs, oldest first.
fn run_infos(issues: &[OldIssue], runs: &[ParsedRun]) -> HashMap<String, IssueInfo> {
    let by_key: HashMap<&str, &OldIssue> = issues.iter().map(|i| (i.key.as_str(), i)).collect();
    let mut grouped: BTreeMap<&str, Vec<&ParsedRun>> = BTreeMap::new();
    for run in runs {
        if by_key.contains_key(run.old.issue.as_str()) {
            grouped.entry(run.old.issue.as_str()).or_default().push(run);
        }
    }
    let mut infos = HashMap::new();
    for (key, mut group) in grouped {
        group.sort_by(|a, b| (a.old.created, &a.old.id).cmp(&(b.old.created, &b.old.id)));
        let issue = by_key[key];
        for (index, run) in group.into_iter().enumerate() {
            infos.insert(
                run.old.id.clone(),
                IssueInfo {
                    key: IssueKey::new(key),
                    title: issue.title.trim().to_string(),
                    body: issue.body.trim().to_string(),
                    number: u32::try_from(index + 1).unwrap_or(u32::MAX),
                },
            );
        }
    }
    infos
}

/// The prefix most keys carry (the first alphabetically on a tie).
fn project_prefix(issues: &[OldIssue]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for issue in issues {
        if let Some((prefix, _)) = split_key(&issue.key) {
            *counts.entry(prefix).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or_else(|| DEFAULT_PREFIX.to_string(), |(prefix, _)| prefix.to_string())
}

fn ensure_project(cx: &Ctx, prefix: &str) -> Result<()> {
    let id = ProjectId::new(PROJECT_ID);
    if cx.dry || cx.store.project(&id)?.is_some() {
        return Ok(());
    }
    cx.store.upsert_project(&Project {
        id,
        name: PROJECT_NAME.to_string(),
        key_prefix: prefix.to_string(),
        repos: Vec::new(),
    })
}

/// Writes one issue with its activity feed. `false` when the key is already in the store.
fn import_issue(
    cx: &Ctx,
    old: &OldIssue,
    runs: &[ParsedRun],
    by_id: &HashMap<&str, &ParsedRun>,
    infos: &HashMap<String, IssueInfo>,
) -> Result<bool> {
    let key = IssueKey::new(&old.key);
    if cx.store.issue(&key)?.is_some() {
        return Ok(false);
    }
    let created = secs_to_ms(old.created);
    let latest = old
        .run
        .as_deref()
        .and_then(|id| by_id.get(id))
        .filter(|r| infos.contains_key(&r.old.id));
    let run_count = runs
        .iter()
        .filter(|r| infos.get(&r.old.id).is_some_and(|i| i.key == key))
        .count();
    let updated = latest.map_or(created, |r| r.old.last_activity().max(created));
    let pr_url = old.pr_url.clone().filter(|u| !u.is_empty());

    let issue = Issue {
        key: key.clone(),
        project: ProjectId::new(PROJECT_ID),
        title: old.title.trim().to_string(),
        body: old.body.trim().to_string(),
        status: status_of(old.status),
        priority: Priority::Medium,
        kind: kind_of(&old.kind),
        owner: String::new(),
        repo: None,
        created,
        updated,
        filed_by: None,
        runner: (!old.pipeline.is_empty()).then(|| RunnableRef::pipeline(&old.pipeline)),
        triggers: StartTriggers::default(),
        run: latest.map(|r| RunId::new(&r.old.id)),
        run_count: u32::try_from(run_count).unwrap_or(u32::MAX),
        pr_url: pr_url.clone(),
        images: Vec::new(),
    };
    if cx.dry {
        return Ok(true);
    }
    // One transaction: the issue and its whole feed, or nothing. Otherwise the next import
    // would find the issue and keep a truncated feed.
    cx.store.write(|tx| {
        issues::upsert_issue_in(tx, &issue)?;
        for (at, kind) in events(old, pr_url, updated) {
            issues::append_issue_event_in(
                tx,
                &key,
                IssueEvent {
                    seq: 0,
                    at,
                    actor: Actor::Me,
                    kind,
                },
            )?;
        }
        Ok(())
    })?;
    Ok(true)
}

/// The activity feed of an old issue: created, its comments, the pull request.
fn events(old: &OldIssue, pr_url: Option<String>, end: Millis) -> Vec<(Millis, IssueEventKind)> {
    let created = secs_to_ms(old.created);
    let mut out = vec![(created, IssueEventKind::Created)];
    let mut comments: Vec<&OldComment> = old.comments.iter().filter(|c| !c.text.trim().is_empty()).collect();
    comments.sort_by_key(|c| c.created);
    for comment in comments {
        let mut mentions: Vec<RunnableRef> = comment
            .mentions
            .iter()
            .map(|name| RunnableRef::agent(&base::id::slug(name, 40)))
            .collect();
        mentions.sort();
        mentions.dedup();
        out.push((
            secs_to_ms(comment.created).max(created),
            IssueEventKind::Comment {
                text: comment.text.trim().to_string(),
                images: Vec::new(),
                mentions,
            },
        ));
    }
    if let Some(url) = pr_url {
        out.push((end, IssueEventKind::PrOpened { url }));
    }
    out
}

/// Moves the key counter of `prefix` to where the old app's counter stood, so a new issue
/// never gets a key the old app used (including keys of issues deleted since). Storing the
/// issues already raised it to the highest key; only a higher old counter needs more work,
/// done once.
fn raise_counter(cx: &Ctx, prefix: &str, old_next: u64, highest: u64) -> Result<()> {
    let marker = format!("v1_issue_counter:{prefix}");
    if old_next <= highest || cx.store.marker(&marker)?.is_some() {
        return Ok(());
    }
    cx.store.raise_issue_counter(prefix, old_next)?;
    cx.store.set_marker(&marker, &old_next.to_string())
}
