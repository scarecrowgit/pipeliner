//! What a chat keeps beside its messages: the display of each finished tool call, and the
//! messages a person sent while a turn was running.

use std::collections::HashMap;

use anyhow::Result;
use model::{Attachment, SessionId, ToolDisplay};
use rusqlite::{OptionalExtension as _, params};

use crate::Store;
use crate::codec::{json, query_all};

/// A message waiting for a session's next turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueuedMessage {
    pub text: String,
    pub images: Vec<Attachment>,
}

impl Store {
    /// Keeps what a finished tool call showed, so history draws it as the live stream did.
    /// A second display for the same call replaces the first.
    pub fn set_tool_display(&self, session: &SessionId, call: &str, display: &ToolDisplay) -> Result<()> {
        let display = json(display)?;
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT INTO tool_displays (session, call, display) VALUES (?1, ?2, ?3)
                 ON CONFLICT (session, call) DO UPDATE SET display = excluded.display",
            )?
            .execute(params![session.as_str(), call, display])?;
            Ok(())
        })
    }

    /// The kept displays of the given calls of a session; calls without one are left out.
    pub fn tool_displays(&self, session: &SessionId, calls: &[String]) -> Result<HashMap<String, ToolDisplay>> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached("SELECT display FROM tool_displays WHERE session = ?1 AND call = ?2")?;
            let mut out = HashMap::new();
            for call in calls {
                let found: Option<String> =
                    stmt.query_row(params![session.as_str(), call], |row| row.get(0)).optional()?;
                // A display this build cannot read falls back to the one history derives.
                if let Some(display) = found.and_then(|text| serde_json::from_str(&text).ok()) {
                    out.insert(call.clone(), display);
                }
            }
            Ok(out)
        })
    }

    /// Copies every kept display of `from` to `to` (a fork of it).
    pub fn copy_tool_displays(&self, from: &SessionId, to: &SessionId) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO tool_displays (session, call, display)
                 SELECT ?2, call, display FROM tool_displays WHERE session = ?1",
                params![from.as_str(), to.as_str()],
            )?;
            Ok(())
        })
    }

    /// Replaces the queue of a session; an empty one removes it.
    pub fn set_queued_messages(&self, session: &SessionId, queue: &[QueuedMessage]) -> Result<()> {
        let rows: Vec<(String, String)> =
            queue.iter().map(|m| Ok((m.text.clone(), json(&m.images)?))).collect::<Result<_>>()?;
        self.write(|tx| {
            tx.execute("DELETE FROM queued_messages WHERE session = ?1", [session.as_str()])?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO queued_messages (session, position, text, images) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (position, (text, images)) in rows.iter().enumerate() {
                insert.execute(params![session.as_str(), position as i64, text, images])?;
            }
            Ok(())
        })
    }

    /// Every stored queue, oldest message first within a session.
    pub fn queued_messages(&self) -> Result<Vec<(SessionId, Vec<QueuedMessage>)>> {
        let rows = self.read(|conn| {
            query_all(
                conn,
                "SELECT session, position, text, images FROM queued_messages ORDER BY session, position",
                [],
                "queued_messages",
                &["session", "position"],
                |d| {
                    let message = QueuedMessage { text: d.string("text")?, images: d.json("images")? };
                    Ok((SessionId::new(d.string("session")?), message))
                },
            )
        })?;
        let mut out: Vec<(SessionId, Vec<QueuedMessage>)> = Vec::new();
        for (session, message) in rows {
            match out.last_mut() {
                Some((last, queue)) if *last == session => queue.push(message),
                _ => out.push((session, vec![message])),
            }
        }
        Ok(out)
    }
}
