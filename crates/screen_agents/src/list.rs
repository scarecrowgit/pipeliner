//! The tab bar with the search field, and the list: group headers, one 56px row per runnable
//! (kind tile, name, kind, description; state; activity; where it is used from), the rows of
//! files that did not load, and the empty, loading and error states.

use gpui::{AnyElement, App, ClickEvent, Context, ElementId, Hsla, IntoElement, SharedString, Window, canvas, div, prelude::*, px};
use hub::LoadState;
use hub::api::{LoadProblem, PipelineId, Runnable, RunnableKind, Uses};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::text_field;

use crate::rows::{self, Dot, Group, Tab, Tone};
use crate::view::{AgentsView, SEARCH};

/// Height of a list row, px (`height:56px` in the mockup).
const ROW_H: f32 = 56.;
/// Widths of the State, Activity and Used from columns, px.
const STATE_W: f32 = 152.;
const ACTIVITY_W: f32 = 182.;
const USES_W: f32 = 104.;
/// Width of the search field: "Filter by name, model or runtime" whole, with its icon and key.
const SEARCH_W: f32 = 276.;
/// The narrowest the search field gets in a narrow window, px.
const SEARCH_MIN_W: f32 = 140.;
/// Most characters of the query the "Nothing matches" title repeats.
const QUERY_CHARS: usize = 60;
/// Widest the words of the empty state wrap at, px.
const EMPTY_TEXT_W: f32 = 520.;
/// The narrowest the Name column gets before a column on the right is left out, px.
const NAME_MIN_W: f32 = 240.;

/// Which columns right of Name fit a list `width` px wide: a narrow list (a window of about
/// 1000px with the sidebar open) leaves out "Used from", then "Activity", then "State",
/// so the name and description keep room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Columns {
    pub state: bool,
    pub activity: bool,
    pub uses: bool,
}

impl Columns {
    pub(crate) fn for_width(width: f32) -> Self {
        let fits = |cols: f32| width >= NAME_MIN_W + cols;
        Self {
            state: fits(STATE_W),
            activity: fits(STATE_W + ACTIVITY_W),
            uses: fits(STATE_W + ACTIVITY_W + USES_W),
        }
    }
}

/// The `ui` status of a row's dot.
pub(crate) fn status(dot: Dot) -> Status {
    match dot {
        Dot::Live => Status::Live,
        Dot::Running => Status::Running,
        Dot::Waiting => Status::Waiting,
        Dot::Error => Status::Error,
        Dot::Draft => Status::Draft,
        Dot::Idle => Status::Idle,
    }
}

/// The colour of a state word.
pub(crate) fn tone_color(tone: Tone) -> Hsla {
    match tone {
        Tone::Secondary => TEXT_SECONDARY,
        Tone::Muted => TEXT_MUTED,
        Tone::Success => SUCCESS,
        Tone::Warning => WARNING,
        Tone::Danger => DANGER,
    }
}

/// The icon of a runnable's kind tile.
pub(crate) fn kind_icon(runnable: &Runnable) -> IconName {
    if rows::is_spawned(runnable) {
        return IconName::Spawn;
    }
    match runnable.kind {
        RunnableKind::Agent => IconName::Agent,
        RunnableKind::Orchestrator => IconName::Orchestrator,
        RunnableKind::Pipeline => IconName::Pipelines,
    }
}

/// A state mark: the small dot and the words.
pub(crate) fn state_cell(mark: &rows::StateMark) -> gpui::Div {
    row()
        .gap(px(SPACE_1_5))
        .min_w_0()
        .type_style(LABEL)
        .child(StatusDot::new(status(mark.dot)).small())
        .child(div().min_w_0().text_color(tone_color(mark.tone)).child(truncate(mark.text.clone())))
}

/// The four "Used from" places with their icons and words.
pub(crate) const PLACES: [(IconName, &str); 4] =
    [(IconName::Chat, "Chat"), (IconName::Issues, "Issues"), (IconName::Pipelines, "Inside pipelines"), (IconName::Clock, "Autopilots")];

/// Whether `uses` has place `ix` of [`PLACES`] on.
pub(crate) fn uses_at(uses: &Uses, ix: usize) -> bool {
    match ix {
        0 => uses.chat,
        1 => uses.issues,
        2 => uses.pipelines,
        _ => uses.autopilots,
    }
}

impl AgentsView {
    pub(crate) fn render_tabs(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.catalog.read(cx).runnables();
        let tabs: Vec<AnyElement> = Tab::ALL
            .iter()
            .filter(|tab| **tab != Tab::Archived || rows::count(all, Tab::Archived) > 0)
            .map(|tab| {
                let tab = *tab;
                let n = rows::count(all, tab);
                let selected = self.tab == tab;
                let id: SharedString = format!("agents-tab-{}", tab.slug()).into();
                let spawn = tab == Tab::Created;
                let fg = match (spawn, selected) {
                    (true, _) => SPAWN_TEXT,
                    (false, true) => TEXT,
                    (false, false) => TEXT_MUTED,
                };
                let hover_fg = if spawn { SPAWN_TEXT } else { TEXT };
                let sel = id.clone();
                motion::fade(div().id(id.clone()).debug_selector(move || sel.to_string()), id.clone(), Look::bg(TRANSPARENT).fg(fg), Look::bg(TRANSPARENT).fg(hover_fg))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(SPACE_1_5))
                    .h(px(TAB_HEIGHT))
                    .border_b_2()
                    .border_color(if selected { ACCENT } else { TRANSPARENT })
                    .type_style(if selected { BODY_STRONG } else { BODY })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(tab.label())
                    .child(div().text_color(if spawn { SPAWN_TEXT } else { TEXT_MUTED }).tabular().child(n.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                    .probe(id)
                    .into_any_element()
            })
            .collect();
        row()
            .flex_none()
            .h(px(TAB_HEIGHT))
            .gap(px(SPACE_5))
            .px(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .children(tabs)
            .child(grow())
            .child(
                // The mockup's small field; it gives way to the tabs in a narrow window.
                div()
                    .flex_shrink()
                    .min_w(px(SEARCH_MIN_W))
                    .w(px(SEARCH_W))
                    .type_style(LABEL)
                    .child(text_field(SEARCH, &self.search).compact().icon(IconName::Search).kbd("F"))
                    .probe(SEARCH),
            )
    }

    pub(crate) fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let catalog = self.catalog.read(cx);
        let state = catalog.state().clone();
        let all: Vec<Runnable> = catalog.runnables().to_vec();
        let problems = self.problems(cx);
        let query = self.search.read(cx).text().to_string();
        let groups = rows::grouped(&all, self.tab, &query);
        let cols = Columns::for_width(self.list_w.get());

        let mut children: Vec<AnyElement> = Vec::new();
        if !problems.is_empty() && self.tab == Tab::All {
            children.push(self.problems_notice(&problems, cx));
        }
        let empty = groups.is_empty();
        for (group, list) in &groups {
            children.push(group_header(*group, list.len()));
            for runnable in list {
                children.push(self.render_row(runnable, cols, window, cx));
            }
        }
        if !problems.is_empty() && self.tab == Tab::All && query.trim().is_empty() {
            children.push(problem_header(problems.len()));
            for (ix, problem) in problems.iter().enumerate() {
                children.push(problem_row(ix, problem, cols, cx));
            }
        }
        if empty {
            children.push(self.empty_state(&state, &query, cx));
        }

        let width = self.list_w.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(measure_width(move |w| width.replace(w) != w))
            .child(header_row(cols))
            .child(
                div()
                    .id("agents-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(children.into_iter().map(|c| div().flex_shrink_0().child(c))),
            )
            .into_any_element()
    }

    fn render_row(&self, runnable: &Runnable, cols: Columns, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let catalog = self.catalog.read(cx);
        let activity = catalog.activity(&runnable.reference);
        let graph = (runnable.kind == RunnableKind::Pipeline)
            .then(|| catalog.pipeline(&PipelineId::new(runnable.reference.id())))
            .flatten()
            .and_then(|g| g.value())
            .and_then(|g| g.as_ref().as_ref());
        let mark = rows::state_mark(runnable, activity);
        let meta = rows::activity_text(runnable, activity, graph);
        let selected = self.selected.as_ref() == Some(&runnable.reference);
        let spawned = rows::is_spawned(runnable);
        let id: SharedString = format!("agents-row-{}", runnable.reference).into();

        let kind_label: SharedString = match &runnable.origin {
            hub::api::Origin::Spawned { by, .. } => {
                let by = catalog.runnable(by).map_or_else(|| by.id().to_string(), |r| r.name.clone());
                format!("by {by}").into()
            }
            _ => rows::kind_word(runnable.kind).into(),
        };
        // A long name ends in an ellipsis and leaves its kind and tags in the row.
        let name = row()
            .gap(px(SPACE_1_5))
            .min_w_0()
            .child(fit(runnable.name.clone()).type_style(BODY_STRONG))
            .child({
                let kind_id = format!("agents-kind-{}", runnable.reference);
                div()
                    .flex_none()
                    .type_style(LABEL)
                    .text_color(if spawned { SPAWN_TEXT } else { TEXT_MUTED })
                    .child(kind_label)
                    .debug_selector(move || kind_id)
            })
            .when_some(rows::origin_tag(&runnable.origin), |d, tag| d.child(Tag::new(tag)))
            .when(runnable.paused && !runnable.archived, |d| d.child(Tag::new("Paused")));
        let tile = Tile::new(kind_icon(runnable)).tone(if spawned { TileTone::Spawn } else { TileTone::Neutral });
        let name_cell = row()
            .gap(px(SPACE_3))
            .flex_1()
            .min_w_0()
            .pl(px(SPACE_4))
            .pr(px(SPACE_3))
            .child(tile)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(name)
                    // Cut at the width it is given, with an ellipsis (a clamped line in this box
                    // is measured on one line and clipped instead).
                    .child(
                        truncate_styled_block(runnable.description.clone(), [], LABEL.line)
                            .type_style(LABEL)
                            .text_color(TEXT_SECONDARY),
                    ),
            );
        let uses = row().gap(px(SPACE_2)).children(PLACES.iter().enumerate().map(|(ix, (icon_name, label))| {
            let on = uses_at(&runnable.uses, ix);
            let tip: SharedString = if on { (*label).into() } else { format!("{label} · off").into() };
            div()
                .id(ElementId::Name(format!("{id}-use-{ix}").into()))
                .child(icon(*icon_name, 14., if on { TEXT_SECONDARY } else { LINE_STRONG }))
                .tooltip(Tooltip::text(tip))
        }));

        let (rest, hover) = motion::row_looks(selected, SURFACE_RAISED);
        let faded = motion::hover_fade_state(id.clone(), rest, hover, window, cx);
        let reference = runnable.reference.clone();
        let sel = id.clone();
        faded
            .apply(div().id(id.clone()).debug_selector(move || sel.to_string()))
            .relative()
            .flex()
            .items_center()
            .h(px(ROW_H))
            .border_b_1()
            .border_color(LINE_SUBTLE)
            .cursor_pointer()
            .when(selected, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(ACCENT)))
            .child(name_cell)
            .when(cols.state, |d| d.child(cell(STATE_W).child(state_cell(&mark))))
            .when(cols.activity, |d| d.child(cell(ACTIVITY_W).type_style(LABEL).text_color(TEXT_MUTED).child(truncate(meta))))
            .when(cols.uses, |d| d.child(cell(USES_W).child(uses)))
            // A click selects; a double-click also opens it as Enter does (the editor for a
            // pipeline, the edit dialog for an agent). The row holds no controls of its own.
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus);
                this.select(Some(reference.clone()), cx);
                if event.click_count() >= 2 {
                    this.open_edit(&reference, window, cx);
                }
            }))
            .probe(id)
            .into_any_element()
    }

    fn empty_state(&mut self, state: &LoadState, query: &str, cx: &mut Context<Self>) -> AnyElement {
        // A long query is cut in the title; the field has all of it.
        let shown = base::fmt::truncate_chars(query.trim(), QUERY_CHARS);
        let (title, detail): (SharedString, SharedString) = match state {
            LoadState::Loading => ("Loading the library…".into(), "".into()),
            LoadState::Failed(why) => ("Could not read the library".into(), why.clone()),
            LoadState::Ready if !query.trim().is_empty() => {
                (format!("Nothing matches “{shown}”").into(), "Try a name, a model or a runtime.".into())
            }
            LoadState::Ready if self.tab == Tab::Created => (
                "No agents created by agents".into(),
                "Orchestrators and agents allowed to create helpers list them here while they live.".into(),
            ),
            LoadState::Ready => (
                "Nothing here yet".into(),
                "Create an agent, an orchestrator or a pipeline, or import one from a file.".into(),
            ),
        };
        let failed = matches!(state, LoadState::Failed(_));
        let clear = !query.trim().is_empty();
        // The words wrap inside the list (a definite width, centred); the reason the library
        // failed is selectable, to copy it.
        let text_w = (self.list_w.get() - 2. * SPACE_5).clamp(80., EMPTY_TEXT_W);
        let detail: Option<AnyElement> = match (failed, detail.is_empty()) {
            (_, true) => None,
            (true, false) => {
                let view = self.text("library-error", detail, LABEL, TEXT_SECONDARY, cx);
                Some(div().w(px(text_w)).debug_selector(|| "agents-library-error".into()).child(view).into_any_element())
            }
            (false, false) => Some(
                div().w(px(text_w)).text_center().type_style(LABEL).text_color(TEXT_MUTED).child(detail).into_any_element(),
            ),
        };
        col()
            .items_center()
            .gap(px(SPACE_2))
            .py(px(SPACE_8 * 2.))
            .child(
                div()
                    .w(px(text_w))
                    .text_center()
                    .type_style(BODY_STRONG)
                    .text_color(if failed { DANGER } else { TEXT })
                    .child(title),
            )
            .children(detail)
            .when(clear, |d| {
                d.child(
                    Button::new("agents-clear-search", "Clear filter")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.search.update(cx, |input, cx| input.set_text("", cx));
                            cx.notify();
                        })),
                )
            })
            .when(matches!(state, LoadState::Ready) && !clear && self.tab == Tab::All, |d| {
                d.child(
                    Button::new("agents-empty-new", "New agent")
                        .secondary()
                        .small()
                        .icon(IconName::Plus)
                        .on_click(cx.listener(|this, _, window, cx| this.open_templates(Some(RunnableKind::Agent), window, cx))),
                )
            })
            .debug_selector(|| "agents-empty".into())
            .probe("agents-empty")
            .into_any_element()
    }
}

/// One line of `text` as wide as it is, ending in `…` when its row is too narrow: a name
/// followed by its kind or tags.
pub(crate) fn fit(text: impl Into<SharedString>) -> gpui::Div {
    truncate(text).flex_initial()
}

/// An invisible layer over its (relative) parent that hands the parent's width to `store`;
/// when `store` says it changed, the window is drawn again with it.
pub(crate) fn measure_width(store: impl Fn(f32) -> bool + 'static) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            if store(f32::from(bounds.size.width)) {
                // A refresh asked for while drawing is dropped: ask once this frame is done.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A fixed-width table cell.
fn cell(width: f32) -> gpui::Div {
    div().flex().items_center().flex_none().w(px(width)).min_w_0().h_full().px(px(SPACE_3)).overflow_hidden()
}

fn header_row(cols: Columns) -> impl IntoElement {
    row()
        .flex_none()
        .h(px(TABLE_HEAD_HEIGHT))
        .border_b_1()
        .border_color(LINE)
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(div().flex_1().min_w_0().pl(px(SPACE_4)).child("Name"))
        .when(cols.state, |d| d.child(cell(STATE_W).child("State")))
        .when(cols.activity, |d| d.child(cell(ACTIVITY_W).child("Activity")))
        .when(cols.uses, |d| d.child(cell(USES_W).child("Used from")))
}

fn group_header(group: Group, n: usize) -> AnyElement {
    let color = if group == Group::Created { SPAWN_TEXT } else { TEXT_SECONDARY };
    row()
        .h(px(TABLE_GROUP_HEIGHT))
        .gap(px(SPACE_1_5))
        .px(px(SPACE_4))
        .bg(BG_SIDEBAR)
        .border_b_1()
        .border_color(LINE)
        .child(div().type_style(BODY_STRONG).text_color(color).child(group.label()))
        .child(div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(n.to_string()))
        .into_any_element()
}

fn problem_header(n: usize) -> AnyElement {
    row()
        .h(px(TABLE_GROUP_HEIGHT))
        .gap(px(SPACE_1_5))
        .px(px(SPACE_4))
        .bg(BG_SIDEBAR)
        .border_b_1()
        .border_color(LINE)
        .child(div().type_style(BODY_STRONG).text_color(WARNING).child("Could not load"))
        .child(div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(n.to_string()))
        .into_any_element()
}

/// A file that did not parse, as a row: its name, the parser's message, the "load problem"
/// state and where it is.
fn problem_row(ix: usize, problem: &LoadProblem, cols: Columns, cx: &App) -> AnyElement {
    let file = problem.path.file_name().map_or_else(|| problem.path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let path = hub::display_path(&problem.path, cx);
    row()
        .id(("agents-problem", ix))
        .h(px(ROW_H))
        .border_b_1()
        .border_color(LINE_SUBTLE)
        .child(
            row()
                .gap(px(SPACE_3))
                .flex_1()
                .min_w_0()
                .pl(px(SPACE_4))
                .pr(px(SPACE_3))
                .child(Tile::new(IconName::Alert).tone(TileTone::Warn))
                .child(
                    col()
                        .flex_1()
                        .min_w_0()
                        .child(div().type_style(BODY_STRONG).font_family(FONT_MONO).child(truncate(file)))
                        .child(truncate_lh(problem.message.clone(), LABEL.line).type_style(LABEL).text_color(TEXT_SECONDARY)),
                ),
        )
        .when(cols.state, |d| {
            d.child(cell(STATE_W).child(state_cell(&rows::StateMark { dot: Dot::Error, text: "load problem".into(), tone: Tone::Danger })))
        })
        .when(cols.activity, |d| {
            let width = if cols.uses { ACTIVITY_W + USES_W } else { ACTIVITY_W };
            d.child(cell(width).type_style(LABEL).text_color(TEXT_MUTED).font_family(FONT_MONO).child(truncate_middle(path, LABEL.line)))
        })
        .debug_selector(move || format!("agents-problem-{ix}"))
        .probe(format!("agents-problem-{ix}"))
        .into_any_element()
}

impl AgentsView {
    /// The notice over the list when files of the library did not load: each file and the
    /// parser's message, wrapping and selectable so the error can be copied.
    fn problems_notice(&mut self, problems: &[LoadProblem], cx: &mut Context<Self>) -> AnyElement {
        let title = if problems.len() == 1 {
            "1 file in the library could not be read".to_string()
        } else {
            format!("{} files in the library could not be read", problems.len())
        };
        // Wrapping text gets a definite width: the list's, less the notice's padding, border,
        // icon and gap.
        let text_w = (self.list_w.get() - 2. * SPACE_3 - 2. * SPACE_3_5 - 2. - 16. - SPACE_3).max(80.);
        let lines: Vec<AnyElement> = problems
            .iter()
            .take(3)
            .enumerate()
            .map(|(ix, p)| {
                let path = self.text(format!("problem-path-{ix}"), hub::display_path(&p.path, cx), LABEL, TEXT, cx);
                let message = self.text(format!("problem-message-{ix}"), p.message.clone(), LABEL, TEXT_SECONDARY, cx);
                col()
                    .w(px(text_w))
                    .child(div().w(px(text_w)).debug_selector(move || format!("agents-problem-path-{ix}")).child(path))
                    .child(div().w(px(text_w)).debug_selector(move || format!("agents-problem-message-{ix}")).child(message))
                    .into_any_element()
            })
            .collect();
        div()
            .p(px(SPACE_3))
            .child(
                Banner::new()
                    .tone(BannerTone::Warning)
                    .child(icon(IconName::Alert, 16., WARNING))
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(SPACE_1))
                            .child(div().type_style(BODY_STRONG).child(title))
                            .children(lines)
                            .child(Note::new("Fix the file and the library reloads it; nothing else is affected.")),
                    ),
            )
            .debug_selector(|| "agents-problems".into())
            .probe("agents-problems")
            .into_any_element()
    }
}
