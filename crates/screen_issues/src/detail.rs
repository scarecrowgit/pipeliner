//! Right-hand issue detail panel: title, properties (status menu, runner), run
//! controls, activity feed and the comment box.

use gpui::{
    AnyElement, App, Context, FontWeight, Hsla, IntoElement, MouseDownEvent, SharedString, Window,
    deferred, div, prelude::*, px,
};
use ui::nav::{Screen, navigate};
use ui::theme::*;
use ui::{Button, ButtonVariant, checkbox, icon, spawn_chip, status_dot};

use crate::IssuesScreen;
use crate::data::{
    Activity, Actor, Column, Issue, RunState, Runner, RunnerKind, Trigger, WARNING_LINE,
    person_name,
};
use crate::widgets::{person_avatar, runner_tile, small};

/// Width of the `dt` column in the properties grid.
const DT_WIDTH: f32 = 96.;

fn prop_row(name: &str, value: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(20.))
        .child(
            div()
                .w(px(DT_WIDTH))
                .flex_none()
                .child(small(name.to_string(), TEXT_MUTED)),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// 28px bordered link-style button (used for "Review variants").
fn warning_link(id: &str, text: &str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(CONTROL_HEIGHT_SM))
        .px(px(10.))
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(WARNING_LINE)
        .bg(WARNING_SOFT)
        .text_color(WARNING)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(text.to_string())
}

impl IssuesScreen {
    pub(crate) fn render_detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(issue) = self.selected_issue() else {
            return div().into_any_element();
        };

        div()
            .id("issue-detail")
            .w(px(360.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(self.render_detail_head(issue, cx))
            .child(self.render_title(issue, window, cx))
            .child(self.render_props(issue, cx))
            .when(self.picker_open, |d| d.child(self.render_picker(cx)))
            .when_some(issue.runner(), |d, runner| {
                d.child(self.render_run_section(issue, runner, cx))
            })
            .child(self.render_activity(issue, cx))
            .into_any_element()
    }

    fn render_detail_head(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(12.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(issue.key.clone()),
            )
            .when_some(issue.created_by.clone(), |d, by| {
                d.child(spawn_chip(format!("created by {by}")))
            })
            .child(div().flex_1())
            .child(
                Button::new("close-detail", "")
                    .ghost()
                    .small()
                    .icon("x")
                    .on_click(cx.listener(|this, _, _, cx| this.close_selection(cx))),
            )
    }

    fn render_title(&self, issue: &Issue, window: &Window, cx: &App) -> impl IntoElement {
        let focused = self.title_input.read(cx).is_focused(window);
        div()
            .flex_none()
            .pt(px(16.))
            .px(px(20.))
            .pb(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .ml(px(-6.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(if focused {
                        LINE_STRONG
                    } else {
                        gpui::transparent_black()
                    })
                    .when(!focused, |d| d.hover(|s| s.border_color(LINE)))
                    .text_size(px(TEXT_TITLE))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(TEXT)
                    .child(self.title_input.clone()),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_color(TEXT_SECONDARY)
                    .child(issue.desc.clone()),
            )
    }

    fn render_props(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = issue.person.and_then(person_name).unwrap_or("No owner");
        let initials = issue.person.unwrap_or("—");

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(20.))
            .pb(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(prop_row("Status", self.render_status_select(issue, cx)))
            .child(prop_row(
                "Priority",
                small(issue.prio.label(), issue.prio.color()),
            ))
            .child(prop_row(
                "Owner",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .child(person_avatar(initials, 20., 9.))
                    .child(owner),
            ))
            .child(prop_row("Runs with", self.render_runs_with(issue, cx)))
    }

    fn render_status_select(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let current = issue.status;
        let menu = self.status_menu_open.then(|| {
            deferred(
                div()
                    .id("status-menu")
                    .absolute()
                    .top(px(32.))
                    .left_0()
                    .w(px(160.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE_RAISED)
                    .shadow(popover_shadow())
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        this.dismiss_status_menu(ev.position, cx)
                    }))
                    .children(Column::ALL.into_iter().map(|c| {
                        let on = c == current;
                        div()
                            .id(SharedString::from(format!("status-{}", c.name())))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(28.))
                            .px(px(8.))
                            .rounded(px(RADIUS_SM))
                            .text_size(px(12.))
                            .text_color(TEXT)
                            .cursor_pointer()
                            .hover(|s| s.bg(SURFACE_HOVER))
                            .child(status_dot(c.dot(), true))
                            .child(div().flex_1().child(c.name()))
                            .when(on, |d| d.child(icon("check", 12., ACCENT_TEXT)))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_status(c, cx)))
                    })),
            )
            .with_priority(1)
        });

        div()
            .relative()
            .flex()
            .child(
                div()
                    .id("status-select")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(CONTROL_HEIGHT_SM))
                    .px(px(8.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(if self.status_menu_open {
                        ACCENT_STRONG
                    } else {
                        LINE_STRONG
                    })
                    .bg(SURFACE_RAISED)
                    .text_size(px(12.))
                    .text_color(TEXT)
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .child(current.name())
                    .child(icon("chevron-down", 12., TEXT_MUTED))
                    .on_click(cx.listener(|this, ev, _, cx| this.toggle_status_menu(ev, cx))),
            )
            .children(menu)
    }

    fn render_runs_with(&self, issue: &Issue, cx: &mut Context<Self>) -> AnyElement {
        if let Some(r) = issue.runner() {
            return div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(runner_tile(r.kind))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_baseline()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .font_weight(FontWeight::MEDIUM)
                                .child(r.name),
                        )
                        .child(small(r.kind.label(), TEXT_MUTED).flex_none()),
                )
                .child(
                    Button::new("change-runner", "Change")
                        .ghost()
                        .small()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)),
                        ),
                )
                .into_any_element();
        }
        div()
            .flex()
            .child(
                div()
                    .id("attach-runner")
                    .flex()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .min_h(px(CONTROL_HEIGHT_SM))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_dashed()
                    .border_color(ACCENT_RING)
                    .when(self.picker_open, |d| d.bg(ACCENT_GLOW))
                    .text_color(ACCENT_TEXT)
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .cursor_pointer()
                    .hover(|s| s.bg(ACCENT_GLOW))
                    .child(icon("plus", 12., ACCENT_TEXT))
                    .child(
                        div()
                            .min_w_0()
                            .text_center()
                            .child("Attach agent, orchestrator or pipeline"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx))),
            )
            .into_any_element()
    }

    fn render_run_section(
        &self,
        issue: &Issue,
        r: &'static Runner,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let steps: Vec<&str> = std::iter::once("Jira")
            .chain(r.steps.iter().copied())
            .collect();
        let last = steps.len() - 1;
        let waiting = issue
            .run
            .as_ref()
            .is_some_and(|run| run.state == RunState::Waiting);

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(10.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            // Route: Jira › Router › … › You
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(6.))
                    .gap_y(px(4.))
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .children(steps.into_iter().enumerate().map(|(i, s)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_color(if i == last { WARNING } else { TEXT_MUTED })
                                    .child(s),
                            )
                            .when(i < last, |d| {
                                d.child(div().text_color(TEXT_DISABLED).child("›"))
                            })
                    })),
            )
            .when_some(issue.run.as_ref(), |d, run| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(RADIUS_LG))
                        .bg(run.state.bg())
                        .child(status_dot(run.state.dot(), true))
                        .child(
                            div()
                                .flex_1()
                                .text_color(run.state.color())
                                .child(run.text.clone()),
                        )
                        .when(run.state == RunState::Running, |d| {
                            d.child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(TEXT_MUTED)
                                    .child(format!("{}%", run.pct)),
                            )
                        }),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(4.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(TEXT_MUTED)
                            .child("Start when"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_x(px(16.))
                            .gap_y(px(4.))
                            .children(Trigger::ALL.into_iter().zip(issue.triggers).map(
                                |(t, on)| {
                                    div()
                                        .id(SharedString::from(format!("trigger-{}", t.label())))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .min_h(px(28.))
                                        .text_size(px(12.))
                                        .cursor_pointer()
                                        .child(checkbox(
                                            SharedString::from(format!(
                                                "trigger-box-{}",
                                                t.label()
                                            )),
                                            on,
                                        ))
                                        .child(t.label())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_trigger(t, cx)
                                        }))
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .when(issue.startable(), |d| {
                        d.child(
                            Button::new("start-run", "Start now")
                                .small()
                                .icon("play")
                                .on_click(cx.listener(|this, _, _, cx| this.start_run(cx))),
                        )
                    })
                    .when(r.kind == RunnerKind::Pipeline, |d| {
                        d.child(
                            Button::new("open-pipeline", "Open pipeline")
                                .small()
                                .on_click(|_, _, cx| navigate(Screen::PipelineEditor, cx)),
                        )
                    })
                    .when(r.kind != RunnerKind::Pipeline, |d| {
                        d.child(
                            Button::new("open-run", "Open run")
                                .small()
                                .on_click(|_, _, cx| navigate(Screen::Chat, cx)),
                        )
                    })
                    .when(waiting, |d| {
                        d.child(
                            warning_link("review-variants", "Review variants")
                                .on_click(|_, _, cx| navigate(Screen::Inbox, cx)),
                        )
                    })
                    .child(
                        Button::new("detach", "Detach")
                            .small()
                            .variant(ButtonVariant::Danger)
                            .on_click(cx.listener(|this, _, _, cx| this.detach(cx))),
                    ),
            )
    }

    fn render_activity(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.comment_input.read(cx).text().trim().is_empty();
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(12.))
            .px(px(20.))
            .py(px(16.))
            .child(div().font_weight(FontWeight::MEDIUM).child("Activity"))
            .children(issue.activity.iter().map(activity_item))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(small(
                        "Comment · @mention an agent or pipeline to wake it up",
                        TEXT_MUTED,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .min_h(px(58.))
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(RADIUS_LG))
                            .border_1()
                            .border_color(LINE_STRONG)
                            .bg(SURFACE_RAISED)
                            .text_color(TEXT)
                            .child(self.comment_input.clone()),
                    )
                    .child(
                        div().flex().child(
                            Button::new("post-comment", "Comment")
                                .small()
                                .disabled(empty)
                                .on_click(cx.listener(|this, _, _, cx| this.post_comment(cx))),
                        ),
                    ),
            )
    }
}

fn activity_item(a: &Activity) -> impl IntoElement {
    let person = a.actor == Actor::Person;
    let tile_bg: Hsla = match a.actor {
        Actor::Pipeline => INFO_SOFT,
        Actor::Agent if a.spawn => SPAWN_SOFT,
        Actor::Agent => ACCENT_SOFT,
        Actor::Person => SURFACE_HOVER,
    };
    let who: SharedString = if person {
        person_name(&a.who)
            .map(SharedString::from)
            .unwrap_or_else(|| a.who.clone())
    } else {
        a.who.clone()
    };
    let tile = if person {
        person_avatar(&a.who, 22., 9.)
    } else {
        div()
            .size(px(22.))
            .flex_none()
            .rounded(px(RADIUS_MD))
            .bg(tile_bg)
    };

    div().flex().gap(px(10.)).child(tile).child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if a.spawn { SPAWN_TEXT } else { TEXT })
                            .child(who),
                    )
                    .child(div().text_color(TEXT_MUTED).child(a.when.clone())),
            )
            .child(
                div()
                    .text_color(if a.spawn_note {
                        SPAWN_TEXT
                    } else {
                        TEXT_SECONDARY
                    })
                    .when(a.blocker, |d| {
                        d.px(px(10.))
                            .py(px(8.))
                            .rounded(px(RADIUS_LG))
                            .bg(WARNING_SOFT)
                            .border_1()
                            .border_color(WARNING_LINE)
                    })
                    .child(a.text.clone()),
            ),
    )
}
