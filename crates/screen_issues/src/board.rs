//! The board and the list of issues, their filters, the new-issue dialog and the autopilots
//! popover.
//!
//! [`IssuesView`] is the whole content of `Route::Issues`: a bar with the Board / List switch
//! and the filters, the board (or the list) of the current project, and the panel of the
//! issue in the route. The shell draws the header from [`IssuesView::title`],
//! [`IssuesView::header_badges`] and [`IssuesView::header_actions`].

mod autopilots;
mod card;
mod columns;
mod filters;
mod list;
mod new_issue;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Global, IntoElement, KeyDownEvent, Render,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use hub::api::{IssueStatus, ProjectId};
use hub::model::IssueKey;
use hub::{CatalogModel, IssuesModel, LoadState, OpsModel, Route, RunsModel};
use ui::dev::Probe;
use ui::keys::{Cancel, Confirm, MoveDown, MoveFirst, MoveLast, MoveLeft, MoveRight, MoveUp};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent};

use crate::panel::{IssuePanel, PanelEvent};
use filters::{Filters, RunsWith};
use list::SortBy;
use new_issue::{NewIssueForm, Seed};

/// Width of the open issue's panel.
const PANEL_WIDTH: f32 = 400.;

/// How the issues are laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Columns by status.
    #[default]
    Board,
    /// A sortable table.
    List,
}

/// The layout and the list's order, remembered for the session (not saved).
#[derive(Default)]
struct Remembered {
    layout: Layout,
    sort: SortBy,
}

impl Global for Remembered {}

/// The whole content of `Route::Issues`.
pub struct IssuesView {
    issue: Option<IssueKey>,
    panel: Option<(Entity<IssuePanel>, Subscription)>,
    pub(crate) issues: Entity<IssuesModel>,
    pub(crate) runs: Entity<RunsModel>,
    pub(crate) catalog: Entity<CatalogModel>,
    ops: Entity<OpsModel>,
    focus: FocusHandle,
    /// The card the arrow keys stand on; Enter opens it.
    pub(crate) cursor: Option<IssueKey>,
    pub(crate) filters: Filters,
    pub(crate) search: Entity<TextInput>,
    /// The dialog's form while it is open.
    form: Option<Entity<NewIssueForm>>,
    focus_pending: bool,
    shown: bool,
    _subs: Vec<Subscription>,
}

impl IssuesView {
    /// The board of the current project, with `issue` open in the panel when given.
    pub fn new(issue: Option<IssueKey>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let issues = hub::issues(cx);
        let runs = hub::runs(cx);
        let catalog = hub::catalog(cx);
        let ops = hub::ops(cx);
        ops.update(cx, |ops, cx| ops.ensure_autopilots(cx));
        let router = hub::router(cx);
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find issues"));
        let subs = vec![
            cx.observe(&issues, |_, _, cx| cx.notify()),
            cx.observe(&runs, |_, _, cx| cx.notify()),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
            cx.observe(&ops, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this: &mut Self, search, event: &TextInputEvent, cx| {
                if *event == TextInputEvent::Changed {
                    this.filters.text = search.read(cx).text().to_string();
                    cx.notify();
                }
            }),
            cx.observe_in(&router, window, |this: &mut Self, router, window, cx| {
                let route = router.read(cx).current().clone();
                match route {
                    Route::Issues { issue } => {
                        if !this.shown {
                            this.shown = true;
                            this.focus_pending = true;
                        }
                        this.show(issue, window, cx);
                    }
                    _ => this.shown = false,
                }
            }),
        ];
        let shown = matches!(router.read(cx).current(), Route::Issues { .. });
        let mut this = Self {
            issue: None,
            panel: None,
            issues,
            runs,
            catalog,
            ops,
            focus: cx.focus_handle(),
            cursor: None,
            filters: Filters::default(),
            search,
            form: None,
            // Focus is taken on the first frame drawn: the shell builds the view before it is
            // in the window.
            focus_pending: true,
            shown,
            _subs: subs,
        };
        this.show(issue, window, cx);
        this
    }

    /// Follows the route: opens the panel of `issue`, or closes it.
    pub fn show(&mut self, issue: Option<IssueKey>, window: &mut Window, cx: &mut Context<Self>) {
        if issue.is_some() {
            self.cursor = issue.clone();
        }
        if self.issue == issue && self.panel.is_some() == issue.is_some() {
            return;
        }
        self.issue = issue.clone();
        self.panel = issue.map(|key| {
            let panel = cx.new(|cx| IssuePanel::new(key, window, cx));
            let sub = cx.subscribe_in(&panel, window, |_: &mut Self, _, event: &PanelEvent, _, cx| match event {
                PanelEvent::Close => hub::navigate(Route::Issues { issue: None }, cx),
            });
            (panel, sub)
        });
        cx.notify();
    }

    /// The panel of an issue is open beside the board.
    pub(crate) fn has_panel(&self) -> bool {
        self.panel.is_some()
    }

    /// The issue whose panel is open.
    pub fn issue(&self) -> Option<&IssueKey> {
        self.issue.as_ref()
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &App) -> SharedString {
        "Issues".into()
    }

    /// The name of the project shown ("Payments"), for the crumb before the title.
    pub fn project_name(&self, cx: &App) -> Option<SharedString> {
        let project = self.issues.read(cx).project()?;
        let projects = self.ops.read(cx).projects().value()?;
        projects.iter().find(|p| &p.id == project).map(|p| p.name.clone().into())
    }

    /// What follows the title in the shell's header: how many issues are open.
    pub fn header_badges(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let issues = self.issues.read(cx);
        if *issues.state() != LoadState::Ready {
            return Vec::new();
        }
        let open = issues.open_count();
        vec![Badge::new(format!("{open} open")).into_any_element()]
    }

    /// The controls on the right of the shell's header, left to right: Autopilots, New issue.
    pub fn header_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let on = self
            .ops
            .read(cx)
            .autopilots()
            .value()
            .map_or(0, |list| list.iter().filter(|autopilot| autopilot.enabled).count());
        let open = overlays(window, cx).is_open(autopilots::BUTTON, cx);
        let label = if on > 0 { format!("Autopilots · {on}") } else { "Autopilots".to_string() };
        let ready = *self.issues.read(cx).state() == LoadState::Ready;
        vec![
            Button::new(autopilots::BUTTON, label)
                .ghost()
                .icon(IconName::Clock)
                .selected(open)
                .on_click(cx.listener(|this, _, window, cx| this.open_autopilots(window, cx)))
                .popover_anchor(autopilots::BUTTON)
                .probe(autopilots::BUTTON)
                .into_any_element(),
            Button::new("issues-new", "New issue")
                .primary()
                .icon(IconName::Plus)
                .kbd("C")
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| this.new_issue(None, window, cx)))
                .probe("issues-new")
                .into_any_element(),
        ]
    }

    // ------------------------------------------------------------ state

    /// Board or list, as last picked in this session.
    pub fn layout(&self, cx: &App) -> Layout {
        cx.try_global::<Remembered>().map(|r| r.layout).unwrap_or_default()
    }

    /// Switches between the board and the list; the choice holds for the session.
    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        cx.default_global::<Remembered>().layout = layout;
        cx.notify();
    }

    pub(crate) fn sort(&self, cx: &App) -> SortBy {
        cx.try_global::<Remembered>().map(|r| r.sort).unwrap_or_default()
    }

    pub(crate) fn set_sort(&mut self, sort: SortBy, cx: &mut Context<Self>) {
        cx.default_global::<Remembered>().sort = sort;
        cx.notify();
    }

    /// Changes the filters.
    pub(crate) fn edit_filters(&mut self, f: impl FnOnce(&mut Filters), cx: &mut Context<Self>) {
        f(&mut self.filters);
        cx.notify();
    }

    /// Clears every filter, the search text included.
    pub(crate) fn clear_filters(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.filters = Filters::default();
        self.search.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    /// Owners of the project's issues, "You" first, for the filter and the dialog.
    pub(crate) fn owners(&self, cx: &App) -> Vec<String> {
        let mut owners: BTreeSet<String> = self.issues.read(cx).iter().map(|issue| issue.owner.clone()).collect();
        let mut list = Vec::new();
        if owners.remove("You") {
            list.push("You".to_string());
        }
        list.extend(owners);
        list
    }

    /// Opens `key` in the panel.
    pub(crate) fn open_issue(&mut self, key: &IssueKey, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = Some(key.clone());
        window.focus(&self.focus);
        hub::navigate(Route::Issues { issue: Some(key.clone()) }, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ overlays

    /// Opens the new-issue dialog, filed under `status` (Backlog when `None`).
    pub fn new_issue(&mut self, status: Option<IssueStatus>, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.seed(status.unwrap_or_default(), cx);
        let project_name = seed.project_name.clone();
        let form = cx.new(|cx| NewIssueForm::new(seed, window, cx));
        self.form = Some(form.clone());
        overlays(window, cx).open_dialog(
            new_issue::DIALOG,
            move |_, cx| {
                let this = form.read(cx);
                let (starts, runner) = (this.primary_starts(), this.runner.is_some());
                let more = this.create_more;
                let toggle_form = form.clone();
                let note = row()
                    .gap(px(SPACE_2))
                    .child(Toggle::new("new-issue-more", more).on_change(move |on, _, cx| {
                        toggle_form.update(cx, |form, cx| {
                            form.create_more = on;
                            cx.notify();
                        })
                    }))
                    .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child("Create more"));
                let (create, start) = (form.clone(), form.clone());
                let create = Button::new(new_issue::CREATE, "Create issue")
                    .on_click(move |_, window, cx| create.update(cx, |form, cx| form.submit(false, window, cx)));
                let start = Button::new(new_issue::CREATE_START, "Create & start")
                    .on_click(move |_, window, cx| start.update(cx, |form, cx| form.submit(true, window, cx)));
                let (create, start) = match (runner, starts) {
                    (false, _) => (create.primary().kbd("⌘↵"), None),
                    (true, true) => (create.secondary(), Some(start.primary().icon(IconName::Play).kbd("⌘↵"))),
                    (true, false) => (create.primary().kbd("⌘↵"), Some(start.secondary().icon(IconName::Play))),
                };
                let confirm = form.clone();
                let subtitle = if project_name.is_empty() { "No project".to_string() } else { project_name.clone() };
                let mut dialog = Dialog::new(new_issue::DIALOG, "New issue")
                    .subtitle(SharedString::from(subtitle))
                    .child(form.clone())
                    .note(note)
                    .cancel_button();
                // Primary last: the foot reads Cancel, the other action, the primary one.
                match start {
                    Some(start) if starts => {
                        dialog = dialog.action(create.probe(new_issue::CREATE)).action(start.probe(new_issue::CREATE_START))
                    }
                    Some(start) => {
                        dialog = dialog.action(start.probe(new_issue::CREATE_START)).action(create.probe(new_issue::CREATE))
                    }
                    None => dialog = dialog.action(create.probe(new_issue::CREATE)),
                }
                dialog
                    .on_confirm(move |window, cx| {
                        confirm.update(cx, |form, cx| {
                            let starts = form.primary_starts();
                            form.submit(starts, window, cx)
                        })
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
    }

    /// What the dialog starts from: the project, its repos, the owners seen on the board.
    fn seed(&self, status: IssueStatus, cx: &App) -> Seed {
        let project: Option<ProjectId> = self.issues.read(cx).project().cloned();
        let found = project.as_ref().and_then(|id| {
            self.ops.read(cx).projects().value().and_then(|list| list.iter().find(|p| &p.id == id).cloned())
        });
        let repos = found
            .as_ref()
            .map(|p| {
                p.repos.iter().filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned())).collect()
            })
            .unwrap_or_default();
        let mut owners = self.owners(cx);
        if !owners.iter().any(|o| o == "You") {
            owners.insert(0, "You".to_string());
        }
        Seed { project, project_name: found.map(|p| p.name).unwrap_or_default(), repos, owners, status }
    }

    fn open_autopilots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let popover = cx.new(|cx| autopilots::AutopilotsPopover::new(me, cx));
        let anchor = Anchor::of(autopilots::BUTTON, window, cx);
        overlays(window, cx).open_popover(
            anchor,
            Placement::below().end(),
            move |_, _| popover.clone().into_any_element(),
            window,
            cx,
        );
        cx.notify();
    }

    fn open_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity();
        let anchor = Anchor::of(filters::FILTER, window, cx);
        overlays(window, cx).open_popover(
            anchor,
            Placement::below().end(),
            move |window, cx| filters::popover(&me, window, cx),
            window,
            cx,
        );
        let search = self.search.clone();
        window.defer(cx, move |window, cx| search.read(cx).focus(window));
    }

    fn open_sort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity();
        let anchor = Anchor::of(list::SORT, window, cx);
        overlays(window, cx).open_menu(anchor, Placement::below().end(), move |_, cx| list::sort_menu(&me, cx), window, cx);
    }

    // ------------------------------------------------------------ drawing

    fn render_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let layout = self.layout(cx);
        let issues = self.issues.read(cx);
        let counts: Vec<(RunsWith, usize)> = RunsWith::ALL
            .into_iter()
            .map(|chip| {
                let n = issues
                    .iter()
                    .filter(|issue| self.filters.matches_besides_runs_with(issue) && chip.matches(issue.runner.as_ref()))
                    .count();
                (chip, n)
            })
            .collect();
        let chips = counts.into_iter().map(|(chip, n)| {
            let id = SharedString::from(format!("issues-chip-{}", chip.slug()));
            Chip::new(id.clone(), chip.label())
                .count(n.to_string())
                .pressed(self.filters.runs_with == chip)
                .on_click(cx.listener(move |this, _, _, cx| this.edit_filters(|f| f.runs_with = chip, cx)))
                .probe(id)
        }).collect::<Vec<_>>();
        let filter_on = self.filters.popover_count() > 0;
        let sort = (layout == Layout::List).then(|| {
            SelectBox::new(list::SORT, format!("Sort: {}", self.sort(cx).label()))
                .small()
                .on_click(cx.listener(|this, _, window, cx| this.open_sort(window, cx)))
                .popover_anchor(list::SORT)
                .probe(list::SORT)
        });
        row()
            .flex_none()
            .h(px(TAB_HEIGHT))
            .px(px(SPACE_5))
            .gap(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .child(self.tab(Layout::Board, layout, cx))
            .child(self.tab(Layout::List, layout, cx))
            .child(grow())
            .child(
                row()
                    .flex_none()
                    .gap(px(SPACE_1_5))
                    .children(sort)
                    .child(
                        Button::new(filters::FILTER, filters::button_label(&self.filters))
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .selected(filter_on)
                            .on_click(cx.listener(|this, _, window, cx| this.open_filters(window, cx)))
                            .popover_anchor(filters::FILTER)
                            .probe(filters::FILTER),
                    )
                    .child(div().flex_none().ml(px(SPACE_1)).mr(px(SPACE_0_5)).type_style(LABEL).text_color(TEXT_MUTED).child("Runs with"))
                    .children(chips),
            )
    }

    /// One of the Board / List tabs (underline tabs with an icon, as drawn).
    fn tab(&self, which: Layout, current: Layout, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (id, label, glyph) = match which {
            Layout::Board => ("issues-view-board", "Board", IconName::Issues),
            Layout::List => ("issues-view-list", "List", IconName::Filter),
        };
        let selected = which == current;
        let fg = if selected { TEXT } else { TEXT_MUTED };
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_1_5))
            .h(px(TAB_HEIGHT))
            .mb(px(-1.))
            .border_b_2()
            .border_color(if selected { ACCENT } else { TRANSPARENT })
            .type_style(if selected { BODY_STRONG } else { BODY })
            .text_color(fg)
            .hover(|s| s.text_color(TEXT))
            .cursor_pointer()
            .child(icon(glyph, 14., fg))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.set_layout(which, cx)))
            .probe(id)
    }

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.issues.read(cx).state().clone();
        match state {
            LoadState::Loading => centered(div().text_color(TEXT_MUTED).child("Loading issues…")).into_any_element(),
            LoadState::Failed(error) => centered(
                col()
                    .items_center()
                    .gap(px(SPACE_1))
                    .child(div().type_style(BODY_STRONG).child("Could not load the issues"))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(error)),
            )
            .probe("issues-error")
            .into_any_element(),
            LoadState::Ready if self.issues.read(cx).is_empty() => centered(
                col()
                    .items_center()
                    .gap(px(SPACE_3))
                    .child(div().type_style(BODY_STRONG).child("No issues yet"))
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_MUTED)
                            .child("File one and attach an agent, an orchestrator or a pipeline to work on it."),
                    )
                    .child(
                        Button::new("issues-empty-new", "New issue")
                            .primary()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| this.new_issue(None, window, cx))),
                    ),
            )
            .probe("issues-empty")
            .into_any_element(),
            LoadState::Ready => {
                let layout = self.layout(cx);
                let any = self.issues.read(cx).iter().any(|issue| self.filters.matches(issue));
                if !any {
                    return filters::nothing_matches(&cx.entity()).into_any_element();
                }
                match layout {
                    Layout::Board => self.render_board(window, cx),
                    Layout::List => self.render_list(window, cx),
                }
            }
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let plain = !keys.modifiers.platform && !keys.modifiers.control && !keys.modifiers.alt;
        if plain && keys.key == "c" && self.focus.is_focused(window) {
            cx.stop_propagation();
            self.new_issue(None, window, cx);
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.cursor.clone() {
            self.open_issue(&key, window, cx);
        }
    }
}

fn centered(content: impl IntoElement) -> gpui::Div {
    div().flex().flex_1().size_full().items_center().justify_center().child(content)
}

impl Render for IssuesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            let focus = self.focus.clone();
            window.defer(cx, move |window, _| window.focus(&focus));
        }
        let main = self.render_main(window, cx);
        let focus = self.focus.clone();
        let board = div()
            .key_context(ui::keys::CHOICE)
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.move_cursor(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.move_cursor(1, 0, cx)))
            .child(
                div()
                    .key_context(ui::keys::MENU)
                    .track_focus(&self.focus)
                    .flex()
                    .size_full()
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| window.focus(&focus))
                    .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_cursor(0, -1, cx)))
                    .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_cursor(0, 1, cx)))
                    .on_action(cx.listener(|this, _: &MoveFirst, _, cx| this.cursor_to_end(false, cx)))
                    .on_action(cx.listener(|this, _: &MoveLast, _, cx| this.cursor_to_end(true, cx)))
                    .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
                    .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                        if this.issue.is_some() {
                            hub::navigate(Route::Issues { issue: None }, cx);
                        } else {
                            cx.propagate();
                        }
                    }))
                    .on_key_down(cx.listener(Self::on_key))
                    .child(main),
            );
        let panel = self.panel.as_ref().map(|(panel, _)| {
            div()
                .flex_none()
                .w(px(PANEL_WIDTH))
                .h_full()
                .border_l_1()
                .border_color(LINE)
                .bg(BG)
                .overflow_hidden()
                .child(panel.clone())
        });
        col()
            .size_full()
            .min_w_0()
            .bg(BG)
            .child(self.render_bar(cx))
            .child(div().flex().flex_row().flex_1().min_h_0().w_full().child(board).children(panel))
    }
}
