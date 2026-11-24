//! Issues, their keys and their activity feed.

use anyhow::{Result, bail};
use model::{Issue, IssueEvent, IssueKey, ProjectId, RunId, RunnableRef};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::Store;
use crate::codec::{Decoder, json, query_all, query_one, tag};

const ISSUE_COLUMNS: &str = "key, project, title, body, status, priority, kind, owner, repo, created, updated, \
     filed_by, runner, triggers, run, run_count, pr_url, images";

fn decode_issue(d: &Decoder) -> Result<Issue> {
    Ok(Issue {
        key: IssueKey(d.string("key")?),
        project: ProjectId(d.string("project")?),
        title: d.string("title")?,
        body: d.string("body")?,
        status: d.tag("status")?,
        priority: d.tag("priority")?,
        kind: d.tag("kind")?,
        owner: d.string("owner")?,
        repo: d.get("repo")?,
        created: d.get("created")?,
        updated: d.get("updated")?,
        filed_by: d.get::<Option<String>>("filed_by")?.map(RunnableRef),
        runner: d.get::<Option<String>>("runner")?.map(RunnableRef),
        triggers: d.json("triggers")?,
        run: d.opt::<RunId>("run")?,
        run_count: d.u32("run_count")?,
        pr_url: d.get("pr_url")?,
        images: d.json("images")?,
    })
}

fn decode_event(d: &Decoder) -> Result<IssueEvent> {
    let mut event: IssueEvent = d.json("data")?;
    event.seq = d.u64("seq")?;
    event.at = d.get("at")?;
    Ok(event)
}

/// `HAR-12` as `("HAR", 12)`.
fn split_key(key: &str) -> Option<(&str, i64)> {
    let (prefix, number) = key.rsplit_once('-')?;
    let number = number.parse::<i64>().ok().filter(|n| *n > 0)?;
    (!prefix.is_empty()).then_some((prefix, number))
}

/// Keeps a prefix's counter at or above `number`, so allocated keys never collide with
/// stored ones.
fn raise_counter(tx: &Transaction, prefix: &str, number: i64) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO issue_counters (prefix, last) VALUES (?1, ?2)
         ON CONFLICT (prefix) DO UPDATE SET last = max(last, excluded.last)",
    )?
    .execute(params![prefix, number])?;
    Ok(())
}

/// Writes an issue inside the caller's transaction; see [`Store::upsert_issue`].
pub(crate) fn upsert_issue_in(tx: &Transaction, issue: &Issue) -> Result<()> {
    let triggers = json(&issue.triggers)?;
    let images = json(&issue.images)?;
    let (status, priority, kind) = (tag(&issue.status)?, tag(&issue.priority)?, tag(&issue.kind)?);
    tx.prepare_cached(&format!(
        "INSERT INTO issues ({ISSUE_COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT (key) DO UPDATE SET project = excluded.project, title = excluded.title,
            body = excluded.body, status = excluded.status, priority = excluded.priority,
            kind = excluded.kind, owner = excluded.owner, repo = excluded.repo,
            created = excluded.created, updated = excluded.updated, filed_by = excluded.filed_by,
            runner = excluded.runner, triggers = excluded.triggers, run = excluded.run,
            run_count = excluded.run_count, pr_url = excluded.pr_url, images = excluded.images"
    ))?
    .execute(params![
        issue.key.as_str(),
        issue.project.as_str(),
        issue.title,
        issue.body,
        status,
        priority,
        kind,
        issue.owner,
        issue.repo,
        issue.created,
        issue.updated,
        issue.filed_by.as_ref().map(RunnableRef::as_str),
        issue.runner.as_ref().map(RunnableRef::as_str),
        triggers,
        issue.run.as_ref().map(RunId::as_str),
        issue.run_count,
        issue.pr_url,
        images,
    ])?;
    if let Some((prefix, number)) = split_key(issue.key.as_str()) {
        raise_counter(tx, prefix, number)?;
    }
    Ok(())
}

/// Appends an activity entry inside the caller's transaction; see [`Store::append_issue_event`].
pub(crate) fn append_issue_event_in(tx: &Transaction, key: &IssueKey, event: IssueEvent) -> Result<IssueEvent> {
    // The variant name, so the feed can be filtered by kind later.
    let kind_tag = serde_json::to_value(&event.kind)?
        .get("event")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let exists = tx.query_row("SELECT 1 FROM issues WHERE key = ?1", [key.as_str()], |_| Ok(())).optional()?;
    if exists.is_none() {
        bail!("no issue {key}");
    }
    let seq: i64 = tx.query_row(
        "SELECT coalesce(max(seq), 0) + 1 FROM issue_events WHERE issue = ?1",
        [key.as_str()],
        |row| row.get(0),
    )?;
    let stored = IssueEvent { seq: seq as u64, ..event };
    tx.prepare_cached("INSERT INTO issue_events (issue, seq, at, event, data) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![key.as_str(), seq, stored.at, kind_tag, json(&stored)?])?;
    Ok(stored)
}

impl Store {
    /// Allocates the next issue key of a project prefix: `HAR-13` after `HAR-12`. Keys are
    /// never handed out twice, even across threads, and stay above every stored issue's.
    pub fn next_issue_key(&self, prefix: &str) -> Result<IssueKey> {
        if prefix.is_empty() {
            bail!("an issue key prefix cannot be empty");
        }
        self.write(|tx| {
            let n: i64 = tx.query_row(
                "INSERT INTO issue_counters (prefix, last) VALUES (?1, 1)
                 ON CONFLICT (prefix) DO UPDATE SET last = last + 1 RETURNING last",
                [prefix],
                |row| row.get(0),
            )?;
            Ok(IssueKey(format!("{prefix}-{n}")))
        })
    }

    /// Moves the key counter of `prefix` up to `number` unless it is already higher, in one
    /// statement. For importing a counter that stood far ahead of the stored issues.
    pub fn raise_issue_counter(&self, prefix: &str, number: u64) -> Result<()> {
        if prefix.is_empty() {
            bail!("an issue key prefix cannot be empty");
        }
        let number = crate::codec::int(number)?;
        self.write(|tx| raise_counter(tx, prefix, number))
    }

    /// Creates an issue or replaces it. Its activity feed is kept.
    pub fn upsert_issue(&self, issue: &Issue) -> Result<()> {
        self.write(|tx| upsert_issue_in(tx, issue))
    }

    /// One issue.
    pub fn issue(&self, key: &IssueKey) -> Result<Option<Issue>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE key = ?1"),
                [key.as_str()],
                "issues",
                &["key"],
                decode_issue,
            )
        })
    }

    /// Deletes an issue and its activity feed. Its key is not handed out again. Returns
    /// whether it existed.
    pub fn delete_issue(&self, key: &IssueKey) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM issues WHERE key = ?1", [key.as_str()])? > 0))
    }

    /// A project's issues, most recently updated first.
    pub fn issues(&self, project: &ProjectId) -> Result<Vec<Issue>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE project = ?1 ORDER BY updated DESC, key"),
                [project.as_str()],
                "issues",
                &["key"],
                decode_issue,
            )
        })
    }

    /// Adds an entry to an issue's activity feed with the next `seq` (the one in `event` is
    /// ignored). Returns the entry as stored.
    pub fn append_issue_event(&self, key: &IssueKey, event: IssueEvent) -> Result<IssueEvent> {
        self.write(|tx| append_issue_event_in(tx, key, event))
    }

    /// An issue's activity feed, oldest first.
    pub fn issue_events(&self, key: &IssueKey) -> Result<Vec<IssueEvent>> {
        self.read(|conn| {
            query_all(
                conn,
                "SELECT issue, seq, at, data FROM issue_events WHERE issue = ?1 ORDER BY seq",
                [key.as_str()],
                "issue_events",
                &["issue", "seq"],
                decode_event,
            )
        })
    }
}
