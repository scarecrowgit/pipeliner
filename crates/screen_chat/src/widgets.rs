//! Small building blocks shared by the transcript, composer and panel.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, ElementId, FontWeight, Hsla, IntoElement, SharedString,
    StyledText, TextRun, Transformation, div, font, percentage, prelude::*, px,
};
use ui::{Status, status_dot, theme::*};

use crate::todo::TodoStatus;

/// One run of inline text: sans or mono, with its own color/weight.
pub struct Seg {
    text: SharedString,
    mono: bool,
    color: Hsla,
    weight: FontWeight,
}

impl Seg {
    pub fn text(text: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            text: text.into(),
            mono: false,
            color,
            weight: FontWeight::NORMAL,
        }
    }
    pub fn mono(text: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            text: text.into(),
            mono: true,
            color,
            weight: FontWeight::NORMAL,
        }
    }
}

/// Wrapping paragraph mixing sans and mono runs (e.g. an inline `@file` mention).
/// The font size is the inherited one for every run.
pub fn rich(segs: Vec<Seg>) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(segs.len());
    for s in segs.into_iter().filter(|s| !s.text.is_empty()) {
        text.push_str(&s.text);
        let mut f = font(if s.mono { FONT_MONO } else { FONT_SANS });
        f.weight = s.weight;
        runs.push(TextRun {
            len: s.text.len(),
            font: f,
            color: s.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    StyledText::new(text).with_runs(runs)
}

/// User text with `@mentions` highlighted in mono accent.
pub fn with_mentions(text: &str, color: Hsla) -> StyledText {
    let mut segs = Vec::new();
    let mut plain = String::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            plain.push(' ');
        }
        if word.len() > 1 && word.starts_with('@') {
            segs.push(Seg::text(std::mem::take(&mut plain), color));
            segs.push(Seg::mono(word.to_string(), ACCENT_TEXT));
        } else {
            plain.push_str(word);
        }
    }
    segs.push(Seg::text(plain, color));
    rich(segs)
}

/// User text laid out word by word in wrapping rows (mentions in mono accent), each word
/// selectable. A single `StyledText` inside the shrink-to-fit bubble is measured at its
/// one-line width by gpui (wrong height or wrong wrap width); per-word flex rows let the
/// layout engine wrap and size it.
pub fn user_text(text: &str, color: Hsla) -> gpui::Div {
    let mut col = div().flex().flex_col().min_w_0();
    for (li, line) in text.split('\n').enumerate() {
        let mut row = div().flex().flex_wrap().min_w_0().min_h(px(20.));
        let words: Vec<&str> = line.split(' ').collect();
        let n = words.len();
        for (wi, word) in words.into_iter().enumerate() {
            let piece = if wi + 1 < n { format!("{word} ") } else { word.to_string() };
            if piece.is_empty() {
                continue;
            }
            let joiner = match (li, wi) {
                (0, 0) => "\n\n",
                (_, 0) => "\n",
                _ => "",
            };
            row = row.child(
                div()
                    .flex_none()
                    .child(ui::selectable::text(with_mentions(&piece, color), joiner)),
            );
        }
        col = col.child(row);
    }
    col
}

/// Bordered surface block (tool call groups, todowrite, edit).
pub fn block() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .bg(SURFACE)
        .overflow_hidden()
}

/// Tool-call row: `tool  arg ………… result`.
pub fn tool_row(
    tool: impl Into<SharedString>,
    arg: impl Into<SharedString>,
    res: impl IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(36.))
        .px(px(12.))
        .py(px(6.))
        .border_b_1()
        .border_color(LINE_SUBTLE)
        .text_size(px(12.))
        .child(tool_name(tool))
        .child(
            ellipsis_cell(arg)
                .font_family(FONT_MONO)
                .text_color(TEXT_SECONDARY),
        )
        .child(res)
}

/// 56px mono muted tool name column.
pub fn tool_name(tool: impl Into<SharedString>) -> gpui::Div {
    div()
        .w(px(56.))
        .flex_none()
        .font_family(FONT_MONO)
        .text_color(TEXT_MUTED)
        .child(tool.into())
}

/// Right-hand meta of a tool row, in the given color.
pub fn row_meta(text: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div()
        .flex_none()
        .max_w(px(280.))
        .truncate()
        .text_color(color)
        .whitespace_nowrap()
        .child(text.into())
}

/// Rotating loader icon.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: Hsla) -> impl IntoElement {
    ui::icon("replay", size, color).with_animation(
        id,
        Animation::new(Duration::from_millis(1100)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

/// Spinner + muted label ("running…").
pub fn running_meta(id: impl Into<ElementId>, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .text_color(TEXT_MUTED)
        .child(spinner(id, 12., TEXT_MUTED))
        .child(text.into())
}

/// `Badge  mono-model` line above an assistant message.
pub fn agent_line(badge: impl IntoElement, model: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(TEXT_MUTED)
        .child(badge)
        .child(div().font_family(FONT_MONO).child(model.into()))
}

/// Assistant paragraph (secondary text, wraps).
pub fn para(text: impl IntoElement) -> gpui::Div {
    div().text_color(TEXT_SECONDARY).child(text)
}

/// Todo state -> dot status.
pub fn todo_status(state: TodoStatus) -> Status {
    match state {
        TodoStatus::Completed => Status::Live,
        TodoStatus::InProgress => Status::Running,
        TodoStatus::Pending => Status::Draft,
    }
}

/// Rounded square tile with an icon (agent kinds, pipeline/orchestrator headers).
pub fn tile(
    icon_name: &str,
    size: f32,
    radius: f32,
    icon_size: f32,
    bg: Hsla,
    fg: Hsla,
) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(radius))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(ui::icon(icon_name, icon_size, fg))
}

/// User message bubble, right-aligned, max 85% wide.
/// Right-aligned user message. `wide` = the text will need more than one line: the bubble
/// then gets a definite width, because gpui sizes a shrink-to-fit box's text at its
/// single-line width and the wrapped lines would overflow the bubble.
pub fn user_bubble(content: impl IntoElement, wide: bool) -> gpui::Div {
    div().flex().justify_end().pb(px(12.)).child(
        div()
            .flex()
            .flex_col()
            .when(wide, |d| d.w(gpui::relative(0.85)))
            .when(!wide, |d| d.max_w(gpui::relative(0.85)))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE)
            .child(content),
    )
}

/// Section in the right panel: 16/20 padding, column, bottom rule.
pub fn panel_section(gap: f32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .px(px(20.))
        .py(px(16.))
        .border_b_1()
        .border_color(LINE)
}

/// `Title …… meta` header line of a panel section.
pub fn panel_head(title: &'static str, meta: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .justify_between()
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(meta.into()),
        )
}

/// Status row in the panel: dot · mono name ……… meta.
pub fn service_row(
    status: Status,
    name: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    meta_color: Hsla,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .child(status_dot(status, true))
        .child(
            ellipsis_cell(name)
                .font_family(FONT_MONO)
                .text_color(TEXT_SECONDARY),
        )
        .child(div().flex_none().text_color(meta_color).child(meta.into()))
}

/// Single-line cell that fills the remaining row width and ends in `…` when too long.
///
/// gpui measures a `nowrap` text once and caches the size, so a flex child measured
/// first under max-content never truncates. Laying the text out in an absolutely
/// positioned box means it is only measured once the cell's width is definite.
pub fn ellipsis_cell(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(LINE_BODY))
        .child(div().absolute().inset_0().truncate().child(text.into()))
}

/// Provider pill: `● GitHub` (18px, rounded, line-strong border, 11px).
pub fn prov_pill(label: impl Into<SharedString>, status: Status) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(18.))
        .px(px(6.))
        .rounded_full()
        .border_1()
        .border_color(LINE_STRONG)
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(TEXT_SECONDARY)
        .whitespace_nowrap()
        .child(status_dot(status, true))
        .child(label.into())
}
