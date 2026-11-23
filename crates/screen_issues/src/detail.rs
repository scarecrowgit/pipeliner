//! Right-hand issue detail panel: head, title and description, the issue's pipeline, run
//! actions, then the run's sections: steps ([`crate::progress`]), review ([`crate::run`]),
//! sub-agent tasks, checks, workdirs and the log ([`crate::work`]).

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use pipeline::{Issue, Run, RunStatus};
use ui::theme::*;
use ui::{Badge, Button, ButtonVariant, Status, Tone};

use crate::IssuesScreen;
use crate::look::{column_dot, column_of};
use crate::menu::Menu;
use crate::widgets::{mono, small};

/// Width of the panel: wide enough for before/after renders side by side.
pub(crate) const DETAIL_WIDTH: f32 = 560.;

/// Panel section with the standard padding and a bottom rule.
pub(crate) fn section() -> Div {
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(SPACE_3))
        .px(px(SPACE_5))
        .py(px(SPACE_4))
        .border_b_1()
        .border_color(LINE)
}

impl IssuesScreen {
    pub(crate) fn render_detail(
        &self,
        issue: &Issue,
        run: Option<&Run>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut panel = div()
            .id("issue-detail")
            .w(px(DETAIL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .track_scroll(&self.detail_scroll)
            .overflow_y_scroll()
            .border_l_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(self.render_detail_head(issue, cx))
            .child(self.render_about(issue))
            .child(self.render_pipeline_row(issue, run, active, cx))
            .child(self.render_actions(issue, run, active, cx));
        if let Some(run) = run {
            panel = panel.child(self.render_progress(run, active, cx));
            if !run.renders.is_empty() || run.status == RunStatus::WaitingApproval {
                panel = panel.child(self.render_review(run, active, window, cx));
            }
            panel = panel.children(self.render_tasks(run));
            if run.verified || !run.checks.trim().is_empty() {
                panel = panel.child(self.render_checks(run));
            }
            if !run.workdirs.is_empty() {
                panel = panel.child(self.render_workdirs(run));
            }
            panel = panel.child(self.render_log(run));
        }
        panel.into_any_element()
    }

    fn render_detail_head(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(SPACE_5))
            .py(px(SPACE_3))
            .border_b_1()
            .border_color(LINE)
            .child(mono(issue.key.clone(), TEXT_MUTED))
            .child(Badge::new(issue.kind.clone()))
            .child(
                Badge::new(column_of(issue.status).label()).dot(column_dot(column_of(issue.status))),
            )
            .child(div().flex_1())
            .child(
                Button::new("close-detail", "")
                    .ghost()
                    .small()
                    .icon("x")
                    .on_click(cx.listener(|this, _, _, cx| this.close_selection(cx))),
            )
    }

    fn render_about(&self, issue: &Issue) -> impl IntoElement {
        let body = issue.body.trim();
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(8.))
            .pt(px(SPACE_4))
            .px(px(SPACE_5))
            .pb(px(SPACE_3))
            .child(
                div()
                    .text_size(px(TEXT_TITLE))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(TEXT)
                    .child(issue.title.clone()),
            )
            .child(if body.is_empty() {
                div().text_color(TEXT_MUTED).child("No description.")
            } else {
                div()
                    .text_color(TEXT_SECONDARY)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(body.lines().map(|l| {
                        div()
                            .min_h(px(LINE_BODY))
                            .child(SharedString::from(l.to_string()))
                    }))
            })
    }

    /// The pipeline the issue runs through; a select while no run works on it.
    fn render_pipeline_row(
        &self,
        issue: &Issue,
        run: Option<&Run>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (name, missing) = {
            let p = self.pipes.read(cx);
            (
                p.pipeline_of(issue).map(|d| d.name.clone()),
                p.pipeline(&issue.pipeline).is_none(),
            )
        };
        let name = name.unwrap_or_else(|| "No pipeline".into());
        let mut notes: Vec<String> = Vec::new();
        if missing && !issue.pipeline.is_empty() {
            notes.push(format!(
                "Pipeline `{}` no longer exists; runs use {name}.",
                issue.pipeline
            ));
        }
        if let Some(r) = run
            && r.pipeline != issue.pipeline
            && !r.pipeline_name.is_empty()
        {
            notes.push(format!(
                "The latest run used {}; the next run uses {name}.",
                r.pipeline_name
            ));
        }
        let value = if active {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(CONTROL_HEIGHT))
                .child(div().text_color(TEXT).child(SharedString::from(name)))
                .child(small("Running; change it after the run.", TEXT_MUTED))
        } else {
            self.pipeline_select(Menu::Issue, Some(&issue.pipeline), cx)
        };
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(6.))
            .px(px(SPACE_5))
            .pb(px(SPACE_4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child("Pipeline"),
                    )
                    .child(div().flex_1().min_w_0().child(value)),
            )
            .children(notes.into_iter().map(|n| {
                div()
                    .pl(px(76.))
                    .child(small(n, TEXT_MUTED))
            }))
    }

    /// Start / Cancel / Retry / Delete, the pull request link, and the run's error.
    fn render_actions(
        &self,
        issue: &Issue,
        run: Option<&Run>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = run.map(|r| r.status);
        let can_retry = !active && matches!(status, Some(RunStatus::Failed | RunStatus::Cancelled));
        let can_start = !active
            && !matches!(status, Some(RunStatus::WaitingApproval))
            && (run.is_none() || can_retry);
        let pr = run
            .and_then(|r| r.pr_url.clone())
            .or_else(|| issue.pr_url.clone());
        let error = run
            .filter(|r| r.status == RunStatus::Failed)
            .and_then(|r| r.error.clone());
        let key = issue.key.clone();

        let buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .when(can_start, |d| {
                let b = Button::new(
                    "issue-start",
                    if run.is_none() {
                        "Start pipeline"
                    } else {
                        "Start over"
                    },
                )
                .icon("play")
                .on_click(cx.listener(|this, _, _, cx| this.start(cx)));
                d.child(if run.is_none() { b.primary() } else { b })
            })
            .when(can_retry, |d| {
                d.child(
                    Button::new("issue-retry", "Retry from here")
                        .primary()
                        .icon("replay")
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                )
            })
            .when_some(pr, |d, url| {
                d.child(
                    Button::new("issue-pr", "Open pull request")
                        .icon("git-branch")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .child(div().flex_1())
            .when(active, |d| {
                d.child(
                    Button::new("issue-cancel", "Cancel run")
                        .variant(ButtonVariant::Danger)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            })
            .when(!active && !self.confirm_delete, |d| {
                d.child(
                    Button::new("issue-delete", "Delete…")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.ask_delete(true, cx))),
                )
            });

        let confirm = (!active && self.confirm_delete).then(|| {
            ui::motion::reveal(
                "issue-delete-confirm",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(DANGER_SOFT)
                    .child(div().flex_1().min_w_0().text_color(TEXT_SECONDARY).child(
                        SharedString::from(format!(
                            "Delete {key}? The issue leaves the board; branches and the PR stay."
                        )),
                    ))
                    .child(
                        Button::new("issue-delete-keep", "Keep")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.ask_delete(false, cx))),
                    )
                    .child(
                        Button::new("issue-delete-yes", "Delete issue")
                            .small()
                            .variant(ButtonVariant::Danger)
                            .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                    ),
            )
        });

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(SPACE_3))
            .px(px(SPACE_5))
            .pb(px(SPACE_4))
            .border_b_1()
            .border_color(LINE)
            .child(buttons)
            .children(confirm)
            .when_some(self.action_error.clone(), |d, e| d.child(small(e, DANGER)))
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .w_full()
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_color(DANGER_SOFT)
                        .bg(DANGER_SOFT)
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(DANGER)
                                .child("Run failed"),
                        )
                        .child(
                            div()
                                .mt(px(4.))
                                .font_family(FONT_MONO)
                                .text_size(px(TEXT_MONO))
                                .text_color(TEXT_SECONDARY)
                                .child(SharedString::from(e)),
                        ),
                )
            })
    }

}

/// Tone of the "Verified" badge.
pub(crate) fn verified_badge(verified: bool) -> Badge {
    if verified {
        Badge::new("Verified").tone(Tone::Success).dot(Status::Live)
    } else {
        Badge::new("Not verified").dot(Status::Draft)
    }
}
