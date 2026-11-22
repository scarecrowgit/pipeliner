//! Agents & pipelines screen: one list of orchestrators, agents and pipelines (filterable card
//! grid) with a detail panel for the selected entry.
//!
//! Modules: [`data`] mock data, [`looks`] per-kind visuals, [`header`] title bar + "New" menu,
//! [`grid`] tabs and cards, [`detail`] the right-hand panel, [`skills`] its skills section.

mod data;
mod detail;
mod grid;
mod header;
mod looks;
mod skills;

use gpui::{
    Context, IntoElement, MouseButton, Render, Subscription, Window, deferred, div, prelude::*,
};
use ui::theme::*;

use data::{Entry, Filter, SkillMode, Use};

/// Width of the details aside (`flex: 1 1 380px`; the grid takes nearly all spare room).
pub(crate) const DETAIL_WIDTH: f32 = 380.;

pub struct AgentsScreen {
    list: Vec<Entry>,
    sel: &'static str,
    filter: Filter,
    new_open: bool,
    add_open: bool,
    /// The "Add skill" listbox in the skills section.
    sk_add_open: bool,
    /// Last measured width of the grid area, for the `auto-fill, minmax(250px, 1fr)` columns.
    grid_width: f32,
    /// Keeps the selection in sync with the sidebar via `ui::nav` (`None` in standalone previews).
    _nav_subscription: Option<Subscription>,
}

impl AgentsScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav_subscription = ui::nav::nav_entity(cx).map(|nav| {
            cx.observe(&nav, |this, nav, cx| {
                if let Some(name) = nav.read(cx).agent.clone() {
                    this.select_by_name(&name, cx);
                }
            })
        });
        let mut screen = Self {
            list: data::seed(),
            sel: "payments-lead",
            filter: Filter::All,
            new_open: false,
            add_open: false,
            sk_add_open: false,
            grid_width: 0.,
            _nav_subscription: nav_subscription,
        };
        if let Some(name) = ui::nav::selected_agent(cx) {
            screen.select_by_name(&name, cx);
        }
        screen
    }

    pub(crate) fn entry(&self, id: &str) -> Option<&Entry> {
        self.list.iter().find(|e| e.id == id)
    }

    pub(crate) fn name_of(&self, id: &str) -> &'static str {
        self.entry(id).map(|e| e.name).unwrap_or("")
    }

    /// The selected entry (falls back to the first one, like the mockup).
    pub(crate) fn selected(&self) -> &Entry {
        self.entry(self.sel).unwrap_or(&self.list[0])
    }

    /// "Orchestrator" / "Agent · created by Payments lead" ...
    pub(crate) fn kind_line(&self, e: &Entry) -> String {
        match e.created_by {
            Some(by) => format!("Agent · created by {}", self.name_of(by)),
            None => e.kind.label().to_string(),
        }
    }

    fn edit(&mut self, id: &'static str, f: impl FnOnce(&mut Entry), cx: &mut Context<Self>) {
        if let Some(e) = self.list.iter_mut().find(|e| e.id == id) {
            f(e);
            cx.notify();
        }
    }

    // ── Handlers ────────────────────────────────────────────────────────

    /// User picked an entry here: select it and tell the sidebar.
    pub(crate) fn select(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.sel = id;
        self.add_open = false;
        self.sk_add_open = false;
        cx.notify();
        let name = self.name_of(id);
        // `select_agent` is a no-op when unchanged, and our observer ignores the echo
        // because the entry is already selected.
        ui::nav::select_agent(name, false, cx);
    }

    /// Selection coming from `ui::nav` (sidebar). Never writes back to nav.
    fn select_by_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(entry) = self.list.iter().find(|e| e.name == name) else {
            return;
        };
        let id = entry.id;
        let hidden = !self.filter.matches(entry);
        if id == self.sel && !hidden {
            return;
        }
        self.sel = id;
        self.add_open = false;
        self.sk_add_open = false;
        if hidden {
            self.filter = Filter::All;
        }
        cx.notify();
    }

    pub(crate) fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    pub(crate) fn set_new_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.new_open = open;
        cx.notify();
    }

    pub(crate) fn toggle_add(&mut self, cx: &mut Context<Self>) {
        self.add_open = !self.add_open;
        cx.notify();
    }

    pub(crate) fn toggle_use(&mut self, u: Use, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.uses[u.index()] = !e.uses[u.index()], cx);
    }

    pub(crate) fn remove_member(&mut self, member: &'static str, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.team.retain(|m| *m != member), cx);
    }

    pub(crate) fn add_member(&mut self, member: &'static str, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.team.push(member), cx);
        self.add_open = false;
    }

    pub(crate) fn toggle_create(&mut self, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.can_create = !e.can_create, cx);
    }

    pub(crate) fn set_max_alive(&mut self, v: i32, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.max_alive = v, cx);
    }

    pub(crate) fn set_depth(&mut self, v: i32, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.depth = v, cx);
    }

    pub(crate) fn toggle_skill_add(&mut self, cx: &mut Context<Self>) {
        self.sk_add_open = !self.sk_add_open;
        cx.notify();
    }

    /// "Force load" / "Forced" toggle of one of the selected entry's own skills.
    pub(crate) fn toggle_force(&mut self, skill: &'static str, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(
            id,
            |e| {
                if let Some((_, mode)) = e.skills.iter_mut().find(|(k, _)| *k == skill) {
                    *mode = match mode {
                        SkillMode::Forced => SkillMode::Auto,
                        SkillMode::Auto => SkillMode::Forced,
                    };
                }
            },
            cx,
        );
    }

    pub(crate) fn remove_skill(&mut self, skill: &'static str, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(id, |e| e.skills.retain(|(k, _)| *k != skill), cx);
    }

    /// Picked in the "Add skill" list: added as an auto skill, list closes.
    pub(crate) fn add_skill(&mut self, skill: &'static str, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(
            id,
            |e| {
                if !e.skills.iter().any(|(k, _)| *k == skill) {
                    e.skills.push((skill, SkillMode::Auto));
                }
            },
            cx,
        );
        self.sk_add_open = false;
    }

    /// "Keep as a permanent agent": the created agent becomes a regular one.
    pub(crate) fn promote(&mut self, cx: &mut Context<Self>) {
        let id = self.selected().id;
        self.edit(
            id,
            |e| {
                e.created_by = None;
                e.status_text = "idle · kept".into();
                e.state = data::RunState::Idle;
                e.uses = [true, true, true, false];
            },
            cx,
        );
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_grid(cx))
            .child(self.render_detail(cx));

        div()
            .id("agents-screen")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .child(body)
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
    }
}
