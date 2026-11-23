//! Lower run sections of the detail panel: the tasks each orchestrating step handed its
//! sub-agents, the checks it ran, the run's workdirs and the live log.

use gpui::{AnyElement, Div, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use pipeline::{CoderTask, Run, StageStatus};
use ui::theme::*;
use ui::{Status, icon, status_dot};

use crate::detail::{section, verified_badge};
use crate::look;
use crate::run::plain_markdown;
use crate::widgets::{Ellipsis, heading, mono, small};
use crate::{IssuesScreen, LOG_LINES};

/// One orchestrating step and the tasks it gave out.
struct TaskGroup<'a> {
    /// Step name (`Build`), or "Sub-agents" for tasks of an unknown step.
    name: String,
    /// The sub-agents' name (`Coder`), when the pipeline says.
    delegate: Option<String>,
    status: StageStatus,
    tasks: Vec<&'a CoderTask>,
}

/// Steps that lead sub-agents and have started, plus any step that has tasks; in step order.
fn task_groups(run: &Run) -> Vec<TaskGroup<'_>> {
    let delegate = |id: &str| {
        run.def
            .as_ref()
            .and_then(|d| d.step(id))
            .and_then(|s| s.delegate.as_ref())
            .map(|d| d.name.clone())
    };
    let mut out: Vec<TaskGroup> = run
        .stages
        .iter()
        .filter_map(|s| {
            let tasks: Vec<&CoderTask> = run.tasks.iter().filter(|t| t.step == s.id).collect();
            let leads = delegate(&s.id);
            let started = !matches!(s.status, StageStatus::Pending | StageStatus::Skipped);
            (!tasks.is_empty() || (leads.is_some() && started)).then(|| TaskGroup {
                name: s.name.clone(),
                delegate: leads,
                status: s.status,
                tasks,
            })
        })
        .collect();
    let orphans: Vec<&CoderTask> = run
        .tasks
        .iter()
        .filter(|t| run.stage(&t.step).is_none())
        .collect();
    if !orphans.is_empty() {
        out.push(TaskGroup {
            name: "Sub-agents".into(),
            delegate: None,
            status: StageStatus::Done,
            tasks: orphans,
        });
    }
    out
}

fn task_dot(s: StageStatus) -> Status {
    match s {
        StageStatus::Pending | StageStatus::Skipped => Status::Draft,
        StageStatus::Running => Status::Running,
        StageStatus::Waiting => Status::Waiting,
        StageStatus::Done => Status::Live,
        StageStatus::Failed => Status::Error,
    }
}

fn task_row(i: usize, t: &CoderTask) -> Div {
    div()
        .flex()
        .gap(px(10.))
        .py(px(8.))
        .when(i > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
        .child(div().pt(px(6.)).child(status_dot(task_dot(t.status), true)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .ellipsis()
                                .child(SharedString::from(t.title.clone())),
                        )
                        .child(small(
                            look::stage_word(t.status),
                            look::stage_color(t.status),
                        )),
                )
                .when(!t.summary.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(plain_markdown(&t.summary))),
                    )
                }),
        )
}

impl IssuesScreen {
    /// One section per orchestrating step: the tasks it handed to its sub-agents.
    pub(crate) fn render_tasks(&self, run: &Run) -> Vec<AnyElement> {
        task_groups(run)
            .into_iter()
            .map(|g| {
                let done = g
                    .tasks
                    .iter()
                    .filter(|t| t.status == StageStatus::Done)
                    .count();
                let who = g
                    .delegate
                    .as_ref()
                    .map_or_else(|| "sub-agents".to_string(), |d| format!("{d}s"));
                let empty = if g.status == StageStatus::Running {
                    format!("No tasks handed to {who} yet.")
                } else {
                    format!("It handed no tasks to {who}.")
                };
                let n = g.tasks.len();
                section()
                    .child(
                        heading(g.name.clone())
                            .child(small(format!("tasks for {who}"), TEXT_MUTED))
                            .child(div().flex_1())
                            .when(n > 0, |d| {
                                d.child(mono(format!("{done} of {n} done"), TEXT_MUTED))
                            }),
                    )
                    .when(n == 0, |d| d.child(small(empty, TEXT_MUTED)))
                    .when(n > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .px(px(12.))
                                .rounded(px(RADIUS_LG))
                                .border_1()
                                .border_color(LINE)
                                .bg(SURFACE)
                                .children(g.tasks.iter().enumerate().map(|(i, t)| task_row(i, t))),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// Whether an orchestrating step verified its work, and the checks it ran.
    pub(crate) fn render_checks(&self, run: &Run) -> impl IntoElement {
        section()
            .child(heading("Checks").child(verified_badge(run.verified)))
            .when(run.checks.trim().is_empty(), |d| {
                d.child(small("No checks were reported.", TEXT_MUTED))
            })
            .when(!run.checks.trim().is_empty(), |d| {
                d.child(
                    div()
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_color(LINE)
                        .bg(BG)
                        .font_family(FONT_MONO)
                        .text_size(px(TEXT_MONO_SM))
                        .line_height(px(16.))
                        .text_color(TEXT_SECONDARY)
                        .children(plain_markdown(run.checks.trim()).lines().map(|l| {
                            div()
                                .min_h(px(16.))
                                .child(SharedString::from(l.to_string()))
                        })),
                )
            })
    }

    /// The folders the run works in: name, branch and path.
    pub(crate) fn render_workdirs(&self, run: &Run) -> impl IntoElement {
        let rows = run.workdirs.iter().enumerate().map(|(i, w)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .py(px(8.))
                .when(i > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(SharedString::from(
                            if w.name.is_empty() { w.id.clone() } else { w.name.clone() },
                        )))
                        .child(div().flex_1())
                        .when_some(w.branch.clone(), |d, b| {
                            d.child(icon("git-branch", 12., TEXT_MUTED))
                                .child(mono(b, TEXT_SECONDARY))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(icon("folder", 12., TEXT_MUTED))
                        .child(
                            div()
                                .flex_1()
                                .ellipsis()
                                .font_family(FONT_MONO)
                                .text_size(px(TEXT_MONO_SM))
                                .text_color(TEXT_MUTED)
                                .child(SharedString::from(pipeline::def::tilde(&w.path))),
                        ),
                )
        });
        section()
            .child(heading("Workdirs").child(div().flex_1()).child(mono(
                run.workdirs.len().to_string(),
                TEXT_MUTED,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(12.))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE)
                    .bg(SURFACE)
                    .children(rows),
            )
    }

    /// The last [`LOG_LINES`] lines, newest at the bottom; follows new lines.
    pub(crate) fn render_log(&self, run: &Run) -> impl IntoElement {
        let offset = ui::time::utc_offset(ui::time::now_secs());
        let total = run.log.len();
        let from = total.saturating_sub(LOG_LINES);
        let lines = run.log[from..].iter().map(|l| {
            div()
                .flex()
                .gap(px(8.))
                .py(px(1.))
                .child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .text_color(TEXT_DISABLED)
                        .child(look::clock(l.at, offset)),
                )
                .child(
                    div()
                        .w(px(96.))
                        .flex_none()
                        .ellipsis()
                        .text_color(look::log_who_color(l.kind))
                        .child(SharedString::from(l.who.clone())),
                )
                // Wrapped text directly in a row is measured too tall; a column cell is not.
                .child(
                    div().flex().flex_col().flex_1().min_w_0().child(
                        div()
                            .text_color(look::log_text_color(l.kind))
                            .child(SharedString::from(clip(&l.text))),
                    ),
                )
        });
        let meta = if total > LOG_LINES {
            format!("last {LOG_LINES} of {total}")
        } else if total == 1 {
            "1 line".to_string()
        } else {
            format!("{total} lines")
        };

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(SPACE_3))
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .child(
                heading("Log")
                    .child(div().flex_1())
                    .child(mono(meta, TEXT_MUTED)),
            )
            .child(
                div()
                    .id("run-log")
                    .track_scroll(&self.log_scroll)
                    .overflow_y_scroll()
                    .h(px(320.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE)
                    .bg(BG)
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO_SM))
                    .line_height(px(16.))
                    .when(total == 0, |d| {
                        d.child(small("No log lines yet.", TEXT_MUTED))
                    })
                    .child(div().flex().flex_col().children(lines)),
            )
    }
}

/// A log line's text, cut to a few lines (tool output can be long).
fn clip(text: &str) -> String {
    const MAX_LINES: usize = 6;
    const MAX_CHARS: usize = 600;
    let mut out: String = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect();
    }
    if out.len() < text.trim_end().len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::config::legacy_default;

    #[test]
    fn tasks_group_by_step() {
        let mut run = Run::new("HAR-1", &legacy_default());
        run.tasks = vec![
            CoderTask {
                title: "a".into(),
                status: StageStatus::Done,
                summary: String::new(),
                step: "build".into(),
            },
            CoderTask {
                title: "b".into(),
                status: StageStatus::Running,
                summary: String::new(),
                step: "gone".into(),
            },
        ];
        let g = task_groups(&run);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].name.as_str(), g[0].delegate.as_deref()), ("Build", Some("Coder")));
        assert_eq!(g[1].name, "Sub-agents");
        // A delegating step that has not started and has no tasks is left out.
        run.tasks.clear();
        assert!(task_groups(&run).is_empty());
    }
}
