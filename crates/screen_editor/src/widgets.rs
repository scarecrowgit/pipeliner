//! Small presentational pieces of the inspector and run panel that `ui` does not have.

use gpui::{ElementId, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;

/// Inspector section: column, 16/20 padding, bottom rule.
pub fn section(gap: f32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .px(px(20.))
        .py(px(16.))
        .border_b_1()
        .border_color(LINE)
}

/// Field label above a control (12/18 muted, 6px below).
pub fn field_label(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(TEXT_MUTED)
        .mb(px(6.))
        .child(text.into())
}

/// Muted 12/18 help paragraph.
pub fn help(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

/// Secondary-colored 13px paragraph.
pub fn para(text: impl Into<SharedString>) -> gpui::Div {
    div().text_color(TEXT_SECONDARY).child(text.into())
}

/// 500-weight heading line inside a section.
pub fn strong(text: impl Into<SharedString>) -> gpui::Div {
    div().font_weight(FontWeight::MEDIUM).child(text.into())
}

pub fn chip_row() -> gpui::Div {
    div().flex().flex_wrap().items_center().gap(px(6.))
}

/// `label ……… value` row; value in 12px mono unless `mono` is false.
pub fn kv(label: impl Into<SharedString>, value: impl IntoElement, mono: bool) -> gpui::Div {
    div()
        .flex()
        .justify_between()
        .gap(px(12.))
        .child(div().flex_none().text_color(TEXT_MUTED).child(label.into()))
        .child(
            div()
                .min_w_0()
                .truncate()
                .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
                .child(value),
        )
}

/// Two equal columns, bottom-aligned (`grid-template-columns: repeat(2, 1fr)`).
pub fn two_cols(a: impl IntoElement, b: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_end()
        .gap(px(12.))
        .child(div().flex_1().min_w_0().child(a))
        .child(div().flex_1().min_w_0().child(b))
}

/// Link styled as a secondary button (`.pl-linkbtn`).
pub fn link_button(id: impl Into<ElementId>, height: f32) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .h(px(height))
        .px(px(if height < 40. { 12. } else { 14. }))
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(LINE_STRONG)
        .bg(SURFACE_RAISED)
        .text_color(TEXT)
        .font_weight(FontWeight::MEDIUM)
        .when(height < 40., |d| d.text_size(px(12.)))
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_HOVER))
}
