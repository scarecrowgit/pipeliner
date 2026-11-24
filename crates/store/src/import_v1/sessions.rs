//! Chat sessions: `sessions/<id>.json` and the pinned/archived lists in `chat_flags.json`.
//!
//! A session is found again by its legacy id and, once imported, is skipped whole: running the
//! import again adds only the sessions not imported before; it never merges messages the old
//! app saved into a session already imported.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use base::paths::Paths;
use model::{
    ChatMode, ContentBlock, Message, Millis, ModelRef, RepoHost, Role, RunnableRef, Session, SessionId, SessionOrigin,
    SessionState, StoredMessage, Usage, UsageEntry, WorkMode, Workdir,
};
use serde::Deserialize;

use super::report::Kind;
use crate::{ledger, sessions};
use super::{Ctx, json_files, modified_ms, read_json, rel, text};

/// Ids the new sessions get: the old id behind a prefix, so a half-written import is found
/// again and cleaned up.
const ID_PREFIX: &str = "v1-";

/// Ids below this are not millisecond clocks (1973 in milliseconds).
const MIN_EPOCH_MS: Millis = 100_000_000_000;

#[derive(Deserialize)]
struct OldSession {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    last_usage: Option<OldLastUsage>,
    #[serde(default)]
    usage: Option<OldUsage>,
    messages: Vec<OldMessage>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    forced_skills: Vec<String>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    work_mode: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct OldUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
}

impl From<OldUsage> for Usage {
    fn from(old: OldUsage) -> Self {
        Usage {
            input: old.input_tokens,
            output: old.output_tokens,
            cache_read: old.cache_read_tokens,
            cache_write: old.cache_write_tokens,
        }
    }
}

#[derive(Deserialize)]
struct OldLastUsage {
    #[serde(default)]
    model: Option<String>,
    #[serde(flatten)]
    usage: OldUsage,
}

#[derive(Deserialize)]
struct OldMessage {
    role: OldRole,
    content: Vec<OldBlock>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum OldRole {
    User,
    Assistant,
}

/// The old `llm::ContentBlock`, externally tagged.
#[derive(Deserialize)]
enum OldBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

/// Pinned and archived chat ids from `chat_flags.json`.
#[derive(Default)]
struct Flags {
    pinned: BTreeSet<String>,
    archived: BTreeSet<String>,
}

fn load_flags(cx: &mut Ctx) -> Flags {
    let path = cx.data.join("chat_flags.json");
    if !path.exists() {
        return Flags::default();
    }
    match read_json(&path) {
        Ok(value) => {
            let set = |key: &str| -> BTreeSet<String> {
                value[key]
                    .as_array()
                    .map(|items| items.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default()
            };
            Flags {
                pinned: set("pinned"),
                archived: set("archived"),
            }
        }
        Err(err) => {
            cx.report.fail(Kind::Flags, "chat_flags.json", format!("{err:#}"));
            Flags::default()
        }
    }
}

pub(super) fn import(cx: &mut Ctx) {
    let flags = load_flags(cx);
    let dir = cx.data.join("sessions");
    let files = match json_files(cx, &dir, Kind::Session) {
        Ok(files) => files,
        Err(err) => {
            cx.report.fail(Kind::Session, "sessions", format!("{err:#}"));
            return;
        }
    };
    for file in files {
        let name = rel(&cx.data, &file);
        match import_one(cx, &file, &flags) {
            Ok(Outcome::Imported { messages }) => {
                cx.report.imported(Kind::Session, 1);
                cx.report.imported(Kind::Message, messages);
            }
            Ok(Outcome::Present { messages }) => {
                cx.report.skipped(Kind::Session, 1);
                cx.report.skipped(Kind::Message, messages);
            }
            Err(err) => cx.report.fail(Kind::Session, name, format!("{err:#}")),
        }
    }
}

enum Outcome {
    Imported { messages: usize },
    Present { messages: usize },
}

fn import_one(cx: &mut Ctx, file: &Path, flags: &Flags) -> Result<Outcome> {
    let value = read_json(file)?;
    let old: OldSession = serde_json::from_value(value).context("not a saved session")?;
    if old.id.trim().is_empty() {
        bail!("the session has no id");
    }
    if let Some(found) = cx.store.session_by_legacy_id(&old.id)? {
        let messages = found.message_count as usize;
        cx.sessions.insert(old.id, found.id);
        return Ok(Outcome::Present { messages });
    }
    let updated_from_file = modified_ms(file);
    let converted = convert(cx.paths, old, flags, updated_from_file);
    let messages = converted.messages.len();
    if !cx.dry {
        write(cx, &converted)?;
    }
    cx.sessions.insert(converted.legacy_id, converted.session.id);
    Ok(Outcome::Imported { messages })
}

/// A session ready to be written.
struct Converted {
    legacy_id: String,
    session: Session,
    messages: Vec<StoredMessage>,
    ledger: Option<UsageEntry>,
}

fn convert(paths: &Paths, old: OldSession, flags: &Flags, file_time: Option<Millis>) -> Converted {
    let created = id_time(&old.id).or(file_time).unwrap_or_default();
    let updated = file_time.unwrap_or(created).max(created);
    let mode = if old.mode.as_deref() == Some("plan") {
        ChatMode::Plan
    } else {
        ChatMode::Build
    };
    let agent = old.agent.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let (runnable, agent_name) = match (agent, mode) {
        (Some(name), _) => (RunnableRef::agent(&base::id::slug(name, 40)), name.to_string()),
        (None, ChatMode::Plan) => (RunnableRef::agent("plan"), "Plan".to_string()),
        (None, ChatMode::Build) => (RunnableRef::agent("build"), "Build".to_string()),
    };
    let model = old
        .model
        .as_deref()
        .filter(|m| !m.is_empty())
        .or(old.last_usage.as_ref().and_then(|u| u.model.as_deref()))
        .map(ModelRef::parse)
        .unwrap_or_default();
    let usage: Usage = old.usage.unwrap_or_default().into();
    let context_tokens = old
        .last_usage
        .as_ref()
        .map_or(0, |u| Usage::from(u.usage).context_tokens());
    let cost_usd = old.cost_usd.filter(|c| c.is_finite()).unwrap_or(0.0);

    let title = old
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| first_typed_line(&old.messages))
        .unwrap_or_else(|| "Untitled session".to_string());
    let messages = convert_messages(old.messages);
    let id = SessionId(format!("{ID_PREFIX}{}", old.id));
    let stored = space_out(messages, created, updated);

    let work_mode = match old.work_mode.as_deref() {
        Some("worktree") => WorkMode::Worktree,
        Some("read_only") => WorkMode::ReadOnly,
        _ => WorkMode::InPlace,
    };
    let workdir = old
        .workdir
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| workdir_of(paths, w, work_mode));
    let ledger = (cost_usd > 0.0 || usage.total() > 0).then(|| UsageEntry {
        at: updated,
        session: Some(id.clone()),
        run: None,
        node: None,
        runnable: runnable.clone(),
        name: agent_name,
        created_by: None,
        model: model.clone(),
        usage,
        cost_usd,
        imported: true,
    });
    let session = Session {
        id,
        title,
        parent: None,
        depth: 0,
        runnable,
        mode,
        model,
        workdir,
        state: SessionState::Idle,
        origin: SessionOrigin::Imported,
        issue: None,
        pinned: flags.pinned.contains(&old.id),
        archived: flags.archived.contains(&old.id),
        created,
        updated,
        message_count: 0,
        cost_usd,
        usage,
        context_tokens,
        context_window: 0,
        forced_skills: old.forced_skills,
        pr_url: None,
    };
    Converted {
        legacy_id: old.id,
        session,
        messages: stored,
        ledger,
    }
}

/// The creation time in an old id (`1763560000123-4821`): its millisecond epoch prefix.
fn id_time(id: &str) -> Option<Millis> {
    let ms: Millis = id.split('-').next()?.parse().ok()?;
    (ms >= MIN_EPOCH_MS).then_some(ms)
}

/// Where the session works. The repo is the folder name, except under the worktrees folder,
/// where it is the repo the worktree was cut from (`worktrees/<repo>/<branch>`), as the old
/// sidebar showed it. The branch is not recorded in the old file.
fn workdir_of(paths: &Paths, path: &str, mode: WorkMode) -> Workdir {
    let path = paths.expand(path);
    let repo = path
        .strip_prefix(paths.worktrees_dir())
        .ok()
        .and_then(|rest| rest.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());
    Workdir {
        path,
        repo,
        branch: None,
        host: RepoHost::Local,
        mode,
        source: None,
        runtime: "local".into(),
    }
}

/// The old message list as new messages: tool blocks one to one, user text with its inline
/// encodings written out as plain text. Messages left without content are dropped.
fn convert_messages(old: Vec<OldMessage>) -> Vec<Message> {
    let mut out = Vec::with_capacity(old.len());
    for message in old {
        let role = match message.role {
            OldRole::User => Role::User,
            OldRole::Assistant => Role::Assistant,
        };
        let content: Vec<ContentBlock> = message
            .content
            .into_iter()
            .filter_map(|block| match block {
                OldBlock::Text(t) if t.trim().is_empty() => None,
                OldBlock::Text(t) if role == Role::User => Some(ContentBlock::text(text::readable(&t))),
                OldBlock::Text(t) => Some(ContentBlock::text(t)),
                OldBlock::ToolUse { id, name, input } => Some(ContentBlock::ToolUse { id, name, input }),
                OldBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                }),
            })
            .collect();
        if !content.is_empty() {
            out.push(Message { role, content });
        }
    }
    out
}

/// The first line of the first thing the person typed, for sessions saved without a title.
fn first_typed_line(messages: &[OldMessage]) -> Option<String> {
    messages
        .iter()
        .filter(|m| matches!(m.role, OldRole::User))
        .find_map(|m| {
            m.content.iter().find_map(|block| match block {
                OldBlock::Text(t) if !text::is_encoded(t) => t
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|line| text::truncate(line, 60)),
                _ => None,
            })
        })
}

/// Gives each message a time: the old files have none, so they are spread evenly from the
/// session's creation to its last change.
fn space_out(messages: Vec<Message>, created: Millis, updated: Millis) -> Vec<StoredMessage> {
    let count = messages.len() as i128;
    let span = i128::from(updated.saturating_sub(created).max(0));
    messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let offset = if count > 1 {
                span * index as i128 / (count - 1)
            } else {
                0
            };
            StoredMessage {
                seq: 0,
                at: created.saturating_add(i64::try_from(offset).unwrap_or(0)),
                message,
                compacted: false,
                model: None,
                usage: Usage::default(),
                cost_usd: 0.0,
            }
        })
        .collect()
}

/// Writes a converted session in one transaction, so a failure at any step leaves nothing
/// behind: no half-written session and no ledger row for a retry to count twice. A session
/// left half-written by an earlier version of the import (without its legacy id) is replaced.
fn write(cx: &Ctx, converted: &Converted) -> Result<()> {
    let id = &converted.session.id;
    cx.store.write(|tx| {
        sessions::delete_session_in(tx, id)?;
        sessions::upsert_session_in(tx, &converted.session)?;
        for message in &converted.messages {
            sessions::append_message_in(tx, id, message.clone())?;
        }
        if let Some(entry) = &converted.ledger {
            ledger::record_usage_in(tx, entry)?;
        }
        sessions::set_session_legacy_id_in(tx, id, &converted.legacy_id)
    })
}
