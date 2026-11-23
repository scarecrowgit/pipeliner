//! The "Pipeline" section of the issue panel: one row per step of the run, whatever the
//! steps are — marker, name, kind, status, timing and summary, with a decision's answer and
//! an agent's or command's notes under it.

use gpui::{Context, Div, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use pipeline::{Decision, Run, Stage, StageStatus, StepKind};
use ui::theme::*;
use ui::{Button, Status, icon, status_dot};

use crate::IssuesScreen;
use crate::detail::section;
use crate::look;
use crate::run::plain_markdown;
use crate::widgets::{heading, mono, small};

/// Lines of a step's notes shown before "Show all".
const NOTE_LINES: usize = 6;

/// 16px stage marker: ring (pending), dot (running / waiting), check, dash or cross.
fn stage_marker(s: StageStatus) -> Div {
    let inner = match s {
        StageStatus::Pending => status_dot(Status::Draft, false),
        StageStatus::Running => status_dot(Status::Running, false),
        StageStatus::Waiting => status_dot(Status::Waiting, false),
        StageStatus::Done => div().child(icon("check", 14., SUCCESS)),
        StageStatus::Skipped => div().child(icon("minus", 14., TEXT_MUTED)),
        StageStatus::Failed => div().child(icon("x", 14., DANGER)),
    };
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
}

/// Small outlined chip (a decision's notes).
fn chip(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(6.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(LINE_STRONG)
        .bg(SURFACE_RAISED)
        .font_family(FONT_MONO)
        .text_size(px(TEXT_MONO_SM))
        .text_color(TEXT_SECONDARY)
        .child(text.into())
}

/// `ran 4m 12s · 48.2k tokens · $1.20`.
fn run_meta(run: &Run, active: bool, now: u64) -> String {
    let mut meta = format!(
        "ran {} · {} tokens",
        look::duration(look::run_elapsed(run, active, now)),
        look::tokens(run.tokens)
    );
    if run.cost_usd > 0. {
        meta.push_str(&format!(" · ${:.2}", run.cost_usd));
    }
    meta
}

impl IssuesScreen {
    /// Every step of the run with its status, timing and what it reported.
    pub(crate) fn render_progress(
        &self,
        run: &Run,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let now = ui::time::now_secs();
        let last = run.stages.len().saturating_sub(1);
        let rows: Vec<Div> = run
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| self.stage_row(run, i, s, i == last, now, cx))
            .collect();
        let name = if run.pipeline_name.is_empty() {
            "Pipeline".to_string()
        } else {
            run.pipeline_name.clone()
        };

        section()
            .child(
                heading(name)
                    .child(div().flex_1())
                    .child(mono(run_meta(run, active, now), TEXT_MUTED)),
            )
            .when(run.stages.is_empty(), |d| {
                d.child(small("This run has no steps.", TEXT_MUTED))
            })
            .child(div().flex().flex_col().children(rows))
    }

    fn stage_row(
        &self,
        run: &Run,
        ix: usize,
        s: &Stage,
        last: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Div {
        let timing = match (s.started, s.finished, s.status) {
            (0, _, _) => None,
            (t0, _, StageStatus::Running | StageStatus::Waiting) => {
                Some(look::duration(now.saturating_sub(t0)))
            }
            (t0, t1, _) if t1 >= t0 => Some(look::duration(t1 - t0)),
            _ => None,
        };
        let color = look::stage_color(s.status);
        let dim = matches!(s.status, StageStatus::Pending | StageStatus::Skipped);
        let mut detail = div().flex().flex_col().gap(px(6.)).min_w_0();
        // "waiting for you" would only repeat the status word.
        if !s.summary.is_empty() && !s.summary.eq_ignore_ascii_case(look::stage_word(s.status)) {
            detail = detail.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_SECONDARY)
                    .child(SharedString::from(s.summary.clone())),
            );
        }
        if let Some(d) = run.decisions.iter().rev().find(|d| d.step == s.id) {
            detail = detail.child(decision(d));
        }
        if let Some(notes) = run.notes.get(&s.id).filter(|n| !n.trim().is_empty()) {
            let mono_notes = s.step_kind() == Some(StepKind::Command);
            detail = detail.child(self.notes(ix, &s.id, notes, mono_notes, cx));
        }

        div()
            .flex()
            .gap(px(10.))
            // Marker column with the guide line down to the next stage.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .w(px(16.))
                    .flex_none()
                    .pt(px(2.))
                    .child(stage_marker(s.status))
                    .when(!last, |d| {
                        d.child(
                            div()
                                .w(px(1.))
                                .flex_1()
                                .min_h(px(8.))
                                .my(px(2.))
                                .bg(LINE_STRONG),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .pb(px(if last { 0. } else { 12. }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if dim { TEXT_MUTED } else { TEXT })
                                    .child(SharedString::from(s.name.clone())),
                            )
                            .child(small(look::kind_label(&s.kind), TEXT_MUTED))
                            .child(small(look::stage_word(s.status), color))
                            .child(div().flex_1())
                            .when_some(timing, |d, t| d.child(mono(t, TEXT_MUTED))),
                    )
                    .child(detail),
            )
    }

    /// A step's notes as plain text: the first lines, or all of them when expanded.
    fn notes(
        &self,
        ix: usize,
        step: &str,
        notes: &str,
        mono_notes: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let text = plain_markdown(notes.trim());
        let lines: Vec<&str> = text.lines().collect();
        let open = self.notes_open.contains(step);
        let long = lines.len() > NOTE_LINES;
        let shown = if open || !long { lines.len() } else { NOTE_LINES };
        let body = lines[..shown].join("\n");
        let step = step.to_string();

        let block = div()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(LINE)
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(TEXT_SECONDARY);
        let block = if mono_notes {
            block
                .bg(BG)
                .font_family(FONT_MONO)
                .text_size(px(TEXT_MONO_SM))
                .line_height(px(16.))
                .children(
                    lines[..shown]
                        .iter()
                        .map(|l| div().min_h(px(16.)).child(SharedString::from(l.to_string()))),
                )
        } else {
            block.bg(SURFACE).child(SharedString::from(body))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(block)
            .when(long, |d| {
                d.child(
                    div().flex().child(
                        Button::new(
                            ("notes-more", ix),
                            if open {
                                "Show less".to_string()
                            } else {
                                format!("Show all {} lines", lines.len())
                            },
                        )
                        .small()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_notes(step.clone(), cx)
                        })),
                    ),
                )
            })
    }
}

/// `Yes · reason` (or No) with the decision's notes as chips.
fn decision(d: &Decision) -> Div {
    let (word, color) = if d.yes { ("Yes", SUCCESS) } else { ("No", WARNING) };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            // Wrapped text directly in a row is measured too tall; a column cell is not.
            div()
                .flex()
                .gap(px(6.))
                .text_size(px(12.))
                .line_height(px(18.))
                .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(color).child(word))
                .when(!d.reason.trim().is_empty(), |r| {
                    r.child(div().flex_none().text_color(TEXT_DISABLED).child("·"))
                        .child(
                            div().flex().flex_col().flex_1().min_w_0().child(
                                div()
                                    .text_color(TEXT_SECONDARY)
                                    .child(SharedString::from(d.reason.trim().to_string())),
                            ),
                        )
                }),
        )
        .when(!d.notes.is_empty(), |c| {
            c.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(d.notes.iter().map(|n| chip(n.clone()))),
            )
        })
}
