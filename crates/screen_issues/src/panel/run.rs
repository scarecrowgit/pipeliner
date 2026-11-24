//! Runs of the issue: the one-line summary inside the runner card (path, state, progress), and
//! the run section under the properties: what the latest run waits for (a review, a question,
//! a resume after a failure), its steps with variant, state, time, cost and a link to each
//! agent's session, and the earlier runs folded ("Run 1 on this issue").

use std::sync::Arc;

use gpui::{AnyElement, App, Context, Hsla, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{InboxKind, Issue, NodeInstance, NodeStatus, Run, RunState, RunWait, SpanKind};
use hub::{InboxFocus, Route};
use ui::dev::Probe;
use ui::motion::Look;
use ui::prelude::*;
use ui_rich::text_field;

use super::{Named, IssuePanel, section as psec, section_head};

/// Inner width of a callout's text, px: the content less the callout's padding and icon.
fn callout_text(content: f32) -> f32 {
    content - 2. * SPACE_2 - 12. - SPACE_2
}

/// The state of a run in one line, as the board card says it.
struct Line {
    status: Status,
    text: String,
    color: Hsla,
    /// An agent another agent created, in violet: "Log digger spawned".
    spawned: Option<String>,
    /// Finished steps over all, while running.
    percent: Option<u32>,
}

fn is_spawned(node: &NodeInstance) -> bool {
    node.kind == SpanKind::Spawned || node.spawned_by.is_some()
}

fn line(run: &Run, wait: Option<RunWait>) -> Line {
    let plain = |status, text: &str, color| Line { status, text: text.to_string(), color, spawned: None, percent: None };
    match run.state {
        RunState::Queued => plain(Status::Idle, "Queued", TEXT_MUTED),
        RunState::Running => {
            let active: Vec<&NodeInstance> = run.nodes.iter().filter(|n| n.status == NodeStatus::Running).collect();
            let main: Vec<&&NodeInstance> = active.iter().filter(|n| !is_spawned(n)).collect();
            let text = match main.as_slice() {
                [] => "Running".to_string(),
                [one] => format!("{} running", one.name),
                many if many.iter().all(|n| n.name == many[0].name) => format!("{} ×{} running", many[0].name, many.len()),
                many => format!("{} agents running", many.len()),
            };
            let spawned = active.iter().rev().find(|n| is_spawned(n)).map(|n| n.name.clone());
            let percent = (!run.nodes.is_empty()).then(|| base::fmt::percent(f64::from(run.progress()), 1.0));
            Line { status: Status::Running, text, color: TEXT_SECONDARY, spawned, percent }
        }
        RunState::WaitingReview => {
            // The variants of the agent that made them; helpers it spawned do not count.
            let n = run.nodes.iter().filter(|n| !is_spawned(n) && n.variant > 0).map(|n| n.variant).max().unwrap_or(0);
            let n = u64::from(n);
            let text = if n > 0 { format!("{} to review", base::fmt::plural(n, "variant", "variants")) } else { "Ready to review".into() };
            Line { status: Status::Waiting, text, color: WARNING, spawned: None, percent: None }
        }
        RunState::WaitingAnswer => {
            let question = run
                .nodes
                .iter()
                .rev()
                .filter(|n| n.status == NodeStatus::Waiting)
                .find_map(|n| n.question.clone().or_else(|| (!n.summary.is_empty()).then(|| n.summary.clone())));
            let text = match question {
                Some(q) => format!("asks: {}", base::fmt::truncate_chars(base::fmt::first_line(&q), 80)),
                None if wait == Some(RunWait::Budget) => "Out of budget · waiting for a raise".into(),
                None => "asks you a question".into(),
            };
            Line { status: Status::Waiting, text, color: WARNING, spawned: None, percent: None }
        }
        RunState::Done => {
            let text = match run.pr_url.as_deref().and_then(pr_number) {
                Some(n) => format!("Finished · PR #{n}"),
                None => "Finished".into(),
            };
            Line { status: Status::Live, text, color: SUCCESS, spawned: None, percent: None }
        }
        RunState::Failed => {
            let error = run.error.as_deref().map(base::fmt::first_line).unwrap_or("");
            let text = if error.is_empty() { "Failed".to_string() } else { format!("Failed · {}", base::fmt::truncate_chars(error, 80)) };
            Line { status: Status::Error, text, color: DANGER, spawned: None, percent: None }
        }
        RunState::Cancelled => plain(Status::Idle, "Cancelled", TEXT_MUTED),
        RunState::Interrupted => plain(Status::Error, "Interrupted · resume it", DANGER),
    }
}

/// Why the engine says `run` waits for a person, if it does.
pub(super) fn wait_of(this: &IssuePanel, run: &Run, cx: &App) -> Option<RunWait> {
    this.runs.read(cx).waiting(&run.id)
}

/// The number at the end of a pull request URL: `.../pull/2280` gives 2280.
pub(super) fn pr_number(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// The path the item took: `Jira puller › Router › Fixer ×3 › Review › You`.
fn path(run: &Run) -> String {
    let mut parts: Vec<(&str, &str, u32)> = Vec::new();
    for n in run.nodes.iter().filter(|n| !is_spawned(n)) {
        match parts.iter_mut().find(|(node, _, _)| *node == n.node.as_str()) {
            Some(part) => part.2 = part.2.max(n.variant),
            None => parts.push((n.node.as_str(), n.name.as_str(), n.variant)),
        }
    }
    let mut out: Vec<String> =
        parts.into_iter().map(|(_, name, variants)| if variants > 1 { format!("{name} ×{variants}") } else { name.to_string() }).collect();
    if run.state.needs_person() {
        out.push("You".into());
    }
    out.join(" › ")
}

/// The run in the runner card: its path, its state in one line and its progress.
pub(super) fn summary(run: &Run, wait: Option<RunWait>) -> impl IntoElement {
    let l = line(run, wait);
    let mut out = col().flex_none().w_full();
    let path = path(run);
    if !path.is_empty() {
        out = out.child(truncate_block(path, MONO_SM.line).mt(px(SPACE_2)).type_style(MONO_SM).text_color(TEXT_SECONDARY));
    }
    let percent = l.percent;
    out = out.child(
        row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .mt(px(SPACE_1_5))
            .type_style(LABEL)
            .child(StatusDot::new(l.status).small())
            .child(div().flex_none().text_color(l.color).child(if l.spawned.is_some() { format!("{} ·", l.text) } else { l.text }))
            .when_some(l.spawned, |d, name| d.child(div().flex_none().text_color(SPAWN_TEXT).child(format!("{name} spawned"))))
            .when_some(percent, |d, p| d.child(grow()).child(div().flex_none().type_style(MONO).text_color(TEXT_MUTED).child(format!("{p}%"))))
            .probe_value("issue-run-line", run_state_label(run.state)),
    );
    if let Some(p) = percent {
        out = out.child(div().mt(px(SPACE_1_5)).child(Progress::new(p as f32 / 100.).tone(ProgressTone::Info)));
    }
    out
}

/// A run's state in words.
fn run_state_label(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::WaitingReview => "waiting for review",
        RunState::WaitingAnswer => "waiting for an answer",
        RunState::Done => "done",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
        RunState::Interrupted => "interrupted",
    }
}

fn elapsed(started: Option<i64>, finished: Option<i64>, now: i64) -> Option<String> {
    let started = started?;
    let end = finished.unwrap_or(now);
    Some(base::fmt::duration((end.saturating_sub(started).max(0) / 1000) as u64))
}

/// "running · 20:00 · $0.12".
fn facts(run: &Run, now: i64) -> SharedString {
    let mut parts = vec![run_state_label(run.state).to_string()];
    if let Some(time) = elapsed(Some(run.started), run.finished, now) {
        parts.push(time);
    }
    parts.push(base::fmt::cost(run.cost_usd));
    parts.join(" · ").into()
}

/// The run section: present when the issue has a run.
pub(super) fn section(this: &IssuePanel, issue: &Arc<Issue>, window: &mut Window, cx: &mut Context<IssuePanel>) -> Option<AnyElement> {
    let latest = this.latest_run(cx)?;
    let now = base::time::now_ms();
    let earlier = this.earlier_runs(Some(&latest.id));
    let title = if latest.number > 0 { format!("Run {}", latest.number) } else { "Run".to_string() };
    let mut sec = psec().child(section_head(title, Some(facts(&latest, now))));
    let mut block = col().flex_none().w_full().gap(px(SPACE_2));
    block = block.children(callouts(this, issue, &latest, cx));
    let open = !this.toggled.contains(&latest.id);
    block = block.child(steps_toggle(&latest, open, cx));
    if open {
        block = block.child(steps(&latest, now, window, cx));
    }
    sec = sec.child(block.named(format!("issue-run-{}", latest.id)));
    if !earlier.is_empty() {
        sec = sec.child(div().flex_none().mt(px(SPACE_3)).type_style(LABEL).text_color(TEXT_MUTED).child("Earlier runs"));
        for run in &earlier {
            sec = sec.child(earlier_run(this, run, now, window, cx));
        }
    }
    Some(sec.into_any_element())
}

/// What the run waits for: a review in the Inbox, an agent's question, a resume.
fn callouts(this: &IssuePanel, issue: &Issue, run: &Arc<Run>, cx: &mut Context<IssuePanel>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let cw = this.content_width();
    let text_w = callout_text(cw);
    let (review, question_item) = {
        let inbox = this.inbox.read(cx);
        let review = inbox.reviews().iter().find(|review| review.run == run.id).map(|review| review.id.clone());
        let asks = |item: &&Arc<hub::api::InboxItem>| matches!(item.kind, InboxKind::Question { .. });
        let item = inbox
            .items()
            .iter()
            .filter(asks)
            .find(|item| item.run.as_ref() == Some(&run.id))
            .or_else(|| inbox.items().iter().filter(asks).find(|item| item.issue.as_ref() == Some(&issue.key)))
            .map(|item| item.id.clone());
        (review, item)
    };
    if run.state == RunState::WaitingReview {
        let route = Route::Inbox { item: review.map(InboxFocus::Review) };
        let text = line(run, wait_of(this, run, cx)).text;
        out.push(
            callout(IconName::Inbox, WARNING, WARNING_SOFT, cw)
                .child(
                    col()
                        .w(px(text_w))
                        .gap(px(SPACE_1))
                        .child(div().text_color(TEXT).child(format!("Waiting for your review · {text}")))
                        .child(
                            Link::new("issue-run-review", "Open in Inbox")
                                .on_click(move |_, _, cx| hub::navigate(route.clone(), cx))
                                .probe("issue-run-review"),
                        ),
                )
                .into_any_element(),
        );
    }
    let mut first = true;
    for (ix, node) in run.nodes.iter().enumerate() {
        let Some(question) = node.question.clone() else { continue };
        // The first question is answered in place; the Inbox has every one.
        let field = std::mem::take(&mut first).then(|| answer_row(this, ix, cx));
        let item = question_item.clone();
        let route = Route::Inbox { item: item.map(InboxFocus::Item) };
        let name = SharedString::from(format!("issue-run-question-{ix}"));
        out.push(
            callout(IconName::Alert, WARNING, WARNING_SOFT, cw)
                .child(
                    col()
                        .w(px(text_w))
                        .gap(px(SPACE_1))
                        .child(div().type_style(LABEL_STRONG).text_color(WARNING).child(format!("{} asks", node.name)))
                        .child(div().w(px(text_w)).text_color(TEXT).child(question))
                        .children(field)
                        .child(
                            Link::new(SharedString::from(format!("issue-run-answer-{ix}")), "Open in Inbox")
                                .on_click(move |_, _, cx| hub::navigate(route.clone(), cx))
                                .probe(format!("issue-run-answer-{ix}")),
                        ),
                )
                .named(name)
                .into_any_element(),
        );
    }
    if wait_of(this, run, cx) == Some(RunWait::Budget) {
        let spent = base::fmt::cost(run.cost_usd);
        out.push(
            callout(IconName::Alert, WARNING, WARNING_SOFT, cw)
                .child(
                    col()
                        .w(px(text_w))
                        .gap(px(SPACE_1))
                        .child(div().w(px(text_w)).text_color(TEXT).child(format!(
                            "Out of budget · {spent} spent. Raise it and the run goes on where it stopped."
                        )))
                        .child(budget_row(this, cx)),
                )
                .named("issue-run-budget")
                .into_any_element(),
        );
    }
    if matches!(run.state, RunState::Failed | RunState::Interrupted) {
        let text = match (run.state, run.error.as_deref()) {
            (RunState::Interrupted, _) => "Interrupted when the app closed. Resume picks it up where it stopped.".to_string(),
            (_, Some(error)) if !error.is_empty() => format!("Run failed · {error}"),
            _ => "Run failed".to_string(),
        };
        let id = run.id.clone();
        out.push(
            callout(IconName::X, DANGER, DANGER_SOFT, cw)
                .child(
                    col()
                        .w(px(text_w))
                        .gap(px(SPACE_1_5))
                        .child(div().w(px(text_w)).text_color(TEXT).child(text))
                        .child(
                            row().child(
                                Button::new("issue-resume", "Resume")
                                    .small()
                                    .secondary()
                                    .icon(IconName::Replay)
                                    .on_click(move |_, _, cx| hub::commands(cx).resume_run(&id, cx))
                                    .probe("issue-resume"),
                            ),
                        ),
                )
                .named("issue-run-failure")
                .into_any_element(),
        );
    }
    out
}

/// The answer field and its button, under the question.
fn answer_row(this: &IssuePanel, ix: usize, cx: &mut Context<IssuePanel>) -> AnyElement {
    let text_w = callout_text(this.content_width());
    let field = SharedString::from(format!("issue-run-answer-field-{ix}"));
    let send = SharedString::from(format!("issue-run-answer-send-{ix}"));
    row()
        .flex_none()
        .w(px(text_w))
        .gap(px(SPACE_1_5))
        .mt(px(SPACE_1))
        .child(div().flex_1().min_w_0().child(text_field(field.clone(), &this.answer).small()).named(field))
        .child(
            Button::new(send.clone(), "Answer")
                .small()
                .secondary()
                .on_click(cx.listener(|this, _, _, cx| this.send_answer(cx)))
                .probe(send),
        )
        .into_any_element()
}

/// "$ [1.00] Raise budget".
fn budget_row(this: &IssuePanel, cx: &mut Context<IssuePanel>) -> AnyElement {
    row()
        .flex_none()
        .gap(px(SPACE_1_5))
        .mt(px(SPACE_1))
        .child(div().flex_none().text_color(TEXT_MUTED).child("$"))
        .child(div().flex_none().w(px(88.)).child(text_field("issue-run-budget-field", &this.budget).small()).named("issue-run-budget-field"))
        .child(
            Button::new("issue-run-raise", "Raise budget")
                .small()
                .secondary()
                .on_click(cx.listener(|this, _, window, cx| this.raise_budget(window, cx)))
                .probe("issue-run-raise"),
        )
        .into_any_element()
}

impl IssuePanel {
    /// Lets the latest run spend the amount in the budget field more, and go on.
    pub(super) fn raise_budget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.budget.read(cx).text().trim().trim_start_matches('$').to_string();
        let Some(usd) = typed.parse::<f64>().ok().filter(|usd| usd.is_finite() && *usd > 0.) else {
            ui::overlays(window, cx).toast(Toast::new("Type an amount in dollars, like 1.50").error(), cx);
            return;
        };
        let Some(run) = self.latest_run(cx) else { return };
        hub::commands(cx).raise_run_budget(&run.id, usd, cx);
        cx.notify();
    }

    /// Sends what the answer field holds to the agent that asks.
    pub(super) fn send_answer(&mut self, cx: &mut Context<Self>) {
        let text = self.answer.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(run) = self.latest_run(cx) else { return };
        let Some(node) = run.nodes.iter().find(|n| n.question.is_some()).map(|n| n.node.clone()) else { return };
        hub::commands(cx).answer_run(&run.id, Some(&node), &text, cx);
        self.answer.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }
}

/// A tinted box with an icon at the top left.
fn callout(glyph: IconName, color: Hsla, bg: Hsla, width: f32) -> gpui::Div {
    row()
        .flex_none()
        .w(px(width))
        .items_start()
        .gap(px(SPACE_2))
        .px(px(SPACE_2))
        .py(px(SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .bg(bg)
        .type_style(LABEL)
        .child(div().flex_none().pt(px(3.)).child(icon(glyph, 12., color)))
}

/// "4 steps" with a chevron that folds them.
fn steps_toggle(run: &Arc<Run>, open: bool, cx: &mut Context<IssuePanel>) -> impl IntoElement {
    let id = run.id.clone();
    let count = run.nodes.len() as u64;
    let name = SharedString::from(format!("issue-run-steps-{}", run.id));
    row()
        .id(name.clone())
        .debug_selector({
            let name = name.clone();
            move || name.to_string()
        })
        .flex_none()
        .gap(px(SPACE_1))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .cursor_pointer()
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., TEXT_MUTED))
        .child(base::fmt::plural(count, "step", "steps"))
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_run(&id, cx)))
        .probe(name)
}

/// The steps of a run, in creation order.
fn steps(run: &Arc<Run>, now: i64, _window: &mut Window, _cx: &mut Context<IssuePanel>) -> impl IntoElement {
    let mut list = col().flex_none().w_full();
    for (ix, node) in run.nodes.iter().enumerate() {
        let status = match node.status {
            NodeStatus::Pending => Status::Draft,
            NodeStatus::Running => Status::Running,
            NodeStatus::Waiting => Status::Waiting,
            NodeStatus::Done => Status::Live,
            NodeStatus::Failed => Status::Error,
            NodeStatus::Skipped | NodeStatus::Cancelled | NodeStatus::Interrupted => Status::Idle,
        };
        let spawned = is_spawned(node);
        let name = SharedString::from(format!("issue-step-{}-{ix}", run.id));
        let letter = node.letter();
        let time = elapsed(node.started, node.finished, now).unwrap_or_default();
        let session = node.session.clone();
        let mut r = row()
            .id(("issue-step", ix))
            .flex_none()
            .w_full()
            .h(px(28.))
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .child(StatusDot::new(status).small())
            .child(div().flex_none().text_color(if spawned { SPAWN_TEXT } else { TEXT }).child(node.name.clone()))
            .when(!letter.is_empty(), |d| d.child(Tag::new(letter).mono()))
            .child(truncate_lh(node.summary.clone(), LABEL.line).text_color(TEXT_MUTED))
            .child(div().flex_none().type_style(MONO_SM).text_color(TEXT_MUTED).child(time))
            .child(div().flex_none().w(px(44.)).type_style(MONO_SM).text_color(TEXT_MUTED).child(base::fmt::cost(node.cost_usd)));
        r = match session {
            Some(session) => r.child(
                IconButton::new(SharedString::from(format!("{name}-session")), IconName::Chat)
                    .small()
                    .tooltip("Open the agent's session")
                    .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: session.clone() }, cx))
                    .probe(format!("{name}-session")),
            ),
            None => r.child(div().flex_none().size(px(CONTROL_HEIGHT_SM))),
        };
        list = list.child(r.named(name));
    }
    list
}

/// An earlier run: one line that unfolds its steps.
fn earlier_run(this: &IssuePanel, run: &Arc<Run>, now: i64, window: &mut Window, cx: &mut Context<IssuePanel>) -> AnyElement {
    let open = this.toggled.contains(&run.id);
    let id = run.id.clone();
    let name = SharedString::from(format!("issue-run-{}", run.id));
    let l = line(run, wait_of(this, run, cx));
    let head = row()
        .id(name.clone())
        .debug_selector({
            let name = name.clone();
            move || name.to_string()
        })
        .flex_none()
        .w_full()
        .h(px(28.))
        .gap(px(SPACE_1_5))
        .px(px(SPACE_1))
        .rounded(px(RADIUS_MD))
        .type_style(LABEL)
        .cursor_pointer()
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., TEXT_MUTED))
        .child(StatusDot::new(l.status).small())
        .child(div().flex_none().text_color(TEXT).child(format!("Run {} on this issue", run.number)))
        .child(grow())
        .child(div().flex_none().text_color(TEXT_MUTED).child(facts(run, now)))
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_run(&id, cx)));
    let head = ui::motion::hover_fade(head, name.clone(), Look::bg(TRANSPARENT), Look::bg(SURFACE_RAISED), window, cx);
    let mut out = col().flex_none().w_full().child(head.probe(name));
    if open {
        out = out.child(div().pl(px(SPACE_5)).child(steps(run, now, window, cx)));
    }
    out.into_any_element()
}
