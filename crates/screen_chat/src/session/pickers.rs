//! The composer's leading controls: the agent picker (`chat-agent-picker.html`: Build and
//! Plan, then the catalog's agents and orchestrators, external harnesses as "Coming soon"),
//! the model picker (providers and their models, read off the UI thread on first open), and
//! the Build ⇄ Plan switch (Tab in the composer, or the key hint under it).

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, KeyDownEvent, SharedString, Task, Window, div, prelude::*,
};
use hub::api::{ChatMode, ModelRef, Origin, ProviderModels, Runnable, RunnableKind, RunnableRef, Session, ToolAccess};
use hub::{CatalogModel, Route};
use ui::dev::Probe;
use ui::prelude::*;

use super::{SessionView, mode_label};

/// Anchor key and probe of the agent picker's trigger.
pub(super) const AGENT_ANCHOR: &str = "chat-agent-picker";
/// Anchor key and probe of the model picker's trigger.
pub(super) const MODEL_ANCHOR: &str = "chat-model-picker";

/// The built-in chat agent that Build and Plan run as.
pub(crate) fn build_ref() -> RunnableRef {
    RunnableRef::agent("build")
}

/// One row of the agent picker.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentChoice {
    /// Element id and probe: `chat-mode-build`, `chat-mode-plan`, `chat-agent-picker-<n>`.
    pub id: SharedString,
    pub label: SharedString,
    pub sub: SharedString,
    /// The model it runs on, short (`opus-5.5`).
    pub model: SharedString,
    pub icon: IconName,
    pub tone: TileTone,
    pub runnable: RunnableRef,
    pub mode: ChatMode,
    /// The one answering in this chat now.
    pub current: bool,
}

/// The models the picker offers, read on first open.
#[derive(Default)]
pub(super) enum ModelList {
    #[default]
    NotLoaded,
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Vec<ProviderModels>),
}

/// Agents and orchestrators a person can chat with, other than Build itself.
pub(crate) fn chat_runnables(catalog: &CatalogModel) -> impl Iterator<Item = &Runnable> {
    let build = build_ref();
    catalog.runnables().iter().filter(move |r| {
        matches!(r.kind, RunnableKind::Agent | RunnableKind::Orchestrator)
            && !r.archived
            && r.uses.chat
            && !matches!(r.origin, Origin::Spawned { .. })
            && r.reference != build
    })
}

fn model_of(runnable: Option<&Runnable>) -> SharedString {
    runnable.and_then(|r| r.agent.as_ref()).map(|a| SharedString::from(a.model.short().to_string())).unwrap_or_default()
}

/// The picker's rows for `session`, in order: Build, Plan, then the catalog's.
pub(crate) fn agent_choices(catalog: &CatalogModel, session: Option<&Session>) -> Vec<AgentChoice> {
    let build = build_ref();
    let build_model = model_of(catalog.runnable(&build));
    let on_build = session.is_some_and(|s| s.runnable == build);
    let mode = session.map_or(ChatMode::Build, |s| s.mode);
    let mut out = vec![
        AgentChoice {
            id: "chat-mode-build".into(),
            label: "Build".into(),
            sub: "Full access · bash asks first by policy".into(),
            model: build_model.clone(),
            icon: IconName::Agent,
            tone: TileTone::Neutral,
            runnable: build.clone(),
            mode: ChatMode::Build,
            current: on_build && mode == ChatMode::Build,
        },
        AgentChoice {
            id: "chat-mode-plan".into(),
            label: "Plan".into(),
            sub: "Read-only · proposes a plan, edits nothing".into(),
            model: build_model,
            icon: IconName::Policy,
            tone: TileTone::Neutral,
            runnable: build,
            mode: ChatMode::Plan,
            current: on_build && mode == ChatMode::Plan,
        },
    ];
    for (n, r) in chat_runnables(catalog).enumerate() {
        let orchestrator = r.kind == RunnableKind::Orchestrator;
        out.push(AgentChoice {
            id: format!("chat-agent-picker-{n}").into(),
            label: r.name.clone().into(),
            sub: r.description.clone().into(),
            model: model_of(Some(r)),
            icon: if orchestrator { IconName::Orchestrator } else { IconName::Agent },
            tone: TileTone::Neutral,
            runnable: r.reference.clone(),
            mode,
            current: session.is_some_and(|s| s.runnable == r.reference),
        });
    }
    out
}

/// The name the agent picker shows, and the composer's placeholder uses.
pub(super) fn current_label(this: &SessionView, cx: &App) -> SharedString {
    let Some(session) = this.header_session(cx) else { return "Build".into() };
    if session.runnable == build_ref() {
        return mode_label(session.mode).into();
    }
    let catalog = this.catalog.read(cx);
    catalog
        .runnable(&session.runnable)
        .map_or_else(|| SharedString::from(session.runnable.id().to_string()), |r| r.name.clone().into())
}

/// What the agent may do, for the line under the composer.
pub(super) fn mode_line(this: &SessionView, cx: &App) -> SharedString {
    let name = current_label(this, cx);
    let Some(session) = this.header_session(cx) else { return name };
    if session.mode == ChatMode::Plan {
        return format!("{name} · read-only · proposes a plan, edits nothing").into();
    }
    let access = this
        .catalog
        .read(cx)
        .runnable(&session.runnable)
        .and_then(|r| r.agent.as_ref())
        .map_or(ToolAccess::Full, |a| a.tools.access);
    let words = match access {
        ToolAccess::Full => "full access · bash asks first by policy",
        ToolAccess::ReadAndRun => "reads and runs commands · edits nothing",
        ToolAccess::ReadOnly => "read-only",
    };
    format!("{name} · {words}").into()
}

/// The agent picker's trigger.
pub(super) fn agent_trigger(this: &SessionView, window: &mut Window, cx: &mut Context<SessionView>) -> AnyElement {
    let open = ui::overlays(window, cx).is_open(AGENT_ANCHOR, cx);
    let view = cx.entity();
    SelectBox::new(AGENT_ANCHOR, current_label(this, cx))
        .icon(IconName::Agent)
        .small()
        .open(open)
        .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open_agent_picker(window, cx)))
        .popover_anchor(AGENT_ANCHOR)
        .probe(AGENT_ANCHOR)
        .into_any_element()
}

/// The model picker's trigger.
pub(super) fn model_trigger(this: &SessionView, window: &mut Window, cx: &mut Context<SessionView>) -> AnyElement {
    let open = ui::overlays(window, cx).is_open(MODEL_ANCHOR, cx);
    let label: SharedString =
        this.header_session(cx).map_or_else(|| "Model".into(), |s| s.model.short().to_string().into());
    let view = cx.entity();
    SelectBox::new(MODEL_ANCHOR, label)
        .small()
        .mono()
        .open(open)
        .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open_model_picker(window, cx)))
        .popover_anchor(MODEL_ANCHOR)
        .probe(MODEL_ANCHOR)
        .into_any_element()
}

impl SessionView {
    fn locked(&self, cx: &App) -> bool {
        self.composer.read(cx).disabled_reason().is_some()
    }

    fn open_agent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        let anchor = Anchor::of(AGENT_ANCHOR, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_popover(
            anchor,
            Placement::above(),
            move |_, cx| agent_popover(&view, cx),
            window,
            cx,
        );
    }

    fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if matches!(self.models, ModelList::NotLoaded) {
            let models = hub::queries(cx).models(cx);
            self.models = ModelList::Loading(cx.spawn(async move |this, cx| {
                let list = models.await;
                this.update(cx, |this, cx| {
                    this.models = ModelList::Ready(list);
                    cx.notify();
                })
                .ok();
            }));
        }
        let anchor = Anchor::of(MODEL_ANCHOR, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_popover(
            anchor,
            Placement::above(),
            move |_, cx| model_popover(&view, cx),
            window,
            cx,
        );
    }

    /// Switches who answers from the next message on.
    pub(super) fn choose_agent(&mut self, choice: &AgentChoice, cx: &mut Context<Self>) {
        hub::commands(cx).set_session_runnable(&self.session, &choice.runnable, choice.mode, cx);
    }

    /// Build ⇄ Plan: the same agent in the other mode.
    pub(super) fn toggle_mode(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        let Some(session) = self.header_session(cx) else { return };
        let mode = match session.mode {
            ChatMode::Build => ChatMode::Plan,
            ChatMode::Plan => ChatMode::Build,
        };
        let runnable = session.runnable.clone();
        hub::commands(cx).set_session_runnable(&self.session, &runnable, mode, cx);
    }
}

fn agent_popover(view: &Entity<SessionView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let choices = agent_choices(this.catalog.read(cx), this.header_session(cx));
    let mut menu = Menu::new("chat-agent-menu")
        .label_with(PopLabel::new("Answer with").trailing(div().child("same thread, same workdir")));
    for (n, choice) in choices.iter().enumerate() {
        let (v, c) = (view.clone(), choice.clone());
        let item = MenuItem::new(gpui::ElementId::Name(choice.id.clone()), choice.label.clone())
            .check_column()
            .checked(choice.current)
            .tile(Tile::new(choice.icon).medium().tone(choice.tone))
            .subline(choice.sub.clone())
            .hint(choice.model.clone())
            .when(n < 9, |item| item.kbd((n + 1).to_string()))
            .on_click(move |_, _, cx| v.update(cx, |this, cx| this.choose_agent(&c, cx)));
        menu = menu.item(item);
    }
    let menu = menu
        .separator()
        .item(
            MenuItem::new("chat-agent-picker-claude-code", "Claude Code")
                .check_column()
                .icon(IconName::Terminal)
                .subline("External harness")
                .coming_soon(),
        )
        .item(
            MenuItem::new("chat-agent-picker-codex-cli", "Codex CLI")
                .check_column()
                .icon(IconName::Terminal)
                .subline("External harness")
                .coming_soon(),
        )
        .separator()
        .item(
            MenuItem::new("chat-agent-picker-all", "All agents and pipelines…")
                .check_column()
                .icon(IconName::Orchestrator)
                .on_click(|_, _, cx| hub::navigate(Route::Agents { runnable: None }, cx)),
        );
    let v = view.clone();
    let by_digit = move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
        let Some(n) = event.keystroke.key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) else { return };
        if let Some(choice) = choices.get(n - 1) {
            v.update(cx, |this, cx| this.choose_agent(choice, cx));
            ui::overlays(window, cx).close(AGENT_ANCHOR, window, cx);
            cx.stop_propagation();
        }
    };
    div()
        .on_key_down(by_digit)
        .child(
            Popover::new()
                .width(420.)
                .child(menu)
                .footer(KeyHints::new().hint(["Tab"], "Build ⇄ Plan").hint(["↵"], "switch").hint(["Esc"], "close")),
        )
        .into_any_element()
}

fn model_popover(view: &Entity<SessionView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let current = this.header_session(cx).map(|s| s.model.clone());
    let mut menu = Menu::new("chat-model-menu").max_height(360.);
    match &this.models {
        ModelList::Ready(list) if list.iter().any(|p| !p.models.is_empty()) => {
            let mut n: usize = 0;
            for provider in list {
                let label = PopLabel::new(provider.provider.id.clone())
                    .when(provider.error.is_some(), |l| l.trailing(div().child("configured list")));
                menu = menu.label_with(label);
                for name in &provider.models {
                    let model = ModelRef::new(provider.provider.id.clone(), name.clone());
                    let (v, m) = (view.clone(), model.clone());
                    menu = menu.item(
                        MenuItem::new(("chat-model-picker", n), name.clone())
                            .check_column()
                            .checked(current.as_ref() == Some(&model))
                            .on_click(move |_, _, cx| {
                                v.update(cx, |this, cx| hub::commands(cx).set_session_model(&this.session, &m, cx))
                            }),
                    );
                    n += 1;
                }
            }
        }
        ModelList::Ready(_) => {
            menu = menu.item(MenuItem::new("chat-model-none", "No models configured").disabled(true));
        }
        _ => {
            menu = menu.item(MenuItem::new("chat-model-loading", "Loading models…").disabled(true));
        }
    }
    Popover::new().width(280.).child(menu).into_any_element()
}
