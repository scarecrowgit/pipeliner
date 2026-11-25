//! The chat header: breadcrumb with the (inline editable) title, mode and live status,
//! the chat's cost, "New chat" and "Save as agent" (with its inline form).

use gpui::{Animation, AnimationExt as _};
use harness_core::{ChatMode, Role};

use super::{ChatScreen, Form};
use crate::screens::prelude::*;

impl ChatScreen {
    pub(super) fn render_header(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme::S3)
            .min_h(theme::HEADER_H)
            .px(theme::S6)
            .border_b_1()
            .border_color(theme::LINE)
            .child(div().text_label().fg_muted().child("Chat"))
            .child(icon_sized(AppIcon::ChevronRight, theme::ICON_SM).text_color(theme::TEXT_MUTED));

        let Some(d) = &self.detail else {
            let title =
                if self.rows.as_ref().is_some_and(|r| r.is_empty()) || self.selected.is_none() {
                    "New chat"
                } else {
                    ""
                };
            return bar
                .child(div().text_heading().fg().child(title))
                .child(div().flex_1())
                .when(self.selected.is_some(), |b| {
                    b.child(
                        Button::new("chat-new", "New chat")
                            .ghost()
                            .icon(AppIcon::Plus)
                            .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
                    )
                })
                .into_any_element();
        };

        let chat = &d.chat;
        let title_el = if self.editing_title {
            div()
                .w(px(360.))
                .child(ui::input::field_sm(&self.title_input))
                .into_any_element()
        } else {
            div()
                .id("chat-title")
                .min_w_0()
                .flex_shrink()
                .px(theme::S1)
                .rounded(theme::RADIUS_SM)
                .text_heading()
                .fg()
                .truncate()
                .cursor_text()
                .hover(|s| s.bg(theme::SURFACE_RAISED))
                .tooltip(|window, cx| {
                    gpui_component::tooltip::Tooltip::new("Click to rename").build(window, cx)
                })
                .child(chat.title.clone())
                .on_click(cx.listener(|this, _, window, cx| this.start_edit_title(window, cx)))
                .into_any_element()
        };

        let status = self.header_status();
        let cost = d.task.as_ref().map(|t| t.cost_usd).unwrap_or(0.0);
        let runs_cost: f64 = d.runs.values().map(|r| r.run.cost_usd).sum();
        let cost_text = if d.runs.is_empty() {
            format!("this chat {}", ui::fmt_usd(cost))
        } else {
            format!(
                "this chat {} · runs {}",
                ui::fmt_usd(cost),
                ui::fmt_usd(runs_cost)
            )
        };
        let has_user_msg = self.messages.iter().any(|m| m.role == Role::User);
        let can_save_agent = chat.mode == ChatMode::Direct;

        let bar = bar
            .child(title_el)
            .child(Badge::new(chat.mode.label()).accent())
            .children(status)
            .child(div().flex_1())
            .child(
                div()
                    .text_label()
                    .fg_muted()
                    .whitespace_nowrap()
                    .child(cost_text),
            )
            .child(
                Button::new("chat-new", "New chat")
                    .ghost()
                    .icon(AppIcon::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
            )
            .when(can_save_agent, |b| {
                b.child(
                    Button::new("chat-save-agent", "Save as agent")
                        .secondary()
                        .icon(AppIcon::Bot)
                        .disabled(!has_user_msg)
                        .tooltip(if has_user_msg {
                            "Keep this chat's prompt as a reusable agent"
                        } else {
                            "Send a message first: it becomes the agent's role"
                        })
                        .selected(self.form == Some(Form::SaveAgent))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.form == Some(Form::SaveAgent) {
                                this.form = None;
                                cx.notify();
                            } else {
                                this.open_form(Form::SaveAgent, window, cx);
                            }
                        })),
                )
            });

        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(bar)
            .when(self.form == Some(Form::SaveAgent), |c| {
                c.child(self.render_agent_form(window, cx))
            })
            .into_any_element()
    }

    /// Badge for what the chat is doing right now: an active run, or its own task.
    fn header_status(&self) -> Option<AnyElement> {
        let d = self.detail.as_ref()?;
        if let Some(r) = d.active_run() {
            let since = ui::fmt_ago(r.run.created_at);
            let text = match r.run.status {
                Status::Waiting => format!("run #{} · waiting for you", r.run.number),
                s => format!(
                    "run #{} · {} · {since}",
                    r.run.number,
                    s.label().to_lowercase()
                ),
            };
            let badge = Badge::new(text).dot();
            return Some(
                match r.run.status {
                    Status::Waiting => badge.warning(),
                    _ => badge.info(),
                }
                .into_any_element(),
            );
        }
        let t = d.task.as_ref()?;
        match t.status {
            Status::Running | Status::Waiting | Status::Failed => {
                let since = t.started_at.map(ui::fmt_ago).unwrap_or_default();
                Some(
                    StatusPill::new(t.status)
                        .id("chat-head-status")
                        .suffix(since)
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    /// "Save as agent": name, role (prefilled from the first message), the chat's model.
    fn render_agent_form(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self
            .detail
            .as_ref()
            .map(|d| super::data::short_model(&d.chat.model))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "the default model".into());
        div()
            .flex()
            .flex_col()
            .gap(theme::S3)
            .px(theme::S6)
            .py(theme::S4)
            .bg(theme::BG_SIDEBAR)
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .child(icon_tile(
                        AppIcon::Bot,
                        theme::ACCENT_SOFT,
                        theme::ACCENT_TEXT,
                    ))
                    .child(
                        div()
                            .text_heading()
                            .fg()
                            .child("Save this chat as an agent"),
                    )
                    .child(div().text_label().fg_muted().child(format!(
                        "Runs on {model} · no tools until you add them in Agents"
                    ))),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .child(
                        div()
                            .w(px(260.))
                            .flex()
                            .flex_col()
                            .gap(theme::S1)
                            .child(label("Name"))
                            .child(ui::input::field(&self.name_input)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme::S1)
                            .child(label("Role · prompt layer 2"))
                            .child(ui::input::composer_field(&self.role_input)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .child(
                        Button::new("agent-form-save", "Save agent")
                            .primary()
                            .small()
                            .icon(AppIcon::Check)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_form(cx))),
                    )
                    .child(
                        Button::new("agent-form-cancel", "Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                    ),
            )
            .with_animation(
                "agent-form-in",
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t).mt(px(-6.) * (1.0 - t)),
            )
    }
}
