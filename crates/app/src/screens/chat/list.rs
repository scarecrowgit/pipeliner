//! The chat list: search, chats grouped by day, a live dot while a chat works.

use chrono::{DateTime, Local, Utc};
use gpui::{Animation, AnimationExt as _};

use super::data::ChatRow;
use super::{eid, mode_icon, ChatScreen};
use crate::screens::prelude::*;

impl ChatScreen {
    pub(super) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.as_deref().unwrap_or_default();
        let query = self.search.read(cx).value().trim().to_lowercase();
        let shown: Vec<&ChatRow> = rows
            .iter()
            .filter(|r| query.is_empty() || r.chat.title.to_lowercase().contains(&query))
            .collect();

        let mut items: Vec<AnyElement> = vec![];
        let mut last_group = String::new();
        for r in &shown {
            let g = day_group(r.chat.updated_at);
            if g != last_group {
                items.push(
                    div()
                        .mt(theme::S3)
                        .mb(theme::S1)
                        .px(px(10.))
                        .text_label()
                        .fg_muted()
                        .child(g.clone())
                        .into_any_element(),
                );
                last_group = g;
            }
            items.push(self.render_row(r, cx).into_any_element());
        }

        let body = if rows.is_empty() {
            div()
                .px(theme::S2)
                .py(theme::S4)
                .text_label()
                .fg_muted()
                .child("No chats yet. Write below to start one.")
                .into_any_element()
        } else if shown.is_empty() {
            div()
                .px(theme::S2)
                .py(theme::S4)
                .text_label()
                .fg_muted()
                .child("No chat matches.")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(items)
                .into_any_element()
        };

        div()
            .id("chat-list")
            .w(px(280.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::LINE)
            .px(px(10.))
            .py(theme::S4)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .pl(px(10.))
                    .pr(theme::S1)
                    .mb(theme::S1)
                    .child(div().flex_1().text_heading().fg().child("Chats"))
                    .child(div().text_label().fg_muted().child(rows.len().to_string())),
            )
            .child(
                div()
                    .px(theme::S1)
                    .mb(px(6.))
                    .child(ui::input::field_sm(&self.search)),
            )
            .child(
                div()
                    .id("chat-list-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }

    fn render_row(&self, r: &ChatRow, cx: &mut Context<Self>) -> impl IntoElement {
        let id = r.chat.id.clone();
        let active = self.selected.as_ref() == Some(&r.chat.id);
        let live = r.live_status();

        // "Orchestrator · run #128 · waiting 6m", "Direct · Sonnet 5.5 · running · $0.02"
        let mut meta: Vec<String> = vec![r.chat.mode.label().into()];
        if !r.detail.is_empty() {
            meta.push(r.detail.clone());
        }
        if let Some((n, rs)) = r.run.filter(|(_, s)| s.is_active()) {
            meta.push(format!("run #{n}"));
            if rs == Status::Waiting {
                meta.push("waiting".into());
            }
        } else if let Some(s) = r.status {
            match s {
                Status::Running => meta.push("running".into()),
                Status::Waiting => meta.push(match r.since {
                    Some(t) => format!("waiting {}", ui::fmt_ago(t)),
                    None => "waiting".into(),
                }),
                Status::Failed => meta.push("failed".into()),
                _ => {}
            }
        }
        if r.cost > 0.0 {
            meta.push(ui::fmt_usd(r.cost));
        }

        let (tile_bg, tile_fg) = if active {
            (theme::ACCENT_SOFT, theme::ACCENT_TEXT)
        } else {
            (theme::SURFACE_HOVER, theme::TEXT_SECONDARY)
        };
        div()
            .id(eid("chat-row", &r.chat.id))
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(theme::S2)
            .rounded(theme::RADIUS_MD)
            .cursor_pointer()
            .when(active, |d| {
                d.bg(theme::SURFACE_HOVER)
                    .border_l_2()
                    .border_color(theme::ACCENT)
            })
            .when(!active, |d| d.hover(|s| s.bg(theme::SURFACE_RAISED)))
            .child(icon_tile(mode_icon(r.chat.mode), tile_bg, tile_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_body_strong()
                            .fg()
                            .ellipsis()
                            .child(r.chat.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_label()
                            .fg_muted()
                            .when_some(live, |d, s| {
                                d.child(
                                    StatusDot::new(s)
                                        .id(eid("chat-row-dot", &r.chat.id))
                                        .small(),
                                )
                            })
                            .child(div().min_w_0().ellipsis().child(meta.join(" · "))),
                    ),
            )
            .child(if live == Some(Status::Waiting) {
                Badge::new("waiting").warning().into_any_element()
            } else {
                div()
                    .flex_none()
                    .text_label()
                    .fg_disabled()
                    .child(ui::fmt_ago(r.chat.updated_at))
                    .into_any_element()
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open_chat(id.clone(), cx)))
            .with_animation(
                eid("chat-row-in", &r.chat.id),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t),
            )
    }
}

/// "Today", "Yesterday", or "Mon, Nov 23".
fn day_group(t: DateTime<Utc>) -> String {
    let day = t.with_timezone(&Local).date_naive();
    let today = Local::now().date_naive();
    if day == today {
        "Today".into()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else {
        day.format("%a, %b %-d").to_string()
    }
}
