//! The project switcher (`project-switcher.html`): the popover under the sidebar's project
//! button. A search field filters the projects; ↑↓ move, ↵ switches, ⌘1–⌘5 jump; the
//! current project is marked; "New project…" and "Project settings" open the project form.
//!
//! Probes: `project-search`, `project-<id>`, `project-new`, `project-settings`.

pub(crate) mod form;

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, IntoElement, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use hub::OpsModel;
use model::{Project, ProjectId};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::logo;

/// The anchor key (and probe) of the project button.
pub(crate) const ANCHOR: &str = "sidebar-project";
/// The rail's anchor, when the sidebar is collapsed.
const RAIL_ANCHOR: &str = "sidebar-expand";
/// Projects reachable with ⌘1 to ⌘N.
pub const JUMP_KEYS: usize = 5;

/// The open switcher: its search field and the highlighted row.
pub(crate) struct Switcher {
    ops: Entity<OpsModel>,
    search: Entity<TextInput>,
    selected: usize,
    /// The search field took focus (it can only once it has been drawn).
    focused: bool,
    /// The key the popover was opened under (the button's, or the rail logo's).
    layer: &'static str,
    _subscription: Subscription,
}

impl Switcher {
    fn new(ops: Entity<OpsModel>, layer: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find a project"));
        let subscription = cx.subscribe_in(&search, window, |this: &mut Self, _, event, window, cx| {
            this.on_search(event, window, cx)
        });
        Self { ops, search, selected: 0, focused: false, layer, _subscription: subscription }
    }

    fn on_search(&mut self, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        match event {
            TextInputEvent::Changed => self.selected = 0,
            TextInputEvent::Down if count > 0 => self.selected = (self.selected + 1) % count,
            TextInputEvent::Up if count > 0 => self.selected = (self.selected + count - 1) % count,
            TextInputEvent::Submit => {
                if let Some(project) = self.matches(cx).get(self.selected) {
                    let id = project.id.clone();
                    self.choose(&id, window, cx);
                }
                return;
            }
            TextInputEvent::Cancel => {
                ui::overlays(window, cx).close(self.layer, window, cx);
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn choose(&self, id: &ProjectId, window: &mut Window, cx: &mut App) {
        ui::overlays(window, cx).close(self.layer, window, cx);
        switch_to(id, cx);
    }

    /// Projects whose name, key prefix or a repo folder contains the query, in list order.
    fn matches(&self, cx: &App) -> Vec<Project> {
        let query = self.search.read(cx).text().trim().to_lowercase();
        let ops = self.ops.read(cx);
        let all = ops.projects().value().map(|p| p.to_vec()).unwrap_or_default();
        all.into_iter().filter(|project| matches(project, &query)).collect()
    }
}

/// `project` is found by `query` (lower case; empty finds everything).
pub(crate) fn matches(project: &Project, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    project.name.to_lowercase().contains(query)
        || project.key_prefix.to_lowercase().contains(query)
        || project
            .repos
            .iter()
            .any(|repo| repo.file_name().is_some_and(|name| name.to_string_lossy().to_lowercase().contains(query)))
}

/// Open the switcher under the project button (or the rail's logo when collapsed).
pub(crate) fn open(window: &mut Window, cx: &mut App) {
    let ops = hub::ops(cx);
    let layer = if crate::collapse::is_collapsed(ops.read(cx), cx) { RAIL_ANCHOR } else { ANCHOR };
    let anchor = Anchor::of(layer, window, cx);
    let switcher = cx.new(|cx| Switcher::new(ops, layer, window, cx));
    ui::overlays(window, cx).open_popover(
        anchor,
        Placement::below(),
        move |window, cx| render(&switcher, window, cx),
        window,
        cx,
    );
}

fn render(switcher: &Entity<Switcher>, window: &mut Window, cx: &mut App) -> AnyElement {
    if !switcher.read(cx).focused {
        // Focus the field once this frame has drawn it.
        let switcher = switcher.clone();
        window.defer(cx, move |window, cx| {
            switcher.update(cx, |this, cx| {
                this.focused = true;
                this.search.read(cx).focus(window);
            })
        });
    }
    let this = switcher.read(cx);
    let ops = this.ops.read(cx);
    let all: Vec<Project> = ops.projects().value().map(|p| p.to_vec()).unwrap_or_default();
    let current = ops.current_project().map(|project| project.id.clone());
    let shown = this.matches(cx);
    let selected = this.selected.min(shown.len().saturating_sub(1));
    let search = this.search.clone();

    let mut list = col().flex_none();
    for (ix, project) in shown.iter().enumerate() {
        let jump = all.iter().position(|p| p.id == project.id).filter(|ix| *ix < JUMP_KEYS).map(|ix| ix + 1);
        let row = ProjectRow {
            project: project.clone(),
            current: current.as_ref() == Some(&project.id),
            selected: ix == selected,
            jump,
        };
        list = list.child(row.render(ix, switcher, window, cx));
    }
    if shown.is_empty() {
        list = list.child(div().px(px(SPACE_2_5)).py(px(SPACE_2)).text_color(TEXT_MUTED).child("No project matches"));
    }

    let has_current = current.is_some();
    Popover::new()
        .width(360.)
        .search(text_field("project-search", &search).small().icon(IconName::Search).kbd("⌘O").probe("project-search"))
        .child(PopLabel::new("Projects").trailing(SharedString::from(base::fmt::plural(all.len() as u64, "project", "projects"))))
        .child(list)
        .child(PopSeparator)
        .child(
            MenuItem::new("project-new", "New project…")
                .icon(IconName::Plus)
                .on_click(|_, window, cx| form::open_new(window, cx))
                .probe("project-new"),
        )
        .child(MenuItem::new("project-invite", "Invite people").icon(IconName::Users).coming_soon())
        .child(
            MenuItem::new("project-settings", "Project settings")
                .icon(IconName::Settings)
                .hint("⌘,")
                .disabled(!has_current)
                .on_click(|_, window, cx| form::open_edit(window, cx))
                .probe("project-settings"),
        )
        .child(PopSeparator)
        .child(MenuItem::new("project-account", "Switch account").tile(Avatar::new("ME")).coming_soon())
        .footer(KeyHints::new().hint(["↑↓"], "move").hint(["↵"], "open").hint(["⌘1–5"], "jump"))
        .into_any_element()
}

/// One project row (`.mi.ws-row`): tile, name with a check when current, its repos, the jump
/// key at the right.
struct ProjectRow {
    project: Project,
    current: bool,
    selected: bool,
    jump: Option<usize>,
}

impl ProjectRow {
    fn render(self, ix: usize, switcher: &Entity<Switcher>, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let name: SharedString = format!("project-{}", self.project.id).into();
        let (rest, hover) = if self.selected {
            let look = Look::bg(SURFACE_HOVER);
            (look, look)
        } else {
            (Look::bg(TRANSPARENT), Look::bg(SURFACE_HOVER))
        };
        let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
        let selector = name.to_string();
        let tile = if self.current { logo(28.).into_any_element() } else { letter_tile(&self.project.name).into_any_element() };
        let (pick, choose) = (switcher.clone(), switcher.clone());
        let id = self.project.id.clone();
        let el = row()
            .id(ElementId::Name(name.clone()))
            .debug_selector(move || selector)
            .w_full()
            .gap(px(SPACE_2_5))
            .min_h(px(MENU_ITEM_HEIGHT))
            .px(px(SPACE_2_5))
            .py(px(SPACE_1_5))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .on_mouse_move(move |_, _, cx| {
                pick.update(cx, |this, cx| {
                    if this.selected != ix {
                        this.selected = ix;
                        cx.notify();
                    }
                })
            })
            .on_click(move |_, window, cx| {
                let this = choose.read(cx);
                let (layer, id) = (this.layer, id.clone());
                ui::overlays(window, cx).close(layer, window, cx);
                switch_to(&id, cx);
            });
        faded
            .apply(el)
            .child(tile)
            .child(
                col()
                    .flex_1()
                    .child(
                        row()
                            .gap(px(SPACE_1_5))
                            .child(div().type_style(BODY_STRONG).child(self.project.name.clone()))
                            .when(self.current, |d| d.child(icon(IconName::Check, 14., ACCENT_TEXT))),
                    )
                    .child(truncate_block(repos_line(&self.project), LABEL.line).type_style(LABEL).text_color(TEXT_MUTED)),
            )
            .when_some(self.jump, |d, n| d.child(Kbd::new(format!("⌘{n}"))))
            .probe(name)
    }
}

/// "payments-api, checkout-web, ledger · 3 repos".
pub(crate) fn repos_line(project: &Project) -> String {
    let repos: Vec<String> = project
        .repos
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if repos.is_empty() {
        "No repos yet".to_string()
    } else {
        format!("{} · {}", repos.join(", "), base::fmt::plural(repos.len() as u64, "repo", "repos"))
    }
}

/// The first letter of a project's name on a neutral tile.
fn letter_tile(name: &str) -> impl IntoElement {
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(RADIUS_MD))
        .bg(SURFACE_HOVER)
        .text_color(TEXT_SECONDARY)
        .type_style(CAPTION_STRONG)
        .child(letter)
}

/// Make `id` the current project (saved in the settings).
pub(crate) fn switch_to(id: &ProjectId, cx: &mut App) {
    let ops = hub::ops(cx);
    let Some(settings) = ops.read(cx).settings().value().cloned() else { return };
    if settings.current_project.as_ref() == Some(id) {
        return;
    }
    let mut settings = (*settings).clone();
    settings.current_project = Some(id.clone());
    hub::commands(cx).set_settings(settings, cx);
}

/// Switch to the `n`th project of the list (1-based), for ⌘1–⌘5.
pub fn switch_to_nth(n: usize, cx: &mut App) {
    let ops = hub::ops(cx);
    let id = ops.read(cx).projects().value().and_then(|p| p.get(n.wrapping_sub(1)).map(|p| p.id.clone()));
    if let Some(id) = id {
        switch_to(&id, cx);
    }
}
