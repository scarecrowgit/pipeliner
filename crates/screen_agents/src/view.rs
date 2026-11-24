//! The agents view: state, selection, keyboard, and the layout of the tab bar, the list and the
//! detail panel. The parts are drawn by [`crate::list`], [`crate::detail`] and
//! [`crate::pipeline`]; menus and dialogs live in [`crate::menus`] and [`crate::dialogs`].

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, Render, ScrollHandle,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use hub::api::{GraphChange, LoadProblem, PipelineId, Runnable, RunnableKind, Template, UsedBy};
use hub::model::RunnableRef;
use hub::{CatalogModel, Route};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::markdown::Markdown;
use ui_rich::{TextInput, TextInputEvent};

use crate::dialogs::Form;
use crate::rows::{self, Tab};

/// Probe and anchor of the header's "New" button.
pub(crate) const NEW: &str = "agents-new";
/// Probe of the search field.
pub(crate) const SEARCH: &str = "agents-search";
/// Width of the detail panel, px (`.panel` is 420 in the mockup).
pub(crate) const PANEL_W: f32 = 420.;

/// The whole content of `Route::Agents`.
pub struct AgentsView {
    pub(crate) selected: Option<RunnableRef>,
    pub(crate) catalog: Entity<CatalogModel>,
    pub(crate) tab: Tab,
    pub(crate) search: Entity<TextInput>,
    /// The list's own focus: arrows, Enter and the letter shortcuts work while it has it.
    pub(crate) focus: FocusHandle,
    /// Focus was taken on the first frame the view was drawn.
    pub(crate) shown: bool,
    pub(crate) scroll: ScrollHandle,
    /// The detail panel's scroll position ("Run history" scrolls to the recent work).
    pub(crate) panel_scroll: ScrollHandle,
    /// How many sections the panel drew last frame.
    pub(crate) panel_sections: usize,
    /// What would break if a runnable were deleted, read when it is selected.
    pub(crate) used_by: HashMap<RunnableRef, UsedBy>,
    /// A pipeline's draft against its live version, read when it is selected.
    pub(crate) changes: HashMap<PipelineId, Vec<GraphChange>>,
    pub(crate) templates: Vec<Template>,
    /// The selected agent's role, rendered.
    pub(crate) role: Option<(RunnableRef, Entity<Markdown>)>,
    /// The open dialog's inputs.
    pub(crate) form: Option<Form>,
    pub(crate) form_subs: Vec<Subscription>,
    /// Load problems to show instead of the catalog's (tests: the fixture has none).
    #[cfg(test)]
    pub(crate) problems_override: Option<Vec<LoadProblem>>,
    _subs: Vec<Subscription>,
}

impl AgentsView {
    /// The library, with `selected` open when given.
    pub fn new(selected: Option<RunnableRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let catalog = hub::catalog(cx);
        let router = hub::router(cx);
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Filter by name, model or runtime"));
        let subs = vec![
            cx.observe(&catalog, |this, _, cx| this.sync(cx)),
            cx.observe(&router, |this, router, cx| {
                if let Route::Agents { runnable } = router.read(cx).current()
                    && *runnable != this.selected
                {
                    let runnable = runnable.clone();
                    this.apply(runnable, cx);
                }
            }),
            cx.subscribe_in(&search, window, Self::on_search),
        ];
        let templates = hub::queries(cx).templates(cx);
        cx.spawn(async move |this, cx| {
            let templates = templates.await;
            this.update(cx, |this, cx| {
                this.templates = templates;
                cx.notify();
            })
            .ok();
        })
        .detach();
        let mut this = Self {
            selected: None,
            catalog,
            tab: Tab::All,
            search,
            focus: cx.focus_handle(),
            shown: false,
            scroll: ScrollHandle::new(),
            panel_scroll: ScrollHandle::new(),
            panel_sections: 0,
            used_by: HashMap::new(),
            changes: HashMap::new(),
            templates: Vec::new(),
            role: None,
            form: None,
            form_subs: Vec::new(),
            #[cfg(test)]
            problems_override: None,
            _subs: subs,
        };
        this.apply(selected, cx);
        this.sync(cx);
        this
    }

    /// Follows the route: selects `runnable`, or nothing.
    pub fn show(&mut self, runnable: Option<RunnableRef>, _window: &mut Window, cx: &mut Context<Self>) {
        if runnable != self.selected {
            self.apply(runnable, cx);
        }
    }

    /// What is selected.
    pub fn selected(&self) -> Option<&RunnableRef> {
        self.selected.as_ref()
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &App) -> SharedString {
        "Agents & pipelines".into()
    }

    /// What follows the title in the shell's header: the one-line explanation.
    pub fn header_badges(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            div()
                .flex_shrink()
                .min_w_0()
                .ml(px(SPACE_3))
                .type_style(BODY)
                .text_color(TEXT_MUTED)
                .child(truncate("One list. Anything here can take an issue, answer in chat or sit inside a pipeline."))
                .into_any_element(),
        ]
    }

    /// The controls on the right of the shell's header, left to right: "New".
    pub fn header_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let open = ui::overlays(window, cx).is_open(NEW, cx);
        let view = cx.entity();
        vec![
            Button::new(NEW, "New")
                .primary()
                .icon(IconName::Plus)
                .selected(open)
                .on_click(move |_, window, cx| view.update(cx, |this, cx| this.open_new_menu(window, cx)))
                .popover_anchor(NEW)
                .probe(NEW)
                .into_any_element(),
        ]
    }

    // ------------------------------------------------------------ data

    /// The selected runnable, when the catalog has it.
    pub(crate) fn current<'a>(&self, cx: &'a App) -> Option<&'a Runnable> {
        let selected = self.selected.as_ref()?;
        self.catalog.read(cx).runnable(selected)
    }

    /// Files of the library that could not be read.
    pub(crate) fn problems(&self, cx: &App) -> Vec<LoadProblem> {
        #[cfg(test)]
        if let Some(problems) = &self.problems_override {
            return problems.clone();
        }
        self.catalog.read(cx).problems().value().map(|p| p.as_ref().clone()).unwrap_or_default()
    }

    /// The list's rows in drawing order, after the tab and the search.
    pub(crate) fn order(&self, cx: &App) -> Vec<RunnableRef> {
        let query = self.search.read(cx).text().to_string();
        rows::order(&rows::grouped(self.catalog.read(cx).runnables(), self.tab, &query))
    }

    /// The catalog changed: read the pipeline graphs the list counts agents in, and what the
    /// selection needs.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let pipelines: Vec<PipelineId> = self
            .catalog
            .read(cx)
            .runnables()
            .iter()
            .filter(|r| r.kind == RunnableKind::Pipeline)
            .map(|r| PipelineId::new(r.reference.id()))
            .collect();
        self.catalog.update(cx, |catalog, cx| {
            for id in &pipelines {
                catalog.ensure_pipeline(id, cx);
            }
        });
        self.sync_role(cx);
        cx.notify();
    }

    /// Selects `runnable` without touching the route, and reads what its panel needs.
    fn apply(&mut self, runnable: Option<RunnableRef>, cx: &mut Context<Self>) {
        self.selected = runnable.clone();
        if let Some(reference) = runnable {
            self.load_used_by(&reference, cx);
            if reference.scheme() == "pipeline" {
                let id = PipelineId::new(reference.id());
                self.catalog.update(cx, |catalog, cx| {
                    catalog.ensure_pipeline(&id, cx);
                    catalog.ensure_versions(&id, cx);
                });
                self.load_changes(id, cx);
            }
            if let Some(ix) = self.order(cx).iter().position(|r| *r == reference) {
                self.scroll.scroll_to_item(self.row_child_index(ix, cx));
            }
        }
        self.sync_role(cx);
        cx.notify();
    }

    /// Selects `runnable` and records it in the route (a selection inside the screen).
    pub(crate) fn select(&mut self, runnable: Option<RunnableRef>, cx: &mut Context<Self>) {
        self.apply(runnable.clone(), cx);
        let router = hub::router(cx);
        if matches!(router.read(cx).current(), Route::Agents { .. }) {
            router.update(cx, |router, cx| router.replace(Route::Agents { runnable }, cx));
        }
    }

    pub(crate) fn load_used_by(&mut self, reference: &RunnableRef, cx: &mut Context<Self>) {
        let task = hub::queries(cx).used_by(reference, cx);
        let reference = reference.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(used) = task.await {
                this.update(cx, |this, cx| {
                    this.used_by.insert(reference, used);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn load_changes(&mut self, id: PipelineId, cx: &mut Context<Self>) {
        let task = hub::queries(cx).pipeline_changes(&id, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(changes) = task.await {
                this.update(cx, |this, cx| {
                    this.changes.insert(id, changes);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Keeps the rendered role in step with the selected agent's definition.
    fn sync_role(&mut self, cx: &mut Context<Self>) {
        let role = self.current(cx).and_then(|r| r.agent.as_ref().map(|a| (r.reference.clone(), a.role.clone())));
        match (role, &self.role) {
            (None, _) => self.role = None,
            (Some((reference, text)), Some((shown, md))) if *shown == reference => {
                if md.read(cx).text() != text {
                    md.update(cx, |md, cx| md.set_text(text, cx));
                }
            }
            (Some((reference, text)), _) => {
                let md = cx.new(|cx| Markdown::new(text, cx).color(TEXT_SECONDARY).on_link(|url, _, cx| cx.open_url(url)));
                self.role = Some((reference, md));
            }
        }
    }

    /// The scroll child index of list row `ix`: group headers sit between rows.
    fn row_child_index(&self, ix: usize, cx: &App) -> usize {
        let query = self.search.read(cx).text().to_string();
        let groups = rows::grouped(self.catalog.read(cx).runnables(), self.tab, &query);
        let mut seen = 0;
        // The notice of unreadable files sits above the first group.
        let mut child = usize::from(self.tab == Tab::All && !self.problems(cx).is_empty());
        for (_, list) in &groups {
            child += 1;
            if ix < seen + list.len() {
                return child + (ix - seen);
            }
            seen += list.len();
            child += list.len();
        }
        child
    }

    // ------------------------------------------------------------ actions

    /// Switches the tab; the selection stays when the new tab lists it.
    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Moves the selection `step` rows.
    pub(crate) fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.order(cx);
        if let Some(next) = rows::step(&order, self.selected.as_ref(), step) {
            self.select(Some(next), cx);
        }
    }

    /// Enter on a row: a pipeline opens in the editor, an agent opens its definition.
    pub(crate) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runnable) = self.current(cx).cloned() else { return };
        if runnable.kind == RunnableKind::Pipeline {
            open_editor(&runnable.reference, cx);
        } else {
            self.open_edit(&runnable.reference, window, cx);
        }
    }

    fn on_search(&mut self, _: &Entity<TextInput>, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Changed => {
                // Keep the selection when it is still listed, else take the first match.
                let order = self.order(cx);
                if self.selected.as_ref().is_none_or(|s| !order.contains(s))
                    && let Some(first) = order.first().cloned()
                {
                    self.select(Some(first), cx);
                }
                cx.notify();
            }
            TextInputEvent::Down => self.move_selection(1, cx),
            TextInputEvent::Up => self.move_selection(-1, cx),
            TextInputEvent::Submit => self.open_selected(window, cx),
            TextInputEvent::Cancel => {
                if self.search.read(cx).is_empty() {
                    window.focus(&self.focus);
                } else {
                    self.search.update(cx, |input, cx| input.set_text("", cx));
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Keys while the list has focus (not the search field or a dialog).
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let k = &event.keystroke;
        let primary = k.modifiers.platform || k.modifiers.control;
        let handled = match k.key.as_str() {
            "down" | "j" if !primary => {
                self.move_selection(1, cx);
                true
            }
            "up" | "k" if !primary => {
                self.move_selection(-1, cx);
                true
            }
            "enter" if !primary => {
                self.open_selected(window, cx);
                true
            }
            "f" | "/" if !primary => {
                self.search.read(cx).focus(window);
                true
            }
            "e" if !primary => {
                if let Some(reference) = self.selected.clone() {
                    self.open_edit(&reference, window, cx);
                }
                true
            }
            "d" if primary => {
                if let Some(reference) = self.selected.clone() {
                    self.duplicate(&reference, cx);
                }
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }
}

/// Opens a pipeline in the editor.
pub(crate) fn open_editor(reference: &RunnableRef, cx: &mut App) {
    hub::navigate(Route::Editor { pipeline: PipelineId::new(reference.id()), node: None }, cx);
}

impl Render for AgentsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The shell builds views before they are in a window: take focus on the first frame.
        if !self.shown {
            self.shown = true;
            window.focus(&self.focus);
        }
        let sections = self.render_panel(window, cx);
        self.panel_sections = sections.as_ref().map_or(0, Vec::len);
        let body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_list(window, cx))
            .when_some(sections, |d, sections| {
                d.child(
                    div()
                        .id("agents-panel")
                        .flex()
                        .flex_col()
                        .flex_none()
                        .w(px(PANEL_W))
                        .h_full()
                        .border_l_1()
                        .border_color(LINE)
                        .bg(BG)
                        .overflow_y_scroll()
                        .track_scroll(&self.panel_scroll)
                        .children(sections.into_iter().map(|s| div().flex_shrink_0().child(s))),
                )
            });
        div()
            .id("agents")
            .key_context("Agents")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| {
                // A press anywhere outside a field gives the list the keyboard.
                if !this.search.read(cx).is_focused(window) {
                    window.focus(&this.focus);
                }
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .child(self.render_tabs(window, cx))
            .child(body)
    }
}
