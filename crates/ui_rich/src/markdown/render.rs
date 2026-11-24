//! Blocks to elements. Walks the document in the same order as [`units`](super::rich::units)
//! so the n-th text element shows the n-th unit.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, Context, Hsla, IntoElement, Window, div, prelude::*, px};
use ui::assets::{IconName, icon};
use ui::components::Button;
use ui::layout::{col, grow, hr, row};
use ui::theme::*;

use super::element::{Shared, UnitElement};
use super::layout::{InlineLayout, Look};
use super::parse::{Align, Block, List, Table};
#[cfg(test)]
use super::rich::UnitKind;
use super::rich::{Unit, shown_marker};
use super::view::Markdown;

/// Narrowest and widest a table column gets before its text wraps.
const COL_MIN: f32 = 40.;
const COL_MAX: f32 = 360.;

pub(crate) struct Renderer<'a> {
    units: &'a [Unit],
    next: usize,
    shared: Rc<RefCell<Shared>>,
    focused: bool,
    color: Hsla,
    /// Type style of body text: paragraphs, list markers, table cells, verbatim text.
    body: TypeStyle,
    copied: Option<usize>,
    codes: usize,
    tables: usize,
}

impl<'a> Renderer<'a> {
    pub fn new(
        units: &'a [Unit],
        shared: Rc<RefCell<Shared>>,
        focused: bool,
        color: Hsla,
        body: TypeStyle,
        copied: Option<usize>,
    ) -> Self {
        Self {
            units,
            next: 0,
            shared,
            focused,
            color,
            body,
            copied,
            codes: 0,
            tables: 0,
        }
    }

    /// The element of the next unit, drawn with `look`.
    fn text(&mut self, look: Look) -> AnyElement {
        let ix = self.next;
        self.next += 1;
        match self.units.get(ix) {
            Some(u) => {
                UnitElement::new(ix, u.text.clone(), look, self.shared.clone(), self.focused)
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    /// The element of a view in verbatim mode: its one unit in the body style.
    pub fn verbatim(&mut self) -> AnyElement {
        self.text(Look::text(self.body, self.color))
    }

    pub fn block(
        &mut self,
        block: &Block,
        window: &mut Window,
        cx: &mut Context<Markdown>,
    ) -> AnyElement {
        match block {
            Block::Paragraph(_) => self.text(Look::text(self.body, self.color)),
            Block::Heading { level, .. } => {
                let t = match level {
                    1 => TITLE,
                    2 => TITLE_SM,
                    3 => HEADING,
                    _ => BODY_STRONG,
                };
                self.text(Look::text(t, TEXT))
            }
            Block::Code { lang, .. } => self.code(lang, cx),
            Block::Quote(blocks) => {
                let saved = self.color;
                self.color = TEXT_MUTED;
                let children: Vec<_> = blocks.iter().map(|b| self.block(b, window, cx)).collect();
                self.color = saved;
                col()
                    .w_full()
                    .flex_none()
                    .gap(px(SPACE_2))
                    .pl(px(SPACE_3))
                    .border_l_2()
                    .border_color(LINE_STRONG)
                    .children(children)
                    .into_any_element()
            }
            Block::List(list) => self.list(list, window, cx),
            Block::Table(t) => self.table(t, window),
            Block::Rule => hr().my(px(SPACE_1)).into_any_element(),
        }
    }

    fn list(&mut self, list: &List, window: &mut Window, cx: &mut Context<Markdown>) -> AnyElement {
        let tasks = list.items.iter().any(|i| i.task.is_some());
        let indent = match list.start {
            _ if tasks => 22.,
            Some(n) => {
                let last = n.saturating_add(list.items.len() as u64);
                8. * last.to_string().len() as f32 + 12.
            }
            None => 16.,
        };
        let (color, style) = (self.color, self.body);
        let mut items = Vec::with_capacity(list.items.len());
        for (i, item) in list.items.iter().enumerate() {
            let marker = match item.task {
                Some(done) => checkbox(done),
                None => div()
                    .h(px(style.line))
                    .type_style(style)
                    .text_color(TEXT_MUTED)
                    .child(shown_marker(list.start, i))
                    .into_any_element(),
            };
            let body: Vec<_> = item
                .blocks
                .iter()
                .map(|b| self.block(b, window, cx))
                .collect();
            // The marker hangs in the left padding; a flex row here would measure the
            // item's text at zero width inside nested lists.
            items.push(
                div()
                    .relative()
                    .w_full()
                    .flex_none()
                    .min_h(px(style.line))
                    .pl(px(indent))
                    .child(div().absolute().left_0().top_0().child(marker))
                    .child(
                        col()
                            .w_full()
                            .gap(px(SPACE_2))
                            .text_color(color)
                            .children(body),
                    ),
            );
        }
        col()
            .w_full()
            .flex_none()
            .gap(px(SPACE_1))
            .children(items)
            .into_any_element()
    }

    fn code(&mut self, lang: &str, cx: &mut Context<Markdown>) -> AnyElement {
        let unit = self.next;
        let nth = self.codes;
        self.codes += 1;
        let look = Look {
            wrap: false,
            fill: false,
            ..Look::text(MONO, TEXT_SECONDARY)
        };
        let text = self.text(look);
        let copied = self.copied == Some(nth);
        let label = if lang.is_empty() { "text" } else { lang };
        let button = Button::new(("md-copy", nth), if copied { "Copied" } else { "Copy" })
            .ghost()
            .small()
            .when(copied, |b| b.icon(IconName::Check))
            .on_click(
                cx.listener(move |this, _, window, cx| this.copy_code(unit, nth, window, cx)),
            );
        col()
            .w_full()
            .flex_none()
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .overflow_hidden()
            .child(
                row()
                    .h(px(CONTROL_HEIGHT_MD))
                    .pl(px(SPACE_3))
                    .pr(px(SPACE_0_5))
                    .border_b_1()
                    .border_color(LINE_SUBTLE)
                    .child(
                        div()
                            .type_style(MONO_SM)
                            .text_color(TEXT_MUTED)
                            .child(label.to_string()),
                    )
                    .child(grow())
                    .child(button),
            )
            .child(
                div()
                    .id(("md-code", nth))
                    .flex()
                    .w_full()
                    .overflow_x_scroll()
                    .px(px(SPACE_3))
                    .py(px(SPACE_2))
                    .child(text),
            )
            .into_any_element()
    }

    fn table(&mut self, t: &Table, window: &mut Window) -> AnyElement {
        let nth = self.tables;
        self.tables += 1;
        let cols = t.head.len();
        let head_look = |a: Align| Look {
            fill: a == Align::Left,
            ..Look::text(LABEL, TEXT_MUTED)
        };
        let body = self.body;
        let body_look = |a: Align, c: Hsla| Look {
            fill: a == Align::Left,
            ..Look::text(body, c)
        };
        // Column widths: the widest cell, measured unwrapped, within [COL_MIN, COL_MAX].
        let mut widths = vec![COL_MIN; cols];
        let rows = 1 + t.rows.len();
        for r in 0..rows {
            for (c, w) in widths.iter_mut().enumerate() {
                let Some(u) = self.units.get(self.next + r * cols + c) else {
                    continue;
                };
                let look = if r == 0 {
                    head_look(Align::Left)
                } else {
                    body_look(Align::Left, self.color)
                };
                let natural =
                    InlineLayout::build(u.text.clone(), &look, None, None, window).width();
                *w = w.max(natural.ceil().min(COL_MAX));
            }
        }
        let pad = SPACE_3;
        let total: f32 = widths.iter().map(|w| w + 2. * pad).sum();
        let mut out = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut cells = Vec::with_capacity(cols);
            for (c, w) in widths.iter().enumerate() {
                let a = t.align.get(c).copied().unwrap_or_default();
                let mut look = if r == 0 {
                    head_look(a)
                } else {
                    body_look(a, self.color)
                };
                look.max_width = Some(*w);
                let el = self.text(look);
                cells.push(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(w + 2. * pad))
                        .px(px(pad))
                        .py(px(SPACE_1_5))
                        .when(a == Align::Right, |d| d.justify_end())
                        .when(a == Align::Center, |d| d.justify_center())
                        .child(el),
                );
            }
            let last = r + 1 == rows;
            out.push(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .w(px(total))
                    .when(!last, |d| {
                        d.border_b_1()
                            .border_color(if r == 0 { LINE } else { LINE_SUBTLE })
                    })
                    .children(cells),
            );
        }
        div()
            .id(("md-table", nth))
            .flex()
            .w_full()
            .flex_none()
            .overflow_x_scroll()
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(LINE)
            .child(col().flex_none().w(px(total)).children(out))
            .into_any_element()
    }
}

/// The square of a task-list item: empty, or filled with a check.
fn checkbox(done: bool) -> AnyElement {
    div()
        .mt(px(3.))
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(if done { ACCENT_STRONG } else { LINE_STRONG })
        .bg(if done { ACCENT_STRONG } else { TRANSPARENT })
        .when(done, |d| d.child(icon(IconName::Check, 10., ON_ACCENT)))
        .into_any_element()
}

/// Whether `units` and the walk agree on what the unit at `ix` is (for debug checks).
#[cfg(test)]
pub(crate) fn is_code(units: &[Unit], ix: usize) -> bool {
    matches!(units.get(ix).map(|u| &u.kind), Some(UnitKind::Code { .. }))
}
