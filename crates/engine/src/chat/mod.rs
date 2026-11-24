//! Chat sessions: [`ChatApi`] over the store, the live registry and the turn threads.

mod commands;
mod detail;
mod export;
mod turn;

pub(crate) use turn::mcp_view;

/// Marker holding a chat's spending cap, set when an autopilot with a budget starts it.
pub(crate) fn budget_marker(id: &model::SessionId) -> String {
    format!("session-budget:{id}")
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent::Input;
use anyhow::{Result, anyhow, bail};
use model::{
    Attachment, BlobId, ChatMode, Event, ModelRef, PermissionDecision, RequestId, RunnableKind, RunnableRef, Session,
    SessionId, SessionOrigin, SessionState, StoredMessage, ToolDisplay,
};

use crate::api::{
    ChatApi, ExportFormat, ExportOptions, NewSession, ProviderModels, SearchResult, SessionDetail, SessionFilter,
    SessionQuery, SlashCommand, UserMessage, WorkdirChoice,
};
use crate::engine::{Engine, Inner, NOT_WIRED};
use crate::host::summary_marker;
use crate::live::Sent;

/// The title of a session nobody has written to yet.
pub(crate) const NEW_TITLE: &str = "New chat";
/// Longest title taken from a first message, in characters.
const TITLE_MAX: usize = 60;
/// The end of a `seq` range that reaches the last message.
pub(crate) const TO_END: u64 = i64::MAX as u64;

impl Inner {
    /// The model a new session gets when nothing names one.
    pub fn default_model(&self) -> ModelRef {
        self.store.settings().ok().and_then(|s| s.default_model).or_else(|| self.llm.default_model()).unwrap_or_default()
    }
}

impl Engine {
    /// Starts a turn without a new message (retry, compact).
    fn start_bare(&self, id: &SessionId, input: Input) -> Result<()> {
        let session = self.inner.session(id)?;
        if session.parent.is_some() {
            bail!("A sub-agent's session cannot be continued on its own");
        }
        let Some(cancel) = self.inner.live.claim(id) else { bail!("The chat is busy") };
        if let Err(error) = turn::spawn(&self.inner, id.clone(), input, cancel) {
            self.inner.live.release(id, false);
            return Err(error);
        }
        Ok(())
    }
}

impl ChatApi for Engine {
    fn sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>> {
        self.inner.store.sessions(filter)
    }

    fn session_detail(&self, id: &SessionId) -> Result<Option<SessionDetail>> {
        detail::session_detail(&self.inner, id)
    }

    fn messages(&self, id: &SessionId, from: u64, to: u64) -> Result<Vec<StoredMessage>> {
        self.inner.store.messages(id, from..to.min(TO_END))
    }

    fn tool_displays(&self, id: &SessionId, calls: &[String]) -> Result<HashMap<String, ToolDisplay>> {
        self.inner.store.tool_displays(id, calls)
    }

    fn search_sessions(&self, query: &SessionQuery) -> Result<SearchResult> {
        self.inner.store.search_sessions(query)
    }

    fn new_session(&self, new: NewSession) -> Result<Session> {
        let inner = &self.inner;
        let runnable = new.runnable.clone().unwrap_or_else(catalog::Catalog::default_chat_agent);
        let folder = new.workdir.as_ref().map(|w| inner.paths.expand(&w.path.to_string_lossy()));
        let catalog = inner.catalog_for(folder.as_deref().filter(|f| f.is_dir()));
        let Some(found) = catalog.get(&runnable) else { bail!("{runnable} does not exist in the library") };
        if found.kind == RunnableKind::Pipeline {
            bail!("{NOT_WIRED}");
        }
        let model = new
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or_else(|| found.agent.as_ref().map(|d| d.model.clone()).filter(|m| !m.is_empty()))
            .unwrap_or_else(|| inner.default_model());
        let workdir = match &new.workdir {
            Some(choice) => Some(inner.prepare_workdir(choice, "chat")?),
            None => None,
        };
        let now = inner.now();
        let session = Session {
            id: SessionId::new(base::id::new_id()),
            title: NEW_TITLE.to_string(),
            parent: None,
            depth: 0,
            runnable,
            mode: new.mode,
            context_window: llm::pricing::context_window(&model),
            model,
            workdir,
            state: SessionState::Idle,
            origin: SessionOrigin::Me,
            issue: new.issue,
            pinned: false,
            archived: false,
            created: now,
            updated: now,
            message_count: 0,
            cost_usd: 0.0,
            usage: Default::default(),
            context_tokens: 0,
            forced_skills: new.forced_skills,
            pr_url: None,
        };
        inner.store.upsert_session(&session)?;
        inner.publish(Event::SessionChanged { session: session.clone() });
        Ok(session)
    }

    fn send(&self, id: &SessionId, message: UserMessage) -> Result<()> {
        let inner = &self.inner;
        let mut session = inner.session(id)?;
        if session.parent.is_some() {
            bail!("A sub-agent's session takes no messages");
        }
        if message.text.trim().is_empty() && message.images.is_empty() {
            bail!("The message is empty");
        }
        let message = match commands::expand(&session, message) {
            commands::Expanded::Message(message) => message,
            commands::Expanded::Compact => return self.compact(id),
        };
        if session.model.is_empty() {
            let model = inner.default_model();
            if model.is_empty() {
                bail!("No model is configured: add a provider to config.toml or pick a model for this chat");
            }
            session = inner.update_session(id, |s| s.model = model)?.ok_or_else(|| anyhow!("There is no chat {id}"))?;
        }
        if session.title == NEW_TITLE && !message.text.trim().is_empty() {
            let title = base::fmt::truncate_chars(base::fmt::first_line(message.text.trim()), TITLE_MAX);
            inner.update_session(id, |s| s.title = title)?;
        }
        match inner.live.send(id, message) {
            Sent::Start { cancel, first } => {
                if let Err(error) = turn::spawn(inner, id.clone(), Input::User(crate::host::to_input(first.clone())), cancel) {
                    inner.live.requeue(id, vec![first]);
                    inner.live.release(id, false);
                    return Err(error);
                }
            }
            Sent::Queued => {
                if let Some(session) = inner.store.session(id)? {
                    inner.publish(Event::SessionChanged { session });
                }
            }
        }
        Ok(())
    }

    fn interrupt(&self, id: &SessionId) {
        self.inner.live.cancel(id);
    }

    fn retry(&self, id: &SessionId) -> Result<()> {
        if self.inner.store.messages(id, 1..TO_END)?.is_empty() {
            bail!("There is nothing to retry yet");
        }
        self.start_bare(id, Input::Continue)
    }

    fn compact(&self, id: &SessionId) -> Result<()> {
        self.start_bare(id, Input::Compact)
    }

    fn answer_permission(&self, request: &RequestId, decision: PermissionDecision) -> Result<()> {
        self.inner.asks.answer(request, decision)
    }

    fn set_session_runnable(&self, id: &SessionId, runnable: &RunnableRef, mode: ChatMode) -> Result<()> {
        let inner = &self.inner;
        let session = inner.session(id)?;
        let folder = session.workdir.as_ref().map(|w| w.path.clone());
        let catalog = inner.catalog_for(folder.as_deref());
        let Some(found) = catalog.get(runnable) else { bail!("{runnable} does not exist in the library") };
        if found.kind == RunnableKind::Pipeline {
            bail!("{NOT_WIRED}");
        }
        let model = found.agent.as_ref().map(|d| d.model.clone()).filter(|m| !m.is_empty());
        inner.update_session(id, |s| {
            s.runnable = runnable.clone();
            s.mode = mode;
            if let Some(model) = model {
                s.context_window = llm::pricing::context_window(&model);
                s.model = model;
            }
        })?;
        Ok(())
    }

    fn set_session_model(&self, id: &SessionId, model: &ModelRef) -> Result<()> {
        if model.is_empty() {
            bail!("Pick a model");
        }
        self.inner.update_session(id, |s| {
            s.model = model.clone();
            s.context_window = llm::pricing::context_window(model);
        })?;
        Ok(())
    }

    fn set_session_workdir(&self, id: &SessionId, workdir: WorkdirChoice) -> Result<Session> {
        let inner = &self.inner;
        let session = inner.session(id)?;
        if inner.live.is_running(id) {
            bail!("Wait for the turn to finish before moving the chat");
        }
        let workdir = inner.prepare_workdir(&workdir, &session.title)?;
        inner.update_session(id, |s| s.workdir = Some(workdir))?.ok_or_else(|| anyhow!("There is no chat {id}"))
    }

    fn set_forced_skills(&self, id: &SessionId, skills: Vec<String>) -> Result<()> {
        self.inner.update_session(id, |s| s.forced_skills = skills)?;
        Ok(())
    }

    fn rename_session(&self, id: &SessionId, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("A chat needs a title");
        }
        self.inner.update_session(id, |s| s.title = title.to_string())?;
        Ok(())
    }

    fn pin_session(&self, id: &SessionId, pinned: bool) -> Result<()> {
        self.inner.update_session(id, |s| s.pinned = pinned)?;
        Ok(())
    }

    fn archive_session(&self, id: &SessionId, archived: bool) -> Result<()> {
        self.inner.update_session(id, |s| s.archived = archived)?;
        Ok(())
    }

    fn delete_session(&self, id: &SessionId) -> Result<()> {
        let inner = &self.inner;
        inner.session(id)?;
        let mut stack = vec![id.clone()];
        let mut order = Vec::new();
        while let Some(next) = stack.pop() {
            stack.extend(inner.store.child_sessions(&next)?.into_iter().map(|c| c.id));
            order.push(next);
        }
        for victim in order {
            inner.live.forget(&victim);
            inner.store.delete_session(&victim)?;
            let _ = inner.store.remove_marker(&summary_marker(&victim));
            inner.publish(Event::SessionRemoved { session: victim });
        }
        Ok(())
    }

    fn fork_session(&self, id: &SessionId, up_to_seq: Option<u64>) -> Result<Session> {
        let inner = &self.inner;
        let source = inner.session(id)?;
        let end = up_to_seq.map_or(TO_END, |seq| seq.saturating_add(1));
        let messages = inner.store.messages(id, 1..end)?;
        let now = inner.now();
        let fork = Session {
            id: SessionId::new(base::id::new_id()),
            title: format!("{} (fork)", source.title),
            parent: None,
            depth: 0,
            state: SessionState::Idle,
            origin: SessionOrigin::Me,
            pinned: false,
            archived: false,
            created: now,
            updated: now,
            message_count: 0,
            cost_usd: 0.0,
            usage: Default::default(),
            pr_url: None,
            ..source
        };
        inner.store.upsert_session(&fork)?;
        for message in messages {
            inner.store.append_message(&fork.id, message)?;
        }
        inner.store.copy_tool_displays(id, &fork.id)?;
        if let Some(summary) = inner.store.marker(&summary_marker(id))? {
            inner.store.set_marker(&summary_marker(&fork.id), &summary)?;
        }
        let fork = inner.session(&fork.id)?;
        inner.publish(Event::SessionChanged { session: fork.clone() });
        Ok(fork)
    }

    fn export_session(&self, id: &SessionId, format: ExportFormat, options: ExportOptions, to: &Path) -> Result<PathBuf> {
        export::export(&self.inner, id, format, options, to)
    }

    fn put_image(&self, name: &str, media_type: &str, bytes: &[u8]) -> Result<Attachment> {
        let blob = self.inner.store.put_blob(bytes)?;
        Ok(Attachment { blob, name: name.to_string(), media_type: media_type.to_string(), bytes: bytes.len() as u64 })
    }

    fn blob_path(&self, id: &BlobId) -> PathBuf {
        self.inner.store.blob_path(id).unwrap_or_else(|_| self.inner.paths.blobs_dir().join(id.as_str()))
    }

    fn models(&self) -> Vec<ProviderModels> {
        commands::models(&self.inner)
    }

    fn mention_files(&self, id: &SessionId, prefix: &str, limit: usize) -> Vec<String> {
        commands::mention_files(&self.inner, id, prefix, limit)
    }

    fn slash_commands(&self, id: &SessionId) -> Vec<SlashCommand> {
        commands::slash_commands(&self.inner, id)
    }

    fn reconnect_mcp(&self, id: &SessionId, server: &str) -> Result<()> {
        let session = self.inner.session(id)?;
        let folder = session.workdir.as_ref().map(|w| w.path.clone());
        // Only that server: the others stay as they are, not started ones too.
        if turn::mcp_peek(&self.inner, folder.as_deref()).reconnect(server) {
            Ok(())
        } else {
            bail!("There is no MCP server {server} to reconnect")
        }
    }
}
