//! Full-text search over chats for the history palette: matches, the best message snippet of
//! each, facet counts for the filter chips and index statistics.

use std::collections::HashMap;
use std::ops::Range;

use anyhow::Result;
use model::{Millis, RunnableRef, Session, SessionState};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use crate::Store;
use crate::codec::{Filter, query_all, tag, text};
use crate::sessions::{SESSION_COLUMNS, decode_session};

/// Marks around matched words in FTS output. Private-use characters never appear in what
/// people type, and they are stripped before the text is handed out.
const MARK_OPEN: char = '\u{E000}';
const MARK_CLOSE: char = '\u{E001}';
/// Words of context around a match in a message snippet.
const SNIPPET_TOKENS: i64 = 16;

/// Who started a session, for the "Mine" and "Started by agents" chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartedBy {
    Me,
    /// A sub-agent session or a pipeline node's session.
    Agents,
}

/// What the chat palette asks for. Results are sorted by last activity, newest first.
///
/// `text`, `repo`, `runnable`, `pinned` and `archived` scope the search; `state`, `started_by`,
/// `has_pr` and `from_issue` are the chips, and the facet counts are computed within the scope
/// without them.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionQuery {
    /// Words to find in titles and messages. Every word must match; the last one matches as a
    /// prefix unless the text ends with a space. Quotes and FTS operators are taken literally.
    pub text: String,
    /// Repo name of the session's workdir (`in:payments-api`).
    pub repo: Option<String>,
    pub runnable: Option<RunnableRef>,
    pub pinned: Option<bool>,
    /// `Some(false)` (the default) hides archived sessions; `None` shows both.
    pub archived: Option<bool>,
    pub state: Option<SessionState>,
    pub started_by: Option<StartedBy>,
    pub has_pr: bool,
    pub from_issue: bool,
    pub limit: usize,
    pub offset: usize,
}

impl Default for SessionQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            repo: None,
            runnable: None,
            pinned: None,
            archived: Some(false),
            state: None,
            started_by: None,
            has_pr: false,
            from_issue: false,
            limit: 50,
            offset: 0,
        }
    }
}

/// The answer to a [`SessionQuery`].
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub hits: Vec<SessionHit>,
    /// Matches of the whole query, beyond `limit`.
    pub total: u64,
    pub facets: FacetCounts,
    pub index: IndexStats,
}

/// One session in the results.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionHit {
    pub session: Session,
    /// Number of direct sub-agent sessions ("+2 sub").
    pub children: u32,
    /// Byte ranges of matched words in `session.title`.
    pub title_highlights: Vec<Range<usize>>,
    /// The message that matches best, when one does.
    pub snippet: Option<MessageSnippet>,
}

/// An excerpt of a matching message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSnippet {
    pub seq: u64,
    /// The excerpt; `…` marks cut text.
    pub text: String,
    /// Byte ranges of matched words in `text`.
    pub highlights: Vec<Range<usize>>,
}

/// Counts for the filter chips, within the query's scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FacetCounts {
    pub all: u64,
    pub mine: u64,
    pub started_by_agents: u64,
    pub waiting: u64,
    pub running: u64,
    pub failed: u64,
    pub has_pr: u64,
    pub from_issue: u64,
}

/// The palette's footer: "Index: 2,418 chats · 61k messages · updated 2 min ago".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub sessions: u64,
    pub messages: u64,
    /// The latest `updated` of any session.
    pub updated: Option<Millis>,
}

/// The FTS5 query for what a person typed: every word as a quoted string (so operators and
/// punctuation are literal), the last one as a prefix. `None` when nothing searchable is left.
pub(crate) fn fts_query(typed: &str) -> Option<String> {
    let words: Vec<&str> = typed.split_whitespace().filter(|word| word.chars().any(char::is_alphanumeric)).collect();
    let prefix_last = !typed.ends_with(char::is_whitespace);
    let last = words.len().checked_sub(1)?;
    let terms: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            let quoted = format!("\"{}\"", word.replace('"', "\"\""));
            if index == last && prefix_last { format!("{quoted}*") } else { quoted }
        })
        .collect();
    Some(terms.join(" "))
}

/// Text with FTS marks, split into the plain text and the byte ranges between the marks.
pub(crate) fn split_marks(marked: &str) -> (String, Vec<Range<usize>>) {
    let mut plain = String::with_capacity(marked.len());
    let mut ranges = Vec::new();
    let mut open = None;
    for ch in marked.chars() {
        match ch {
            MARK_OPEN => open = Some(plain.len()),
            MARK_CLOSE => {
                if let Some(start) = open.take()
                    && start < plain.len()
                {
                    ranges.push(start..plain.len());
                }
            }
            _ => plain.push(ch),
        }
    }
    (plain, ranges)
}

/// The scope of a query (text, repo, runnable, pinned, archived) as a filter on `sessions s`.
fn scope(query: &SessionQuery, fts: Option<&str>) -> Result<Filter> {
    let mut f = Filter::default();
    if let Some(fts) = fts {
        f.and(
            "s.id IN (SELECT m.session FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid
                      WHERE messages_fts MATCH ?
                      UNION SELECT t.id FROM sessions_fts JOIN sessions t ON t.n = sessions_fts.rowid
                      WHERE sessions_fts MATCH ?)",
            [text(fts), text(fts)],
        );
    }
    if let Some(repo) = &query.repo {
        f.and("s.repo = ?", [text(repo.as_str())]);
    }
    if let Some(runnable) = &query.runnable {
        f.and("s.runnable = ?", [text(runnable.as_str())]);
    }
    if let Some(pinned) = query.pinned {
        f.and("s.pinned = ?", [Value::Integer(pinned.into())]);
    }
    if let Some(archived) = query.archived {
        f.and("s.archived = ?", [Value::Integer(archived.into())]);
    }
    Ok(f)
}

/// The chips of a query as a filter on `sessions s`.
fn chips(query: &SessionQuery) -> Result<Filter> {
    let mut f = Filter::default();
    if let Some(state) = query.state {
        f.and("s.state = ?", [text(tag(&state)?)]);
    }
    match query.started_by {
        Some(StartedBy::Me) => f.and("s.by_agent = 0", []),
        Some(StartedBy::Agents) => f.and("s.by_agent = 1", []),
        None => {}
    }
    if query.has_pr {
        f.and("s.pr_url IS NOT NULL", []);
    }
    if query.from_issue {
        f.and("s.issue IS NOT NULL", []);
    }
    Ok(f)
}

impl Store {
    /// Searches chats for the history palette. See [`SessionQuery`] for what is matched.
    pub fn search_sessions(&self, query: &SessionQuery) -> Result<SearchResult> {
        let fts = fts_query(&query.text);
        let scope = scope(query, fts.as_deref())?;
        let chips = chips(query)?;
        self.read(|conn| {
            let (total, facets) = counts(conn, &scope, &chips)?;
            let mut where_ = scope.params.clone();
            where_.extend(chips.params.iter().cloned());
            where_.push(Value::Integer(query.limit as i64));
            where_.push(Value::Integer(query.offset as i64));
            let sql = format!(
                "SELECT {SESSION_COLUMNS}, (SELECT count(*) FROM sessions c WHERE c.parent = s.id) AS children
                 FROM sessions s WHERE ({}) AND ({}) ORDER BY s.updated DESC, s.id LIMIT ? OFFSET ?",
                scope.expr(),
                chips.expr()
            );
            let mut hits = query_all(conn, &sql, params_from_iter(&where_), "sessions", &["id"], |d| {
                Ok(SessionHit {
                    session: decode_session(d)?,
                    children: d.u32("children")?,
                    title_highlights: Vec::new(),
                    snippet: None,
                })
            })?;
            if let Some(fts) = &fts
                && !hits.is_empty()
            {
                add_highlights(conn, fts, &mut hits)?;
            }
            Ok(SearchResult { hits, total, facets, index: index_stats(conn)? })
        })
    }

    /// Size of the search index, for the palette's footer.
    pub fn index_stats(&self) -> Result<IndexStats> {
        self.read(index_stats)
    }
}

fn counts(conn: &Connection, scope: &Filter, chips: &Filter) -> Result<(u64, FacetCounts)> {
    let mut params = chips.params.clone();
    params.extend(scope.params.iter().cloned());
    let sql = format!(
        "SELECT total(({chips})), count(*), total(s.by_agent = 0), total(s.by_agent = 1),
                total(s.state = 'waiting'), total(s.state = 'running'), total(s.state = 'failed'),
                total(s.pr_url IS NOT NULL), total(s.issue IS NOT NULL)
         FROM sessions s WHERE {scope}",
        chips = chips.expr(),
        scope = scope.expr()
    );
    let row = conn.query_row(&sql, params_from_iter(&params), |row| {
        let n = |i: usize| -> rusqlite::Result<u64> { Ok(row.get::<_, f64>(i)? as u64) };
        Ok((
            n(0)?,
            FacetCounts {
                all: row.get::<_, i64>(1)? as u64,
                mine: n(2)?,
                started_by_agents: n(3)?,
                waiting: n(4)?,
                running: n(5)?,
                failed: n(6)?,
                has_pr: n(7)?,
                from_issue: n(8)?,
            },
        ))
    })?;
    Ok(row)
}

/// Fills in title highlights and the best-matching message of each hit.
fn add_highlights(conn: &Connection, fts: &str, hits: &mut [SessionHit]) -> Result<()> {
    let marks = [text(MARK_OPEN.to_string()), text(MARK_CLOSE.to_string())];
    let ids: Vec<Value> = hits.iter().map(|hit| text(hit.session.id.as_str())).collect();
    let holes = vec!["?"; ids.len()].join(", ");

    let mut params: Vec<Value> = marks.to_vec();
    params.push(text(fts));
    params.extend(ids.iter().cloned());
    let titles = query_all(
        conn,
        &format!(
            "SELECT s.id, highlight(sessions_fts, 0, ?, ?) AS marked
             FROM sessions_fts JOIN sessions s ON s.n = sessions_fts.rowid
             WHERE sessions_fts MATCH ? AND s.id IN ({holes})"
        ),
        params_from_iter(&params),
        "sessions",
        &["id"],
        |d| Ok((d.string("id")?, d.string("marked")?)),
    )?;
    let titles: HashMap<String, String> = titles.into_iter().collect();

    let mut params: Vec<Value> = marks.to_vec();
    params.push(text("…"));
    params.push(Value::Integer(SNIPPET_TOKENS));
    params.push(text(fts));
    params.extend(ids);
    let messages = query_all(
        conn,
        &format!(
            "SELECT m.session, m.seq, snippet(messages_fts, 0, ?, ?, ?, ?) AS marked, bm25(messages_fts) AS rank
             FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid
             WHERE messages_fts MATCH ? AND m.session IN ({holes})"
        ),
        params_from_iter(&params),
        "messages",
        &["session", "seq"],
        |d| Ok((d.string("session")?, d.u64("seq")?, d.string("marked")?, d.get::<f64>("rank")?)),
    )?;
    // bm25 is lower for better matches; ties go to the latest message.
    let mut best: HashMap<String, (u64, String, f64)> = HashMap::new();
    for (session, seq, marked, rank) in messages {
        let better = best.get(&session).is_none_or(|(old_seq, _, old_rank)| {
            rank < *old_rank || (rank == *old_rank && seq > *old_seq)
        });
        if better {
            best.insert(session, (seq, marked, rank));
        }
    }

    for hit in hits {
        if let Some(marked) = titles.get(hit.session.id.as_str()) {
            hit.title_highlights = split_marks(marked).1;
        }
        if let Some((seq, marked, _)) = best.remove(hit.session.id.as_str()) {
            let (text, highlights) = split_marks(&marked);
            hit.snippet = Some(MessageSnippet { seq, text, highlights });
        }
    }
    Ok(())
}

fn index_stats(conn: &Connection) -> Result<IndexStats> {
    Ok(conn.query_row("SELECT count(*), total(message_count), max(updated) FROM sessions", [], |row| {
        Ok(IndexStats {
            sessions: row.get::<_, i64>(0)? as u64,
            messages: row.get::<_, f64>(1)? as u64,
            updated: row.get(2)?,
        })
    })?)
}
