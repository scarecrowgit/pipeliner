//! Board view: horizontally scrolling status columns with draggable issue cards.

use gpui::{
    Context, Div, FontWeight, IntoElement, SharedString, Stateful, Window, div, prelude::*, px,
};
use ui::theme::*;
use ui::{Badge, icon, progress, status_dot};

use crate::IssuesScreen;
use crate::data::{Column, Issue};
use crate::widgets::{Ellipsis, person_avatar, runner_tile, small, spawn_pill};

/// Drag payload: the key of the card being moved.
#[derive(Clone)]
pub struct DraggedIssue {
    key: SharedString,
    title: SharedString,
}

/// Floating card shown under the cursor while dragging.
struct DragGhost(DraggedIssue);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(236.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(ACCENT_RING)
            .bg(SURFACE)
            .shadow(popover_shadow())
            .opacity(0.9)
            .font_family(FONT_SANS)
            .text_size(px(TEXT_BODY))
            .text_color(TEXT)
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(self.0.key.clone()),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(18.))
                    .child(self.0.title.clone()),
            )
    }
}

impl IssuesScreen {
    pub(crate) fn render_board(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("board-scroll")
            .size_full()
            .flex()
            .items_start()
            .overflow_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_start()
                    .gap(px(12.))
                    .pt(px(16.))
                    .px(px(24.))
                    .pb(px(24.))
                    .children(
                        Column::ALL
                            .into_iter()
                            .map(|c| self.render_column(c, cx))
                            .collect::<Vec<_>>(),
                    ),
            )
    }

    fn render_column(&self, column: Column, cx: &mut Context<Self>) -> Stateful<Div> {
        let cards: Vec<&Issue> = self.visible().filter(|i| i.status == column).collect();
        let waiting = cards.iter().filter(|i| i.needs_you()).count();

        div()
            .id(SharedString::from(format!("col-{}", column.name())))
            .w(px(252.))
            .flex_none()
            .min_h(px(640.))
            .p(px(8.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(gpui::transparent_black())
            .flex()
            .flex_col()
            .gap(px(8.))
            .drag_over::<DraggedIssue>(|s, _, _, _| {
                s.bg(ACCENT_GLOW).border_color(ACCENT_RING).border_dashed()
            })
            .on_drop(cx.listener(move |this, d: &DraggedIssue, window, cx| {
                this.drop_on(&d.key, column, window, cx)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(4.))
                    .child(status_dot(column.dot(), true))
                    .child(div().font_weight(FontWeight::MEDIUM).child(column.name()))
                    .child(
                        div()
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(cards.len().to_string())),
                    )
                    .child(div().flex_1())
                    .when(waiting > 0, |d| {
                        d.child(small(format!("{waiting} need you"), WARNING))
                    }),
            )
            .children(
                cards
                    .into_iter()
                    .map(|i| self.render_card(i, cx))
                    .collect::<Vec<_>>(),
            )
            .child(
                div()
                    .id(SharedString::from(format!("add-{}", column.name())))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(RADIUS_MD))
                    .text_color(TEXT_MUTED)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_RAISED).text_color(TEXT_SECONDARY))
                    .child(icon("plus", 12., TEXT_MUTED))
                    .child("Add issue")
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.add_issue(column, window, cx)),
                    ),
            )
    }

    fn render_card(&self, issue: &Issue, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.selected.as_ref() == Some(&issue.key);
        // `dragging` is only cleared lazily, so it counts only while a drag is active.
        let dragging = cx.has_active_drag() && self.dragging.as_ref() == Some(&issue.key);
        let runner = issue.runner();
        let key = issue.key.clone();
        let payload = DraggedIssue {
            key: issue.key.clone(),
            title: issue.title.clone(),
        };
        let weak = cx.entity().downgrade();

        div()
            .id(SharedString::from(format!("card-{}", issue.key)))
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(if selected { ACCENT_RING } else { LINE })
            .when(selected, |d| d.shadow(selected_glow()))
            .when(!selected, |d| d.hover(|s| s.border_color(LINE_STRONG)))
            .bg(SURFACE)
            .when(dragging, |d| d.opacity(0.4))
            .cursor_grab()
            .on_click(cx.listener(move |this, _, window, cx| this.select(key.clone(), window, cx)))
            .on_drag(payload, move |d, _, _, cx| {
                let key = d.key.clone();
                weak.update(cx, |this, cx| {
                    this.dragging = Some(key);
                    cx.notify();
                })
                .ok();
                cx.new(|_| DragGhost(d.clone()))
            })
            // Head: key · priority · owner · runner tile
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(issue.key.clone()),
                    )
                    .child(small(issue.prio.label(), issue.prio.color()))
                    .child(div().flex_1())
                    .when_some(issue.person, |d, p| d.child(person_avatar(p, 22., 10.)))
                    .when_some(runner, |d, r| d.child(runner_tile(r.kind))),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(18.))
                    .child(issue.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(Badge::new(issue.label.clone()))
                    .when_some(issue.created_by.clone(), |d, by| {
                        d.child(spawn_pill(format!("by {by}")))
                    }),
            )
            .when_some(runner, |d, r| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .w_full()
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(LINE_SUBTLE)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .min_w_0()
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(TEXT_MUTED)
                                        .child(r.kind.label()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .ellipsis()
                                        .text_color(TEXT_SECONDARY)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(r.name),
                                ),
                        )
                        .when_some(issue.run.as_ref(), |d, run| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .child(status_dot(run.state.dot(), true))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(run.state.color())
                                            .child(run.text.clone()),
                                    ),
                            )
                            .when(issue.is_running(), |d| {
                                d.child(progress(run.pct as f32 / 100., ACCENT))
                            })
                        }),
                )
            })
    }
}
