//! The conversation: messages (markdown, tool calls, notes), inline cards (plan, run
//! progress, task) and the live sub-task board above the composer.

use std::time::Duration;

use gpui::{
    pulsating_between, Animation, AnimationExt as _, HighlightStyle, ScrollWheelEvent, StyledText,
};
use harness_core::{Card, ChatMode, DecisionKind, Message, Role, Task, ToolCall};

use super::data::{self, PlanState, RunInfo};
use super::{clock, eid, mode_icon, ChatScreen, Form};
use crate::screens::prelude::*;

impl ChatScreen {
    pub(super) fn render_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = self.detail.is_none() && self.selected.is_none();
        let loading = self.selected.is_some()
            && (self.detail.is_none() || self.messages_for != self.selected);

        let list: AnyElement = if empty {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new(
                        AppIcon::Chat,
                        "Start a chat",
                        "Talk to a model directly, hand a task to an agent, let an orchestrator plan it, or start a pipeline. Pick a mode below and write your message.",
                    )
                    .id("chat-empty"),
                )
                .into_any_element()
        } else if loading {
            div()
                .p(theme::S6)
                .child(skeleton_rows("chat-msgs-loading", 4))
                .into_any_element()
        } else if self.messages.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new(AppIcon::Chat, "No messages yet", self.mode_blurb())
                        .compact()
                        .id(eid(
                            "chat-empty",
                            self.selected.as_deref().unwrap_or_default(),
                        )),
                )
                .into_any_element()
        } else {
            self.render_messages(window, cx).into_any_element()
        };

        let show_jump = !empty && !loading && !self.messages.is_empty() && !self.near_bottom();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(list)
                    .when(show_jump, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(theme::S4)
                                .right(theme::S6)
                                .child(
                                    Button::new("chat-jump", "Jump to latest")
                                        .secondary()
                                        .small()
                                        .icon(AppIcon::ChevronDown)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.scroll.scroll_to_bottom();
                                            cx.notify();
                                        })),
                                )
                                .with_animation(
                                    "chat-jump-in",
                                    Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                                    |d, t| d.opacity(t),
                                ),
                        )
                    }),
            )
            .children(self.render_subtasks(cx))
            .child(self.render_composer(window, cx))
    }

    fn render_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut items: Vec<AnyElement> = vec![];
        let mut last_day = None;
        for ix in 0..self.messages.len() {
            let m = &self.messages[ix];
            // Tool results render inside the call that asked for them.
            if m.role == Role::Tool && m.tool_call_id.as_ref().is_some_and(|id| self.has_call(id)) {
                continue;
            }
            let day = m.created_at.with_timezone(&chrono::Local).date_naive();
            if last_day != Some(day) {
                last_day = Some(day);
                items.push(day_rule(m.created_at).into_any_element());
            }
            let el = self.render_message(ix, window, cx);
            let animate = m.created_at > self.opened_at;
            items.push(if animate {
                div()
                    .child(el)
                    .with_animation(
                        eid("msg-in", &m.id),
                        Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                        |d, t| d.opacity(t).mt(px(8.) * (1.0 - t)),
                    )
                    .into_any_element()
            } else {
                el
            });
        }
        div()
            .id("chat-messages")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.notify()))
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(theme::S5)
                    .p(theme::S6)
                    .children(items),
            )
    }

    fn has_call(&self, call_id: &str) -> bool {
        self.call_ids.contains(call_id)
    }

    fn render_message(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.messages[ix];
        match (&m.card, m.role) {
            (Some(kind), _) => self.render_card(m, kind, window, cx),
            (None, Role::User) => {
                let steering = !m.delivered && self.task_status().is_some_and(|s| s.is_active());
                user_message(m, steering).into_any_element()
            }
            (None, Role::Assistant) => self.assistant_message(m, window, cx).into_any_element(),
            (None, Role::Tool) => self
                .tool_row(None, m.author.clone(), Some(m), m, cx)
                .into_any_element(),
            (None, Role::System) => system_note(&m.content).into_any_element(),
        }
    }

    fn assistant_message(
        &self,
        m: &Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mode = self
            .detail
            .as_ref()
            .map(|d| d.chat.mode)
            .unwrap_or_default();
        let author = if m.author.is_empty() {
            "Assistant".to_string()
        } else {
            m.author.clone()
        };
        let text = m.content.trim().to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .text_label()
                    .fg_muted()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .rounded(theme::RADIUS_MD)
                            .bg(theme::ACCENT_SOFT)
                            .text_color(theme::ACCENT_TEXT)
                            .child(icon_sized(
                                if mode == ChatMode::Direct {
                                    AppIcon::Sparkles
                                } else {
                                    mode_icon(mode)
                                },
                                px(12.),
                            )),
                    )
                    .child(
                        div()
                            .text_color(theme::TEXT_SECONDARY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(author),
                    )
                    .child(clock(m.created_at)),
            )
            .when(!text.is_empty(), |d| {
                d.child(ui::markdown(eid("msg-md", &m.id), text, window, cx))
            })
            .when(m.partial, |d| d.child(caret(&m.id)))
            .children(m.tool_calls.iter().map(|c| {
                let result = self.tool_results.get(&c.id).map(|ix| &self.messages[*ix]);
                self.tool_row(Some(c), c.name.clone(), result, m, cx)
            }))
    }

    /// A compact, expandable tool call: name, one-line args, then args and result.
    fn tool_row(
        &self,
        call: Option<&ToolCall>,
        name: String,
        result: Option<&Message>,
        owner: &Message,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = call
            .map(|c| c.id.clone())
            .unwrap_or_else(|| owner.id.clone());
        let open = self.expanded.contains(&key);
        let summary = call
            .map(|c| data::args_summary(&c.arguments))
            .unwrap_or_default();
        let working = result.is_none() && self.task_status().is_some_and(|s| s == Status::Running);
        let failed = result.is_some_and(|r| r.content.starts_with("Error"));
        let state_icon = if working {
            spinner(&key).into_any_element()
        } else if failed {
            icon_sized(AppIcon::CircleX, theme::ICON_SM)
                .text_color(theme::DANGER)
                .into_any_element()
        } else if result.is_some() {
            icon_sized(AppIcon::Check, theme::ICON_SM)
                .text_color(theme::SUCCESS)
                .into_any_element()
        } else {
            icon_sized(AppIcon::Terminal, theme::ICON_SM)
                .text_color(theme::TEXT_MUTED)
                .into_any_element()
        };
        let toggle_key = key.clone();
        div()
            .flex()
            .flex_col()
            .rounded(theme::RADIUS_LG)
            .border_1()
            .border_color(theme::LINE)
            .bg(theme::SURFACE)
            .overflow_hidden()
            .child(
                div()
                    .id(eid("tool-row", &key))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(30.))
                    .px(px(10.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .child(state_icon)
                    .child(div().flex_none().text_mono().fg_secondary().child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_mono()
                            .fg_muted()
                            .truncate()
                            .child(summary),
                    )
                    .child(
                        icon_sized(
                            if open {
                                AppIcon::ChevronDown
                            } else {
                                AppIcon::ChevronRight
                            },
                            theme::ICON_SM,
                        )
                        .text_color(theme::TEXT_MUTED),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&toggle_key) {
                            this.expanded.insert(toggle_key.clone());
                        }
                        cx.notify();
                    })),
            )
            .when(open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::S2)
                        .p(px(10.))
                        .border_t_1()
                        .border_color(theme::LINE_SUBTLE)
                        .when_some(call, |d, c| {
                            d.child(label("Arguments")).child(
                                CodeBlock::new(
                                    eid("tool-args", &c.id),
                                    data::pretty_args(&c.arguments),
                                )
                                .max_height(px(220.)),
                            )
                        })
                        .child(label(if result.is_some() {
                            "Result"
                        } else {
                            "No result yet"
                        }))
                        .when_some(result, |d, r| {
                            d.child(
                                CodeBlock::new(eid("tool-result", &r.id), r.content.clone())
                                    .max_height(px(320.)),
                            )
                        })
                        .with_animation(
                            eid("tool-open", &key),
                            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                            |d, t| d.opacity(t).mt(px(-4.) * (1.0 - t)),
                        ),
                )
            })
    }

    // -----------------------------------------------------------------------
    // Cards
    // -----------------------------------------------------------------------

    fn render_card(
        &self,
        m: &Message,
        kind: &Card,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match kind {
            Card::Plan { task_id, steps } => {
                self.plan_card(m, task_id, steps, cx).into_any_element()
            }
            Card::Run { run_id } => div()
                .flex()
                .flex_col()
                .gap(theme::S3)
                .when(!m.content.trim().is_empty(), |d| d.child(event_line(m)))
                .child(
                    match self.detail.as_ref().and_then(|d| d.runs.get(run_id)) {
                        Some(info) => self.run_card(info, cx).into_any_element(),
                        None => card()
                            .child(Skeleton::new(eid("run-skel", run_id)).h(px(60.)))
                            .into_any_element(),
                    },
                )
                .into_any_element(),
            Card::SubTasks { .. } => div()
                .flex()
                .flex_col()
                .gap(theme::S2)
                .child(system_note(if m.content.is_empty() {
                    "Sub-agents started"
                } else {
                    &m.content
                }))
                .into_any_element(),
            Card::Task { task_id } => {
                let task = self.find_task(task_id);
                let id = task_id.clone();
                let mut row = div()
                    .id(eid("task-card", &m.id))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .px(theme::S3)
                    .h(theme::ROW_H)
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(theme::LINE)
                    .bg(theme::SURFACE)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .on_click(move |_, _, cx| state::open_task(id.clone(), cx));
                row = match task {
                    Some(t) => row
                        .child(StatusDot::new(t.status).id(eid("task-card-dot", &m.id)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_body_strong()
                                .truncate()
                                .child(t.title.clone()),
                        )
                        .child(div().text_label().fg_muted().child(t.actor.clone())),
                    None => row.child(div().flex_1().text_body().fg_secondary().child(
                        if m.content.is_empty() {
                            "Open task".to_string()
                        } else {
                            m.content.clone()
                        },
                    )),
                };
                let _ = window;
                row.child(
                    icon_sized(AppIcon::ChevronRight, theme::ICON_SM).text_color(theme::TEXT_MUTED),
                )
                .into_any_element()
            }
        }
    }

    fn find_task(&self, id: &str) -> Option<&Task> {
        let d = self.detail.as_ref()?;
        d.task.iter().chain(d.children.iter()).find(|t| t.id == id)
    }

    fn plan_card(
        &self,
        m: &Message,
        task_id: &Id,
        steps: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self
            .plan_states
            .get(&m.id)
            .cloned()
            .unwrap_or(PlanState::Closed);
        let pending = state == PlanState::Pending;
        let badge = match &state {
            PlanState::Pending => {
                Badge::new(format!("waiting for you · {}", ui::fmt_ago(m.created_at)))
                    .warning()
                    .dot()
            }
            PlanState::Approved(t) => Badge::new(format!("approved by you · {}", clock(*t)))
                .success()
                .dot(),
            PlanState::Feedback(t) => Badge::new(format!("feedback sent · {}", clock(*t))).accent(),
            PlanState::Rejected(t) => Badge::new(format!("rejected · {}", clock(*t)))
                .danger()
                .dot(),
            PlanState::Closed => Badge::new("closed"),
        };
        let saving = matches!(&self.form, Some(Form::SavePlan { message, .. }) if message == &m.id);
        let approve_id = task_id.clone();
        let reject_id = task_id.clone();
        let save_steps = steps.to_vec();
        let save_msg = m.id.clone();

        card()
            .gap(theme::S3)
            .when(pending, |c| c.border_color(theme::alpha(theme::WARNING, 0.35)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::S2)
                    .child(icon_tile(AppIcon::GitFork, theme::ACCENT_SOFT, theme::ACCENT_TEXT))
                    .child(div().text_heading().fg().child(format!("Plan · {} steps", steps.len())))
                    .child(badge),
            )
            .child(div().flex().flex_col().children(steps.iter().enumerate().map(|(i, s)| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .py(px(7.))
                    .border_t_1()
                    .border_color(theme::LINE_SUBTLE)
                    .child(div().w(px(20.)).flex_none().text_mono().fg_muted().child((i + 1).to_string()))
                    .child(div().flex_1().min_w_0().text_body().fg_secondary().child(s.clone()))
            })))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::S2)
                    .when(pending, |d| {
                        d.child(
                            Button::new(eid("plan-approve", &m.id), "Approve")
                                .primary()
                                .small()
                                .icon(AppIcon::Check)
                                .on_click(move |_, _, cx| {
                                    let id = approve_id.clone();
                                    state::run_engine(cx, move |e| e.decide(&id, DecisionKind::Approve)).detach();
                                }),
                        )
                        .child(
                            Button::new(eid("plan-feedback", &m.id), "Send feedback")
                                .secondary()
                                .small()
                                .icon(AppIcon::Chat)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.feedback = true;
                                    this.composer.update(cx, |s, cx| s.focus(window, cx));
                                    cx.notify();
                                })),
                        )
                        .child(Button::new(eid("plan-reject", &m.id), "Reject").danger().small().on_click(move |_, _, cx| {
                            let id = reject_id.clone();
                            state::run_engine(cx, move |e| e.decide(&id, DecisionKind::Reject { reason: String::new() }))
                                .detach();
                        }))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new(eid("plan-save", &m.id), "Save plan as pipeline")
                            .ghost()
                            .small()
                            .icon(AppIcon::Workflow)
                            .selected(saving)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if matches!(&this.form, Some(Form::SavePlan { message, .. }) if *message == save_msg) {
                                    this.form = None;
                                    cx.notify();
                                } else {
                                    let form = Form::SavePlan { message: save_msg.clone(), steps: save_steps.clone() };
                                    this.open_form(form, window, cx);
                                }
                            })),
                    ),
            )
            .when(saving, |c| {
                c.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .pt(theme::S1)
                        .child(div().w(px(280.)).child(ui::input::field_sm(&self.name_input)))
                        .child(
                            Button::new(eid("plan-save-ok", &m.id), "Save")
                                .primary()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.submit_form(cx))),
                        )
                        .child(Button::new(eid("plan-save-cancel", &m.id), "Cancel").ghost().small().on_click(cx.listener(
                            |this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            },
                        )))
                        .child(div().text_label().fg_muted().child("One agent step per plan step, then a review gate"))
                        .with_animation(
                            eid("plan-save-in", &m.id),
                            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                            |d, t| d.opacity(t),
                        ),
                )
            })
    }

    fn run_card(&self, info: &RunInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let run = &info.run;
        let total = info.nodes.len().max(1);
        let done = info.done_nodes();
        let active = run.status.is_active();
        let run_id = run.id.clone();
        let pipeline_id = run.pipeline_id.clone();
        let rid = run.id.as_str();
        let _ = cx;
        card()
            .gap(theme::S3)
            .when(run.status == Status::Waiting, |c| {
                c.border_color(theme::alpha(theme::WARNING, 0.35))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::S2)
                    .child(icon_tile(
                        AppIcon::Activity,
                        theme::INFO_SOFT,
                        theme::INFO_TEXT,
                    ))
                    .child(
                        div()
                            .text_heading()
                            .fg()
                            .child(format!("Run #{}", run.number)),
                    )
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .child(format!("{} v{}", info.pipeline, run.pipeline_version)),
                    )
                    .child(StatusPill::new(run.status).id(eid("run-pill", rid)))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .child(ui::fmt_ago(run.created_at)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div().flex_1().child(
                            ProgressBar::new(eid("run-progress", rid), done as f64 / total as f64)
                                .color(theme::status_color(if active {
                                    Status::Running
                                } else {
                                    run.status
                                })),
                        ),
                    )
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .whitespace_nowrap()
                            .child(format!(
                                "{} · {done} of {} steps",
                                ui::fmt_usd(run.cost_usd),
                                info.nodes.len()
                            )),
                    ),
            )
            .child(div().flex().flex_wrap().gap(px(6.)).children(
                info.nodes.iter().enumerate().map(|(i, n)| {
                    let task = n.task_id.clone();
                    let chip_id = ElementId::Name(format!("run-node-{rid}-{i}").into());
                    let dot = match n.status {
                        Some(s) => StatusDot::new(s)
                            .id(ElementId::Name(format!("run-node-dot-{rid}-{i}").into()))
                            .small()
                            .into_any_element(),
                        None => ring_el(theme::TEXT_DISABLED, px(6.)).into_any_element(),
                    };
                    let fg = match n.status {
                        None => theme::TEXT_DISABLED,
                        Some(Status::Waiting) => theme::WARNING,
                        Some(Status::Failed) => theme::DANGER,
                        _ => theme::TEXT_SECONDARY,
                    };
                    div()
                        .id(chip_id.clone())
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(24.))
                        .px(theme::S2)
                        .rounded(theme::RADIUS_MD)
                        .bg(n.status.map(theme::status_soft).unwrap_or(theme::SURFACE))
                        .border_1()
                        .border_color(theme::LINE)
                        .text_label()
                        .text_color(fg)
                        .child(dot)
                        .child(n.name.clone())
                        .when_some(task, |d, t| {
                            d.cursor_pointer()
                                .hover(|s| s.border_color(theme::LINE_STRONG))
                                .on_click(move |_, _, cx| state::open_task(t.clone(), cx))
                        })
                        .with_animation(
                            ElementId::Name(format!("run-node-in-{rid}-{i}-{:?}", n.status).into()),
                            Animation::new(theme::DUR_STATE).with_easing(theme::ease_out),
                            |d, t| d.opacity(0.4 + 0.6 * t),
                        )
                }),
            ))
            .when_some(run.error.clone(), |c, e| {
                c.child(div().text_label().text_color(theme::DANGER).child(e))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::S2)
                    .when_some(info.waiting_task.clone(), |d, t| {
                        d.child(
                            Button::new(eid("run-review", rid), "Open in Review")
                                .primary()
                                .small()
                                .icon(AppIcon::Inbox)
                                .on_click(move |_, _, cx| {
                                    state::navigate(
                                        Route::Review {
                                            task: Some(t.clone()),
                                        },
                                        cx,
                                    )
                                }),
                        )
                    })
                    .child(
                        Button::new(eid("run-open", rid), "Open run")
                            .ghost()
                            .small()
                            .icon(AppIcon::Workflow)
                            .on_click(move |_, _, cx| {
                                state::navigate(
                                    Route::Pipelines {
                                        pipeline: Some(pipeline_id.clone()),
                                    },
                                    cx,
                                )
                            }),
                    )
                    .when_some(run.pr_url.clone(), |d, url| {
                        d.child(
                            Button::new(eid("run-pr", rid), "Pull request")
                                .ghost()
                                .small()
                                .icon(AppIcon::GitPullRequest)
                                .icon_right(AppIcon::ExternalLink)
                                .tooltip(url.clone())
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .child(div().flex_1())
                    .when(active, |d| {
                        d.child(
                            Button::new(eid("run-cancel", rid), "Cancel run")
                                .danger()
                                .small()
                                .on_click(move |_, _, cx| {
                                    let id = run_id.clone();
                                    state::run_engine(cx, move |e| e.cancel_run(&id)).detach();
                                }),
                        )
                    }),
            )
    }

    // -----------------------------------------------------------------------
    // Sub-task board
    // -----------------------------------------------------------------------

    /// A compact live board of the sub-tasks the chat's task delegated.
    fn render_subtasks(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.detail.as_ref()?;
        if d.children.is_empty() {
            return None;
        }
        let kids = &d.children;
        let running = kids
            .iter()
            .filter(|t| matches!(t.status, Status::Running | Status::Waiting))
            .count();
        let cost: f64 = kids.iter().map(|t| t.cost_usd).sum();
        let columns: [(&str, Vec<&Task>); 3] = [
            (
                "Queued",
                kids.iter().filter(|t| t.status == Status::Queued).collect(),
            ),
            (
                "Running",
                kids.iter()
                    .filter(|t| matches!(t.status, Status::Running | Status::Waiting))
                    .collect(),
            ),
            (
                "Done",
                kids.iter().filter(|t| t.status.is_finished()).collect(),
            ),
        ];
        let open = self.board_open;
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(theme::S2)
                .px(theme::S6)
                .py(theme::S3)
                .border_t_1()
                .border_color(theme::LINE)
                .child(
                    div()
                        .id("subtasks-head")
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .cursor_pointer()
                        .child(icon_tile(
                            AppIcon::GitFork,
                            theme::SPAWN_SOFT,
                            theme::SPAWN_TEXT,
                        ))
                        .child(div().text_body_strong().fg().child("Sub-agents"))
                        .child(div().text_label().fg_muted().child(format!(
                            "{} · {running} running · {}",
                            kids.len(),
                            ui::fmt_usd(cost)
                        )))
                        .child(div().flex_1())
                        .child(
                            icon_sized(
                                if open {
                                    AppIcon::ChevronDown
                                } else {
                                    AppIcon::ChevronRight
                                },
                                theme::ICON_SM,
                            )
                            .text_color(theme::TEXT_MUTED),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.board_open = !this.board_open;
                            cx.notify();
                        })),
                )
                .when(open, |c| {
                    c.child(
                        div()
                            .id("subtasks-board")
                            .flex()
                            .gap(theme::S3)
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .children(columns.into_iter().map(|(title, tasks)| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_label()
                                            .fg_muted()
                                            .child(format!("{title} · {}", tasks.len())),
                                    )
                                    .children(tasks.into_iter().map(subtask_item))
                            }))
                            .with_animation(
                                "subtasks-open",
                                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                                |d, t| d.opacity(t),
                            ),
                    )
                })
                .into_any_element(),
        )
    }

    /// Short description of the open mode (empty states).
    pub(super) fn mode_blurb(&self) -> &'static str {
        mode_blurb(self.settings().mode)
    }
}

/// What each mode does, one line.
pub(super) fn mode_blurb(mode: ChatMode) -> &'static str {
    match mode {
        ChatMode::Direct => "You and a model. It answers; you apply the result.",
        ChatMode::Agent => {
            "A saved agent works on it with its role and tools, as a task on the Board."
        }
        ChatMode::Orchestrator => "Plans steps and starts sub-agents. You approve the plan first.",
        ChatMode::Pipeline => "Starts the chosen pipeline with your message as its input.",
    }
}

fn subtask_item(t: &Task) -> impl IntoElement {
    let id = t.id.clone();
    div()
        .id(eid("subtask", &t.id))
        .flex()
        .items_center()
        .gap(theme::S2)
        .px(theme::S2)
        .py(px(5.))
        .rounded(theme::RADIUS_MD)
        .border_1()
        .border_color(if t.status == Status::Waiting {
            theme::alpha(theme::WARNING, 0.35)
        } else {
            theme::LINE
        })
        .bg(theme::SURFACE)
        .cursor_pointer()
        .hover(|s| s.bg(theme::SURFACE_RAISED))
        .child(
            StatusDot::new(t.status)
                .id(eid("subtask-dot", &t.id))
                .small(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_label().fg().truncate().child(t.title.clone()))
                .child(div().text_mono_sm().fg_muted().truncate().child(format!(
                    "{} · {}",
                    t.actor,
                    ui::fmt_usd(t.cost_usd)
                ))),
        )
        .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
        // Keyed by status: a status change replays the fade, so moves between columns show.
        .with_animation(
            ElementId::Name(format!("subtask-in-{}-{}", t.id, t.status.as_str()).into()),
            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
            |d, t| d.opacity(t).ml(px(6.) * (1.0 - t)),
        )
}

/// "You · 09:40" and the message in an accent bubble, right aligned.
fn user_message(m: &Message, steering: bool) -> impl IntoElement {
    let content = m.content.trim().to_string();
    // Highlight a leading `/run name` or `@Agent` like the composer suggests them.
    let token_len = if let Some(rest) = content.strip_prefix("/run ") {
        rest.find(char::is_whitespace)
            .map(|i| i + 5)
            .unwrap_or(content.len())
    } else if content.starts_with('@') || content.starts_with('/') {
        content.find(char::is_whitespace).unwrap_or(content.len())
    } else {
        0
    };
    let text = StyledText::new(content).with_highlights(
        (token_len > 0)
            .then(|| {
                (
                    0..token_len,
                    HighlightStyle {
                        color: Some(theme::ACCENT_TEXT),
                        ..Default::default()
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>(),
    );
    let author = if m.author.is_empty() || m.author == "you" {
        "You".to_string()
    } else {
        m.author.clone()
    };
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .text_label()
                .fg_muted()
                .child(
                    div()
                        .text_color(theme::TEXT_SECONDARY)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(author),
                )
                .child(clock(m.created_at)),
        )
        .child(
            div()
                .max_w(relative(0.85))
                .px(px(14.))
                .py(px(10.))
                .rounded(theme::RADIUS_XL)
                .bg(theme::ACCENT_SOFT)
                .border_1()
                .border_color(theme::ACCENT_RING)
                .text_body()
                .fg()
                .child(text),
        )
        .when(steering, |d| {
            d.child(
                div()
                    .text_label()
                    .fg_muted()
                    .child("Steering · the agent reads it before its next step"),
            )
        })
}

/// An engine note: small, centered.
fn system_note(text: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(theme::S3)
        .child(div().flex_1().h(px(1.)).bg(theme::LINE))
        .child(
            div()
                .max_w(relative(0.8))
                .text_label()
                .fg_muted()
                .text_center()
                .child(text.to_string()),
        )
        .child(div().flex_1().h(px(1.)).bg(theme::LINE))
}

/// `09:41 · Started run #12 · title` in mono, like the mockup's event lines.
fn event_line(m: &Message) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .text_mono()
        .fg_muted()
        .child(div().w(px(16.)).h(px(1.)).bg(theme::LINE_STRONG))
        .child(format!("{} · {}", clock(m.created_at), m.content.trim()))
}

/// "Today · Wednesday, Nov 25" between days.
fn day_rule(t: chrono::DateTime<chrono::Utc>) -> impl IntoElement {
    let local = t.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    let prefix = if local.date_naive() == today {
        "Today · "
    } else if Some(local.date_naive()) == today.pred_opt() {
        "Yesterday · "
    } else {
        ""
    };
    div()
        .flex()
        .items_center()
        .gap(theme::S3)
        .child(div().flex_1().h(px(1.)).bg(theme::LINE))
        .child(
            div()
                .text_label()
                .fg_muted()
                .child(format!("{prefix}{}", local.format("%A, %b %-d"))),
        )
        .child(div().flex_1().h(px(1.)).bg(theme::LINE))
}

/// The blinking caret shown while a reply streams.
fn caret(id: &str) -> impl IntoElement {
    div()
        .w(px(7.))
        .h(px(14.))
        .rounded(px(1.))
        .bg(theme::ACCENT)
        .with_animation(
            eid("caret", id),
            Animation::new(Duration::from_millis(1000))
                .repeat()
                .with_easing(pulsating_between(0.15, 1.0)),
            |d, t| d.opacity(t),
        )
}

/// A small rotating-feel loader for a tool call in flight.
fn spinner(id: &str) -> impl IntoElement {
    div()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .child(dot_el(theme::INFO, px(6.)))
        .with_animation(
            eid("tool-spin", id),
            Animation::new(Duration::from_millis(900))
                .repeat()
                .with_easing(pulsating_between(0.3, 1.0)),
            |d, t| d.opacity(t),
        )
}
