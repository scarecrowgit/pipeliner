//! Agents & pipelines screen: one list of the pipelines, their model steps and sub-agents,
//! the chat agents and the tasks sub-agents worked on (filterable card grid) with a detail
//! panel for the selected entry. Everything shown is real: the pipeline service, agent/skill
//! files on disk and the chat screen's session list.
//!
//! Modules: [`model`] the data, [`entries`] the card list, [`fmt`] text formatting,
//! [`looks`] per-kind visuals, [`header`] title bar + "New" menu, [`grid`] tabs and cards,
//! [`detail`] the right-hand panel (pipelines and created tasks), [`detail_agents`] its
//! step / sub-agent / chat agent sections, [`skills`] the skills section, [`step_edit`]
//! model, workdir, max steps and skills editing.

mod detail;
mod detail_agents;
mod entries;
mod fmt;
mod grid;
mod header;
mod looks;
mod model;
mod skills;
mod step_edit;

use gpui::{
    Context, IntoElement, MouseButton, Render, SharedString, Subscription, Window, deferred, div,
    prelude::*,
};
use ui::nav::ChatSessionInfo;
use ui::theme::*;

use model::{Catalog, Entry, Filter, Snapshot};

/// Width of the details aside (`flex: 1 1 380px`; the grid takes nearly all spare room).
pub(crate) const DETAIL_WIDTH: f32 = 380.;

pub struct AgentsScreen {
    snap: Snapshot,
    catalog: Catalog,
    /// Chat sessions published by the chat screen (pinned and recent).
    chats: Vec<ChatSessionInfo>,
    list: Vec<Entry>,
    sel: SharedString,
    filter: Filter,
    new_open: bool,
    /// The skills list of the selected entry is expanded.
    skills_all: bool,
    /// Last measured width of the grid area, for the `auto-fill, minmax(250px, 1fr)` columns.
    grid_width: f32,
    /// Model, workdir, max steps and skills fields of the selected step or sub-agent.
    editor: step_edit::StepEditor,
    /// Keeps the selection in sync with the sidebar via `ui::nav` (`None` in standalone previews).
    _nav_subscription: Option<Subscription>,
}

impl AgentsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav_subscription = ui::nav::nav_entity(cx).map(|nav| {
            cx.observe(&nav, |this, nav, cx| {
                let (agent, chats) = {
                    let n = nav.read(cx);
                    (n.agent.clone(), n.chat_sessions.clone())
                };
                if chats != this.chats {
                    this.chats = chats;
                    this.rebuild();
                    cx.notify();
                }
                if let Some(name) = agent {
                    this.select_by_name(&name, cx);
                }
            })
        });
        let editor = step_edit::StepEditor::new(window, cx);
        let snap = Snapshot::read(editor.pipes.read(cx));
        let catalog = Catalog::load(&snap.root);
        let chats = ui::nav::nav_entity(cx)
            .map(|n| n.read(cx).chat_sessions.clone())
            .unwrap_or_default();
        let mut screen = Self {
            snap,
            catalog,
            chats,
            list: Vec::new(),
            sel: SharedString::default(),
            filter: Filter::All,
            new_open: false,
            skills_all: false,
            grid_width: 0.,
            editor,
            _nav_subscription: nav_subscription,
        };
        screen.rebuild();
        // Test hooks: `HARNESS_AGENTS_TAB=pipelines|steps|sub|chat|created` opens a tab,
        // `HARNESS_AGENTS_SELECT=<name or id>` selects an entry, `HARNESS_AGENTS_NEW=open`
        // opens the "New" menu.
        screen.new_open = std::env::var("HARNESS_AGENTS_NEW").is_ok_and(|v| v == "open");
        if let Some(f) = std::env::var("HARNESS_AGENTS_TAB")
            .ok()
            .and_then(|v| Filter::from_env(&v))
        {
            screen.set_filter(f, cx);
        }
        if let Ok(name) = std::env::var("HARNESS_AGENTS_SELECT") {
            screen.select_by_name(&name, cx);
        }
        if let Some(name) = ui::nav::selected_agent(cx) {
            screen.select_by_name(&name, cx);
        }
        screen.sync_editor(cx);
        screen
    }

    /// Re-read the pipeline service (called when it changes).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.snap = Snapshot::read(self.editor.pipes.read(cx));
        if self.snap.root != self.catalog.root {
            self.catalog = Catalog::load(&self.snap.root);
        }
        self.rebuild();
        self.sync_editor(cx);
        cx.notify();
    }

    fn rebuild(&mut self) {
        self.list = entries::entries(&self.snap, &self.catalog, &self.chats);
    }

    pub(crate) fn entry(&self, id: &str) -> Option<&Entry> {
        self.list.iter().find(|e| e.id == id)
    }

    /// The selected entry; falls back to the first one of the current tab.
    pub(crate) fn selected(&self) -> Option<&Entry> {
        self.entry(&self.sel)
            .filter(|e| self.filter.matches(e))
            .or_else(|| self.list.iter().find(|e| self.filter.matches(e)))
            .or_else(|| self.list.first())
    }

    // ── Handlers ────────────────────────────────────────────────────────

    /// User picked an entry here: select it and tell the sidebar.
    pub(crate) fn select(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(name) = self.entry(&id).map(|e| e.name.clone()) else {
            return;
        };
        if !self.entry(&id).is_some_and(|e| self.filter.matches(e)) {
            self.filter = Filter::All;
        }
        self.sel = id;
        self.skills_all = false;
        self.editor.close_menu();
        self.sync_editor(cx);
        cx.notify();
        // `select_agent` is a no-op when unchanged, and our observer ignores the echo
        // because the entry is already selected.
        ui::nav::select_agent(name, false, cx);
    }

    /// Selection coming from `ui::nav` (sidebar, home) or a test hook: matches the display
    /// name (exactly, then ignoring case) or the entry id. Never writes back to nav.
    fn select_by_name(&mut self, name: &str, cx: &mut Context<Self>) {
        // Names repeat (a "Build" step and the Build chat agent): keep a matching selection.
        if self
            .entry(&self.sel)
            .is_some_and(|e| e.name == name && self.filter.matches(e))
        {
            return;
        }
        let Some(entry) = self
            .list
            .iter()
            .find(|e| e.name == name)
            .or_else(|| {
                self.list
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(name) || e.id == name)
            })
        else {
            return;
        };
        let id = entry.id.clone();
        let hidden = !self.filter.matches(entry);
        if id == self.sel && !hidden {
            return;
        }
        self.sel = id;
        self.skills_all = false;
        self.editor.close_menu();
        if hidden {
            self.filter = Filter::All;
        }
        self.sync_editor(cx);
        cx.notify();
    }

    pub(crate) fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.editor.close_menu();
        cx.notify();
        // Keep the selection on the tab, and nav in step (so its echo does not reset the tab).
        if let Some((id, name)) = self.selected().map(|e| (e.id.clone(), e.name.clone())) {
            self.sel = id;
            ui::nav::select_agent(name, false, cx);
        }
        self.sync_editor(cx);
    }

    pub(crate) fn set_new_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.new_open = open;
        cx.notify();
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("agents-screen")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_grid(cx))
                    .child(self.render_detail(window, cx)),
            )
            // While the "New" menu is open, a transparent backdrop above the page closes it on any
            // outside press (the menu itself is deferred, so it paints and hit-tests above this).
            .when(self.new_open, |d| {
                d.child(deferred(
                    div()
                        .id("agents-new-backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.set_new_open(false, cx)),
                        ),
                ))
            })
            .children(self.render_wd_backdrop(cx))
    }
}
