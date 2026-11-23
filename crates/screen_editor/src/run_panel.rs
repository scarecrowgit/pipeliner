//! Bottom run panel: the latest run of every issue on this pipeline (per-step timeline,
//! status, tokens, cost) with totals, and the Problems tab (definition problems, unreadable
//! pipeline files, failed runs).

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, relative};
use pipeline::{PipelineDef, RunStatus, StageStatus, StepKind};
use ui::{Button, Stat, TabItem, Tabs, Tone, icon, theme::*};

use crate::PipelineEditorScreen;
use crate::snap::{Problem, RunView, Snap, cost, duration, segment_color, segments, tokens};
use crate::widgets::link_button;

/// Widths of the run list columns.
const ISSUE_W: f32 = 240.;
const STATUS_W: f32 = 170.;
const TIME_W: f32 = 64.;
const TOKENS_W: f32 = 56.;
const COST_W: f32 = 56.;

impl PipelineEditorScreen {
    pub(crate) fn render_run_panel(&self, def: &PipelineDef, snap: &Snap, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let problems = snap.problems();
        let select = cx.listener(|this, i: &usize, _, cx| {
            this.run_tab = *i;
            cx.notify();
        });
        let mut problems_tab = TabItem::new("Problems");
        if !problems.is_empty() {
            problems_tab = problems_tab.count(problems.len().to_string());
        }
        let tabs = Tabs::new(
            "run-tabs",
            vec![TabItem::new("Runs").count(snap.runs.len().to_string()), problems_tab],
            self.run_tab,
        )
        .on_select(move |i, window, cx| select(&i, window, cx));

        let body = match self.run_tab {
            0 => self.runs_tab(def, snap),
            _ => self.problems_tab(problems, cx),
        };
        div()
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(div().px(px(24.)).border_b_1().border_color(LINE).child(tabs))
            .child(body)
    }

    fn runs_tab(&self, def: &PipelineDef, snap: &Snap) -> AnyElement {
        let waiting = snap.waiting();
        let failed = snap.failed();
        let has_pr = def.steps.iter().any(|s| s.kind == StepKind::PullRequest);
        let stat_cell = |first: bool, stat: Stat| {
            div()
                .px(px(24.))
                .when(first, |d| d.pl_0())
                .when(!first, |d| d.border_l_1().border_color(LINE))
                .child(stat)
        };
        let stats = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap_y(px(16.))
            .px(px(24.))
            .py(px(14.))
            .border_b_1()
            .border_color(LINE)
            .child(stat_cell(true, Stat::new("Runs", snap.runs.len().to_string()).sub("latest per issue")))
            .child(stat_cell(
                false,
                Stat::new("Active", snap.active().to_string())
                    .sub("agents working")
                    .tone(if snap.active() > 0 { Tone::Info } else { Tone::Neutral }),
            ))
            .child(stat_cell(
                false,
                Stat::new("Waiting for you", waiting.to_string())
                    .sub("approvals")
                    .tone(if waiting > 0 { Tone::Warning } else { Tone::Neutral }),
            ))
            .child(stat_cell(
                false,
                Stat::new("Failed", failed.to_string())
                    .sub("retry from the issue")
                    .tone(if failed > 0 { Tone::Danger } else { Tone::Neutral }),
            ))
            .when(has_pr || snap.prs() > 0, |d| {
                d.child(stat_cell(false, Stat::new("PRs opened", snap.prs().to_string()).sub("by these runs")))
            })
            .child(stat_cell(
                false,
                Stat::new("Spend", cost(snap.cost())).sub(format!("{} tokens", tokens(snap.tokens()))),
            ))
            .child(div().flex_1())
            .when(waiting > 0, |d| {
                let first = snap
                    .runs
                    .iter()
                    .find(|r| r.status == RunStatus::WaitingApproval)
                    .map(|r| r.key.clone());
                d.child(
                    link_button("open-waiting", 40.)
                        .on_click(move |_, _, cx| {
                            if let Some(k) = &first {
                                ui::nav::open_issue(k.clone(), cx);
                            }
                        })
                        .child(div().size(px(8.)).rounded_full().bg(WARNING))
                        .child(SharedString::from(format!("Review · {waiting} waiting"))),
                )
            });

        if snap.runs.is_empty() {
            return div()
                .child(stats)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(24.))
                        .py(px(20.))
                        .text_color(TEXT_MUTED)
                        .child("No runs yet. Create an issue on this pipeline and start it.")
                        .child(
                            Button::new("empty-open-board", "Open board")
                                .small()
                                .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Issues, cx)),
                        ),
                )
                .into_any_element();
        }

        let now = ui::time::now_secs();
        let head = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(24.))
            .h(px(28.))
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .child(div().w(px(ISSUE_W)).flex_none().child("Issue"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child("Steps")
                    .child(legend("done", segment_color(StageStatus::Done)))
                    .child(legend("running", segment_color(StageStatus::Running)))
                    .child(legend("waiting", segment_color(StageStatus::Waiting)))
                    .child(legend("failed", segment_color(StageStatus::Failed))),
            )
            .child(div().w(px(STATUS_W)).flex_none().child("Status"))
            .child(div().w(px(TIME_W)).flex_none().text_right().child("Time"))
            .child(div().w(px(TOKENS_W)).flex_none().text_right().child("Tokens"))
            .child(div().w(px(COST_W)).flex_none().text_right().child("Cost"));

        let rows: Vec<AnyElement> = snap.runs.iter().enumerate().map(|(i, r)| run_row(i, r, now)).collect();
        div()
            .child(stats)
            .child(div().pt(px(8.)).pb(px(12.)).child(head).children(rows))
            .into_any_element()
    }

    fn problems_tab(&self, problems: Vec<Problem>, cx: &mut Context<Self>) -> AnyElement {
        if problems.is_empty() {
            return div()
                .px(px(24.))
                .py(px(20.))
                .text_color(TEXT_MUTED)
                .child("No problems. Every step has what it needs, the folders are there and no run has failed.")
                .into_any_element();
        }
        div()
            .px(px(24.))
            .pt(px(12.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(problems.into_iter().enumerate().map(|(i, p)| {
                let step = p.step.clone();
                let workdir = p.workdir.clone();
                let issue = p.issue.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(12.))
                    .border_1()
                    .border_color(LINE)
                    .rounded(px(RADIUS_XL))
                    .bg(SURFACE)
                    .child(icon("policy", 14., if p.severe { DANGER } else { WARNING }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::MEDIUM).child(SharedString::from(p.title)))
                            .when(!p.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(TEXT_MUTED)
                                        .line_clamp(2)
                                        .child(SharedString::from(p.detail)),
                                )
                            }),
                    )
                    .when_some(step, |d, id| {
                        d.child(
                            Button::new(("problem-step", i), "Show step")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx))),
                        )
                    })
                    .when_some(workdir, |d, id| {
                        d.child(
                            Button::new(("problem-workdir", i), "Show workdir")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_tab(crate::Tab::Workdirs, cx);
                                    this.expand_workdir(Some(id.clone()), cx);
                                })),
                        )
                    })
                    .when_some(issue, |d, key| {
                        d.child(
                            Button::new(("problem-issue", i), "Open issue")
                                .small()
                                .on_click(move |_, _, cx| ui::nav::open_issue(key.clone(), cx)),
                        )
                    })
            }))
            .into_any_element()
    }
}

fn legend(label: &'static str, color: gpui::Hsla) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(div().size(px(8.)).rounded_full().bg(color))
        .child(label)
}

/// One run: issue, step timeline, status, duration, tokens, cost. Click → the issue.
fn run_row(i: usize, r: &RunView, now: u64) -> AnyElement {
    let (segs, secs) = segments(r, now);
    let (status, status_color) = r.status_label();
    let key = r.key.clone();
    let mono = |text: String, w: f32| {
        div()
            .w(px(w))
            .flex_none()
            .text_right()
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .text_color(TEXT_SECONDARY)
            .child(SharedString::from(text))
    };
    div()
        .id(("run-row", i))
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(24.))
        .h(px(32.))
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE))
        .on_click(move |_, _, cx| ui::nav::open_issue(key.clone(), cx))
        .child(
            div()
                .w(px(ISSUE_W))
                .flex_none()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child(SharedString::from(r.key.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(r.title.clone())),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .relative()
                .h(px(8.))
                .rounded_full()
                .bg(LINE_SUBTLE)
                .children(segs.into_iter().map(|s| {
                    // Keep short steps visible; a 1px gap separates neighbours.
                    let w = (s.end - s.start).max(0.008);
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(s.start.min(1. - w)))
                        .w(relative(w))
                        .rounded_full()
                        .border_r_1()
                        .border_color(BG_SIDEBAR)
                        .bg(segment_color(s.status))
                })),
        )
        .child(
            div()
                .w(px(STATUS_W))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(status_color)
                .child(SharedString::from(status)),
        )
        .child(mono(duration(secs), TIME_W))
        .child(mono(tokens(r.tokens), TOKENS_W))
        .child(mono(cost(r.cost_usd), COST_W))
        .into_any_element()
}
