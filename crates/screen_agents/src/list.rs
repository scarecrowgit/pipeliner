//! The tab bar with the search field, and the list: group headers, one 56px row per runnable
//! (kind tile, name, kind, description; state; activity; where it is used from), the rows of
//! files that did not load, and the empty, loading and error states.

use gpui::{AnyElement, App, Context, ElementId, Hsla, IntoElement, SharedString, Window, div, prelude::*, px};
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
const STATE_W: f32 = 168.;
const ACTIVITY_W: f32 = 196.;
const USES_W: f32 = 104.;

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
                div()
                    .flex_none()
                    .w(px(240.))
                    .child(text_field(SEARCH, &self.search).small().icon(IconName::Search).kbd("F"))
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

        let mut children: Vec<AnyElement> = Vec::new();
        if !problems.is_empty() && self.tab == Tab::All {
            children.push(problems_notice(&problems, cx));
        }
        let empty = groups.is_empty();
        for (group, list) in &groups {
            children.push(group_header(*group, list.len()));
            for runnable in list {
                children.push(self.render_row(runnable, window, cx));
            }
        }
        if !problems.is_empty() && self.tab == Tab::All && query.trim().is_empty() {
            children.push(problem_header(problems.len()));
            for (ix, problem) in problems.iter().enumerate() {
                children.push(problem_row(ix, problem, cx));
            }
        }
        if empty {
            children.push(self.empty_state(&state, &query, cx));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header_row())
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

    fn render_row(&self, runnable: &Runnable, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
        let name = row()
            .gap(px(SPACE_1_5))
            .min_w_0()
            .child(div().flex_none().type_style(BODY_STRONG).child(runnable.name.clone()))
            .child(
                div()
                    .flex_none()
                    .type_style(LABEL)
                    .text_color(if spawned { SPAWN_TEXT } else { TEXT_MUTED })
                    .child(kind_label),
            )
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
                    .child(truncate_lh(runnable.description.clone(), LABEL.line).type_style(LABEL).text_color(TEXT_SECONDARY)),
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
            .child(cell(STATE_W).child(state_cell(&mark)))
            .child(cell(ACTIVITY_W).type_style(LABEL).text_color(TEXT_MUTED).child(truncate(meta)))
            .child(cell(USES_W).child(uses))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.focus);
                this.select(Some(reference.clone()), cx);
            }))
            .probe(id)
            .into_any_element()
    }

    fn empty_state(&self, state: &LoadState, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let (title, detail): (SharedString, SharedString) = match state {
            LoadState::Loading => ("Loading the library…".into(), "".into()),
            LoadState::Failed(why) => ("Could not read the library".into(), why.clone()),
            LoadState::Ready if !query.trim().is_empty() => {
                (format!("Nothing matches “{}”", query.trim()).into(), "Try a name, a model or a runtime.".into())
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
        col()
            .items_center()
            .gap(px(SPACE_2))
            .py(px(SPACE_8 * 2.))
            .child(div().type_style(BODY_STRONG).text_color(if failed { DANGER } else { TEXT }).child(title))
            .when(!detail.is_empty(), |d| d.child(div().type_style(LABEL).text_color(TEXT_MUTED).child(detail)))
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

/// A fixed-width table cell.
fn cell(width: f32) -> gpui::Div {
    div().flex().items_center().flex_none().w(px(width)).min_w_0().h_full().px(px(SPACE_3)).overflow_hidden()
}

fn header_row() -> impl IntoElement {
    row()
        .flex_none()
        .h(px(TABLE_HEAD_HEIGHT))
        .border_b_1()
        .border_color(LINE)
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(div().flex_1().min_w_0().pl(px(SPACE_4)).child("Name"))
        .child(cell(STATE_W).child("State"))
        .child(cell(ACTIVITY_W).child("Activity"))
        .child(cell(USES_W).child("Used from"))
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
fn problem_row(ix: usize, problem: &LoadProblem, cx: &App) -> AnyElement {
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
        .child(cell(STATE_W).child(state_cell(&rows::StateMark { dot: Dot::Error, text: "load problem".into(), tone: Tone::Danger })))
        .child(cell(ACTIVITY_W + USES_W).type_style(LABEL).text_color(TEXT_MUTED).font_family(FONT_MONO).child(truncate(path)))
        .debug_selector(move || format!("agents-problem-{ix}"))
        .probe(format!("agents-problem-{ix}"))
        .into_any_element()
}

/// The notice over the list when files of the library did not load.
fn problems_notice(problems: &[LoadProblem], cx: &App) -> AnyElement {
    let title = if problems.len() == 1 {
        "1 file in the library could not be read".to_string()
    } else {
        format!("{} files in the library could not be read", problems.len())
    };
    let lines = problems.iter().take(3).map(|p| {
        row()
            .gap(px(SPACE_2))
            .min_w_0()
            .type_style(LABEL)
            .child(div().flex_none().font_family(FONT_MONO).text_color(TEXT).child(hub::display_path(&p.path, cx)))
            .child(div().flex_1().min_w_0().text_color(TEXT_SECONDARY).child(truncate(p.message.clone())))
    });
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
                        .gap(px(SPACE_0_5))
                        .child(div().type_style(BODY_STRONG).child(title))
                        .children(lines)
                        .child(Note::new("Fix the file and the library reloads it; nothing else is affected.")),
                ),
        )
        .debug_selector(|| "agents-problems".into())
        .probe("agents-problems")
        .into_any_element()
}
