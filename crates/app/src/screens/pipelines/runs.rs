//! Runs tab: runs of this pipeline, newest first; a row expands into its steps.

use gpui::{Animation, AnimationExt as _};
use harness_core::{Config, Task as HTask, TaskKind};

use super::data::RunRow;
use super::PipelinesScreen;
use crate::screens::prelude::*;

impl PipelinesScreen {
    pub(super) fn render_runs_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = &self.detail else {
            return div().into_any_element();
        };
        if d.runs.is_empty() {
            return body()
                .size_full()
                .child(
                    EmptyState::new(
                        AppIcon::Play,
                        "No runs yet",
                        format!("Start one with Run: the text you give becomes the run's input. Runs use the live v{}.", d.pipeline.version),
                    )
                    .action(
                        Button::new("pl-runs-empty-run", "Run")
                            .primary()
                            .icon(AppIcon::Play)
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_run_dialog(window, cx))),
                    ),
                )
                .into_any_element();
        }
        let more = d.total_runs.saturating_sub(d.runs.len());
        body()
            .size_full()
            .gap(theme::S3)
            .child(
                panel()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S3)
                            .h(px(36.))
                            .px(theme::S4)
                            .border_b_1()
                            .border_color(theme::LINE)
                            .text_label()
                            .fg_muted()
                            .child(div().w(px(16.)))
                            .child(div().w(px(52.)).child("Run"))
                            .child(div().flex_1().child("Title"))
                            .child(div().w(px(150.)).child("Status"))
                            .child(div().w(px(150.)).child("Step"))
                            .child(div().w(px(64.)).text_right().child("Cost"))
                            .child(div().w(px(64.)).text_right().child("Time"))
                            .child(div().w(px(72.)).text_right().child("Started"))
                            .child(div().w(px(132.))),
                    )
                    .children(
                        d.runs
                            .iter()
                            .enumerate()
                            .map(|(ix, r)| self.run_row(ix, r, cx)),
                    ),
            )
            .when(more > 0, |el| {
                el.child(
                    div()
                        .text_label()
                        .fg_muted()
                        .child(format!("{more} older runs are not shown.")),
                )
            })
            .into_any_element()
    }

    fn run_row(&self, ix: usize, r: &RunRow, cx: &mut Context<Self>) -> AnyElement {
        let run = &r.run;
        let expanded = self.expanded_run.as_deref() == Some(run.id.as_str());
        let run_id = run.id.clone();
        let secs =
            (run.finished_at.unwrap_or_else(chrono::Utc::now) - run.created_at).num_seconds();
        let actions = div()
            .w(px(132.))
            .flex()
            .justify_end()
            .gap(theme::S1)
            .when_some(run.pr_url.clone(), |el, url| {
                el.child(
                    IconButton::new(("pl-run-pr", ix), AppIcon::GitPullRequest)
                        .small()
                        .tooltip(format!("Open {url}"))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when_some(r.waiting_gate.clone(), |el, gate| {
                el.child(
                    Button::new(("pl-run-review", ix), "Review")
                        .small()
                        .on_click(move |_, _, cx| {
                            state::navigate(
                                Route::Review {
                                    task: Some(gate.clone()),
                                },
                                cx,
                            )
                        }),
                )
            })
            .when(run.status.is_active(), |el| {
                let id = run.id.clone();
                el.child(
                    Button::new(("pl-run-cancel", ix), "Cancel")
                        .danger()
                        .small()
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            state::run_engine(cx, move |e| e.cancel_run(&id)).detach();
                        }),
                )
            })
            .when_some(
                r.failed_task
                    .clone()
                    .filter(|_| run.status == Status::Failed),
                |el, task| {
                    el.child(
                        Button::new(("pl-run-retry", ix), "Retry")
                            .small()
                            .icon(AppIcon::RotateCcw)
                            .on_click(move |_, _, cx| {
                                let task = task.clone();
                                let job = state::run_engine(cx, move |e| e.retry(&task));
                                cx.spawn(async move |cx| {
                                    if job.await.is_some() {
                                        let _ = cx.update(|cx| {
                                            state::toast(
                                                ToastKind::Info,
                                                "Retrying the failed step",
                                                cx,
                                            )
                                        });
                                    }
                                })
                                .detach();
                            }),
                    )
                },
            );

        let row = div()
            .id(("pl-run-row", ix))
            .flex()
            .items_center()
            .gap(theme::S3)
            .min_h(px(44.))
            .px(theme::S4)
            .cursor_pointer()
            .when(expanded, |el| el.bg(theme::SURFACE_RAISED))
            .hover(|s| s.bg(theme::SURFACE_RAISED))
            .child(div().w(px(16.)).fg_muted().child(icon_sized(
                if expanded {
                    AppIcon::ChevronDown
                } else {
                    AppIcon::ChevronRight
                },
                theme::ICON_SM,
            )))
            .child(
                div()
                    .w(px(52.))
                    .text_mono()
                    .fg_secondary()
                    .child(format!("#{}", run.number)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .ellipsis()
                            .text_body()
                            .fg()
                            .child(if run.title.is_empty() {
                                "Untitled run".to_string()
                            } else {
                                run.title.clone()
                            }),
                    )
                    .when(
                        run.pipeline_version
                            != self
                                .detail
                                .as_ref()
                                .map(|d| d.pipeline.version)
                                .unwrap_or(0),
                        |el| {
                            el.child(
                                div()
                                    .text_label()
                                    .fg_muted()
                                    .child(format!("v{}", run.pipeline_version)),
                            )
                        },
                    )
                    .when_some(
                        run.error.clone().filter(|_| run.status == Status::Failed),
                        |el, e| {
                            el.child(
                                div()
                                    .ellipsis()
                                    .text_label()
                                    .text_color(theme::DANGER)
                                    .child(e),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .child(StatusPill::new(run.status).id(("pl-run-status", ix))),
            )
            .child(
                div()
                    .w(px(150.))
                    .ellipsis()
                    .text_label()
                    .fg_secondary()
                    .child(r.current.clone().unwrap_or_else(|| "—".into())),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_right()
                    .text_mono()
                    .fg_secondary()
                    .child(fmt_usd(run.cost_usd)),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_right()
                    .text_mono()
                    .fg_muted()
                    .child(fmt_duration(secs)),
            )
            .child(
                div()
                    .w(px(72.))
                    .text_right()
                    .text_label()
                    .fg_muted()
                    .child(format!("{} ago", fmt_ago(run.created_at))),
            )
            .child(actions)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.expanded_run = if this.expanded_run.as_deref() == Some(run_id.as_str()) {
                    None
                } else {
                    Some(run_id.clone())
                };
                cx.notify();
            }));

        let mut wrap = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::LINE_SUBTLE)
            .child(row);
        if expanded {
            let tasks = if r.tasks.is_empty() {
                div()
                    .px(theme::S4)
                    .py(theme::S3)
                    .text_label()
                    .fg_muted()
                    .child("No steps have started yet.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .children(
                        r.tasks
                            .iter()
                            .enumerate()
                            .map(|(ti, t)| task_row(ix, ti, t)),
                    )
                    .into_any_element()
            };
            wrap = wrap.child(
                div()
                    .pl(px(44.))
                    .pr(theme::S4)
                    .pb(theme::S2)
                    .bg(theme::SURFACE_RAISED)
                    .child(tasks)
                    .with_animation(
                        ElementId::NamedInteger("pl-run-expand".into(), ix as u64),
                        Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                        |el, t| el.opacity(t),
                    ),
            );
        }
        wrap.into_any_element()
    }
}

/// One step of an expanded run; opens its thread (or Review for a waiting gate).
fn task_row(run_ix: usize, ix: usize, t: &HTask) -> impl IntoElement {
    let id = t.id.clone();
    let gate = t.kind == TaskKind::Gate && t.status == Status::Waiting;
    let secs = match (t.started_at, t.finished_at) {
        (Some(a), Some(b)) => Some((b - a).num_seconds()),
        (Some(a), None) => Some((chrono::Utc::now() - a).num_seconds()),
        _ => None,
    };
    let who = [
        t.actor.clone(),
        if t.model.is_empty() {
            String::new()
        } else {
            Config::short_model(&t.model)
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    div()
        .id(ElementId::NamedInteger(
            format!("pl-task-{run_ix}").into(),
            ix as u64,
        ))
        .flex()
        .items_center()
        .gap(theme::S3)
        .h(px(34.))
        .px(theme::S2)
        .rounded(theme::RADIUS_MD)
        .cursor_pointer()
        .hover(|s| s.bg(theme::SURFACE_HOVER))
        .child(StatusDot::new(t.status).id(ElementId::NamedInteger(
            format!("pl-task-dot-{run_ix}").into(),
            ix as u64,
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .ellipsis()
                .text_body()
                .fg()
                .child(match t.variant {
                    Some(v) => format!("{} · {}", t.title, harness_core::variant_letter(v)),
                    None => t.title.clone(),
                }),
        )
        .child(
            div()
                .w(px(80.))
                .text_label()
                .fg_muted()
                .child(t.kind.label()),
        )
        .child(
            div()
                .w(px(160.))
                .ellipsis()
                .text_label()
                .fg_muted()
                .child(who),
        )
        .child(
            div()
                .w(px(64.))
                .text_right()
                .text_mono()
                .fg_secondary()
                .child(if t.cost_usd > 0.0 {
                    fmt_usd(t.cost_usd)
                } else {
                    String::new()
                }),
        )
        .child(
            div()
                .w(px(64.))
                .text_right()
                .text_mono()
                .fg_muted()
                .child(secs.map(fmt_duration).unwrap_or_default()),
        )
        .child(
            div()
                .w(px(72.))
                .flex()
                .justify_end()
                .when(gate, |el| el.child(Badge::new("Decide").warning().dot()))
                .when(!gate, |el| {
                    el.child(
                        icon_sized(AppIcon::ChevronRight, theme::ICON_SM)
                            .text_color(theme::TEXT_MUTED),
                    )
                }),
        )
        .on_click(move |_, _, cx| {
            if gate {
                state::navigate(
                    Route::Review {
                        task: Some(id.clone()),
                    },
                    cx,
                );
            } else {
                state::open_task(id.clone(), cx);
            }
        })
}
