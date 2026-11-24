//! "Working now": every run, chat turn and orchestrator that is going on, with its state.

use gpui::{AnyElement, Context, ElementId, InteractiveElement, IntoElement, SharedString, Window, div, prelude::*, px, relative};
use hub::api::{RunnableKind, Working, WorkingState};
use hub::Route;
use ui::motion;
use ui::prelude::*;

use crate::blocks::{Block, empty_view, error_view, footer_link, probed, skeleton};
use crate::view::HomeView;

/// How a column takes its width.
#[derive(Clone, Copy)]
enum Width {
    Fixed(f32),
    Flex(f32),
}

/// `Who, Working on, Runtime, Time, Cost, State`.
const COLUMNS: [(&str, Width, bool); 6] = [
    ("Who", Width::Flex(1.07), false),
    ("Working on", Width::Flex(1.), false),
    ("Runtime", Width::Fixed(92.), false),
    ("Time", Width::Fixed(48.), true),
    ("Cost", Width::Fixed(60.), true),
    ("State", Width::Fixed(164.), false),
];

/// One cell of the table: padded like `.tbl td`, sized by its column.
fn cell(ix: usize, content: impl IntoElement) -> gpui::Div {
    let (_, width, right) = COLUMNS[ix];
    let mut cell = div()
        .flex()
        .items_center()
        .h_full()
        .min_w_0()
        .overflow_hidden()
        .pl(px(if ix == 0 { SPACE_4 } else { SPACE_3 }))
        .pr(px(if ix + 1 == COLUMNS.len() { SPACE_4 } else { SPACE_3 }));
    match width {
        Width::Fixed(px_) => cell = cell.w(px(px_)).flex_none(),
        Width::Flex(ratio) => {
            let style = gpui::Styled::style(&mut cell);
            style.flex_grow = Some(ratio);
            style.flex_shrink = Some(1.);
            style.flex_basis = Some(relative(0.).into());
        }
    }
    if right {
        cell = cell.justify_end();
    }
    cell.child(content)
}

/// The icon of the tile in front of a name.
fn mark(kind: RunnableKind, spawned: bool) -> IconName {
    match (spawned, kind) {
        (true, _) => IconName::Spawn,
        (false, RunnableKind::Orchestrator) => IconName::Orchestrator,
        (false, RunnableKind::Pipeline) => IconName::Pipelines,
        (false, RunnableKind::Agent) => IconName::Agent,
    }
}

/// How long something has been going: `31m`, `3h`, then days.
fn elapsed(now: i64, since: i64, offset: i32) -> String {
    let secs = now.saturating_sub(since) / 1000;
    if secs < 24 * 3600 {
        base::time::ago(now, since, offset)
    } else {
        base::fmt::duration_words(secs as u64)
    }
}

impl HomeView {
    pub(crate) fn render_working(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = self.now();
        let offset = base::time::utc_offset(now);
        let state = self.state(Some(self.ops.read(cx).working()));
        let mut header = CardHeader::new("Working now");
        let body = match state {
            Block::Ready(rows) if rows.is_empty() => {
                empty_view("home-working-empty", "Nothing is working right now", "Runs and chats that are going show up here.")
            }
            Block::Ready(rows) => {
                header = header.meta(summary(&rows));
                col()
                    .child(head_row())
                    .children(rows.iter().enumerate().map(|(ix, working)| {
                        self.working_row(ix, working, ix + 1 == rows.len(), now, offset)
                    }))
                    .into_any_element()
            }
            Block::Loading => col()
                .child(head_row())
                .children((0..3).map(|ix| {
                    row()
                        .flex_none()
                        .h(px(TABLE_ROW_HEIGHT))
                        .px(px(SPACE_4))
                        .gap(px(SPACE_3))
                        .border_b_1()
                        .border_color(LINE_SUBTLE)
                        .child(skeleton(24., 24.).rounded(px(RADIUS_MD)))
                        .child(col().gap(px(SPACE_1)).child(skeleton(140. - 10. * ix as f32, 12.)).child(skeleton(90., 10.)))
                }))
                .into_any_element(),
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                error_view("what is working", message, "working", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                })
            }
        };
        let total = self.catalog.read(cx).runnables().iter().filter(|runnable| !runnable.archived).count();
        let label = if total > 0 {
            format!("All agents & pipelines · {}", base::fmt::count(total as u64))
        } else {
            "All agents & pipelines".to_string()
        };
        let footer = CardFooter::new().child(probed(
            footer_link("home-open-agents", label, window, cx)
                .on_click(|_, _, cx| hub::navigate(Route::Agents { runnable: None }, cx)),
            "home-open-agents",
        ));
        Card::new().flush().header(header).footer(footer).child(body).into_any_element()
    }

    fn working_row(&self, ix: usize, working: &Working, last: bool, now: i64, offset: i32) -> AnyElement {
        let id = ElementId::from(("home-working", ix));
        let target = working.target.clone();
        let (tone, kind_color) = if working.spawned { (TileTone::Spawn, SPAWN_TEXT) } else { (TileTone::Neutral, TEXT_MUTED) };
        let who = row()
            .gap(px(SPACE_2_5))
            .child(Tile::new(mark(working.runnable_kind, working.spawned)).tone(tone))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex().type_style(BODY_STRONG).text_color(TEXT).child(truncate(working.name.clone())))
                    .child(
                        div()
                            .flex()
                            .type_style(LABEL)
                            .text_color(kind_color)
                            .child(truncate_lh(working.kind.clone(), LABEL.line)),
                    ),
            );
        let task = col()
            .flex_1()
            .min_w_0()
            .child(div().flex().type_style(BODY).text_color(TEXT_SECONDARY).child(truncate(working.task.clone())))
            .child(
                div()
                    .flex()
                    .type_style(MONO_SM)
                    .text_color(TEXT_MUTED)
                    .child(truncate_lh(working.key.clone(), MONO_SM.line)),
            );
        let el = motion::fade_row(div().id(id.clone()), id, false, SURFACE_RAISED)
            .flex()
            .flex_none()
            .items_center()
            .h(px(TABLE_ROW_HEIGHT))
            .when(!last, |d| d.border_b_1().border_color(LINE_SUBTLE))
            .cursor_pointer()
            .child(cell(0, who))
            .child(cell(1, task))
            .child(cell(2, div().type_style(MONO).text_color(TEXT_MUTED).child(working.runtime.clone())))
            .child(cell(
                3,
                div().type_style(MONO).text_color(TEXT_MUTED).tabular().child(elapsed(now, working.since, offset)),
            ))
            .child(cell(
                4,
                div().type_style(MONO).text_color(TEXT).tabular().child(base::fmt::cost(working.cost_usd)),
            ))
            .child(cell(5, state(ix, &working.state)))
            .on_click(move |_, _, cx| hub::open(target.clone(), cx));
        probed(el, format!("home-working-{ix}")).into_any_element()
    }
}

/// The header of the table.
fn head_row() -> gpui::Div {
    row()
        .flex_none()
        .h(px(TABLE_HEAD_HEIGHT))
        .border_b_1()
        .border_color(LINE)
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .whitespace_nowrap()
        .children(COLUMNS.iter().enumerate().map(|(ix, (header, _, _))| cell(ix, div().child(*header))))
}

/// The state cell of row `ix`: a dot, then progress or the question.
fn state(ix: usize, state: &WorkingState) -> AnyElement {
    match state {
        WorkingState::Running { percent: Some(percent) } => row()
            .gap(px(SPACE_2))
            .child(StatusDot::new(Status::Running))
            .child(Progress::new(*percent as f32 / 100.).tone(ProgressTone::Info).width(72.))
            .child(div().type_style(MONO).text_color(TEXT_MUTED).tabular().child(format!("{percent}%")))
            .into_any_element(),
        WorkingState::Running { percent: None } => row()
            .gap(px(SPACE_2))
            .child(StatusDot::new(Status::Running))
            .child(div().type_style(BODY).text_color(TEXT_MUTED).child("running"))
            .into_any_element(),
        // The question is cut to the cell: it never runs into the cell's padding or past the card.
        WorkingState::Waiting { text } => row()
            .flex_1()
            .gap(px(SPACE_2))
            .child(StatusDot::new(Status::Waiting))
            .child(probed(
                div().flex().flex_1().min_w_0().type_style(BODY).text_color(WARNING).child(truncate(text.clone())),
                format!("home-working-question-{ix}"),
            ))
            .into_any_element(),
    }
}

/// "1 orchestrator · 2 agents · 1 pipeline · 2 created by agents" as the header's meta.
fn summary(rows: &[Working]) -> AnyElement {
    let count = |kind| rows.iter().filter(|w| !w.spawned && w.runnable_kind == kind).count() as u64;
    let spawned = rows.iter().filter(|w| w.spawned).count() as u64;
    let mut parts: Vec<SharedString> = Vec::new();
    for (kind, one, many) in [
        (RunnableKind::Orchestrator, "orchestrator", "orchestrators"),
        (RunnableKind::Agent, "agent", "agents"),
        (RunnableKind::Pipeline, "pipeline", "pipelines"),
    ] {
        if count(kind) > 0 {
            parts.push(base::fmt::plural(count(kind), one, many).into());
        }
    }
    row()
        .gap(px(SPACE_3))
        .children(parts.into_iter().map(|part| div().child(part)))
        .when(spawned > 0, |d| {
            d.child(
                row()
                    .gap(px(SPACE_1))
                    .text_color(SPAWN_TEXT)
                    .child(StatusDot::new(Status::Spawn).small())
                    .child(format!("{spawned} created by agents")),
            )
        })
        .into_any_element()
}
