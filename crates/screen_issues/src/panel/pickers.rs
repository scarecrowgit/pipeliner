//! The small pickers of the panel: status, priority, owner and type menus that send an
//! `IssuePatch`, and the runner picker (`picker_popover()` of the mockups): a search field over
//! the orchestrators, agents and pipelines of the catalog, the current one checked.

use gpui::{AnyElement, App, Context, Entity, Hsla, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{IssuePatch, IssueStatus, IssueType, Origin, Priority, Runnable, RunnableKind, RunnableRef, RunnableState, ToolAccess};
use hub::CatalogModel;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::TextInputEvent;

use super::IssuePanel;

/// Anchor key, element id and probe of the status picker.
pub(super) const STATUS: &str = "issue-prop-status";
/// Anchor key, element id and probe of the priority picker.
pub(super) const PRIORITY: &str = "issue-prop-priority";
/// Anchor key, element id and probe of the owner picker.
pub(super) const OWNER: &str = "issue-prop-owner";
/// Anchor key, element id and probe of the type picker (the tag in the header).
pub(super) const TYPE: &str = "issue-prop-type";
/// Anchor key, element id and probe of the runner picker's trigger.
pub(super) const RUNNER: &str = "issue-prop-pipeline";

/// "Agent", "Orchestrator", "Pipeline".
pub(super) fn kind_label(kind: RunnableKind) -> &'static str {
    match kind {
        RunnableKind::Agent => "Agent",
        RunnableKind::Orchestrator => "Orchestrator",
        RunnableKind::Pipeline => "Pipeline",
    }
}

/// The icon of a runnable kind.
pub(super) fn kind_icon(kind: RunnableKind) -> IconName {
    match kind {
        RunnableKind::Agent => IconName::Agent,
        RunnableKind::Orchestrator => IconName::Orchestrator,
        RunnableKind::Pipeline => IconName::Pipelines,
    }
}

/// What a runnable is doing, in two or three words, and the colour that says it.
pub(super) fn state_label(state: &RunnableState) -> (SharedString, Hsla) {
    match state {
        RunnableState::Idle => ("idle".into(), TEXT_MUTED),
        RunnableState::Ready => ("ready".into(), SUCCESS),
        RunnableState::Running { .. } => ("running".into(), INFO_TEXT),
        RunnableState::Waiting { .. } => ("waiting".into(), WARNING),
        RunnableState::Queued { count } => (format!("{count} queued").into(), TEXT_MUTED),
        RunnableState::Paused => ("paused".into(), TEXT_MUTED),
        RunnableState::Draft { version } => (format!("draft v{version}").into(), TEXT_MUTED),
        RunnableState::Live { version } => (format!("v{version} live").into(), SUCCESS),
        RunnableState::Alive { .. } => ("alive".into(), INFO_TEXT),
        RunnableState::Ended { .. } => ("ended".into(), TEXT_MUTED),
    }
}

/// The tone of [`state_label`]'s words as a menu hint: running in info blue, ready and live in
/// green, waiting in amber, the rest muted.
pub(super) fn state_tone(state: &RunnableState) -> HintTone {
    match state {
        RunnableState::Ready | RunnableState::Live { .. } => HintTone::Success,
        RunnableState::Running { .. } | RunnableState::Alive { .. } => HintTone::Info,
        RunnableState::Waiting { .. } => HintTone::Warning,
        RunnableState::Idle
        | RunnableState::Queued { .. }
        | RunnableState::Paused
        | RunnableState::Draft { .. }
        | RunnableState::Ended { .. } => HintTone::Muted,
    }
}

/// The line under a runnable's name in the picker: a pipeline's path, an orchestrator's team,
/// an agent's model and runtime.
pub(super) fn subline(runnable: &Runnable, catalog: &CatalogModel) -> SharedString {
    let Some(agent) = runnable.agent.as_ref().filter(|_| runnable.kind != RunnableKind::Pipeline) else {
        return runnable.description.clone().into();
    };
    if runnable.kind == RunnableKind::Orchestrator && !agent.team.is_empty() {
        let names: Vec<String> = agent
            .team
            .iter()
            .map(|r| catalog.runnable(r).map_or_else(|| r.id().to_string(), |r| r.name.clone()))
            .collect();
        let helpers = if agent.spawn.enabled { " · creates helpers" } else { "" };
        return format!("Runs {}{helpers}", names.join(", ")).into();
    }
    let mut parts = vec![agent.model.short().to_string()];
    if !agent.runtime.is_empty() {
        parts.push(format!("runtime {}", agent.runtime));
    }
    if agent.spawn.enabled && agent.spawn.per_run > 0 {
        parts.push(format!("may spawn ≤{}", agent.spawn.per_run));
    }
    if agent.tools.access == ToolAccess::ReadOnly {
        parts.push("read-only".into());
    }
    parts.join(" · ").into()
}

/// Initials for an owner's avatar: `ME` for you, `AR` for Alex Rivera.
pub(super) fn initials(owner: &str) -> SharedString {
    if owner.is_empty() {
        return "–".into();
    }
    if owner == "You" {
        return "ME".into();
    }
    owner.split_whitespace().take(2).filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect::<String>().into()
}

/// Runnables an issue can run with, in the picker's groups and order.
pub(super) fn runner_choices<'a>(catalog: &'a CatalogModel, query: &str) -> Vec<&'a Runnable> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();
    for kind in [RunnableKind::Orchestrator, RunnableKind::Agent, RunnableKind::Pipeline] {
        out.extend(catalog.of_kind(kind).filter(|r| {
            r.uses.issues && !matches!(r.origin, Origin::Spawned { .. }) && (q.is_empty() || r.name.to_lowercase().contains(&q))
        }));
    }
    out
}

impl IssuePanel {
    /// Opens a menu of choices under the trigger `anchor`; choosing one sends `patch(choice)`.
    fn open_choice_menu<T: Copy + PartialEq + 'static>(
        &mut self,
        anchor: &'static str,
        choices: Vec<(T, SharedString)>,
        current: T,
        patch: fn(T) -> IssuePatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let at = Anchor::of(anchor, window, cx);
        ui::overlays(window, cx).open_menu(
            at,
            Placement::below(),
            move |_, _| {
                let mut menu = Menu::new(SharedString::from(format!("{anchor}-menu"))).width(220.);
                for (ix, (value, label)) in choices.iter().enumerate() {
                    let (v, value) = (view.clone(), *value);
                    menu = menu.item(
                        MenuItem::new(SharedString::from(format!("{anchor}-{ix}")), label.clone())
                            .check_column()
                            .checked(value == current)
                            .on_click(move |_, _, cx| v.update(cx, |this, cx| this.patch(patch(value), cx))),
                    );
                }
                menu
            },
            window,
            cx,
        );
    }

    pub(super) fn open_status(&mut self, current: IssueStatus, window: &mut Window, cx: &mut Context<Self>) {
        let choices = IssueStatus::ALL.iter().map(|s| (*s, SharedString::from(s.label()))).collect();
        self.open_choice_menu(STATUS, choices, current, |s| IssuePatch { status: Some(s), ..IssuePatch::default() }, window, cx);
    }

    pub(super) fn open_priority(&mut self, current: Priority, window: &mut Window, cx: &mut Context<Self>) {
        let choices = [Priority::High, Priority::Medium, Priority::Low].iter().map(|p| (*p, SharedString::from(p.label()))).collect();
        self.open_choice_menu(PRIORITY, choices, current, |p| IssuePatch { priority: Some(p), ..IssuePatch::default() }, window, cx);
    }

    pub(super) fn open_type(&mut self, current: IssueType, window: &mut Window, cx: &mut Context<Self>) {
        let choices = IssueType::ALL.iter().map(|k| (*k, SharedString::from(k.label()))).collect();
        self.open_choice_menu(TYPE, choices, current, |k| IssuePatch { kind: Some(k), ..IssuePatch::default() }, window, cx);
    }

    /// The owner menu: you, the people who own issues of the project, and nobody.
    pub(super) fn open_owner(&mut self, current: String, window: &mut Window, cx: &mut Context<Self>) {
        let mut owners: Vec<String> = self
            .issues
            .read(cx)
            .iter()
            .map(|issue| issue.owner.clone())
            .filter(|owner| !owner.is_empty() && owner != "You")
            .collect();
        owners.sort();
        owners.dedup();
        owners.insert(0, "You".into());
        owners.push(String::new());
        let view = cx.entity();
        let at = Anchor::of(OWNER, window, cx);
        ui::overlays(window, cx).open_menu(
            at,
            Placement::below(),
            move |_, _| {
                let mut menu = Menu::new("issue-owner-menu").width(220.);
                for (ix, owner) in owners.iter().enumerate() {
                    let label = if owner.is_empty() { "No owner".to_string() } else { owner.clone() };
                    let (v, owner) = (view.clone(), owner.clone());
                    menu = menu.item(
                        MenuItem::new(SharedString::from(format!("{OWNER}-{ix}")), label)
                            .check_column()
                            .checked(owner == current)
                            .tile(Avatar::new(initials(&owner)))
                            .on_click(move |_, _, cx| {
                                let patch = IssuePatch { owner: Some(owner.clone()), ..IssuePatch::default() };
                                v.update(cx, |this, cx| this.patch(patch, cx))
                            }),
                    );
                }
                menu
            },
            window,
            cx,
        );
    }

    /// Opens the runner picker with an empty search.
    pub(super) fn open_runner_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.picked = 0;
        let view = cx.entity();
        let at = Anchor::of(RUNNER, window, cx);
        ui::overlays(window, cx).open_popover(at, Placement::below(), move |window, cx| runner_popover(&view, window, cx), window, cx);
        // The host moves focus into the new layer; typing goes to the search field.
        self.search.read(cx).focus(window);
        cx.notify();
    }

    /// Attaches `runner` to the issue and closes the picker.
    pub(super) fn choose_runner(&mut self, runner: RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        ui::overlays(window, cx).close(RUNNER, window, cx);
        let attached = self.current(cx).and_then(|issue| issue.runner.clone());
        if attached.as_ref() != Some(&runner) {
            hub::commands(cx).attach_runner(&self.issue, Some(runner), cx).detach();
        }
    }

    pub(super) fn on_search_event(
        &mut self,
        search: &Entity<ui_rich::TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = search.read(cx).text().to_string();
        let count = runner_choices(self.catalog.read(cx), &query).len();
        match event {
            TextInputEvent::Changed => self.picked = 0,
            TextInputEvent::Up if count > 0 => self.picked = (self.picked + count - 1) % count,
            TextInputEvent::Down if count > 0 => self.picked = (self.picked + 1) % count,
            TextInputEvent::Submit => {
                let chosen = runner_choices(self.catalog.read(cx), &query).get(self.picked).map(|r| r.reference.clone());
                if let Some(runner) = chosen {
                    self.choose_runner(runner, window, cx);
                }
            }
            TextInputEvent::Cancel => ui::overlays(window, cx).close(RUNNER, window, cx),
            _ => {}
        }
        cx.notify();
    }
}

/// The picker's content, rebuilt every frame while it is open.
fn runner_popover(view: &Entity<IssuePanel>, window: &mut Window, cx: &mut App) -> AnyElement {
    let this = view.read(cx);
    let search = this.search.clone();
    let query = search.read(cx).text().to_string();
    let focused = search.read(cx).is_focused(window);
    let attached = this.current(cx).and_then(|issue| issue.runner.clone());
    let picked = this.picked;
    let width = this.content_width();
    let catalog = this.catalog.read(cx);
    let choices = runner_choices(catalog, &query);
    let field = InputFrame::new("issue-runner-search").small().sunken().icon(IconName::Search).focused(focused).child(search);
    let mut pop = Popover::new().width(width).search(field);
    let mut list = col().id("issue-runner-list").flex_none().max_h(px(420.)).overflow_y_scroll();
    let mut last_kind = None;
    for (n, runnable) in choices.iter().enumerate() {
        if last_kind != Some(runnable.kind) {
            last_kind = Some(runnable.kind);
            let group = match runnable.kind {
                RunnableKind::Orchestrator => "Orchestrators",
                RunnableKind::Agent => "Agents",
                RunnableKind::Pipeline => "Pipelines",
            };
            list = list.child(div().flex_none().child(PopLabel::new(group)));
        }
        let hint = catalog.activity(&runnable.reference).map(|a| (state_label(&a.state).0, state_tone(&a.state)));
        let (v, reference) = (view.clone(), runnable.reference.clone());
        let name = SharedString::from(format!("issue-runner-{n}"));
        // Rows of a scrolling column shrink unless they are told not to.
        list = list.child(div().flex_none().child(
            MenuItem::new(name.clone(), runnable.name.clone())
                .tile(Tile::new(kind_icon(runnable.kind)).medium())
                .subline(subline(runnable, catalog))
                .when_some(hint, |item, (hint, tone)| item.hint(hint).hint_tone(tone))
                .check_column()
                .checked(attached.as_ref() == Some(&runnable.reference))
                .selected(n == picked)
                .on_click(move |_, window, cx| {
                    let r = reference.clone();
                    v.update(cx, |this, cx| this.choose_runner(r, window, cx))
                })
                .probe(name),
        ));
    }
    if choices.is_empty() {
        let text = if catalog.runnables().is_empty() { "Loading agents and pipelines…" } else { "Nothing matches" };
        list = list.child(div().px(px(SPACE_2_5)).py(px(SPACE_2)).type_style(LABEL).text_color(TEXT_MUTED).child(text));
    }
    pop = pop.child(list).footer(KeyHints::new().hint(["↑", "↓"], "move").hint(["↵"], "attach").hint(["Esc"], "close"));
    pop.into_any_element()
}
