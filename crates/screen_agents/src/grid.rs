//! Kind tabs and the scrollable card grid.

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, canvas, div, prelude::*, px,
};
use ui::theme::*;
use ui::{TabItem, Tabs, status_dot};

use crate::AgentsScreen;
use crate::looks::{chip, looks, tile};
use crate::model::{Entry, Filter, State};

/// `grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 12px`.
const CARD_MIN: f32 = 250.;
const GRID_GAP: f32 = 12.;
const GRID_PAD_X: f32 = SPACE_6;

impl AgentsScreen {
    /// Tabs with at least one entry (All is always shown).
    fn visible_tabs(&self) -> Vec<(Filter, usize)> {
        Filter::TABS
            .iter()
            .map(|&f| (f, self.list.iter().filter(|e| f.matches(e)).count()))
            .filter(|&(f, n)| f == Filter::All || n > 0)
            .collect()
    }

    pub(crate) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.visible_tabs();
        let items: Vec<TabItem> = tabs
            .iter()
            .map(|(f, n)| TabItem::new(f.label()).count(n.to_string()))
            .collect();
        let selected = tabs
            .iter()
            .position(|(f, _)| *f == self.filter)
            .unwrap_or(0);
        let filters: Vec<Filter> = tabs.into_iter().map(|(f, _)| f).collect();
        let this = cx.entity().downgrade();
        div()
            .flex_none()
            .px(px(SPACE_6))
            .border_b_1()
            .border_color(LINE)
            .child(
                Tabs::new("agents-tabs", items, selected).on_select(move |i, _, cx| {
                    if let Some(f) = filters.get(i).copied() {
                        this.update(cx, |this, cx| this.set_filter(f, cx)).ok();
                    }
                }),
            )
    }

    pub(crate) fn render_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.grid_width;
        let this = cx.entity().downgrade();
        let cols = if measured <= 0. {
            2
        } else {
            let inner = measured - 2. * GRID_PAD_X;
            (((inner + GRID_GAP) / (CARD_MIN + GRID_GAP)).floor() as u16).max(1)
        };

        let cards: Vec<AnyElement> = self
            .list
            .iter()
            .enumerate()
            .filter(|(_, e)| self.filter.matches(e))
            .map(|(i, e)| self.render_card(i, e, cx).into_any_element())
            .collect();

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            // Measure the area so the column count follows `auto-fill, minmax(250px, 1fr)`.
            // Notifying is ignored while the window is drawing, so the update is deferred to
            // after this frame; it only fires when the width actually changed.
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        if (width - measured).abs() > 0.5 {
                            window.defer(cx, move |_, cx| {
                                this.update(cx, |this, cx| {
                                    if (this.grid_width - width).abs() > 0.5 {
                                        this.grid_width = width;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("agents-grid-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .pt(px(SPACE_4))
                    .px(px(GRID_PAD_X))
                    .pb(px(SPACE_6))
                    .child(
                        div()
                            .w_full()
                            .grid()
                            .grid_cols(cols)
                            .gap(px(GRID_GAP))
                            .children(cards),
                    ),
            )
    }

    fn render_card(&self, ix: usize, e: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.selected().is_some_and(|s| s.id == e.id);
        let created = e.created();
        let l = looks(e);
        let dot = if created && e.state == State::Running {
            ui::Status::Spawn
        } else {
            e.state.dot()
        };
        let border = if on {
            ACCENT_RING
        } else if created {
            SPAWN_LINE
        } else {
            LINE
        };
        let id = e.id.clone();

        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .child(tile(l.icon, 28., RADIUS_MD, 14., l.tile_bg, l.tile_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(l.name_color)
                            .child(e.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(l.kind_color)
                            .child(e.kind.label()),
                    ),
            )
            .child(status_dot(dot, false));

        let status = div()
            .flex()
            .gap_x(px(10.))
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .child(
                div()
                    .flex_none()
                    .text_color(e.state.color())
                    .child(e.status.clone()),
            )
            .child(div().min_w_0().truncate().child(e.meta.clone()));

        let chips = div()
            .flex()
            .gap(px(4.))
            .overflow_hidden()
            .children(e.chips.iter().map(|c| chip(c.clone(), false)));

        div()
            .id(("agents-card", ix))
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(12.))
            .px(px(14.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .when(created, |d| d.border_dashed())
            .border_color(border)
            .when(on, |d| d.shadow(selected_glow()))
            .bg(SURFACE)
            .cursor_pointer()
            .when(!on, |d| d.hover(|s| s.border_color(LINE_STRONG)))
            .child(head)
            .child(
                div()
                    .h(px(36.))
                    .overflow_hidden()
                    .line_clamp(2)
                    .text_ellipsis()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_SECONDARY)
                    .child(e.desc.clone()),
            )
            .child(status)
            .child(chips)
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
    }
}
