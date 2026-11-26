//! The left column: everything waiting for you, failed work from today, decided today.

use gpui::{Animation, AnimationExt as _};

use super::data::{AlsoRow, DecidedRow, Item, ItemKind, ReviewData};
use super::{ReviewScreen, LEAVE};
use crate::screens::prelude::*;

impl ReviewScreen {
    pub(super) fn render_queue(
        &self,
        data: &ReviewData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.current().map(|i| i.task.id.clone());
        let live = self.live().len();
        div()
            .id("review-queue")
            .w(px(300.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme::LINE)
            .p(theme::S3)
            .flex()
            .flex_col()
            .gap(theme::S1)
            .child(
                div()
                    .px(theme::S2)
                    .py(theme::S1)
                    .child(SectionHeader::new("Waiting for you").meta(live.to_string())),
            )
            .children(
                data.items
                    .iter()
                    .map(|item| self.queue_row(item, selected.as_ref() == Some(&item.task.id), cx)),
            )
            .when(data.items.is_empty(), |d| {
                d.child(
                    div()
                        .px(theme::S2)
                        .py(theme::S2)
                        .text_label()
                        .fg_muted()
                        .child("Nothing is waiting for you."),
                )
            })
            .when(!data.also.is_empty(), |d| {
                d.child(group_label(format!("Also needs you · {}", data.also.len())))
                    .children(data.also.iter().map(|a| also_row(a, cx)))
            })
            .child(group_label("Decided today"))
            .when(data.decided.is_empty(), |d| {
                d.child(
                    div()
                        .px(theme::S2)
                        .text_label()
                        .fg_muted()
                        .child("Nothing decided yet today."),
                )
            })
            .children(
                data.decided
                    .iter()
                    .enumerate()
                    .map(|(ix, r)| decided_row(ix, r)),
            )
            .when_some(data.median_secs, |d, s| {
                d.child(
                    div()
                        .px(theme::S2)
                        .pt(theme::S2)
                        .text_label()
                        .fg_accent()
                        .child(format!("Median time to decide {}", ui::fmt_duration(s))),
                )
            })
    }

    /// One waiting item. Rows that were just decided slide out; new rows slide in.
    fn queue_row(&self, item: &Item, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = item.task.id.clone();
        let leaving = self.leaving.contains(&id);
        let (icon_name, ref_text, sub) = match &item.run {
            Some(r) => {
                let mut reference = String::new();
                if let Some(e) = &r.external_ref {
                    reference.push_str(e);
                    reference.push(' ');
                }
                reference.push_str(&format!("#{}", r.number));
                let node = item.task.title.clone();
                let sub = if r.pipeline.is_empty() {
                    node
                } else {
                    format!("{} › {}", r.pipeline, node)
                };
                let icon = match item.kind {
                    ItemKind::ApprovePr => AppIcon::GitPullRequest,
                    ItemKind::Pick => AppIcon::GitFork,
                    _ => AppIcon::Inbox,
                };
                (icon, reference, sub)
            }
            None => (
                if item.kind == ItemKind::Plan {
                    AppIcon::Chat
                } else {
                    AppIcon::Inbox
                },
                item.task.kind.label().to_string(),
                item.task.title.clone(),
            ),
        };
        let row = div()
            .id(SharedString::from(format!("review-item-{id}")))
            .flex()
            .items_start()
            .gap(theme::S3)
            .p(theme::S3)
            .rounded(theme::RADIUS_LG)
            .border_1()
            .cursor_pointer()
            .when(active, |d| {
                d.bg(theme::SURFACE_RAISED).border_color(theme::LINE_STRONG)
            })
            .when(!active, |d| {
                d.border_color(theme::TRANSPARENT)
                    .hover(|s| s.bg(theme::SURFACE))
            })
            .child(
                ui::icon_tile(
                    icon_name,
                    if active {
                        theme::ACCENT_SOFT
                    } else {
                        theme::SURFACE_RAISED
                    },
                    if active {
                        theme::ACCENT_TEXT
                    } else {
                        theme::TEXT_MUTED
                    },
                )
                .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(theme::S2)
                            .text_mono_sm()
                            .fg_muted()
                            .child(div().truncate().child(ref_text))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme::WARNING)
                                    .child(ui::fmt_ago(item.task.created_at)),
                            ),
                    )
                    .child(div().text_body_strong().fg().child(item.kind_line.clone()))
                    .child(div().text_label().fg_muted().truncate().child(sub)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)));

        let wrapper = div().flex_none().overflow_hidden().child(row);
        if leaving {
            wrapper
                .with_animation(
                    SharedString::from(format!("review-row-out-{}", item.task.id)),
                    Animation::new(LEAVE).with_easing(theme::ease_in),
                    |d, t| {
                        d.opacity(1.0 - t)
                            .max_h(px(110.) * (1.0 - t))
                            .ml(px(-28.) * t)
                    },
                )
                .into_any_element()
        } else {
            wrapper
                .with_animation(
                    SharedString::from(format!("review-row-in-{}", item.task.id)),
                    Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                    |d, t| d.opacity(t).ml(px(-12.) * (1.0 - t)),
                )
                .into_any_element()
        }
    }
}

/// Small uppercase group title ("ALSO NEEDS YOU · 3").
fn group_label(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .px(theme::S2)
        .pt(theme::S5)
        .pb(theme::S1)
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .fg_muted()
        .child(text.to_uppercase())
}

/// A failed task from today with Retry / Open.
fn also_row(a: &AlsoRow, cx: &mut Context<ReviewScreen>) -> impl IntoElement {
    let (retry_id, open_id) = (a.task_id.clone(), a.task_id.clone());
    let sub = match (a.run_ref.is_empty(), a.error.is_empty()) {
        (false, false) => format!("{} · {}", a.run_ref, a.error),
        (false, true) => a.run_ref.clone(),
        (true, _) => a.error.clone(),
    };
    div()
        .flex()
        .items_start()
        .gap(theme::S3)
        .px(theme::S2)
        .py(theme::S2)
        .child(
            ui::icon_sized(AppIcon::Alert, theme::ICON_SM)
                .text_color(theme::DANGER)
                .mt(px(3.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_body().fg().child(a.title.clone()))
                .when(!sub.is_empty(), |d| {
                    d.child(div().text_label().fg_muted().line_clamp(2).child(sub))
                })
                .child(
                    div()
                        .flex()
                        .gap(theme::S2)
                        .pt(theme::S1)
                        .when(a.retryable, |d| {
                            d.child(
                                Button::new(
                                    SharedString::from(format!("review-retry-{}", a.task_id)),
                                    "Retry",
                                )
                                .small()
                                .icon(AppIcon::RotateCcw)
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        let id = retry_id.clone();
                                        // run_engine toasts the engine's reason if it refuses.
                                        let job = state::run_engine(cx, move |e| e.retry(&id));
                                        cx.spawn(async move |_, cx| {
                                            if job.await.is_some() {
                                                let _ = cx.update(|cx| {
                                                    state::toast(
                                                        ToastKind::Info,
                                                        "Retrying the step",
                                                        cx,
                                                    )
                                                });
                                            }
                                        })
                                        .detach();
                                    },
                                )),
                            )
                        })
                        .child(
                            Button::new(
                                SharedString::from(format!("review-open-{}", a.task_id)),
                                "Open",
                            )
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| state::open_task(open_id.clone(), cx)),
                        ),
                ),
        )
}

/// "11:40  Picked B · title  #129", opens the task thread.
fn decided_row(ix: usize, r: &DecidedRow) -> impl IntoElement {
    let id = r.task_id.clone();
    div()
        .id(SharedString::from(format!(
            "review-decided-{ix}-{}",
            r.task_id
        )))
        .flex()
        .gap(theme::S3)
        .px(theme::S2)
        .py(px(5.))
        .rounded(theme::RADIUS_MD)
        .cursor_pointer()
        .hover(|s| s.bg(theme::SURFACE))
        .child(
            div()
                .flex_none()
                .text_mono_sm()
                .fg_muted()
                .child(r.time.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_label()
                .fg_secondary()
                .line_clamp(2)
                .child(r.what.clone()),
        )
        .when(!r.run_ref.is_empty(), |d| {
            d.child(
                div()
                    .flex_none()
                    .text_mono_sm()
                    .fg_muted()
                    .child(r.run_ref.clone()),
            )
        })
        .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
}
