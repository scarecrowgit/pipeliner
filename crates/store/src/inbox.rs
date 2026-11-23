//! The Inbox: what waits for a person and reports of finished work.

use anyhow::{Result, bail};
use model::{InboxId, InboxItem, InboxKind, IssueKey, RequestId, ReviewId, RunId, SessionId};
use rusqlite::params;

use crate::Store;
use crate::codec::{Decoder, json, query_all, query_one};

const INBOX_COLUMNS: &str =
    "id, kind, target, title, summary, outcome, source, issue, session, run, links, created, read, archived";

/// The SQL twin of `InboxItem::needs_person`.
const NEEDS_PERSON: &str = "archived = 0 AND (kind <> 'report' OR read = 0)";

/// The kind's name and the id it points at.
fn split_kind(kind: &InboxKind) -> (&'static str, Option<&str>) {
    match kind {
        InboxKind::Review { review } => ("review", Some(review.as_str())),
        InboxKind::Report => ("report", None),
        InboxKind::Question { request } => ("question", Some(request.as_str())),
        InboxKind::Failure => ("failure", None),
    }
}

fn decode_item(d: &Decoder) -> Result<InboxItem> {
    let kind_name = d.string("kind")?;
    let target: Option<String> = d.get("target")?;
    let kind = match (kind_name.as_str(), target) {
        ("review", Some(review)) => InboxKind::Review { review: ReviewId(review) },
        ("report", _) => InboxKind::Report,
        ("question", Some(request)) => InboxKind::Question { request: RequestId(request) },
        ("failure", _) => InboxKind::Failure,
        (other, target) => bail!("decode inbox row {}: kind {other:?} with target {target:?}", d.string("id")?),
    };
    Ok(InboxItem {
        id: InboxId(d.string("id")?),
        kind,
        title: d.string("title")?,
        summary: d.string("summary")?,
        outcome: d.opt_tag("outcome")?,
        source: d.string("source")?,
        issue: d.opt::<IssueKey>("issue")?,
        session: d.opt::<SessionId>("session")?,
        run: d.opt::<RunId>("run")?,
        links: d.json("links")?,
        created: d.get("created")?,
        read: d.get("read")?,
        archived: d.get("archived")?,
    })
}

impl Store {
    /// Creates an Inbox item or replaces it.
    pub fn upsert_inbox_item(&self, item: &InboxItem) -> Result<()> {
        let (kind, target) = split_kind(&item.kind);
        let outcome = item.outcome.as_ref().map(crate::codec::tag).transpose()?;
        let links = json(&item.links)?;
        self.write(|tx| {
            tx.prepare_cached(&format!(
                "INSERT INTO inbox ({INBOX_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, target = excluded.target,
                    title = excluded.title, summary = excluded.summary, outcome = excluded.outcome,
                    source = excluded.source, issue = excluded.issue, session = excluded.session,
                    run = excluded.run, links = excluded.links, created = excluded.created,
                    read = excluded.read, archived = excluded.archived"
            ))?
            .execute(params![
                item.id.as_str(),
                kind,
                target,
                item.title,
                item.summary,
                outcome,
                item.source,
                item.issue.as_ref().map(IssueKey::as_str),
                item.session.as_ref().map(SessionId::as_str),
                item.run.as_ref().map(RunId::as_str),
                links,
                item.created,
                item.read,
                item.archived,
            ])?;
            Ok(())
        })
    }

    /// One Inbox item.
    pub fn inbox_item(&self, id: &InboxId) -> Result<Option<InboxItem>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {INBOX_COLUMNS} FROM inbox WHERE id = ?1"),
                [id.as_str()],
                "inbox",
                &["id"],
                decode_item,
            )
        })
    }

    /// The Inbox (`archived` false) or its archive (true), newest first.
    pub fn inbox(&self, archived: bool) -> Result<Vec<InboxItem>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {INBOX_COLUMNS} FROM inbox WHERE archived = ?1 ORDER BY created DESC, id"),
                [archived],
                "inbox",
                &["id"],
                decode_item,
            )
        })
    }

    /// Marks an item read or unread. Returns whether it exists.
    pub fn set_inbox_read(&self, id: &InboxId, read: bool) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("UPDATE inbox SET read = ?2 WHERE id = ?1", params![id.as_str(), read])? > 0))
    }

    /// Moves an item to the archive or back. Returns whether it exists.
    pub fn set_inbox_archived(&self, id: &InboxId, archived: bool) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute("UPDATE inbox SET archived = ?2 WHERE id = ?1", params![id.as_str(), archived])? > 0)
        })
    }

    /// Removes an item for good. Returns whether it existed.
    pub fn delete_inbox_item(&self, id: &InboxId) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM inbox WHERE id = ?1", [id.as_str()])? > 0))
    }

    /// "N waiting": items a person has to do something about (`InboxItem::needs_person`).
    pub fn inbox_needs_person(&self) -> Result<u32> {
        self.read(|conn| {
            Ok(conn.query_row(&format!("SELECT count(*) FROM inbox WHERE {NEEDS_PERSON}"), [], |row| row.get(0))?)
        })
    }
}
