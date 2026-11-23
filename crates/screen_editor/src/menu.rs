//! Popup menus (pipeline switcher, "New pipeline", "Add step", inspector dropdowns): a list of
//! items anchored below their trigger. The screen draws a click-away backdrop while one is
//! open.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, ElementId, FontWeight, Hsla, IntoElement, SharedString, Window, anchored,
    deferred, div, point, prelude::*, px,
};
use ui::{icon, select_box, theme::*};

use crate::{Menu, PipelineEditorScreen};

type Action = Rc<dyn Fn(&mut PipelineEditorScreen, &mut Window, &mut Context<PipelineEditorScreen>)>;

pub(crate) struct Item {
    label: SharedString,
    detail: Option<SharedString>,
    icon: Option<(&'static str, Hsla, Hsla)>,
    trailing: Option<SharedString>,
    checked: bool,
    /// A section heading, not clickable.
    heading: bool,
    action: Option<Action>,
}

impl Item {
    pub fn new(
        label: impl Into<SharedString>,
        action: impl Fn(&mut PipelineEditorScreen, &mut Window, &mut Context<PipelineEditorScreen>) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            detail: None,
            icon: None,
            trailing: None,
            checked: false,
            heading: false,
            action: Some(Rc::new(action)),
        }
    }
    pub fn heading(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            icon: None,
            trailing: None,
            checked: false,
            heading: true,
            action: None,
        }
    }
    /// Muted line under the label.
    pub fn detail(mut self, d: impl Into<SharedString>) -> Self {
        self.detail = Some(d.into());
        self
    }
    /// Icon in a small tile (icon, tile background, icon color).
    pub fn icon(mut self, name: &'static str, bg: Hsla, fg: Hsla) -> Self {
        self.icon = Some((name, bg, fg));
        self
    }
    pub fn trailing(mut self, t: impl Into<SharedString>) -> Self {
        self.trailing = Some(t.into());
        self
    }
    pub fn checked(mut self, c: bool) -> Self {
        self.checked = c;
        self
    }
}

impl PipelineEditorScreen {
    /// The open menu's list, anchored below the element it follows in a column.
    pub(crate) fn popup(&self, menu: Menu, width: f32, items: Vec<Item>, cx: &mut Context<Self>) -> AnyElement {
        let empty = items.is_empty();
        let list = div()
            .id(("menu", menu as usize))
            .w(px(width))
            .max_h(px(560.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(px(4.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_LG))
            .shadow(popover_shadow())
            .occlude()
            .when(empty, |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child("Nothing to add"),
                )
            })
            .children(items.into_iter().enumerate().map(|(i, it)| item_el(i, it, cx)));
        div()
            .relative()
            .h_0()
            .child(
                deferred(
                    anchored()
                        .offset(point(px(0.), px(4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(ui::motion::pop_in(("menu-in", menu as usize), ui::motion::Pop::Below, list)),
                )
                .with_priority(2),
            )
            .into_any_element()
    }

    /// Select box that opens `menu` with `items`.
    pub(crate) fn select_field(
        &self,
        menu: Menu,
        value: impl Into<SharedString>,
        mono: bool,
        width: f32,
        items: impl FnOnce() -> Vec<Item>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu == Some(menu);
        div()
            .flex()
            .flex_col()
            .child(
                select_box(ElementId::from(("select", menu as usize)), value)
                    .h(px(CONTROL_HEIGHT_SM + 4.))
                    .px(px(10.))
                    .when(open, |d| d.border_color(ACCENT_STRONG))
                    .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(menu, cx))),
            )
            .when(open, |d| d.child(self.popup(menu, width, items(), cx)))
            .into_any_element()
    }
}

fn item_el(i: usize, it: Item, cx: &mut Context<PipelineEditorScreen>) -> AnyElement {
    if it.heading {
        return div()
            .px(px(8.))
            .pt(px(if i == 0 { 4. } else { 10. }))
            .pb(px(4.))
            .text_size(px(11.))
            .text_color(TEXT_MUTED)
            .child(it.label)
            .into_any_element();
    }
    let action = it.action.clone();
    let tall = it.detail.is_some();
    div()
        .id(("menu-item", i))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .min_h(px(32.))
        .px(px(8.))
        .py(px(if tall { 6. } else { 0. }))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .text_color(if it.checked { TEXT } else { TEXT_SECONDARY })
        .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT))
        .when_some(it.icon, |d, (name, bg, fg)| {
            d.child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(name, 13., fg)),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .when(tall, |d| d.font_weight(FontWeight::MEDIUM).text_color(TEXT))
                        .child(it.label),
                )
                .when_some(it.detail, |d, t| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .child(t),
                    )
                }),
        )
        .when_some(it.trailing, |d, t| {
            d.child(div().flex_none().text_size(px(12.)).text_color(TEXT_MUTED).child(t))
        })
        .when(it.checked, |d| d.child(icon("check", 12., ACCENT_TEXT)))
        .when_some(action, |d, action| {
            d.on_click(cx.listener(move |this, _, window, cx| {
                this.menu = None;
                action(this, window, cx);
                cx.notify();
            }))
        })
        .into_any_element()
}
