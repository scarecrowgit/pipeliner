//! Chat sessions and their messages, todo lists and changed files.

use std::ops::Range;

use anyhow::{Result, bail};
use model::{
    FileChange, IssueKey, Message, ModelRef, RunnableRef, Session, SessionId, SessionOrigin, SessionState,
    StoredMessage, TodoItem,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension as _, Transaction, params, params_from_iter};

use crate::Store;
use crate::codec::{Decoder, Filter, check_path, int, json, query_all, query_one, tag, text};

/// Session columns read back into a [`Session`], prefixed for joins.
pub(crate) const SESSION_COLUMNS: &str = "s.id, s.title, s.parent, s.depth, s.runnable, s.mode, s.model, \
     s.workdir, s.state, s.origin, s.issue, s.pinned, s.archived, s.created, s.updated, s.message_count, \
     s.cost_usd, s.usage, s.context_tokens, s.context_window, s.forced_skills, s.pr_url";

const MESSAGE_COLUMNS: &str = "session, seq, at, role, content, compacted, model, usage, cost_usd";

/// Which sessions [`Store::sessions`] lists. The default lists every session, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionFilter {
    /// `Some(false)` for the sidebar, `Some(true)` for the archive, `None` for both.
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub state: Option<SessionState>,
    pub parent: ParentFilter,
    /// `None` lists all.
    pub limit: Option<usize>,
    pub offset: usize,
}

/// Where in the session tree [`SessionFilter`] looks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ParentFilter {
    #[default]
    Any,
    /// Sessions without a parent.
    TopLevel,
    /// Direct children of this session.
    Of(SessionId),
}

pub(crate) fn decode_session(d: &Decoder) -> Result<Session> {
    Ok(Session {
        id: SessionId(d.string("id")?),
        title: d.string("title")?,
        parent: d.opt("parent")?,
        depth: d.u32("depth")?,
        runnable: RunnableRef(d.string("runnable")?),
        mode: d.tag("mode")?,
        model: ModelRef::parse(&d.string("model")?),
        workdir: d.opt_json("workdir")?,
        state: d.tag("state")?,
        origin: d.json("origin")?,
        issue: d.opt::<IssueKey>("issue")?,
        pinned: d.get("pinned")?,
        archived: d.get("archived")?,
        created: d.get("created")?,
        updated: d.get("updated")?,
        message_count: d.u32("message_count")?,
        cost_usd: d.get("cost_usd")?,
        usage: d.json("usage")?,
        context_tokens: d.u64("context_tokens")?,
        context_window: d.u64("context_window")?,
        forced_skills: d.json("forced_skills")?,
        pr_url: d.get("pr_url")?,
    })
}

fn decode_message(d: &Decoder) -> Result<StoredMessage> {
    Ok(StoredMessage {
        seq: d.u64("seq")?,
        at: d.get("at")?,
        message: Message { role: d.tag("role")?, content: d.json("content")? },
        compacted: d.get("compacted")?,
        model: d.get::<Option<String>>("model")?.map(|text| ModelRef::parse(&text)),
        usage: d.json("usage")?,
        cost_usd: d.get("cost_usd")?,
    })
}

/// Started by an agent or a pipeline rather than by the person.
fn by_agent(session: &Session) -> bool {
    match session.origin {
        SessionOrigin::Agent { .. } | SessionOrigin::Run { .. } => true,
        SessionOrigin::Me | SessionOrigin::Imported => session.parent.is_some(),
    }
}

/// Writes a session header inside the caller's transaction.
pub(crate) fn upsert_session_in(tx: &Transaction, session: &Session) -> Result<()> {
    if let Some(workdir) = &session.workdir {
        check_path(&workdir.path, "workdir")?;
        if let Some(source) = &workdir.source {
            check_path(source, "workdir source")?;
        }
    }
    let workdir = session.workdir.as_ref().map(json).transpose()?;
    let repo = session.workdir.as_ref().map(|w| w.repo.clone());
    let model = session.model.to_string();
    tx.prepare_cached(
        "INSERT INTO sessions (id, title, parent, depth, runnable, mode, model, workdir, repo, state, origin,
            by_agent, issue, pinned, archived, created, updated, cost_usd, usage, context_tokens,
            context_window, forced_skills, pr_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
            ?21, ?22, ?23)
         ON CONFLICT (id) DO UPDATE SET title = excluded.title, parent = excluded.parent,
            depth = excluded.depth, runnable = excluded.runnable, mode = excluded.mode,
            model = excluded.model, workdir = excluded.workdir, repo = excluded.repo,
            state = excluded.state, origin = excluded.origin, by_agent = excluded.by_agent,
            issue = excluded.issue, pinned = excluded.pinned, archived = excluded.archived,
            created = excluded.created, updated = max(sessions.updated, excluded.updated),
            cost_usd = excluded.cost_usd, usage = excluded.usage,
            context_tokens = excluded.context_tokens, context_window = excluded.context_window,
            forced_skills = excluded.forced_skills, pr_url = excluded.pr_url",
    )?
    .execute(params![
        session.id.as_str(),
        session.title,
        session.parent.as_ref().map(SessionId::as_str),
        session.depth,
        session.runnable.as_str(),
        tag(&session.mode)?,
        model,
        workdir,
        repo,
        tag(&session.state)?,
        json(&session.origin)?,
        by_agent(session),
        session.issue.as_ref().map(IssueKey::as_str),
        session.pinned,
        session.archived,
        session.created,
        session.updated,
        session.cost_usd,
        json(&session.usage)?,
        int(session.context_tokens)?,
        int(session.context_window)?,
        json(&session.forced_skills)?,
        session.pr_url,
    ])?;
    Ok(())
}

/// Deletes a session inside the caller's transaction; whether it existed.
pub(crate) fn delete_session_in(tx: &Transaction, id: &SessionId) -> Result<bool> {
    Ok(tx.execute("DELETE FROM sessions WHERE id = ?1", [id.as_str()])? > 0)
}

/// Records a session's legacy id inside the caller's transaction.
pub(crate) fn set_session_legacy_id_in(tx: &Transaction, id: &SessionId, legacy_id: &str) -> Result<()> {
    let changed = tx.execute("UPDATE sessions SET legacy_id = ?2 WHERE id = ?1", [id.as_str(), legacy_id])?;
    if changed == 0 {
        bail!("no session {id}");
    }
    Ok(())
}

/// Appends a message inside the caller's transaction; see [`Store::append_message`].
pub(crate) fn append_message_in(tx: &Transaction, session: &SessionId, message: StoredMessage) -> Result<StoredMessage> {
    let content = json(&message.message.content)?;
    let role = tag(&message.message.role)?;
    let plain = message.message.plain_text();
    let model = message.model.as_ref().map(ModelRef::to_string);
    let usage = json(&message.usage)?;
    let exists = tx
        .query_row("SELECT 1 FROM sessions WHERE id = ?1", [session.as_str()], |_| Ok(()))
        .optional()?;
    if exists.is_none() {
        bail!("no session {session}");
    }
    let seq: i64 = tx.query_row(
        "SELECT coalesce(max(seq), 0) + 1 FROM messages WHERE session = ?1",
        [session.as_str()],
        |row| row.get(0),
    )?;
    tx.prepare_cached(
        "INSERT INTO messages (session, seq, at, role, content, text, compacted, model, usage, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(params![
        session.as_str(),
        seq,
        message.at,
        role,
        content,
        plain,
        message.compacted,
        model,
        usage,
        message.cost_usd
    ])?;
    tx.prepare_cached(
        "UPDATE sessions SET message_count = message_count + 1, updated = max(updated, ?2) WHERE id = ?1",
    )?
    .execute(params![session.as_str(), message.at])?;
    Ok(StoredMessage { seq: seq as u64, ..message })
}

impl Store {
    /// Creates a session or replaces its header. The store owns `message_count` (the number of
    /// stored messages) and never moves `updated` back, so a stale copy cannot undo an append.
    pub fn upsert_session(&self, session: &Session) -> Result<()> {
        self.write(|tx| upsert_session_in(tx, session))
    }

    /// One session's header.
    pub fn session(&self, id: &SessionId) -> Result<Option<Session>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.id = ?1"),
                [id.as_str()],
                "sessions",
                &["id"],
                decode_session,
            )
        })
    }

    /// Deletes a session with its messages, todo list and changed files. Its children stay.
    /// Returns whether it existed.
    pub fn delete_session(&self, id: &SessionId) -> Result<bool> {
        self.write(|tx| delete_session_in(tx, id))
    }

    /// Sessions matching `filter`, most recently active first.
    pub fn sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>> {
        let mut f = Filter::default();
        if let Some(archived) = filter.archived {
            f.and("s.archived = ?", [Value::Integer(archived.into())]);
        }
        if let Some(pinned) = filter.pinned {
            f.and("s.pinned = ?", [Value::Integer(pinned.into())]);
        }
        if let Some(state) = filter.state {
            f.and("s.state = ?", [text(tag(&state)?)]);
        }
        match &filter.parent {
            ParentFilter::Any => {}
            ParentFilter::TopLevel => f.and("s.parent IS NULL", []),
            ParentFilter::Of(parent) => f.and("s.parent = ?", [text(parent.as_str())]),
        }
        let limit = filter.limit.map_or(-1, |limit| limit as i64);
        f.params.push(Value::Integer(limit));
        f.params.push(Value::Integer(filter.offset as i64));
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions s WHERE {} ORDER BY s.updated DESC, s.id LIMIT ? OFFSET ?",
            f.expr()
        );
        self.read(|conn| query_all(conn, &sql, params_from_iter(&f.params), "sessions", &["id"], decode_session))
    }

    /// Direct sub-agent sessions of `parent`, archived ones included, most recent first.
    pub fn child_sessions(&self, parent: &SessionId) -> Result<Vec<Session>> {
        self.sessions(&SessionFilter { parent: ParentFilter::Of(parent.clone()), ..SessionFilter::default() })
    }

    /// Records which old-app session a session was imported from.
    pub fn set_session_legacy_id(&self, id: &SessionId, legacy_id: &str) -> Result<()> {
        self.write(|tx| set_session_legacy_id_in(tx, id, legacy_id))
    }

    /// The session imported from the old app's session `legacy_id`, so an import can run twice.
    pub fn session_by_legacy_id(&self, legacy_id: &str) -> Result<Option<Session>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.legacy_id = ?1"),
                [legacy_id],
                "sessions",
                &["id"],
                decode_session,
            )
        })
    }

    /// Appends a message to a session: gives it the next `seq` (the one in `message` is
    /// ignored), counts it in the session, moves the session's `updated` to its time and
    /// indexes its text for search. Returns the message as stored.
    pub fn append_message(&self, session: &SessionId, message: StoredMessage) -> Result<StoredMessage> {
        self.write(|tx| append_message_in(tx, session, message))
    }

    /// Messages whose `seq` is in `seqs`, in order: a page of the transcript. Seqs start at 1.
    pub fn messages(&self, session: &SessionId, seqs: Range<u64>) -> Result<Vec<StoredMessage>> {
        // `u64::MAX` is how callers say "to the end"; clamp what SQLite cannot hold.
        let clamp = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        let (start, end) = (clamp(seqs.start), clamp(seqs.end));
        self.read(|conn| {
            query_all(
                conn,
                &format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages WHERE session = ?1 AND seq >= ?2 AND seq < ?3 ORDER BY seq"
                ),
                params![session.as_str(), start, end],
                "messages",
                &["session", "seq"],
                decode_message,
            )
        })
    }

    /// The messages still sent to the model: every one not replaced by a summary, in order.
    pub fn model_messages(&self, session: &SessionId) -> Result<Vec<StoredMessage>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE session = ?1 AND compacted = 0 ORDER BY seq"),
                [session.as_str()],
                "messages",
                &["session", "seq"],
                decode_message,
            )
        })
    }

    /// Marks every message up to and including `up_to_seq` as replaced by a summary. Returns
    /// how many changed.
    pub fn compact_messages(&self, session: &SessionId, up_to_seq: u64) -> Result<usize> {
        let up_to = int(up_to_seq)?;
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE messages SET compacted = 1 WHERE session = ?1 AND seq <= ?2 AND compacted = 0",
                params![session.as_str(), up_to],
            )?)
        })
    }

    /// A session's todo list; empty for an unknown session.
    pub fn todos(&self, session: &SessionId) -> Result<Vec<TodoItem>> {
        self.session_json(session, "todos")
    }

    /// Replaces a session's todo list.
    pub fn set_todos(&self, session: &SessionId, items: &[TodoItem]) -> Result<()> {
        self.set_session_json(session, "todos", json(&items)?)
    }

    /// The files a session changed; empty for an unknown session.
    pub fn file_changes(&self, session: &SessionId) -> Result<Vec<FileChange>> {
        self.session_json(session, "files")
    }

    /// Replaces the list of files a session changed.
    pub fn set_file_changes(&self, session: &SessionId, files: &[FileChange]) -> Result<()> {
        self.set_session_json(session, "files", json(&files)?)
    }

    fn session_json<T: serde::de::DeserializeOwned>(&self, session: &SessionId, column: &str) -> Result<Vec<T>> {
        let sql = format!("SELECT id, {column} FROM sessions WHERE id = ?1");
        let found = self.read(|conn| {
            query_one(conn, &sql, [session.as_str()], "sessions", &["id"], |d| d.json::<Vec<T>>(column))
        })?;
        Ok(found.unwrap_or_default())
    }

    fn set_session_json(&self, session: &SessionId, column: &str, value: String) -> Result<()> {
        let sql = format!("UPDATE sessions SET {column} = ?2 WHERE id = ?1");
        self.write(|tx| {
            if tx.execute(&sql, params![session.as_str(), value])? == 0 {
                bail!("no session {session}");
            }
            Ok(())
        })
    }
}
