//! Chat: conversations with a model (Direct), a saved agent (Agent), an orchestrator that
//! plans and delegates (Orchestrator) or a pipeline (Pipeline).
//! Design: design/screens/chat.html; behaviour: design/explanation/chat-modes.html.
//!
//! Layout: chat list | conversation (messages, live cards, sub-task board, composer).
//! Data is loaded in pieces so streaming only re-reads what changed: the list on chat
//! changes, the open chat's detail (task, sub-tasks, runs, decisions) on task/run changes,
//! and single messages on `Change::Message` of the open thread.

mod composer;
mod data;
mod header;
mod list;
mod messages;

use std::collections::{HashMap, HashSet};
use std::sync::Once;

use chrono::{DateTime, Utc};
use gpui::{Focusable as _, KeyBinding, ScrollHandle};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;
use harness_core::{AgentDef, Attachment, Chat, ChatMode, Message};

use super::prelude::*;
use data::{ChatRow, Detail, Pickers, PlanState};
use ui::input::InputEvent;
use ui::select::{SelectEvent, SelectOption};

/// Settings a chat (or the not-yet-created draft chat) runs with.
#[derive(Clone, Debug, PartialEq, Default)]
struct Settings {
    mode: ChatMode,
    model: String,
    agent_id: Option<Id>,
    pipeline_id: Option<Id>,
    cwd: Option<String>,
}

impl Settings {
    fn of(chat: &Chat) -> Self {
        Settings {
            mode: chat.mode,
            model: chat.model.clone(),
            agent_id: chat.agent_id.clone(),
            pipeline_id: chat.pipeline_id.clone(),
            cwd: chat.cwd.clone(),
        }
    }
    fn apply(&self, chat: &mut Chat) {
        chat.mode = self.mode;
        chat.model = self.model.clone();
        chat.agent_id = self.agent_id.clone();
        chat.pipeline_id = self.pipeline_id.clone();
        chat.cwd = self.cwd.clone();
    }
}

/// An inline form that is open.
#[derive(Clone, Debug, PartialEq)]
enum Form {
    /// "Save as agent" under the header.
    SaveAgent,
    /// "Save plan as pipeline" inside the plan card of this message.
    SavePlan { message: Id, steps: Vec<String> },
}

pub struct ChatScreen {
    // ---- data
    rows: Option<Vec<ChatRow>>,
    selected: Option<Id>,
    detail: Option<Detail>,
    messages: Vec<Message>,
    /// Chat the `messages` belong to.
    messages_for: Option<Id>,
    plan_states: HashMap<Id, PlanState>,
    /// Sequence of incremental message reads, and the newest one applied per message.
    msg_seq: u64,
    msg_applied: HashMap<Id, u64>,
    tool_results: HashMap<String, usize>,
    /// Ids of every tool call in the open chat (their results render inside the call).
    call_ids: HashSet<String>,
    pickers: Pickers,
    /// Settings for the chat that sending from the empty state creates.
    draft: Settings,
    /// Validity of the working folder shown in the settings row.
    cwd_valid: Option<bool>,
    // ---- view state
    /// Messages created after this animate in.
    opened_at: DateTime<Utc>,
    scroll: ScrollHandle,
    /// Scroll to the bottom after the next messages load (chat just opened / sent).
    force_bottom: bool,
    expanded: HashSet<String>,
    board_open: bool,
    editing_title: bool,
    form: Option<Form>,
    /// The next message goes as plan feedback (after "Send feedback").
    feedback: bool,
    suggest_ix: usize,
    /// Placeholder currently set on the composer.
    placeholder: SharedString,
    /// Key of the settings last pushed into the selects / folder field.
    synced: Option<String>,
    /// Text to put back into the composer (a send failed).
    restore: Option<String>,
    /// Images for the next message (a draft: kept when switching chats).
    attachments: Vec<Attachment>,
    /// Images still being saved (pasted, dropped or picked).
    adding: usize,
    /// A screenshot is being taken.
    capturing: bool,
    // ---- widgets
    composer: Entity<InputState>,
    search: Entity<InputState>,
    title_input: Entity<InputState>,
    cwd_input: Entity<InputState>,
    name_input: Entity<InputState>,
    role_input: Entity<InputState>,
    model_select: Entity<SelectState<ui::select::Options>>,
    agent_select: Entity<SelectState<ui::select::Options>>,
    pipeline_select: Entity<SelectState<ui::select::Options>>,
    // ---- loads
    list_load: Option<Task<()>>,
    detail_load: Option<Task<()>>,
    msg_load: Option<Task<()>>,
    pick_load: Option<Task<()>>,
    model_load: Option<Task<()>>,
    _tick: Task<()>,
    _subs: Vec<Subscription>,
}

static KEYS: Once = Once::new();

impl ChatScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        KEYS.call_once(|| {
            const CTX: &str = "ChatComposer > Input";
            cx.bind_keys([
                KeyBinding::new("enter", composer::Submit, Some(CTX)),
                KeyBinding::new("secondary-enter", composer::Submit, Some(CTX)),
                KeyBinding::new(
                    "shift-enter",
                    gpui_component::input::Enter { secondary: false },
                    Some(CTX),
                ),
                KeyBinding::new("up", composer::SuggestUp, Some(CTX)),
                KeyBinding::new("down", composer::SuggestDown, Some(CTX)),
                KeyBinding::new("tab", composer::SuggestAccept, Some(CTX)),
                KeyBinding::new("secondary-v", composer::PasteImage, Some(CTX)),
            ]);
        });

        let app = AppState::global(cx);
        let mut subs =
            vec![
                cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
                    AppEvent::StoreChanged { changes, .. } => this.on_changes(changes, cx),
                    AppEvent::Navigated(Route::Chat { chat: Some(id) }) => {
                        this.select(Some(id.clone()), cx)
                    }
                    _ => {}
                }),
            ];

        let composer = ui::input::composer("Message…", window, cx);
        let search = ui::input::text_input("Search chats", window, cx);
        let title_input = ui::input::text_input("Chat title", window, cx);
        let cwd_input = ui::input::text_input("Working folder, e.g. ~/code/app", window, cx);
        let name_input = ui::input::text_input("Name", window, cx);
        let role_input = ui::input::text_area("Who the agent is and how it works", 4, window, cx);
        let model_select = ui::select::select_state(vec![], None, true, window, cx);
        let agent_select = ui::select::select_state(vec![], None, true, window, cx);
        let pipeline_select = ui::select::select_state(vec![], None, true, window, cx);

        subs.push(
            cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, _, cx| {
                match ev {
                    InputEvent::Change => {
                        this.suggest_ix = 0;
                        cx.notify();
                    }
                    // The message box border shows focus.
                    InputEvent::Focus | InputEvent::Blur => cx.notify(),
                    _ => {}
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&title_input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.save_title(cx);
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&cwd_input, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let v = input.read(cx).value().trim().to_string();
                    let cwd = (!v.is_empty()).then_some(v);
                    if cwd != this.settings().cwd {
                        this.change_settings(|s| s.cwd = cwd, cx);
                    }
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&name_input, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit_form(cx);
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &model_select,
            window,
            |this, _, ev: &SelectEvent<ui::select::Options>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else {
                    return;
                };
                if this.settings().model != *v {
                    let v = v.clone();
                    this.change_settings(|s| s.model = v, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &agent_select,
            window,
            |this, _, ev: &SelectEvent<ui::select::Options>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else {
                    return;
                };
                if this.settings().agent_id.as_ref() != Some(v) {
                    let v = v.clone();
                    this.change_settings(|s| s.agent_id = Some(v), cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &pipeline_select,
            window,
            |this, _, ev: &SelectEvent<ui::select::Options>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else {
                    return;
                };
                if this.settings().pipeline_id.as_ref() != Some(v) {
                    let v = v.clone();
                    this.change_settings(|s| s.pipeline_id = Some(v), cx);
                }
            },
        ));

        // "6m ago" labels tick while nothing else changes.
        let tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });

        let selected = match state::route(cx) {
            Route::Chat { chat } => chat,
            _ => None,
        };
        let draft = Settings {
            model: state::engine(cx).config().default_model,
            ..Default::default()
        };
        let mut this = ChatScreen {
            rows: None,
            selected: None,
            detail: None,
            messages: vec![],
            messages_for: None,
            plan_states: HashMap::new(),
            msg_seq: 0,
            msg_applied: HashMap::new(),
            tool_results: HashMap::new(),
            call_ids: HashSet::new(),
            pickers: Pickers::default(),
            draft,
            cwd_valid: None,
            opened_at: Utc::now(),
            scroll: ScrollHandle::new(),
            force_bottom: true,
            expanded: HashSet::new(),
            board_open: true,
            editing_title: false,
            form: None,
            feedback: false,
            suggest_ix: 0,
            placeholder: SharedString::default(),
            synced: None,
            restore: None,
            attachments: vec![],
            adding: 0,
            capturing: false,
            composer,
            search,
            title_input,
            cwd_input,
            name_input,
            role_input,
            model_select,
            agent_select,
            pipeline_select,
            list_load: None,
            detail_load: None,
            msg_load: None,
            pick_load: None,
            model_load: None,
            _tick: tick,
            _subs: subs,
        };
        this.reload_list(cx);
        this.reload_pickers(cx);
        this.reload_models(cx);
        if selected.is_some() {
            this.select(selected, cx);
        }
        this
    }

    // -----------------------------------------------------------------------
    // Loading
    // -----------------------------------------------------------------------

    fn reload_list(&mut self, cx: &mut Context<Self>) {
        self.list_load = Some(load(
            cx,
            |s| data::chat_rows(s),
            |this, rows, cx| {
                // With no chat in the route, open the most recent one (without touching the route).
                if this.selected.is_none() {
                    if let Some(first) = rows.first() {
                        let id = first.chat.id.clone();
                        this.rows = Some(rows);
                        this.select(Some(id), cx);
                        return;
                    }
                }
                this.rows = Some(rows);
            },
        ));
    }

    fn reload_detail(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            self.detail = None;
            return;
        };
        self.detail_load = Some(load(
            cx,
            move |s| data::detail(s, &id),
            |this, d, _| {
                if d.as_ref().map(|d| &d.chat.id) != this.selected.as_ref() {
                    return;
                }
                if let Some(d) = &d {
                    this.cwd_valid = d.cwd_valid;
                    this.plan_states = data::plan_states(
                        &this.messages,
                        &d.decisions,
                        d.task.as_ref().map(|t| t.status),
                    );
                }
                this.detail = d;
            },
        ));
    }

    /// Re-reads every message of the open chat.
    fn reload_messages(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let for_id = id.clone();
        self.msg_load = Some(load(
            cx,
            move |s| data::messages(s, &id),
            move |this, msgs, cx| {
                if this.selected.as_ref() != Some(&for_id) {
                    return;
                }
                this.messages_for = Some(for_id);
                this.set_messages(msgs, cx);
            },
        ));
    }

    /// Re-reads only the given messages of the open chat and merges them in.
    fn refresh_messages(&mut self, ids: Vec<Id>, cx: &mut Context<Self>) {
        if self.messages_for != self.selected || ids.len() > 40 {
            return self.reload_messages(cx);
        }
        let for_id = self.selected.clone();
        self.msg_seq += 1;
        let seq = self.msg_seq;
        let job = load(
            cx,
            move |s| {
                ids.into_iter()
                    .map(|id| (s.message(&id), id))
                    .collect::<Vec<_>>()
            },
            move |this, fetched, cx| {
                if this.selected != for_id {
                    return;
                }
                let mut msgs = std::mem::take(&mut this.messages);
                for (m, id) in fetched {
                    // Loads may finish out of order: never let an older read win.
                    if this.msg_applied.get(&id).is_some_and(|s| *s > seq) {
                        continue;
                    }
                    this.msg_applied.insert(id.clone(), seq);
                    match (m, msgs.iter().position(|x| x.id == id)) {
                        (Some(m), Some(ix)) => msgs[ix] = m,
                        (Some(m), None) => {
                            let at = msgs.partition_point(|x| x.created_at <= m.created_at);
                            msgs.insert(at, m);
                        }
                        (None, Some(ix)) => {
                            msgs.remove(ix);
                        }
                        (None, None) => {}
                    }
                }
                this.set_messages(msgs, cx);
            },
        );
        // Detached on purpose: several incremental loads may be in flight while streaming;
        // each merges its own messages, so none may cancel another.
        job.detach();
    }

    fn set_messages(&mut self, msgs: Vec<Message>, cx: &mut Context<Self>) {
        let near_bottom = self.near_bottom();
        let known_runs: HashSet<&Id> = self
            .detail
            .as_ref()
            .map(|d| d.runs.keys().collect())
            .unwrap_or_default();
        let new_run = msgs.iter().any(|m| match &m.card {
            Some(harness_core::Card::Run { run_id }) => !known_runs.contains(run_id),
            _ => false,
        });
        let new_plan = msgs
            .iter()
            .filter(|m| matches!(m.card, Some(harness_core::Card::Plan { .. })))
            .count()
            != self
                .messages
                .iter()
                .filter(|m| matches!(m.card, Some(harness_core::Card::Plan { .. })))
                .count();
        self.messages = msgs;
        self.tool_results = data::tool_results(&self.messages);
        self.call_ids = self
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        if let Some(d) = &self.detail {
            self.plan_states = data::plan_states(
                &self.messages,
                &d.decisions,
                d.task.as_ref().map(|t| t.status),
            );
        }
        if near_bottom || self.force_bottom {
            self.scroll.scroll_to_bottom();
            self.force_bottom = false;
        }
        if new_run || new_plan {
            self.reload_detail(cx);
        }
    }

    fn reload_pickers(&mut self, cx: &mut Context<Self>) {
        self.pick_load = Some(load(
            cx,
            |s| data::pickers(s),
            |this, (agents, pipelines), _| {
                this.pickers.agents = agents;
                this.pickers.pipelines = pipelines;
                this.synced = None;
            },
        ));
    }

    /// `Engine::models` may ask providers for their model lists: run it off the UI thread.
    fn reload_models(&mut self, cx: &mut Context<Self>) {
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move { engine.models() });
        self.model_load = Some(cx.spawn(async move |this, cx| {
            let models = job.await;
            let _ = this.update(cx, |this, cx| {
                this.pickers.models = models;
                this.synced = None;
                cx.notify();
            });
        }));
    }

    /// Re-queries only what a batch of store changes touched.
    fn on_changes(&mut self, changes: &[Change], cx: &mut Context<Self>) {
        let (mut list, mut detail, mut pickers, mut models, mut all_msgs) =
            (false, false, false, false, false);
        let mut msg_ids: Vec<Id> = vec![];
        let open = self.selected.as_deref();
        let chat_tasks: HashSet<&str> = self
            .rows
            .iter()
            .flatten()
            .filter_map(|r| r.chat.task_id.as_deref())
            .collect();
        let detail_tasks = self
            .detail
            .as_ref()
            .map(|d| d.task_ids())
            .unwrap_or_default();
        let open_task_active = self.task_status().is_some_and(|s| s.is_active());
        let open_runs: HashSet<&Id> = self
            .detail
            .as_ref()
            .map(|d| d.runs.keys().collect())
            .unwrap_or_default();
        for c in changes {
            match c {
                Change::External => {
                    (list, detail, pickers, models, all_msgs) = (true, true, true, true, true);
                }
                Change::Chat(id) => {
                    list = true;
                    detail |= open == Some(id.as_str());
                }
                Change::Message { thread_id, id } if open == Some(thread_id.as_str()) => {
                    msg_ids.push(id.clone())
                }
                Change::Task { id, run_id } => {
                    list |= chat_tasks.contains(id.as_str());
                    let in_run = run_id.as_ref().is_some_and(|r| open_runs.contains(r));
                    // An unknown task while the chat works is most likely a new sub-task.
                    detail |= detail_tasks.contains(id)
                        || in_run
                        || (open_task_active && !chat_tasks.contains(id.as_str()));
                }
                Change::Run(id) => {
                    detail |= open_runs.contains(id);
                    list = true;
                }
                Change::Decision { task_id } => {
                    detail |= self
                        .detail
                        .as_ref()
                        .and_then(|d| d.task.as_ref())
                        .is_some_and(|t| &t.id == task_id);
                }
                Change::Agent(_) | Change::Pipeline(_) => {
                    pickers = true;
                    list = true;
                }
                Change::Config => models = true,
                _ => {}
            }
        }
        if list {
            self.reload_list(cx);
        }
        if detail {
            self.reload_detail(cx);
        }
        if all_msgs {
            self.reload_messages(cx);
        } else if !msg_ids.is_empty() {
            self.refresh_messages(msg_ids, cx);
        }
        if pickers {
            self.reload_pickers(cx);
        }
        if models {
            self.reload_models(cx);
        }
    }

    // -----------------------------------------------------------------------
    // Selection and settings
    // -----------------------------------------------------------------------

    /// Opens a chat (or the empty "new chat" composer with `None`).
    fn select(&mut self, id: Option<Id>, cx: &mut Context<Self>) {
        if id == self.selected && id.is_some() {
            return;
        }
        self.selected = id;
        self.detail = None;
        self.messages.clear();
        self.messages_for = None;
        self.plan_states.clear();
        self.msg_applied.clear();
        self.tool_results.clear();
        self.call_ids.clear();
        self.expanded.clear();
        self.editing_title = false;
        self.form = None;
        self.feedback = false;
        self.synced = None;
        self.cwd_valid = None;
        self.opened_at = Utc::now();
        self.force_bottom = true;
        self.reload_detail(cx);
        self.reload_messages(cx);
        cx.notify();
    }

    fn open_chat(&mut self, id: Id, cx: &mut Context<Self>) {
        state::replace_route(
            Route::Chat {
                chat: Some(id.clone()),
            },
            cx,
        );
        self.select(Some(id), cx);
    }

    /// Settings of the open chat, or of the draft when none is open.
    fn settings(&self) -> Settings {
        match &self.detail {
            Some(d) => Settings::of(&d.chat),
            None => self.draft.clone(),
        }
    }

    fn task_status(&self) -> Option<Status> {
        self.detail
            .as_ref()
            .and_then(|d| d.task.as_ref())
            .map(|t| t.status)
    }

    /// Changes the open chat's settings (saved through the store) or the draft's.
    fn change_settings(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut s = self.settings();
        f(&mut s);
        if let Some(cwd) = s.cwd.clone() {
            let job = cx.background_spawn(async move { data::cwd_exists(&cwd) });
            cx.spawn(async move |this, cx| {
                let ok = job.await;
                let _ = this.update(cx, |this, cx| {
                    this.cwd_valid = Some(ok);
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.cwd_valid = None;
        }
        match &mut self.detail {
            Some(d) => {
                s.apply(&mut d.chat);
                let chat = d.chat.clone();
                state::run_engine(cx, move |e| e.store().put_chat(&chat)).detach();
            }
            None => self.draft = s,
        }
        self.synced = None;
        cx.notify();
    }

    fn set_mode(&mut self, mode: ChatMode, cx: &mut Context<Self>) {
        if self.settings().mode == mode {
            return;
        }
        let pickers = self.pickers.clone();
        self.change_settings(
            |s| {
                s.mode = mode;
                // Pick something sensible so the chat is ready to send.
                if mode == ChatMode::Agent && s.agent_id.is_none() {
                    s.agent_id = pickers.agents.first().map(|a| a.id.clone());
                }
                if mode == ChatMode::Pipeline && s.pipeline_id.is_none() {
                    s.pipeline_id = pickers.pipelines.first().map(|p| p.id.clone());
                }
            },
            cx,
        );
    }

    /// Pushes settings into the selects and the folder field when they changed.
    /// Called from render (these widgets need the window).
    fn sync_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.restore.take() {
            self.composer.update(cx, |s, cx| {
                if s.value().trim().is_empty() {
                    s.set_value(text, window, cx);
                }
            });
        }
        let placeholder = self.placeholder_text();
        if placeholder != self.placeholder {
            self.placeholder = placeholder.clone();
            self.composer
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let s = self.settings();
        let key = format!(
            "{:?}|{}|{}|{}|{}",
            self.selected,
            self.pickers.models.len(),
            self.pickers.agents.len(),
            self.pickers.pipelines.len(),
            serde_json::to_string(&(&s.model, &s.agent_id, &s.pipeline_id, &s.cwd))
                .unwrap_or_default()
        );
        if self.synced.as_ref() == Some(&key) {
            return;
        }
        self.synced = Some(key);
        let mut models = self.pickers.models.clone();
        if !s.model.is_empty() && !models.contains(&s.model) {
            models.insert(0, s.model.clone());
        }
        let model_opts = model_options(&models);
        ui::select::set_options(&self.model_select, model_opts, Some(&s.model), window, cx);
        let agent_opts = self
            .pickers
            .agents
            .iter()
            .map(|a| SelectOption::new(a.id.clone(), a.name.clone()))
            .collect();
        ui::select::set_options(
            &self.agent_select,
            agent_opts,
            s.agent_id.as_deref(),
            window,
            cx,
        );
        let pipe_opts = self
            .pickers
            .pipelines
            .iter()
            .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
            .collect();
        ui::select::set_options(
            &self.pipeline_select,
            pipe_opts,
            s.pipeline_id.as_deref(),
            window,
            cx,
        );
        let cwd = s.cwd.unwrap_or_default();
        if !self.cwd_input.focus_handle(cx).is_focused(window)
            && self.cwd_input.read(cx).value().as_ref() != cwd
        {
            self.cwd_input
                .update(cx, |i, cx| i.set_value(cwd, window, cx));
        }
    }

    fn agent_name(&self, id: Option<&Id>) -> Option<&str> {
        let id = id?;
        self.pickers
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.name.as_str())
    }

    fn pipeline_name(&self, id: Option<&Id>) -> Option<&str> {
        let id = id?;
        self.pickers
            .pipelines
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.name.as_str())
    }

    // -----------------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------------

    /// "New chat": a chat in the currently chosen mode, with the current settings.
    fn new_chat(&mut self, cx: &mut Context<Self>) {
        let s = self.settings();
        let task = state::run_engine(cx, move |e| {
            let mut c = e.new_chat(s.mode)?;
            if !s.model.is_empty() {
                c.model = s.model.clone();
            }
            c.agent_id = s.agent_id;
            c.pipeline_id = s.pipeline_id;
            c.cwd = s.cwd;
            e.store().put_chat(&c)?;
            Ok(c.id)
        });
        cx.spawn(async move |this, cx| {
            if let Some(id) = task.await {
                let _ = this.update(cx, |this, cx| {
                    state::navigate(
                        Route::Chat {
                            chat: Some(id.clone()),
                        },
                        cx,
                    );
                    this.select(Some(id), cx);
                });
            }
        })
        .detach();
    }

    fn save_title(&mut self, cx: &mut Context<Self>) {
        if !self.editing_title {
            return;
        }
        self.editing_title = false;
        let title = self.title_input.read(cx).value().trim().to_string();
        if let Some(d) = &mut self.detail {
            if !title.is_empty() && title != d.chat.title {
                d.chat.title = title.clone();
                let mut chat = d.chat.clone();
                let task_id = chat.task_id.clone();
                state::run_engine(cx, move |e| {
                    chat.title = title.clone();
                    e.store().put_chat(&chat)?;
                    if let Some(t) = task_id {
                        e.store().update_task(&t, |t| t.title = title)?;
                    }
                    Ok(())
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn start_edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.detail else { return };
        let title = d.chat.title.clone();
        self.editing_title = true;
        self.title_input.update(cx, |s, cx| {
            s.set_value(title, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn open_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        let (name, role) = match &form {
            Form::SaveAgent => {
                let first = self
                    .messages
                    .iter()
                    .find(|m| m.role == harness_core::Role::User)
                    .map(|m| m.content.trim().to_string())
                    .unwrap_or_default();
                let title = self
                    .detail
                    .as_ref()
                    .map(|d| d.chat.title.clone())
                    .unwrap_or_default();
                (title, first)
            }
            Form::SavePlan { .. } => (
                self.detail
                    .as_ref()
                    .map(|d| d.chat.title.clone())
                    .unwrap_or_default(),
                String::new(),
            ),
        };
        self.name_input.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
        });
        self.role_input
            .update(cx, |s, cx| s.set_value(role, window, cx));
        self.form = Some(form);
        cx.notify();
    }

    fn submit_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.form.clone() else {
            return;
        };
        let Some(d) = &self.detail else { return };
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            state::toast(ToastKind::Warning, "Give it a name first", cx);
            return;
        }
        match form {
            Form::SaveAgent => {
                let role = self.role_input.read(cx).value().trim().to_string();
                let model = if d.chat.model.is_empty() {
                    state::engine(cx).config().default_model
                } else {
                    d.chat.model.clone()
                };
                let now = harness_core::now();
                let agent = AgentDef {
                    id: harness_core::new_id(),
                    name: name.clone(),
                    description: role
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(80)
                        .collect(),
                    role,
                    model,
                    // A direct chat had no tools; widen it in Agents if needed.
                    tools: harness_core::ToolAccess::None,
                    skills: vec![],
                    skills_on_demand: vec![],
                    max_steps: harness_core::default_max_steps(),
                    version: 1,
                    created_at: now,
                    updated_at: now,
                };
                let task = state::run_engine(cx, move |e| {
                    if e.store().agent_by_name(&agent.name).is_some() {
                        anyhow::bail!("an agent named {} already exists", agent.name);
                    }
                    e.store().put_agent(&agent)?;
                    e.store().log(
                        "agent.created",
                        None,
                        None,
                        format!("Saved chat as agent {}", agent.name),
                    );
                    Ok(agent.id)
                });
                cx.spawn(async move |this, cx| {
                    if let Some(id) = task.await {
                        let _ = this.update(cx, |this, cx| {
                            this.form = None;
                            state::toast(ToastKind::Success, format!("Saved agent {name}"), cx);
                            state::navigate(Route::Agents { agent: Some(id) }, cx);
                        });
                    }
                })
                .detach();
            }
            Form::SavePlan { steps, .. } => {
                let cwd = d.chat.cwd.clone();
                let task = state::run_engine(cx, move |e| {
                    e.pipeline_from_plan(&name, &steps, cwd.as_deref())
                });
                cx.spawn(async move |this, cx| {
                    if let Some(p) = task.await {
                        let _ = this.update(cx, |this, cx| {
                            this.form = None;
                            state::toast(
                                ToastKind::Success,
                                format!("Saved pipeline {}", p.name),
                                cx,
                            );
                            state::navigate(
                                Route::Pipelines {
                                    pipeline: Some(p.id),
                                },
                                cx,
                            );
                        });
                    }
                })
                .detach();
            }
        }
    }

    fn near_bottom(&self) -> bool {
        let max = self.scroll.max_offset().height;
        let off = -self.scroll.offset().y;
        max - off < px(48.)
    }
}

/// Model options: short labels, the full id where two models share one.
fn model_options(models: &[String]) -> Vec<SelectOption> {
    let shorts: Vec<String> = models.iter().map(|m| data::short_model(m)).collect();
    models
        .iter()
        .zip(&shorts)
        .map(|(m, s)| {
            let dup = shorts.iter().filter(|x| *x == s).count() > 1;
            SelectOption::new(
                m.clone(),
                if dup || s.is_empty() {
                    m.clone()
                } else {
                    s.clone()
                },
            )
        })
        .collect()
}

/// Icon for a chat mode.
fn mode_icon(mode: ChatMode) -> AppIcon {
    match mode {
        ChatMode::Direct => AppIcon::Chat,
        ChatMode::Agent => AppIcon::Bot,
        ChatMode::Orchestrator => AppIcon::GitFork,
        ChatMode::Pipeline => AppIcon::Workflow,
    }
}

/// `09:41` in local time.
fn clock(t: DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%H:%M").to_string()
}

/// A stable element id from a prefix and a record id.
fn eid(prefix: &str, id: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}-{id}")))
}

impl Render for ChatScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_widgets(window, cx);
        let content = match &self.rows {
            None => div()
                .flex_1()
                .min_h_0()
                .p(theme::S6)
                .child(skeleton_rows("chat-loading", 6)),
            Some(_) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.render_list(cx))
                .child(self.render_conversation(window, cx)),
        };
        screen()
            .child(self.render_header(window, cx))
            .child(content)
    }
}
