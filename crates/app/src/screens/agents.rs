//! Agents: saved agents (role, model, tools, skills) and how they perform.
//! Design: design/screens/agents.html. Left: the saved agents; middle: the editor for the
//! selected agent (prompt layer 2); right: stats, pipelines that use it, recent tasks.
//! Memory scope, versions and helper agents from the mockup are not part of this release.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use gpui::{Animation, AnimationExt as _, ClickEvent};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;
use gpui_component::tooltip::Tooltip;
use harness_core::skills::Skill;
use harness_core::{default_max_steps, new_id, now, AgentDef, ChatMode, NodeKind, ToolAccess};
use harness_core::{Pipeline, Task as WorkTask};
use harness_engine::Engine;

use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self as sel, Options, SelectEvent, SelectOption};

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// What an agent did, from the tasks whose `actor` is its name.
#[derive(Default, Clone)]
struct AgentStats {
    tasks: usize,
    done: usize,
    failed: usize,
    cost: f64,
    last_used: Option<DateTime<Utc>>,
    /// Newest first.
    recent: Vec<RecentTask>,
}

#[derive(Clone)]
struct RecentTask {
    id: Id,
    title: String,
    status: Status,
    cost: f64,
    when: DateTime<Utc>,
}

impl AgentStats {
    fn avg_cost(&self) -> Option<f64> {
        (self.tasks > 0).then(|| self.cost / self.tasks as f64)
    }
    /// Done / (done + failed); cancelled and unfinished tasks don't count.
    fn success_rate(&self) -> Option<f64> {
        let finished = self.done + self.failed;
        (finished > 0).then(|| self.done as f64 / finished as f64)
    }
}

struct AgentsData {
    agents: Vec<AgentDef>,
    stats: HashMap<Id, AgentStats>,
    /// Agent id -> pipelines (id, name) with an Agent node that uses it.
    used_in: HashMap<Id, Vec<(Id, String)>>,
    models: Vec<String>,
    skills: Vec<Skill>,
}

impl AgentsData {
    fn query(store: &harness_core::Store, engine: &Engine) -> AgentsData {
        let agents = store.agents();
        let by_name: HashMap<&str, &Id> = agents
            .iter()
            .rev()
            .map(|a| (a.name.as_str(), &a.id))
            .collect();
        let mut stats: HashMap<Id, AgentStats> = HashMap::new();
        for t in store.tasks() {
            if let Some(id) = by_name.get(t.actor.as_str()) {
                add_task(stats.entry((*id).clone()).or_default(), &t);
            }
        }
        for s in stats.values_mut() {
            s.recent.sort_by_key(|r| std::cmp::Reverse(r.when));
            s.recent.truncate(6);
        }
        let pipelines = store.pipelines();
        let used_in = agents
            .iter()
            .map(|a| {
                let list = pipelines
                    .iter()
                    .filter(|p| !p.archived && uses_agent(p, &a.id))
                    .map(|p| (p.id.clone(), p.name.clone()))
                    .collect();
                (a.id.clone(), list)
            })
            .collect();
        let mut models = engine.models();
        for a in &agents {
            if !a.model.is_empty() && !models.contains(&a.model) {
                models.push(a.model.clone());
            }
        }
        models.sort();
        AgentsData {
            agents,
            stats,
            used_in,
            models,
            skills: engine.skills(),
        }
    }
}

fn add_task(s: &mut AgentStats, t: &WorkTask) {
    s.tasks += 1;
    s.cost += t.cost_usd;
    match t.status {
        Status::Done => s.done += 1,
        Status::Failed => s.failed += 1,
        _ => {}
    }
    let when = t.finished_at.or(t.started_at).unwrap_or(t.created_at);
    if s.last_used.is_none_or(|l| when > l) {
        s.last_used = Some(when);
    }
    s.recent.push(RecentTask {
        id: t.id.clone(),
        title: t.title.clone(),
        status: t.status,
        cost: t.cost_usd,
        when,
    });
}

/// Does the live or draft graph of `p` have an Agent node using agent `id`?
fn uses_agent(p: &Pipeline, id: &str) -> bool {
    let has = |g: &harness_core::PipelineGraph| {
        g.nodes
            .iter()
            .any(|n| matches!(&n.kind, NodeKind::Agent(a) if a.agent.as_deref() == Some(id)))
    };
    has(&p.graph) || p.draft.as_ref().is_some_and(has)
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// The form for one agent: inputs plus the saved agent it was made from (`base`).
struct Editor {
    base: AgentDef,
    name: Entity<InputState>,
    description: Entity<InputState>,
    role: Entity<InputState>,
    steps: Entity<InputState>,
    model: Entity<SelectState<Options>>,
    tools: Entity<SelectState<Options>>,
    skills: Vec<String>,
    on_demand: Vec<String>,
    _subs: Vec<Subscription>,
}

impl Editor {
    fn new(
        agent: &AgentDef,
        data: &AgentsData,
        window: &mut Window,
        cx: &mut Context<AgentsScreen>,
    ) -> Editor {
        let name = input::text_input_with("Agent name", agent.name.clone(), window, cx);
        let description = input::text_input_with(
            "One line: what it's for",
            agent.description.clone(),
            window,
            cx,
        );
        let role = input::text_area(
            "Who the agent is and how it works. One rule per line works well.",
            12,
            window,
            cx,
        );
        role.update(cx, |s, cx| s.set_value(agent.role.clone(), window, cx));
        let steps = input::text_input_with("40", agent.max_steps.to_string(), window, cx);
        let model_options: Vec<SelectOption> = data
            .models
            .iter()
            .map(|m| SelectOption::plain(m.clone()))
            .collect();
        let model = sel::select_state(model_options, Some(&agent.model), true, window, cx);
        let tool_options: Vec<SelectOption> = ToolAccess::ALL
            .iter()
            .map(|t| SelectOption::new(t.label(), t.label()))
            .collect();
        let tools = sel::select_state(tool_options, Some(agent.tools.label()), false, window, cx);

        let mut subs = Vec::new();
        for i in [&name, &description, &role, &steps] {
            subs.push(cx.subscribe_in(i, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        for s in [&model, &tools] {
            subs.push(
                cx.subscribe_in(s, window, |_, _, _: &SelectEvent<Options>, _, cx| {
                    cx.notify()
                }),
            );
        }
        Editor {
            base: agent.clone(),
            name,
            description,
            role,
            steps,
            model,
            tools,
            skills: agent.skills.clone(),
            on_demand: agent.skills_on_demand.clone(),
            _subs: subs,
        }
    }

    /// The agent as currently edited (version and timestamps unchanged).
    fn draft(&self, cx: &App) -> AgentDef {
        let mut a = self.base.clone();
        a.name = self.name.read(cx).value().trim().to_string();
        a.description = self.description.read(cx).value().trim().to_string();
        a.role = self.role.read(cx).value().to_string();
        if let Some(m) = self.model.read(cx).selected_value() {
            a.model = m.clone();
        }
        if let Some(l) = self.tools.read(cx).selected_value() {
            if let Some(t) = ToolAccess::ALL.iter().find(|t| t.label() == l) {
                a.tools = *t;
            }
        }
        a.max_steps = self
            .steps
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(self.base.max_steps);
        a.skills = self.skills.clone();
        a.skills_on_demand = self.on_demand.clone();
        a
    }

    /// Why this can't be saved yet.
    fn error(&self, cx: &App) -> Option<String> {
        if self.name.read(cx).value().trim().is_empty() {
            return Some("Give the agent a name".into());
        }
        match self.steps.read(cx).value().trim().parse::<u32>() {
            Ok(n) if (1..=500).contains(&n) => {}
            _ => return Some("Max steps must be a number between 1 and 500".into()),
        }
        if self.draft(cx).model.is_empty() {
            return Some("Pick a model".into());
        }
        None
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let d = self.draft(cx);
        let b = &self.base;
        d.name != b.name
            || d.description != b.description
            || d.role != b.role
            || d.model != b.model
            || d.tools != b.tools
            || d.max_steps != b.max_steps
            || d.skills != b.skills
            || d.skills_on_demand != b.skills_on_demand
            || self.steps.read(cx).value().trim() != b.max_steps.to_string()
    }
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

pub struct AgentsScreen {
    data: Option<AgentsData>,
    selected: Option<Id>,
    editor: Option<Editor>,
    confirm_delete: bool,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl AgentsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(
            &app,
            window,
            |this, _, ev: &AppEvent, window, cx| match ev {
                AppEvent::StoreChanged { changes, .. } => {
                    let relevant = changes.iter().any(|c| {
                        matches!(
                            c,
                            Change::Agent(_)
                                | Change::Task { .. }
                                | Change::Pipeline(_)
                                | Change::Config
                                | Change::Skills
                                | Change::External
                        )
                    });
                    if relevant {
                        this.reload(window, cx);
                    }
                }
                AppEvent::Navigated(Route::Agents { agent }) => {
                    this.selected = agent.clone();
                    this.sync_editor(window, cx);
                    cx.notify();
                }
                _ => {}
            },
        );
        let selected = match state::route(cx) {
            Route::Agents { agent } => agent,
            _ => None,
        };
        let mut this = AgentsScreen {
            data: None,
            selected,
            editor: None,
            confirm_delete: false,
            load: None,
            _subs: vec![sub],
        };
        this.reload(window, cx);
        this
    }

    /// Re-queries on a background thread, then applies and syncs the editor.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = state::store(cx);
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move { AgentsData::query(&store, &engine) });
        self.load = Some(cx.spawn_in(window, async move |this, cx| {
            let data = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.data = Some(data);
                this.sync_editor(window, cx);
                cx.notify();
            });
        }));
    }

    /// The agent shown: the selected one, else the first.
    fn current<'a>(&self, data: &'a AgentsData) -> Option<&'a AgentDef> {
        self.selected
            .as_ref()
            .and_then(|id| data.agents.iter().find(|a| &a.id == id))
            .or_else(|| data.agents.first())
    }

    /// Makes the editor match the current agent. Unsaved edits survive reloads that don't
    /// change the saved agent; switching agents discards them.
    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.data.take() else { return };
        let agent = self.current(&data).cloned();
        match agent {
            None => {
                self.editor = None;
            }
            Some(a) => {
                let rebuild = match &self.editor {
                    None => true,
                    Some(ed) if ed.base.id != a.id => true,
                    Some(ed) => {
                        (ed.base.version != a.version || ed.base.updated_at != a.updated_at)
                            && !ed.is_dirty(cx)
                    }
                };
                if rebuild {
                    self.confirm_delete = false;
                    self.editor = Some(Editor::new(&a, &data, window, cx));
                }
            }
        }
        self.data = Some(data);
    }

    fn select(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        state::replace_route(Route::Agents { agent: Some(id) }, cx);
        self.sync_editor(window, cx);
        cx.notify();
    }

    /// Creates a blank agent with the default model and selects it.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = now();
        let agent = AgentDef {
            id: new_id(),
            name: "New agent".into(),
            description: String::new(),
            role: String::new(),
            model: state::engine(cx).config().default_model,
            tools: ToolAccess::default(),
            skills: vec![],
            skills_on_demand: vec![],
            max_steps: default_max_steps(),
            version: 1,
            created_at: t,
            updated_at: t,
        };
        let store = state::store(cx);
        match store.put_agent(&agent) {
            Ok(()) => {
                store.log(
                    "agent.created",
                    None,
                    None,
                    format!("Created agent {}", agent.name),
                );
                self.select(agent.id.clone(), window, cx);
                self.reload(window, cx);
            }
            Err(e) => state::toast(
                ToastKind::Danger,
                format!("Couldn't create the agent: {e:#}"),
                cx,
            ),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &mut self.editor else { return };
        if let Some(err) = ed.error(cx) {
            state::toast(ToastKind::Warning, err, cx);
            return;
        }
        if !ed.is_dirty(cx) {
            return;
        }
        let mut agent = ed.draft(cx);
        agent.version = ed.base.version + 1;
        agent.updated_at = now();
        let store = state::store(cx);
        match store.put_agent(&agent) {
            Ok(()) => {
                store.log(
                    "agent.saved",
                    None,
                    None,
                    format!("Saved {} v{}", agent.name, agent.version),
                );
                state::toast(
                    ToastKind::Success,
                    format!("Saved {} v{}", agent.name, agent.version),
                    cx,
                );
                ed.base = agent;
                cx.notify();
            }
            Err(e) => state::toast(ToastKind::Danger, format!("Couldn't save: {e:#}"), cx),
        }
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        self.sync_editor(window, cx);
        cx.notify();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else { return };
        let (id, name) = (ed.base.id.clone(), ed.base.name.clone());
        let store = state::store(cx);
        match store.delete_agent(&id) {
            Ok(()) => {
                store.log("agent.deleted", None, None, format!("Deleted agent {name}"));
                state::toast(ToastKind::Info, format!("Deleted {name}"), cx);
                self.selected = None;
                self.editor = None;
                self.confirm_delete = false;
                state::replace_route(Route::Agents { agent: None }, cx);
                self.reload(window, cx);
            }
            Err(e) => state::toast(ToastKind::Danger, format!("Couldn't delete: {e:#}"), cx),
        }
    }

    /// Opens a new Agent-mode chat bound to the saved agent.
    fn chat_with(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else { return };
        let agent = ed.base.clone();
        let job = state::run_engine(cx, move |e| {
            let mut chat = e.new_chat(ChatMode::Agent)?;
            chat.agent_id = Some(agent.id.clone());
            chat.title = agent.name.clone();
            if !agent.model.is_empty() {
                chat.model = agent.model.clone();
            }
            e.store().put_chat(&chat)?;
            Ok(chat.id)
        });
        cx.spawn(async move |_, cx| {
            if let Some(id) = job.await {
                let _ = cx.update(|cx| state::navigate(Route::Chat { chat: Some(id) }, cx));
            }
        })
        .detach();
    }

    fn toggle_skill(&mut self, name: &str, on_demand: bool, cx: &mut Context<Self>) {
        let Some(ed) = &mut self.editor else { return };
        let (this, other) = if on_demand {
            (&mut ed.on_demand, &mut ed.skills)
        } else {
            (&mut ed.skills, &mut ed.on_demand)
        };
        if let Some(ix) = this.iter().position(|s| s == name) {
            this.remove(ix);
        } else {
            this.push(name.to_string());
            other.retain(|s| s != name);
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn labeled(text: &'static str, hint: Option<&'static str>, el: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::S2)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(theme::S2)
                .child(div().text_body_strong().fg_secondary().child(text))
                .when_some(hint, |d, h| d.child(div().text_label().fg_muted().child(h))),
        )
        .child(el)
}

/// A toggleable skill chip with the skill's description as tooltip.
fn skill_chip(
    id: SharedString,
    name: &str,
    description: Option<String>,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tip = description
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "Not found in the skill folders".into());
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(theme::S1)
        .h(px(26.))
        .px(px(10.))
        .rounded(theme::RADIUS_FULL)
        .border_1()
        .text_label()
        .cursor_pointer()
        .when(on, |d| {
            d.bg(theme::ACCENT_SOFT)
                .border_color(theme::ACCENT_RING)
                .text_color(theme::ACCENT_TEXT)
        })
        .when(!on, |d| {
            d.border_color(theme::LINE_STRONG)
                .fg_secondary()
                .hover(|s| s.bg(theme::SURFACE_RAISED).text_color(theme::TEXT))
        })
        .when(on, |d| d.child(icon_sized(AppIcon::Check, px(12.))))
        .child(name.to_string())
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}

fn stat_tile(label: &'static str, value: String, sub: Option<String>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(theme::S3)
        .child(div().text_label().fg_muted().child(label))
        .child(div().text_stat_sm().fg().ellipsis().child(value))
        .when_some(sub, |d, s| {
            d.child(div().text_label().fg_muted().ellipsis().child(s))
        })
}

impl Render for AgentsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The stats column needs a wide window; below this the form gets the room.
        let show_aside = window.viewport_size().width >= px(1360.);
        let Some(data) = &self.data else {
            return screen()
                .child(PageHeader::new("Agents"))
                .child(body().child(skeleton_rows("agents-loading", 3)));
        };
        if data.agents.is_empty() {
            return screen().child(PageHeader::new("Agents")).child(
                body().child(
                    EmptyState::new(
                        AppIcon::Bot,
                        "No saved agents",
                        "Save an agent to reuse its role, model, tools and skills across pipelines and chats.",
                    )
                    .action(Button::new("agents-empty-new", "New agent").icon(AppIcon::Plus).primary().on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))),
                ),
            );
        }

        let current = self.current(data).map(|a| a.id.clone());
        let list = self.render_list(data, current.as_ref(), cx);
        let mut header = PageHeader::new("Agents");
        let mut main = div().flex_1().min_w_0().h_full();
        if let Some(ed) = &self.editor {
            let dirty = ed.is_dirty(cx);
            let error = ed.error(cx);
            header = header
                .subtitle(ed.base.name.clone())
                .meta(Badge::new(format!("v{}", ed.base.version)).mono())
                .when(dirty, |h| {
                    h.meta(Badge::new("Unsaved changes").accent().dot())
                })
                .action(
                    Button::new("agent-discard", "Discard changes")
                        .ghost()
                        .disabled(!dirty)
                        .on_click(cx.listener(|this, _, window, cx| this.discard(window, cx))),
                )
                .action(
                    Button::new("agent-chat", "Chat with this agent")
                        .icon(AppIcon::Chat)
                        .tooltip("Starts a chat with the saved version of this agent")
                        .on_click(cx.listener(|this, _, _, cx| this.chat_with(cx))),
                )
                .action(
                    Button::new("agent-save", "Save")
                        .primary()
                        .icon(AppIcon::Check)
                        .disabled(!dirty || error.is_some())
                        .when_some(error.clone(), |b, e| b.tooltip(e))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                );
            let stats = data.stats.get(&ed.base.id).cloned().unwrap_or_default();
            let used = data.used_in.get(&ed.base.id).cloned().unwrap_or_default();
            main = main
                .flex()
                .child(self.render_editor(ed, data, error, cx))
                .when(show_aside, |m| m.child(self.render_aside(&stats, &used, cx)));
        }
        screen()
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(list).child(main))
    }
}

impl AgentsScreen {
    /// Left column: saved agents with usage stats, "New agent" on top.
    fn render_list(
        &self,
        data: &AgentsData,
        current: Option<&Id>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .w(px(288.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme::S2)
                    .p(theme::S3)
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .child(format!("Saved agents · {}", data.agents.len())),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("agents-new", "New agent")
                            .small()
                            .icon(AppIcon::Plus)
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("agents-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(theme::S2)
                    .pb(theme::S3)
                    .children(data.agents.iter().enumerate().map(|(ix, a)| {
                        let id = a.id.clone();
                        let active = current == Some(&a.id);
                        let stats = data.stats.get(&a.id);
                        let usage = match stats {
                            Some(s) if s.tasks > 0 => format!(
                                "{} task{} · {}{}",
                                s.tasks,
                                if s.tasks == 1 { "" } else { "s" },
                                ui::fmt_usd(s.cost),
                                s.last_used
                                    .map(|t| format!(" · {} ago", ui::fmt_ago(t)))
                                    .unwrap_or_default()
                            ),
                            _ => "Not used yet".to_string(),
                        };
                        div()
                            .id(SharedString::from(format!("agent-row-{}", a.id)))
                            .flex()
                            .items_start()
                            .gap(theme::S3)
                            .p(theme::S3)
                            .rounded(theme::RADIUS_LG)
                            .border_1()
                            .cursor_pointer()
                            .when(active, |d| {
                                d.bg(theme::SURFACE_RAISED).border_color(theme::ACCENT_RING)
                            })
                            .when(!active, |d| {
                                d.border_color(theme::TRANSPARENT)
                                    .hover(|s| s.bg(theme::SURFACE))
                            })
                            .child(VariantChip::initial(&a.name).selected(active))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div().text_body_strong().ellipsis().child(a.name.clone()),
                                    )
                                    .child(div().text_mono_sm().fg_muted().ellipsis().child(
                                        format!(
                                            "{} · {}",
                                            Config::short_model(&a.model),
                                            a.tools.label()
                                        ),
                                    ))
                                    .child(div().text_label().fg_muted().ellipsis().child(usage)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(id.clone(), window, cx)
                            }))
                            .with_animation(
                                SharedString::from(format!("agent-row-in-{}", a.id)),
                                Animation::new(theme::DUR_ENTER * (ix as u32 + 1).min(6))
                                    .with_easing(theme::ease_out),
                                |d, t| d.opacity(t),
                            )
                    })),
            )
    }

    /// Middle column: the editor form.
    fn render_editor(
        &self,
        ed: &Editor,
        data: &AgentsData,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let role = ed.role.read(cx).value();
        let tokens = role.chars().count().div_ceil(4);
        let skill_row = |kind: &'static str, on_demand: bool, cx: &mut Context<Self>| {
            let selected = if on_demand { &ed.on_demand } else { &ed.skills };
            let mut names: Vec<(String, Option<String>)> = data
                .skills
                .iter()
                .map(|s| (s.name.clone(), Some(s.description.clone())))
                .collect();
            for s in ed.skills.iter().chain(&ed.on_demand) {
                if !names.iter().any(|(n, _)| n == s) {
                    names.push((s.clone(), None));
                }
            }
            div()
                .flex()
                .flex_wrap()
                .gap(theme::S2)
                .children(names.into_iter().map(|(name, desc)| {
                    let on = selected.contains(&name);
                    let n = name.clone();
                    skill_chip(
                        SharedString::from(format!("skill-{kind}-{name}")),
                        &name,
                        desc,
                        on,
                        cx.listener(move |this, _, _, cx| this.toggle_skill(&n, on_demand, cx)),
                    )
                }))
        };
        let no_skills = data.skills.is_empty() && ed.skills.is_empty() && ed.on_demand.is_empty();
        let b = &ed.base;
        // The form keeps a readable width in a wide window (design: ~760px column); the
        // scroll area still takes the whole middle.
        let form = div()
            .flex()
            .flex_col()
            .gap(theme::S5)
            .w(px(820.))
            .max_w_full()
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .child(div().w(px(280.)).flex_none().child(labeled(
                        "Name",
                        None,
                        input::field(&ed.name),
                    )))
                    .child(div().flex_1().min_w_0().child(labeled(
                        "Description",
                        Some("one line: what it's for"),
                        input::field(&ed.description),
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(div().text_body_strong().fg_secondary().child("Role prompt"))
                            .child(Badge::new("prompt layer 2").accent())
                            .child(
                                div()
                                    .text_label()
                                    .fg_muted()
                                    .child("goes to every task of this agent"),
                            )
                            .child(div().flex_1())
                            .child(div().text_label().fg_muted().child(format!(
                                "~{tokens} tokens · {} chars",
                                role.chars().count()
                            ))),
                    )
                    // A role is a paragraph or a list of rules: show ~8 lines, grow
                    // with the text (an empty multi-line input collapses to one line).
                    .child(input::composer_field(&ed.role).min_h(px(8. * 20. + 20.))),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .child(div().flex_1().min_w_0().child(labeled(
                        "Model",
                        None,
                        sel::select(&ed.model),
                    )))
                    .child(div().flex_1().min_w_0().child(labeled(
                        "Tool access",
                        None,
                        sel::select(&ed.tools),
                    )))
                    .child(div().w(px(120.)).flex_none().child(labeled(
                        "Max steps",
                        None,
                        input::field(&ed.steps),
                    ))),
            )
            .child(labeled(
                "Skills",
                Some("always loaded into the prompt"),
                if no_skills {
                    div()
                        .text_label()
                        .fg_muted()
                        .child("No skills found. Add skill folders in Settings, Execution.")
                        .into_any_element()
                } else {
                    skill_row("always", false, cx).into_any_element()
                },
            ))
            .when(!no_skills, |d| {
                d.child(labeled(
                    "Skills on demand",
                    Some("the agent loads these with load_skill when it needs them"),
                    skill_row("demand", true, cx),
                ))
            })
            .when_some(error, |d, e| {
                d.child(div().text_label().text_color(theme::DANGER).child(e))
            })
            .child(divider())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .child(div().text_label().fg_muted().child(format!(
                        "v{} · created {} ago · updated {} ago",
                        b.version,
                        ui::fmt_ago(b.created_at),
                        ui::fmt_ago(b.updated_at)
                    )))
                    .child(div().flex_1())
                    .child(if self.confirm_delete {
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(
                                div().text_label().text_color(theme::DANGER).child(
                                    "Delete this agent? Pipelines using it keep their steps.",
                                ),
                            )
                            .child(
                                Button::new("agent-delete-no", "Cancel")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_delete = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("agent-delete-yes", "Delete")
                                    .small()
                                    .danger()
                                    .icon(AppIcon::Trash)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.delete(window, cx)),
                                    ),
                            )
                            .into_any_element()
                    } else {
                        Button::new("agent-delete", "Delete agent")
                            .small()
                            .danger()
                            .icon(AppIcon::Trash)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_delete = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
            );
        div()
            .id("agent-editor")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p(theme::S6)
            .child(form)
    }

    /// Right column: stats, where the agent is used, recent tasks.
    fn render_aside(
        &self,
        stats: &AgentStats,
        used: &[(Id, String)],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dash = || "—".to_string();
        let tiles = div()
            .flex()
            .flex_col()
            .rounded(theme::RADIUS_XL)
            .border_1()
            .border_color(theme::LINE)
            .bg(theme::SURFACE)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .child(stat_tile(
                        "Runs",
                        stats.tasks.to_string(),
                        Some(format!("{} done · {} failed", stats.done, stats.failed)),
                    ))
                    .child(vdivider())
                    .child(stat_tile(
                        "Success rate",
                        stats
                            .success_rate()
                            .map(|r| format!("{:.0}%", r * 100.0))
                            .unwrap_or_else(dash),
                        Some("of finished tasks".into()),
                    )),
            )
            .child(divider())
            .child(
                div()
                    .flex()
                    .child(stat_tile(
                        "Avg cost",
                        stats.avg_cost().map(ui::fmt_usd).unwrap_or_else(dash),
                        Some(format!("{} total", ui::fmt_usd(stats.cost))),
                    ))
                    .child(vdivider())
                    .child(stat_tile(
                        "Last used",
                        stats
                            .last_used
                            .map(|t| format!("{} ago", ui::fmt_ago(t)))
                            .unwrap_or_else(dash),
                        None,
                    )),
            );

        let used_in = card()
            .child(
                SectionHeader::new("Used in")
                    .icon(AppIcon::Workflow)
                    .meta(format!(
                        "{} pipeline{}",
                        used.len(),
                        if used.len() == 1 { "" } else { "s" }
                    )),
            )
            .when(used.is_empty(), |d| {
                d.child(
                    div()
                        .text_label()
                        .fg_muted()
                        .child("No pipeline has an agent step that uses this agent."),
                )
            })
            .children(used.iter().map(|(pid, name)| {
                let pid = pid.clone();
                div()
                    .id(SharedString::from(format!("used-in-{pid}")))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(32.))
                    .px(theme::S2)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_body()
                            .ellipsis()
                            .child(name.clone()),
                    )
                    .child(
                        icon_sized(AppIcon::ChevronRight, theme::ICON_SM)
                            .text_color(theme::TEXT_MUTED),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        state::navigate(
                            Route::Pipelines {
                                pipeline: Some(pid.clone()),
                            },
                            cx,
                        )
                    }))
            }));

        let recent = card()
            .child(SectionHeader::new("Recent tasks").icon(AppIcon::Activity))
            .when(stats.recent.is_empty(), |d| {
                d.child(
                    div()
                        .text_label()
                        .fg_muted()
                        .child("This agent hasn't worked on anything yet."),
                )
            })
            .children(stats.recent.iter().enumerate().map(|(ix, t)| {
                let id = t.id.clone();
                div()
                    .id(SharedString::from(format!("recent-{}", t.id)))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .min_h(px(36.))
                    .px(theme::S2)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .child(StatusDot::new(t.status).id(("recent-dot", ix)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_body().ellipsis().child(t.title.clone()))
                            .child(div().text_label().fg_muted().child(format!(
                                "{} · {} ago",
                                t.status.label(),
                                ui::fmt_ago(t.when)
                            ))),
                    )
                    .when(t.cost > 0.0, |d| {
                        d.child(div().text_mono_sm().fg_muted().child(ui::fmt_usd(t.cost)))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| state::open_task(id.clone(), cx)))
            }));

        div()
            .id("agent-aside")
            .w(px(320.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .p(theme::S4)
            .border_l_1()
            .border_color(theme::LINE)
            .child(tiles)
            .child(used_in)
            .child(recent)
    }
}
