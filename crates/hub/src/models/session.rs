//! One open chat session: its detail, its transcript, its context use, and the one-shot effects
//! a chat view reacts to.

use std::sync::Arc;

use anyhow::anyhow;
use gpui::{Context, EventEmitter};

use crate::api::{
    AgentEvent, Api, PermissionRequest, RequestId, Role, Session, SessionDetail, SessionId, SessionState,
    SkillInContext, StopReason, UserMessage,
};
use crate::load::{LoadState, failed, query};
use crate::transcript::{Block, LiveEffect, Splice, Transcript, tool_call_ids};

/// Messages per transcript page.
pub const PAGE: u64 = 40;

/// One-shot effects of a session, emitted with `cx.emit`.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    /// Blocks changed: apply to the list with `ListState::splice(old, count)`. One batch may
    /// send several, in order; each covers only rows that changed (edited runs, then the rows
    /// appended at the end), so rows in between keep their measured heights.
    Spliced(Splice),
    /// New content arrived at the end.
    ScrollToBottom,
    TurnFinished(StopReason),
    /// A tool call waits for a person.
    PermissionArrived(RequestId),
    Failed(String),
    /// The session was deleted.
    Removed,
}

/// What the hub routes to a session.
pub(crate) enum SessionInput {
    Header(Session),
    /// A sub-agent session of this one changed.
    Child(Session),
    Removed,
    Agent(AgentEvent),
    Permission(PermissionRequest),
    Resolved(RequestId),
}

/// How full the context window is, and what the session cost so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextUsage {
    pub tokens: u64,
    pub window: u64,
    pub cost_usd: f64,
}

impl ContextUsage {
    /// Share of the window in use, 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.window == 0 { 0.0 } else { (self.tokens as f64 / self.window as f64).min(1.0) as f32 }
    }
}

/// A chat session as the chat screen shows it. Created on demand by [`crate::session`] and
/// dropped when no view holds it.
pub struct SessionModel {
    id: SessionId,
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    detail: Option<Arc<SessionDetail>>,
    transcript: Transcript,
    busy: bool,
    context: ContextUsage,
    oldest_seq: Option<u64>,
    has_older: bool,
    loading_older: bool,
    removed: bool,
    buffer: Vec<SessionInput>,
    detail_generation: u64,
}

impl EventEmitter<SessionEvent> for SessionModel {}

impl SessionModel {
    pub(crate) fn new(id: SessionId, api: Arc<dyn Api>) -> Self {
        Self {
            id,
            api,
            state: LoadState::Loading,
            started: false,
            detail: None,
            transcript: Transcript::default(),
            busy: false,
            context: ContextUsage::default(),
            oldest_seq: None,
            has_older: false,
            loading_older: false,
            removed: false,
            buffer: Vec::new(),
            detail_generation: 0,
        }
    }

    /// The session this model shows.
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Where the first load stands.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Everything around the transcript, once loaded.
    pub fn detail(&self) -> Option<&Arc<SessionDetail>> {
        self.detail.as_ref()
    }

    /// The session header, once loaded.
    pub fn session(&self) -> Option<&Session> {
        self.detail.as_deref().map(|detail| &detail.session)
    }

    /// The transcript with its paging state.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The transcript's rows.
    pub fn blocks(&self) -> &[Arc<Block>] {
        self.transcript.blocks()
    }

    /// A turn is under way: the agent works, or one of its tool calls waits for a person.
    /// The composer queues messages and offers "Stop" while this holds.
    pub fn is_busy(&self) -> bool {
        self.busy || self.is_waiting()
    }

    /// The agent itself is working (streaming, running tools), as opposed to waiting.
    pub fn is_running(&self) -> bool {
        self.busy && !self.is_waiting()
    }

    /// A tool call of this session waits for a person's answer.
    pub fn is_waiting(&self) -> bool {
        !self.pending().is_empty()
    }

    /// Context use and cost so far.
    pub fn context(&self) -> ContextUsage {
        self.context
    }

    /// Messages waiting for the next turn.
    pub fn queued(&self) -> &[UserMessage] {
        self.detail.as_deref().map_or(&[], |detail| &detail.queued)
    }

    /// Tool calls of this session waiting for a person.
    pub fn pending(&self) -> &[PermissionRequest] {
        self.detail.as_deref().map_or(&[], |detail| &detail.pending)
    }

    /// Whether the tool call `id` was answered as interrupted (a stop, a crash, a restart)
    /// instead of finishing. See [`Transcript::is_interrupted`].
    pub fn is_interrupted(&self, id: &str) -> bool {
        self.transcript.is_interrupted(id)
    }

    /// Older messages exist above the first loaded one.
    pub fn has_older(&self) -> bool {
        self.has_older
    }

    /// An older page is being read.
    pub fn is_loading_older(&self) -> bool {
        self.loading_older
    }

    /// The session was deleted while open.
    pub fn is_removed(&self) -> bool {
        self.removed
    }

    /// Reads the detail and the last page of messages, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        let id = self.id.clone();
        query(
            cx,
            &self.api,
            move |api| {
                let detail = api.session_detail(&id)?.ok_or_else(|| anyhow!("This chat no longer exists"))?;
                let from = u64::from(detail.session.message_count).saturating_sub(PAGE);
                let messages = api.messages(&id, from, u64::MAX)?;
                // Without kept displays, history derives them: never a reason to fail.
                let displays = api.tool_displays(&id, &tool_call_ids(&messages)).unwrap_or_default();
                let max_depth = spawn_depth(api, &detail.session);
                Ok((detail, from, messages, displays, max_depth))
            },
            |this: &mut Self, result, cx| match result {
                Err(error) => {
                    this.state = failed(&error);
                    this.buffer.clear();
                    cx.notify();
                }
                Ok((detail, from, messages, displays, max_depth)) => {
                    this.transcript.set_max_depth(max_depth);
                    this.transcript.keep_displays(displays);
                    this.oldest_seq = messages.first().map(|message| message.seq);
                    this.has_older = from > 0 && !messages.is_empty();
                    this.transcript.append_history(&messages);
                    this.transcript.link_subagents(&detail.children);
                    for request in &detail.pending {
                        this.transcript.permission(Arc::new(request.clone()));
                    }
                    this.header(&detail.session);
                    this.detail = Some(Arc::new(detail));
                    this.state = LoadState::Ready;
                    let buffered = std::mem::take(&mut this.buffer);
                    this.apply_inputs(buffered, cx);
                    this.flush(cx);
                }
            },
        );
    }

    /// Loads the page of messages above the first loaded one.
    pub fn load_older(&mut self, cx: &mut Context<Self>) {
        if !self.has_older || self.loading_older || self.state != LoadState::Ready {
            return;
        }
        self.loading_older = true;
        cx.notify();
        let id = self.id.clone();
        let to = self.oldest_seq.unwrap_or(0);
        let from = to.saturating_sub(PAGE);
        query(
            cx,
            &self.api,
            move |api| {
                let messages = api.messages(&id, from, to)?;
                let displays = api.tool_displays(&id, &tool_call_ids(&messages)).unwrap_or_default();
                Ok((messages, displays))
            },
            move |this: &mut Self, result, cx| {
                this.loading_older = false;
                if let Ok((messages, displays)) = result {
                    this.transcript.keep_displays(displays);
                    if let Some(first) = messages.first() {
                        this.oldest_seq = Some(first.seq);
                    }
                    this.has_older = from > 0 && !messages.is_empty();
                    let splice = this.transcript.prepend_history(&messages);
                    if splice.count > 0 {
                        cx.emit(SessionEvent::Spliced(splice));
                    }
                }
                cx.notify();
            },
        );
    }

    /// Re-reads what the event stream does not carry: queued messages, repo facts, MCP state.
    pub fn refresh_detail(&mut self, cx: &mut Context<Self>) {
        if self.state != LoadState::Ready {
            return;
        }
        self.detail_generation += 1;
        let generation = self.detail_generation;
        let id = self.id.clone();
        query(
            cx,
            &self.api,
            move |api| api.session_detail(&id),
            move |this: &mut Self, result, cx| {
                if generation != this.detail_generation {
                    return;
                }
                if let Ok(Some(detail)) = result {
                    this.header(&detail.session);
                    this.detail = Some(Arc::new(detail));
                    cx.notify();
                }
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<SessionInput>, cx: &mut Context<Self>) {
        if !self.started || matches!(self.state, LoadState::Failed(_)) {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        if !inputs.is_empty() {
            self.apply_inputs(inputs, cx);
            self.flush(cx);
        }
    }

    /// Has a pending permission request with this id, loaded or still buffered.
    pub(crate) fn has_request(&self, id: &RequestId) -> bool {
        self.pending().iter().any(|request| &request.id == id)
            || self.buffer.iter().any(|input| matches!(input, SessionInput::Permission(request) if &request.id == id))
    }

    fn apply_inputs(&mut self, inputs: Vec<SessionInput>, cx: &mut Context<Self>) {
        for input in inputs {
            match input {
                SessionInput::Header(session) => {
                    self.header(&session);
                    self.edit_detail(|detail| detail.session = session);
                }
                SessionInput::Child(child) => self.edit_detail(|detail| {
                    match detail.children.iter_mut().find(|existing| existing.id == child.id) {
                        Some(existing) => *existing = child,
                        None => detail.children.push(child),
                    }
                }),
                SessionInput::Removed => {
                    self.removed = true;
                    cx.emit(SessionEvent::Removed);
                }
                SessionInput::Permission(request) => {
                    let id = request.id.clone();
                    let request = Arc::new(request);
                    self.edit_detail(|detail| {
                        if !detail.pending.iter().any(|known| known.id == request.id) {
                            detail.pending.push((*request).clone());
                        }
                    });
                    if self.transcript.permission(request) {
                        cx.emit(SessionEvent::PermissionArrived(id));
                    }
                }
                SessionInput::Resolved(id) => {
                    self.edit_detail(|detail| detail.pending.retain(|request| request.id != id));
                    self.transcript.resolve(&id);
                }
                SessionInput::Agent(event) => self.agent(event, cx),
            }
        }
    }

    fn agent(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        match &event {
            AgentEvent::Usage { cost_usd, context_tokens, context_window, .. } => {
                self.context.tokens = *context_tokens;
                self.context.window = *context_window;
                self.context.cost_usd += cost_usd;
            }
            AgentEvent::Compacted { context_tokens, .. } => self.context.tokens = *context_tokens,
            AgentEvent::FilesChanged { files } => self.edit_detail(|detail| detail.files = files.clone()),
            AgentEvent::TodoUpdated { items } => self.edit_detail(|detail| detail.todos = items.clone()),
            AgentEvent::SkillLoaded { name } => self.edit_detail(|detail| {
                match detail.skills.iter_mut().find(|skill| &skill.name == name) {
                    Some(skill) => skill.loaded = true,
                    None => detail.skills.push(SkillInContext { name: name.clone(), forced: false, loaded: true, tokens: 0 }),
                }
            }),
            AgentEvent::PrDetected { url } => self.edit_detail(|detail| detail.session.pr_url = Some(url.clone())),
            AgentEvent::MessageCommitted { message } if message.message.role == Role::User => {
                let text = message.message.plain_text();
                self.edit_detail(|detail| {
                    if let Some(index) = detail.queued.iter().position(|queued| queued.text == text) {
                        detail.queued.remove(index);
                    }
                });
            }
            _ => {}
        }
        match self.transcript.apply(&event, base::time::now_ms()) {
            Some(LiveEffect::TurnStarted) => self.busy = true,
            Some(LiveEffect::TurnFinished(stop)) => {
                self.busy = false;
                cx.emit(SessionEvent::TurnFinished(stop));
            }
            Some(LiveEffect::Failed(error)) => {
                self.busy = false;
                cx.emit(SessionEvent::Failed(error));
            }
            None => {}
        }
    }

    fn header(&mut self, session: &Session) {
        self.busy = session.state == SessionState::Running;
        self.context = ContextUsage {
            tokens: session.context_tokens,
            window: session.context_window,
            cost_usd: session.cost_usd,
        };
    }

    fn edit_detail(&mut self, f: impl FnOnce(&mut SessionDetail)) {
        if let Some(detail) = self.detail.as_mut() {
            f(Arc::make_mut(detail));
        }
    }

    /// Reports the batch's block changes and notifies once.
    fn flush(&mut self, cx: &mut Context<Self>) {
        let mut grew = false;
        for splice in self.transcript.take_splices() {
            grew |= splice.count > splice.old.len();
            cx.emit(SessionEvent::Spliced(splice));
        }
        if grew {
            cx.emit(SessionEvent::ScrollToBottom);
        }
        cx.notify();
    }
}

/// The depth limit of the spawn policy of the agent answering in `session`, when the library
/// names one and spawning is on.
fn spawn_depth(api: &dyn Api, session: &Session) -> Option<u32> {
    let runnable = api.runnables().into_iter().find(|runnable| runnable.reference == session.runnable)?;
    let spawn = runnable.agent?.spawn;
    spawn.enabled.then_some(spawn.max_depth)
}
