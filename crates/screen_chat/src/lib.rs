//! Chat session screen: a real coding-agent chat. Header, transcript, composer and the
//! session-details panel, driven by the `llm` client and the `agent_tools` workspace.
//!
//! - `agent` — the agent loop (streaming, tool execution, permissions, interrupts, shell).
//! - `session` — transcript items, API history building, JSON persistence.
//! - `markdown` — assistant text rendering. `models` — model selection, windows, prices.
//! - `todo` — the local `todo_write` tool.
//! - `extensions` — skills (`skill` tool) and MCP servers (startup, tools, calls).
//! - `picker` — the "Talk to" picker: Build / Plan, custom agents, forced skills.

mod agent;
mod composer;
mod data;
mod deferred;
mod extensions;
mod header;
mod markdown;
mod models;
mod panel;
mod picker;
mod session;
mod todo;
mod transcript;
mod widgets;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent_tools::Workspace;
use gpui::{
    AppContext as _, Context, Entity, Focusable, IntoElement, Render, ScrollHandle, SharedString,
    Subscription, Task, Window, div, prelude::*,
};
use llm::Usage;
use ui::{TextInput, TextInputEvent, theme::*};

use agent::Run;
use agent_tools::agents::AgentDef;
use agent_tools::skills::Skill;
use data::*;
use extensions::{McpState, SkillMeta};
use models::{ModelRef, ProviderModels};
use session::Item;
use todo::TodoItem;

/// Loaded provider configuration.
pub(crate) enum ConfigState {
    Loading,
    Ready(Arc<llm::Config>),
    Failed(String),
}

/// An @-mention suggestion (a workspace file).
pub(crate) struct Mention {
    pub label: SharedString,
}

/// The popover above the composer derived from the draft.
pub(crate) enum Suggest {
    None,
    /// `/cmd` — matching commands (may be empty: "No command matches").
    Cmds(Vec<&'static Cmd>),
    /// `@query` — matching files, never empty.
    Mentions {
        start: usize,
        items: Vec<Mention>,
    },
}

impl Suggest {
    fn len(&self) -> usize {
        match self {
            Suggest::None => 0,
            Suggest::Cmds(c) => c.len(),
            Suggest::Mentions { items, .. } => items.len(),
        }
    }
}

/// Popover triggers in the composer toolbar (tracked so an outside-click close
/// doesn't immediately reopen when the click lands on the trigger itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    Agent,
    Model,
}

pub struct ChatScreen {
    // Environment
    pub(crate) workspace: Arc<Workspace>,
    pub(crate) repo: Option<(String, Option<String>)>,
    /// Sorted workspace files for @-mentions.
    pub(crate) files: Arc<Vec<String>>,
    pub(crate) context: Option<String>,
    pub(crate) config: ConfigState,
    pub(crate) catalog: Vec<(String, ProviderModels)>,
    pub(crate) model: Option<ModelRef>,
    pub(crate) mode: Mode,
    /// Discovered skills (empty until discovery finishes).
    pub(crate) skills: Arc<Vec<Skill>>,
    /// Size and loaded text of each skill, by name.
    pub(crate) skill_meta: Arc<BTreeMap<String, SkillMeta>>,
    /// Custom agents from `~/.claude/agents` and `<workspace>/.claude/agents`.
    pub(crate) agents: Arc<Vec<AgentDef>>,
    /// Agent discovery finished (an unknown saved agent can be dropped).
    pub(crate) agents_loaded: bool,
    pub(crate) mcp: McpState,

    // Session
    pub(crate) session_id: String,
    /// The session file exists on disk (so an emptied session is saved too).
    pub(crate) persisted: bool,
    pub(crate) items: Vec<Item>,
    pub(crate) todos: Vec<TodoItem>,
    /// `(path, added, removed)` from successful edits/writes.
    pub(crate) modified: Vec<(String, usize, usize)>,
    pub(crate) last_usage: Option<(Usage, String)>,
    pub(crate) total_usage: Usage,
    pub(crate) cost_usd: f64,
    /// Some request used a model without a known price.
    pub(crate) unpriced: bool,
    pub(crate) always_allow: BTreeSet<String>,
    /// Selected custom agent (by name); `mode` is Build while one is selected.
    pub(crate) custom_agent: Option<String>,
    /// Skills whose full text is in the system prompt for every request.
    pub(crate) forced: BTreeSet<String>,
    /// Deferred MCP tools whose schemas the model loaded with `tool_search`.
    pub(crate) loaded_mcp: BTreeSet<String>,
    pub(crate) run: Option<Run>,
    /// Saved sessions (newest first) shown in the app sidebar.
    pub(crate) index: Vec<session::IndexEntry>,
    /// Bumped on every session switch so a late `load` for an older request is ignored.
    pub(crate) switch_seq: u64,

    // Composer
    pub(crate) pick_open: bool,
    /// Keyboard-highlighted agent row in the picker.
    pub(crate) pick_sel: usize,
    pub(crate) pick_scroll: ScrollHandle,
    pub(crate) model_open: bool,
    pub(crate) trigger_hover: Option<Trigger>,
    pub(crate) draft: String,
    /// Highlighted row in the slash / mention popover.
    pub(crate) menu_sel: usize,
    pub(crate) input: Entity<TextInput>,
    pub(crate) model_search: Entity<TextInput>,
    pub(crate) model_query: String,
    /// Optional note sent with "Reject" in a permission prompt.
    pub(crate) note_input: Entity<TextInput>,
    pub(crate) scroll: ScrollHandle,
    /// Mouse text selection over the transcript (see `ui::selectable`).
    pub(crate) selection: ui::selectable::Selection,
    /// Focused when transcript text is pressed, so ctrl-c copies the selection.
    pub(crate) transcript_focus: gpui::FocusHandle,

    config_task: Option<Task<()>>,
    skills_task: Option<Task<()>>,
    agents_task: Option<Task<()>>,
    model_tasks: Vec<Task<()>>,
    _startup: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

fn workspace_root() -> PathBuf {
    std::env::var("HARNESS_WORKSPACE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

impl ChatScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(Self::placeholder_for(Mode::Build), cx));
        let transcript_focus = cx.focus_handle();
        let selection = ui::selectable::new_selection();
        selection.borrow_mut().set_focus(transcript_focus.clone());
        // Dev/test hook: `HARNESS_CHAT_SELECT_ALL=1` selects the whole transcript on start.
        if std::env::var("HARNESS_CHAT_SELECT_ALL").is_ok_and(|v| v == "1") {
            selection.borrow_mut().select_all();
        }
        let model_search = cx.new(|cx| TextInput::new("Search models", cx));
        let note_input =
            cx.new(|cx| TextInput::new("Optional note for the agent, sent with Reject", cx));
        let mut subs = vec![
            cx.subscribe_in(&input, window, Self::on_input_event),
            cx.subscribe_in(&model_search, window, Self::on_search_event),
            cx.subscribe_in(&note_input, window, Self::on_note_event),
        ];
        // Every screen is built up-front by the shell; only the visible one may take focus.
        if ui::nav::nav_entity(cx).is_none() || ui::nav::current(cx) == ui::nav::Screen::Chat {
            input.focus_handle(cx).focus(window);
        }
        subs.extend(ui::nav::on_activate(
            ui::nav::Screen::Chat,
            window,
            cx,
            |this, window, cx| this.focus_input(window, cx),
        ));

        let workspace = Arc::new(Workspace::new(workspace_root()));
        let ws = workspace.clone();
        let info = cx.spawn(async move |this, cx| {
            let (label, mut files, context) = cx
                .background_spawn(
                    async move { (ws.repo_label(), ws.files(2000), ws.context_summary()) },
                )
                .await;
            files.sort();
            this.update(cx, |this, cx| {
                this.repo = Some(label);
                this.files = Arc::new(files);
                this.context = Some(context);
                cx.notify();
            })
            .ok();
        });
        let restore = cx.spawn(async move |this, cx| {
            let (index, saved) = cx
                .background_spawn(async {
                    let dir = session::sessions_dir();
                    (
                        session::list(&dir, SESSION_LIST_LIMIT),
                        session::load_latest(&dir),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.index = index;
                if let Some(saved) = saved {
                    this.restore(saved, cx);
                }
                // Dev/test hook: open a saved session through the same path as a sidebar click.
                if let Some(id) = std::env::var("HARNESS_CHAT_OPEN")
                    .ok()
                    .filter(|s| !s.is_empty())
                {
                    cx.defer(move |cx| {
                        ui::nav::request_chat(ui::nav::ChatRequest::Open(id.into()), cx)
                    });
                }
                cx.notify();
            })
            .ok();
        });
        // Keep the sidebar's session list in sync, and react to its open/new requests.
        subs.push(cx.observe_self(|this, cx| this.publish_sessions(cx)));
        if let Some(nav) = ui::nav::nav_entity(cx) {
            subs.push(cx.observe(&nav, |this, _, cx| this.handle_chat_request(cx)));
        }
        // MCP server processes also stop when the screen drops (`Client` shuts down on drop).
        subs.push(cx.on_app_quit(|this, _| this.shutdown_mcp()));

        let mut this = Self {
            workspace,
            repo: None,
            files: Arc::new(Vec::new()),
            context: None,
            config: ConfigState::Loading,
            catalog: Vec::new(),
            model: None,
            mode: Mode::Build,
            skills: Arc::new(Vec::new()),
            skill_meta: Arc::new(BTreeMap::new()),
            agents: Arc::new(Vec::new()),
            agents_loaded: false,
            mcp: McpState::new(),
            session_id: session::new_id(),
            persisted: false,
            items: Vec::new(),
            todos: Vec::new(),
            modified: Vec::new(),
            last_usage: None,
            total_usage: Usage::default(),
            cost_usd: 0.,
            unpriced: false,
            always_allow: BTreeSet::new(),
            custom_agent: None,
            // Dev/test hook: `HARNESS_CHAT_FORCE=skill-a,skill-b` forces skills on startup.
            forced: std::env::var("HARNESS_CHAT_FORCE")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            loaded_mcp: BTreeSet::new(),
            run: None,
            index: Vec::new(),
            switch_seq: 0,
            // Dev hook for screenshots: open the agent picker on startup.
            pick_open: std::env::var("HARNESS_CHAT_PICKER_OPEN").is_ok_and(|v| v == "1"),
            pick_sel: 0,
            pick_scroll: ScrollHandle::new(),
            model_open: false,
            trigger_hover: None,
            draft: String::new(),
            menu_sel: 0,
            input,
            model_search,
            model_query: String::new(),
            note_input,
            scroll: ScrollHandle::new(),
            selection,
            transcript_focus,
            config_task: None,
            skills_task: None,
            agents_task: None,
            model_tasks: Vec::new(),
            _startup: vec![info, restore],
            _subs: subs,
        };
        this.load_config(cx);
        this.discover_skills(cx);
        this.discover_agents(cx);
        this.load_mcp(cx);
        this
    }

    // ── Derived values ──────────────────────────────────────────────────

    pub(crate) fn is_shell(&self) -> bool {
        self.draft.starts_with('!')
    }

    pub(crate) fn busy(&self) -> bool {
        self.run.is_some()
    }

    pub(crate) fn repo_name(&self) -> String {
        match &self.repo {
            Some((name, _)) => name.clone(),
            None => self
                .workspace
                .root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "workspace".into()),
        }
    }

    pub(crate) fn title(&self) -> String {
        session::title(&self.items).unwrap_or_else(|| "New session".into())
    }

    pub(crate) fn has_providers(&self) -> bool {
        matches!(&self.config, ConfigState::Ready(c) if !c.providers.is_empty())
    }

    fn placeholder_for(mode: Mode) -> String {
        match mode {
            Mode::Plan => "Ask Plan to look into something. It reads files and writes a plan."
                .into(),
            Mode::Build => "Ask Build to change something. @ to add a file, / for commands, ! to run a shell command"
                .into(),
        }
    }

    /// Placeholder for the selected agent.
    pub(crate) fn placeholder(&self) -> String {
        match &self.custom_agent {
            Some(name) => format!(
                "Ask {name} something. @ to add a file, / for commands, ! to run a shell command"
            ),
            None => Self::placeholder_for(self.mode),
        }
    }

    pub(crate) fn refresh_placeholder(&self, cx: &mut Context<Self>) {
        let p = self.placeholder();
        self.input.update(cx, |i, cx| i.set_placeholder(p, cx));
    }

    pub(crate) fn mode_hint(&self) -> String {
        if self.is_shell() {
            return format!(
                "Shell mode · runs in {}, output goes into the context",
                self.repo_name()
            );
        }
        if self.busy() {
            if matches!(self.items.last(), Some(Item::Shell(_))) {
                return "Running shell command · Esc to interrupt".into();
            }
            if self.waiting_permission() {
                return format!("{} is waiting for your permission above", self.agent_name());
            }
            return format!("{} is working · Esc to interrupt", self.agent_name());
        }
        if let Some(name) = &self.custom_agent {
            let tools = match self.active_agent().and_then(|a| a.tools.as_ref()) {
                None => "Build tools".to_string(),
                Some(list) => {
                    let g = agent_tools::agents::map_tools(list);
                    match g.builtin.len() + g.mcp.len() {
                        1 => "1 tool".to_string(),
                        n => format!("{n} tools"),
                    }
                }
            };
            return format!("{name} · custom agent · {tools}, edits and bash ask first");
        }
        match self.mode {
            Mode::Plan => "Plan · read-only: only read tools are offered".into(),
            Mode::Build => "Build · full access: edits and bash ask first".into(),
        }
    }

    pub(crate) fn suggestions(&self) -> Suggest {
        let draft = self.draft.as_str();
        if let Some(q) = draft.strip_prefix('/')
            && !q.contains(char::is_whitespace)
        {
            return Suggest::Cmds(CMDS.iter().filter(|c| c.name.starts_with(q)).collect());
        }
        // `(^|\s)@([\w./-]*)$`
        let start = draft
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let token = &draft[start..];
        let Some(q) = token.strip_prefix('@') else {
            return Suggest::None;
        };
        if !q
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
        {
            return Suggest::None;
        }
        let q = q.to_lowercase();
        let items: Vec<Mention> = self
            .files
            .iter()
            .filter(|f| f.to_lowercase().contains(&q))
            .take(7)
            .map(|f| Mention {
                label: SharedString::from(f.clone()),
            })
            .collect();
        if items.is_empty() {
            Suggest::None
        } else {
            Suggest::Mentions { start, items }
        }
    }

    // ── Input events ────────────────────────────────────────────────────

    fn on_input_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                self.draft = input.read(cx).text().to_string();
                self.menu_sel = 0;
                cx.notify();
            }
            TextInputEvent::Submit => {
                if self.pick_open {
                    self.pick_highlighted(window, cx);
                } else if self.suggestions().len() > 0 {
                    self.pick_suggestion(self.menu_sel, cx);
                } else {
                    self.send(window, cx);
                }
            }
            TextInputEvent::Tab => {
                if self.suggestions().len() > 0 {
                    self.pick_suggestion(self.menu_sel, cx);
                } else if self.draft.is_empty() {
                    self.set_mode(self.mode.toggled(), cx);
                }
            }
            TextInputEvent::Up | TextInputEvent::Down if self.pick_open => {
                self.move_pick(*event == TextInputEvent::Up, cx);
            }
            TextInputEvent::Up | TextInputEvent::Down => {
                let n = self.suggestions().len();
                if n > 0 {
                    self.menu_sel = if *event == TextInputEvent::Up {
                        (self.menu_sel + n - 1) % n
                    } else {
                        (self.menu_sel + 1) % n
                    };
                    cx.notify();
                }
            }
            TextInputEvent::Cancel => {
                if self.pick_open || self.model_open {
                    self.pick_open = false;
                    self.model_open = false;
                    cx.notify();
                } else if self.busy() {
                    self.interrupt(cx);
                } else {
                    self.set_draft("", cx);
                }
            }
            TextInputEvent::Blur => {}
        }
    }

    fn on_search_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                self.model_query = input.read(cx).text().to_string();
                cx.notify();
            }
            TextInputEvent::Submit => {
                if let Some(m) = self.filtered_models().into_iter().next() {
                    self.select_model(m, window, cx);
                }
            }
            TextInputEvent::Cancel => {
                self.model_open = false;
                self.focus_input(window, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_note_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Submit => {
                if let Some(id) = self.asking_tool_id() {
                    self.answer(&id, agent::Decision::Reject, cx);
                    self.focus_input(window, cx);
                }
            }
            TextInputEvent::Cancel => self.focus_input(window, cx),
            _ => {}
        }
    }

    pub(crate) fn set_draft(&mut self, text: &str, cx: &mut Context<Self>) {
        self.draft = text.to_string();
        self.menu_sel = 0;
        let text = SharedString::from(text.to_string());
        self.input.update(cx, |i, cx| i.set_text(text, cx));
        cx.notify();
    }

    pub(crate) fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.focus_handle(cx).focus(window);
    }

    pub(crate) fn pick_suggestion(&mut self, ix: usize, cx: &mut Context<Self>) {
        let next = match self.suggestions() {
            Suggest::None => return,
            Suggest::Cmds(cmds) => match cmds.get(ix) {
                Some(c) => format!("/{} ", c.name),
                None => return,
            },
            Suggest::Mentions { start, items } => match items.get(ix) {
                Some(m) => format!("{}@{} ", &self.draft[..start], m.label),
                None => return,
            },
        };
        self.set_draft(&next, cx);
    }

    // ── Mode / model ────────────────────────────────────────────────────

    pub(crate) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.custom_agent = None;
        self.pick_open = false;
        self.refresh_placeholder(cx);
        self.agent_changed(cx);
        cx.notify();
    }

    pub(crate) fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_open = true;
        self.pick_open = false;
        self.model_query.clear();
        self.model_search.update(cx, |i, cx| i.set_text("", cx));
        if self.model_count() > SEARCH_THRESHOLD {
            self.model_search.focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    pub(crate) fn model_count(&self) -> usize {
        self.catalog
            .iter()
            .map(|(_, m)| match m {
                ProviderModels::Ready(v) => v.len(),
                _ => 0,
            })
            .sum()
    }

    /// Models matching the search query, in catalog order.
    pub(crate) fn filtered_models(&self) -> Vec<ModelRef> {
        let q = self.model_query.trim().to_lowercase();
        self.catalog
            .iter()
            .filter_map(|(p, m)| match m {
                ProviderModels::Ready(v) => Some((p, v)),
                _ => None,
            })
            .flat_map(|(p, v)| {
                v.iter().map(move |m| ModelRef {
                    provider: p.clone(),
                    model: m.clone(),
                })
            })
            .filter(|m| q.is_empty() || m.label().to_lowercase().contains(&q))
            .collect()
    }

    pub(crate) fn select_model(
        &mut self,
        m: ModelRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model = Some(m);
        self.model_open = false;
        self.focus_input(window, cx);
        cx.notify();
    }
}

/// How many saved sessions the sidebar lists.
pub(crate) const SESSION_LIST_LIMIT: usize = 12;

/// Show a search field in the model menu above this many models.
pub(crate) const SEARCH_THRESHOLD: usize = 12;

impl Render for ChatScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_transcript(cx))
                            .child(self.render_composer(cx)),
                    )
                    .child(self.render_panel(cx)),
            )
    }
}
