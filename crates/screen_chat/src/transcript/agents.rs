//! What the agent delegates and plans: sub-agents as rows of one spawn card ("Build started 2
//! sub-agents"), each opening its own session, and the todo list as of that point.

use gpui::{AnyElement, App, Context, ElementId, IntoElement, div, prelude::*, px};
use hub::model::{TodoItem, TodoStatus};
use hub::transcript::{SubagentBlock, SubagentState};
use hub::{Block, Route};
use ui::dev::Probe;
use ui::prelude::*;

use super::TranscriptView;
use super::rows::{GroupPlace, Place, group_card};
use super::texts::{Mode, Part, TextKey};

/// Size of a todo check box (`.check`).
const CHECK: f32 = 16.;
/// Lines of a sub-agent's summary shown in its row; the rest is in its session.
const SUMMARY_LINES: f32 = 3.;

/// "haiku-5.5 · 4 tool calls · 14s · $0.01": what a finished sub-agent used.
pub(crate) fn subagent_meta(model: Option<&str>, state: &SubagentState) -> String {
    let mut parts: Vec<String> = model.filter(|m| !m.is_empty()).map(str::to_string).into_iter().collect();
    if let SubagentState::Finished { tool_calls, secs, cost_usd, .. } = state {
        if *tool_calls > 0 {
            parts.push(base::fmt::plural(u64::from(*tool_calls), "tool call", "tool calls"));
        }
        if *secs > 0 {
            parts.push(base::fmt::duration(*secs));
        }
        if *cost_usd > 0.0 {
            parts.push(base::fmt::cost(*cost_usd));
        }
    }
    parts.join(" · ")
}

/// The line of a sub-agent's summary its row shows: the first one with text, without the
/// `#` of a heading (the row's type stays small).
pub(crate) fn summary_line(summary: &str) -> &str {
    let line = base::fmt::first_line(summary);
    match line.trim_start_matches('#') {
        rest if rest.len() < line.len() && (rest.is_empty() || rest.starts_with(' ')) => rest.trim(),
        _ => line,
    }
}

/// "depth 1 of 2", or "depth 1" when the spawning policy's limit is not known.
pub(crate) fn depth_words(sub: &SubagentBlock) -> String {
    let depth = sub.depth.max(1);
    match sub.max_depth {
        Some(max) if max > 0 => format!("depth {depth} of {max}"),
        _ => format!("depth {depth}"),
    }
}

/// "3 of 4 done".
pub(crate) fn todo_progress(items: &[TodoItem]) -> String {
    let done = items.iter().filter(|item| item.status == TodoStatus::Done).count();
    format!("{done} of {} done", items.len())
}

impl TranscriptView {
    /// One sub-agent: tile, name and task, what it used, the first line of its summary
    /// (selectable, up to three wrapped lines), its state and "Open session".
    pub(crate) fn subagent(
        &mut self,
        index: usize,
        block: &Block,
        sub: &SubagentBlock,
        place: &Place,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = place.group.unwrap_or(GroupPlace { first: true, last: true, size: 1 });
        let model = self.child_model(sub, cx);
        let meta = subagent_meta(model.as_deref(), &sub.state);
        let (state, summary): (AnyElement, Option<String>) = match &sub.state {
            SubagentState::Running => (
                row()
                    .flex_none()
                    .gap(px(SPACE_1))
                    .type_style(LABEL)
                    .text_color(INFO_TEXT)
                    .child(StatusDot::new(Status::Running).small())
                    .child("running")
                    .into_any_element(),
                None,
            ),
            SubagentState::Finished { ok, summary, .. } => (
                row()
                    .flex_none()
                    .gap(px(SPACE_1))
                    .type_style(LABEL)
                    .text_color(if *ok { SUCCESS } else { DANGER })
                    .child(icon(if *ok { IconName::Check } else { IconName::X }, 12., if *ok { SUCCESS } else { DANGER }))
                    .child(if *ok { "done" } else { "failed" })
                    .into_any_element(),
                Some(summary_line(summary).to_string()).filter(|s| !s.is_empty()),
            ),
        };
        let open = sub.session.clone().map(|session| {
            Button::new(ElementId::Name(format!("open-subagent-{}", block.id).into()), "Open session")
                .ghost()
                .small()
                .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: session.clone() }, cx))
                .probe(format!("subagent-{index}"))
        });
        let title = row()
            .gap(px(SPACE_1_5))
            .child(div().flex_none().type_style(BODY_STRONG).text_color(SPAWN_TEXT).child(sub.name.clone()))
            .child(truncate(sub.task.clone()).text_color(TEXT));
        let text = col()
            .flex_1()
            .min_w_0()
            .child(title)
            .when(!meta.is_empty(), |d| d.child(truncate_block(meta, LABEL.line).type_style(LABEL).text_color(TEXT_MUTED)))
            .children(summary.map(|summary| {
                let text = self.text_view(
                    TextKey::Block(block.id, Part::Summary),
                    &summary,
                    Mode::Markdown,
                    LABEL,
                    TEXT_SECONDARY,
                    cx,
                );
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(LABEL.line * SUMMARY_LINES))
                    .overflow_hidden()
                    .debug_selector(move || format!("subagent-summary-{index}"))
                    .child(text)
            }));
        let line = row()
            .gap(px(SPACE_2_5))
            .py(px(SPACE_2_5))
            .px(px(SPACE_3))
            .border_t_1()
            .border_color(SPAWN_LINE)
            .child(Tile::new(IconName::Spawn).tone(TileTone::Spawn))
            .child(text)
            .child(state)
            .children(open);
        let header = row()
            .flex_none()
            .h(px(LIST_ITEM_HEIGHT))
            .px(px(SPACE_3))
            .gap(px(SPACE_2))
            .bg(SPAWN_SOFT)
            .type_style(LABEL)
            .text_color(SPAWN_TEXT)
            .child(icon(IconName::Spawn, 12., SPAWN_TEXT))
            .child(format!(
                "{} started {} · {}",
                self.agent_name(cx),
                base::fmt::plural(group.size as u64, "sub-agent", "sub-agents"),
                depth_words(sub)
            ))
            .into_any_element();
        group_card(group, SPAWN_LINE, Some(header), line.into_any_element())
    }

    /// The short model name of a sub-agent's session, once the detail lists it.
    fn child_model(&self, sub: &SubagentBlock, cx: &App) -> Option<String> {
        let session = sub.session.as_ref()?;
        let detail = self.model.read(cx).detail()?;
        detail.children.iter().find(|child| &child.id == session).map(|child| child.model.short().to_string())
    }
}

/// The todo list card: `todowrite`, "3 of 4 done", one checked row per item.
pub(crate) fn todos(items: &[TodoItem]) -> AnyElement {
    let rows: Vec<AnyElement> = items
        .iter()
        .map(|item| {
            let (bg, border, mark) = match item.status {
                TodoStatus::Done => (ACCENT_STRONG, ACCENT_STRONG, Some(icon(IconName::Check, 10., ON_ACCENT))),
                TodoStatus::InProgress => (SURFACE_RAISED, INFO, None),
                TodoStatus::Pending => (SURFACE_RAISED, LINE_STRONG, None),
            };
            let check = div()
                .flex_none()
                .size(px(CHECK))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(border)
                .bg(bg)
                .children(mark);
            row()
                .h(px(LIST_ITEM_HEIGHT))
                .gap(px(SPACE_2))
                .child(check)
                .child(truncate(item.text.clone()).text_color(if item.status == TodoStatus::Done { TEXT_MUTED } else { TEXT }))
                .into_any_element()
        })
        .collect();
    col()
        .py(px(SPACE_2_5))
        .px(px(SPACE_3))
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .child(
            row()
                .mb(px(SPACE_1))
                .gap(px(SPACE_2))
                .child(div().type_style(MONO).text_color(TEXT_MUTED).child("todowrite"))
                .child(grow())
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(todo_progress(items))),
        )
        .children(rows)
        .probe("todos")
        .into_any_element()
}
