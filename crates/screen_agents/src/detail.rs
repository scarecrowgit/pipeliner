//! Right-hand details panel for the selected entry: the shared head and building blocks
//! (sections, facts, usage stats, run and chat lists), a pipeline's sections and a created
//! task's sections. Step, sub-agent and chat agent sections live in [`crate::detail_agents`].

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use ui::nav::{ChatRequest, ChatSessionInfo, ChatStatus, Screen, navigate, open_issue, request_chat};
use ui::theme::*;
use ui::{Button, Stat};

use crate::looks::{ellipsis_line, heading, looks, tile};
use crate::entries::step_id;
use crate::model::{Entry, Kind, RunInfo, State, plural, stage_word};
use crate::{AgentsScreen, DETAIL_WIDTH, fmt};

/// Rows of the "Recent runs" lists.
const RECENT_RUNS: usize = 8;

/// `section` with the panel's standard padding; `last` drops the bottom rule.
pub(crate) fn section(gap: f32, last: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .py(px(SPACE_4))
        .px(px(SPACE_5))
        .when(!last, |d| d.border_b_1().border_color(LINE))
}

pub(crate) fn muted_12(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

pub(crate) fn body_text(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(TEXT_SECONDARY)
        .child(text.into())
}

/// `<dl>` with a 96px key column (`gap: 4px 12px`, 12px text).
pub(crate) fn facts(rows: Vec<(&'static str, SharedString)>, mono_values: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(12.))
        .children(rows.into_iter().map(|(k, v)| {
            div()
                .flex()
                .gap(px(12.))
                .child(div().w(px(96.)).flex_none().text_color(TEXT_MUTED).child(k))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(TEXT_SECONDARY)
                        .when(mono_values, |d| d.font_family(FONT_MONO))
                        .child(v),
                )
        }))
}

/// A clickable 36px list row; bleeds 8px into the section padding.
pub(crate) fn list_row(id: impl Into<gpui::ElementId>) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(36.))
        .py(px(4.))
        .px(px(8.))
        .mx(px(-8.))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_HOVER))
}

/// Section heading with a muted count on the right.
pub(crate) fn heading_count(title: &'static str, count: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .justify_between()
        .items_baseline()
        .gap(px(8.))
        .child(heading(title))
        .child(muted_12(count))
}

/// Runs / tokens / cost; a muted line when nothing was recorded.
pub(crate) fn usage_stats(runs: usize, tokens: u64, cost: f64) -> Div {
    if tokens == 0 {
        return muted_12("No usage recorded yet.");
    }
    div()
        .flex()
        .gap(px(SPACE_6))
        .child(Stat::new("Runs", runs.to_string()))
        .child(Stat::new("Tokens", fmt::tokens(tokens)))
        // Runs record 0 when the model's price is unknown.
        .child(Stat::new(
            "Cost",
            if cost > 0. { fmt::cost(cost) } else { "Unknown".into() },
        ))
}

impl AgentsScreen {
    pub(crate) fn render_detail(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sections: Vec<AnyElement> = match self.selected().cloned() {
            None => vec![
                section(6., true)
                    .child(muted_12("Nothing to show."))
                    .into_any_element(),
            ],
            Some(e) => {
                let mut s = vec![self.detail_head(&e).into_any_element()];
                s.extend(match e.kind {
                    Kind::Pipeline(i) => self.pipeline_sections(i, cx),
                    Kind::Step(r, role) => self.step_sections(r, role, window, cx),
                    Kind::Delegate(r) => self.delegate_sections(r, window, cx),
                    Kind::Chat(kind) => self.chat_sections(kind, cx),
                    Kind::Custom(i) => self.custom_sections(i, cx),
                    Kind::Task(i) => self.task_sections(i, cx),
                });
                s
            }
        };

        div()
            .id("agents-detail")
            .w(px(DETAIL_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .children(sections)
    }

    /// "Lead agent · Mockup → GPUI build", "Sub-agent task · created by Build"…
    fn kind_line(&self, e: &Entry) -> String {
        match e.kind {
            Kind::Step(r, _) => match self.snap.step(r) {
                Some((def, _)) => format!("{} · {}", e.kind.label(), def.name),
                None => e.kind.label().into(),
            },
            Kind::Delegate(r) => match self.snap.step(r) {
                Some((def, step)) => format!("Sub-agent · led by {} in {}", step.name, def.name),
                None => e.kind.label().into(),
            },
            Kind::Chat(_) => "Chat agent · built-in".into(),
            Kind::Custom(i) => match self.catalog.agents.get(i) {
                Some(a) => format!(
                    "Chat agent · {}",
                    crate::model::source_label(&a.source).to_lowercase()
                ),
                None => e.kind.label().into(),
            },
            Kind::Task(i) => match self.snap.task_lead(i) {
                Some(lead) => format!("Sub-agent task · created by {lead}"),
                None => e.kind.label().into(),
            },
            Kind::Pipeline(i) => match self.snap.pipeline(i) {
                Some(d) => format!("Pipeline · {} step{}", d.steps.len(), plural(d.steps.len())),
                None => e.kind.label().into(),
            },
        }
    }

    fn detail_head(&self, e: &Entry) -> impl IntoElement {
        let l = looks(e);
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(SPACE_4))
            .px(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .child(tile(l.icon, 36., RADIUS_LG, 16., l.tile_bg, l.tile_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(TEXT_TITLE))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(l.name_color)
                            .truncate()
                            .child(e.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(l.kind_color)
                                    .child(SharedString::from(self.kind_line(e))),
                            )
                            .child(div().text_color(TEXT_DISABLED).child("·"))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(e.state.color())
                                    .child(e.status.clone()),
                            ),
                    ),
            )
    }

    /// Row of small buttons under the head.
    pub(crate) fn actions(&self, buttons: Vec<Button>) -> Div {
        div()
            .flex()
            .gap(px(8.))
            .py(px(12.))
            .px(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .children(buttons)
    }

    /// Issue rows (key, title, run status, when); a click opens the issue on the board.
    pub(crate) fn run_rows(&self, prefix: &'static str, runs: Vec<&RunInfo>) -> Vec<AnyElement> {
        runs.into_iter()
            .take(RECENT_RUNS)
            .enumerate()
            .map(|(i, r)| {
                let key = r.issue.clone();
                list_row((prefix, i))
                    .child(
                        div()
                            .w(px(48.))
                            .flex_none()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_MONO_SM))
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(r.issue.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(ellipsis_line(r.title.clone(), 13., TEXT)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(r.state().color())
                            .child(r.status_word()),
                    )
                    .child(muted_12(fmt::when(r.created)).flex_none())
                    .on_click(move |_, _, cx| open_issue(key.clone(), cx))
                    .into_any_element()
            })
            .collect()
    }

    /// "Recent runs" section with an empty state.
    pub(crate) fn runs_section(
        &self,
        prefix: &'static str,
        runs: Vec<&RunInfo>,
        empty: &'static str,
    ) -> Div {
        let n = runs.len();
        section(4., false)
            .child(heading_count(
                "Recent runs",
                if n > 0 { format!("{n} issue{}", plural(n)) } else { String::new() },
            ))
            .when(n == 0, |d| d.child(muted_12(empty)))
            .children(self.run_rows(prefix, runs))
    }

    /// "Recent chats" with an agent, from the chat screen's session list.
    pub(crate) fn chats_section(&self, agent: &str) -> Div {
        let mine: Vec<&ChatSessionInfo> =
            self.chats.iter().filter(|c| c.agent == agent).collect();
        let n = mine.len();
        section(4., false)
            .child(heading_count(
                "Recent chats",
                if n > 0 { n.to_string() } else { String::new() },
            ))
            .when(n == 0, |d| d.child(muted_12("No recent chats with this agent.")))
            .children(mine.into_iter().take(RECENT_RUNS).enumerate().map(|(i, c)| {
                let id = c.id.clone();
                let (word, color) = match c.status {
                    ChatStatus::Running => ("Running", State::Running.color()),
                    ChatStatus::Waiting => ("Waiting for you", State::Waiting.color()),
                    ChatStatus::Error => ("Failed", State::Failed.color()),
                    ChatStatus::Idle => ("", TEXT_MUTED),
                };
                list_row(("agents-chat-row", i))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(ellipsis_line(c.title.clone(), 13., TEXT)),
                    )
                    .when(!word.is_empty(), |d| {
                        d.child(div().flex_none().text_size(px(12.)).text_color(color).child(word))
                    })
                    .when(c.updated > 0, |d| {
                        d.child(muted_12(fmt::when(c.updated)).flex_none())
                    })
                    .on_click(move |_, _, cx| request_chat(ChatRequest::Open(id.clone()), cx))
            }))
    }

    /// Tasks sub-agents worked on, newest run first; a click selects the task.
    pub(crate) fn tasks_section(
        &self,
        title: &'static str,
        ixs: Vec<usize>,
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let n = ixs.len();
        let rows: Vec<AnyElement> = ixs
            .into_iter()
            .enumerate()
            .filter_map(|(row, ix)| {
                let e = self.list.iter().find(|e| e.kind == Kind::Task(ix))?;
                let (run, _) = self.snap.task(ix)?;
                let id = e.id.clone();
                Some(
                    list_row(("agents-task-row", row))
                        .child(
                            div()
                                .size(px(22.))
                                .flex_none()
                                .rounded(px(RADIUS_MD))
                                .bg(SPAWN_SOFT)
                                .border_1()
                                .border_dashed()
                                .border_color(SPAWN_LINE)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui::icon("spawn", 12., SPAWN_TEXT)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(ellipsis_line(e.name.clone(), 13., SPAWN_TEXT)),
                        )
                        .child(muted_12(run.issue.clone()).flex_none())
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(e.state.color())
                                .child(e.status.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                        .into_any_element(),
                )
            })
            .collect();
        section(4., false)
            .child(heading_count(title, if n > 0 { n.to_string() } else { String::new() }))
            .when(n == 0, |d| d.child(muted_12(empty)))
            .children(rows)
    }

    // ── Pipeline ────────────────────────────────────────────────────────

    fn pipeline_sections(&self, pi: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let snap = &self.snap;
        let Some(def) = snap.pipeline(pi) else {
            return vec![section(6., true).child(muted_12("This pipeline is gone.")).into_any_element()];
        };
        let runs = snap.runs_in(&def.id);
        let n_runs = runs.len();

        let editor_id = def.id.clone();
        let actions = self.actions(vec![
            Button::new("agents-open-editor", "Open in editor")
                .small()
                .on_click(move |_, _, cx| ui::nav::open_pipeline(editor_id.clone(), cx)),
            Button::new("agents-new-issue", "New issue")
                .small()
                .icon("plus")
                .on_click(|_, _, cx| navigate(Screen::Issues, cx)),
        ]);

        let problems = def.problems();
        let about = section(8., false)
            .when(!def.description.trim().is_empty(), |d| {
                d.child(body_text(def.description.trim().to_string()))
            })
            .child(facts(
                vec![
                    ("Steps", def.steps.len().to_string().into()),
                    ("Workdirs", def.workdirs.len().to_string().into()),
                    ("File", fmt::path_label(&def.path()).into()),
                ],
                false,
            ))
            .children(problems.iter().map(|p| {
                div()
                    .text_size(px(12.))
                    .text_color(WARNING)
                    .child(SharedString::from(p.clone()))
            }));

        let pipeline_id = def.id.clone();
        let steps = section(4., false)
            .child(heading_count("Steps", def.steps.len().to_string()))
            .when(def.steps.is_empty(), |d| {
                d.child(muted_12("No steps yet. Add them in the editor."))
            })
            .children(def.steps.iter().enumerate().map(|(i, step)| {
                let who = if step.kind.uses_model() {
                    let mut who = fmt::short_model(&step.model).to_string();
                    if let Some(d) = &step.delegate {
                        who.push_str(&format!(" · leads {}s", d.name));
                    }
                    who
                } else {
                    step.kind.about().to_string()
                };
                let num = div()
                    .w(px(16.))
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO_SM))
                    .text_color(TEXT_MUTED)
                    .child(SharedString::from((i + 1).to_string()));
                let body = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(div().min_w_0().truncate().child(SharedString::from(step.name.clone())))
                            .child(muted_12(step.kind.label()).flex_none()),
                    )
                    .child(ellipsis_line(who, 12., TEXT_MUTED));
                if step.kind.uses_model() {
                    let id = step_id(&pipeline_id, &step.id);
                    list_row(("agents-step", i))
                        .child(num)
                        .child(body)
                        .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                        .into_any_element()
                } else {
                    // Same box as a list row, without the click affordance.
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .min_h(px(36.))
                        .py(px(4.))
                        .px(px(8.))
                        .mx(px(-8.))
                        .child(num)
                        .child(body)
                        .into_any_element()
                }
            }));

        let mut rows = Vec::new();
        if let Some(t) = snap.last_run_in(&def.id) {
            rows.push(("Last run", SharedString::from(fmt::when(t))));
        }
        if n_runs > 0 {
            rows.push(("Running now", snap.active_in(&def.id).to_string().into()));
            rows.push(("For approval", snap.waiting_in(&def.id).to_string().into()));
        }
        let activity = section(10., false)
            .child(heading("Activity"))
            .when(n_runs == 0, |d| {
                d.child(muted_12("No runs yet. Put an issue on the board to start one."))
            })
            .when(n_runs > 0, |d| {
                d.child(usage_stats(n_runs, snap.tokens_in(&def.id), snap.cost_in(&def.id)))
                    .child(facts(rows, false))
                    .child(muted_12("Counts the latest run of each issue."))
            });

        let workdirs = section(8., false)
            .child(heading_count("Workdirs", def.workdirs.len().to_string()))
            .when(def.workdirs.is_empty(), |d| d.child(muted_12("No workdirs.")))
            .children(def.workdirs.iter().map(|w| {
                let how = if w.worktree {
                    format!("A worktree per run, from {}", w.base_branch)
                } else {
                    "Used as it is, shared by all runs".to_string()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(div().font_weight(FontWeight::MEDIUM).child(SharedString::from(w.name.clone())))
                            .child(muted_12(how).min_w_0().truncate()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(ui::icon("folder", 12., TEXT_MUTED))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(ellipsis_line(fmt::path_label(&w.path), TEXT_MONO_SM, TEXT_SECONDARY).font_family(FONT_MONO)),
                            ),
                    )
            }))
            .children(snap.errors.iter().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(WARNING)
                    .child(SharedString::from(format!("Could not read: {e}")))
            }));

        let recent = self.runs_section("agents-pipeline-run", runs, "No runs yet.");

        vec![
            actions.into_any_element(),
            about.into_any_element(),
            steps.into_any_element(),
            activity.into_any_element(),
            recent.into_any_element(),
            workdirs.border_b_0().into_any_element(),
        ]
    }

    // ── Created by agents ───────────────────────────────────────────────

    fn task_sections(&self, ix: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((run, task)) = self.snap.task(ix) else {
            return vec![section(6., true).child(muted_12("This task is gone.")).into_any_element()];
        };
        let key = run.issue.clone();
        let actions = self.actions(vec![
            Button::new("agents-task-issue", format!("Open {key}"))
                .small()
                .on_click(move |_, _, cx| open_issue(key.clone(), cx)),
        ]);

        let lead = self.snap.task_lead(ix).unwrap_or_else(|| "a pipeline step".into());
        let origin = section(8., false).child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .py(px(10.))
                .px(px(12.))
                .rounded(px(RADIUS_LG))
                .bg(SPAWN_SOFT)
                .border_1()
                .border_color(SPAWN_LINE)
                .text_size(px(12.))
                .text_color(SPAWN_TEXT)
                .child(SharedString::from(format!(
                    "Created by {lead} for {} · {}",
                    run.issue,
                    fmt::when(run.created)
                )))
                .child(
                    div()
                        .text_color(TEXT_SECONDARY)
                        .child(SharedString::from(run.title.clone())),
                ),
        );

        let usage = self.snap.task_usage(ix);
        let mut rows = vec![("Status", SharedString::from(stage_word(task.status)))];
        if let Some(c) = self.snap.task_agent(ix) {
            rows.push(("Worked by", c.into()));
        }
        if let Some(u) = usage.first() {
            rows.push(("Model", u.model.clone().into()));
        }
        let tokens: u64 = usage.iter().map(|u| u.tokens).sum();
        let cost: f64 = usage.iter().map(|u| u.cost_usd).sum();
        if tokens > 0 {
            rows.push(("Tokens", fmt::tokens(tokens).into()));
        }
        if cost > 0. {
            rows.push(("Cost", fmt::cost(cost).into()));
        }
        let about = section(8., false).child(facts(rows, false));

        let summary = section(8., false)
            .child(heading("What it did"))
            .child(if task.summary.trim().is_empty() {
                muted_12("No summary yet.")
            } else {
                body_text(fmt::plain_markdown(&task.summary))
            });

        let siblings: Vec<usize> = (0..self.snap.tasks.len())
            .filter(|&i| i != ix && self.snap.tasks[i].run == self.snap.tasks[ix].run)
            .collect();
        let others = self.tasks_section(
            "Other tasks in this run",
            siblings,
            "It was the only task in this run.",
            cx,
        );

        vec![
            actions.into_any_element(),
            origin.into_any_element(),
            about.into_any_element(),
            summary.into_any_element(),
            others.border_b_0().into_any_element(),
        ]
    }
}
