//! The Test run tab: the item a test run starts on, the numbers of the run on show, its
//! timeline, and what it waits for (an answer, a review) or how it ended.
//!
//! A test run starts on the saved draft: the save is queued before the run, and commands
//! reach the engine in order. The run is put on show in the draft so the canvas marks its
//! nodes; the dock follows its model.
//!
//! Probes: `test-run-title`, `test-run-text`, `test-run-start`, `test-run-stop`,
//! `test-run-resume`, `test-run-new`, `test-run-answer`, `test-run-answer-send`,
//! `test-run-inbox`, `test-run-state` (its value is the run's state).

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, SharedString, Subscription, Task, Window, div,
    prelude::*, px, relative,
};
use hub::api::{NodeKind, Run, RunInput, RunState, RunStats, SourceKind};
use hub::model::{NodeStatus, RunId, SpanKind};
use hub::{InboxFocus, Route, RunLogEvent, RunModel};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::charts::{BarTone, Tick, Timeline, TimelineRow, time_ticks};
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::EditorView;
use super::dock::DockTab;
use super::log::LogState;

/// The longest node summary shown after a timeline label.
const META_CHARS: usize = 18;
/// Axis steps tried in order, seconds: the first that gives at most this many labels wins.
const TICK_STEPS: [u64; 12] = [5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];
const MAX_TICKS: u64 = 4;

/// The test run part of the editor: the item's fields, the answer field and the run on show.
pub(super) struct TestRun {
    pub(super) title: Entity<TextInput>,
    pub(super) text: Entity<TextInput>,
    pub(super) answer: Entity<TextInput>,
    pub(super) shown: Option<Shown>,
    /// A start was asked for and the engine has not answered yet.
    pub(super) starting: bool,
    pub(super) log: LogState,
    _subscriptions: Vec<Subscription>,
}

/// The run on show and what is known about it.
pub(super) struct Shown {
    pub(super) id: RunId,
    pub(super) model: Entity<RunModel>,
    pub(super) stats: Option<RunStats>,
    /// The run the stats were asked for.
    stats_for: Option<Arc<Run>>,
    stats_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TestRun {
    /// Empty fields and no run on show.
    pub(super) fn new(window: &mut Window, cx: &mut Context<EditorView>) -> Self {
        let title = cx.new(|cx| TextInput::new(cx).placeholder("Title of the item, as an issue would have it"));
        let text = cx.new(|cx| TextInput::new(cx).placeholder("What the first agents read (optional)"));
        let answer = cx.new(|cx| TextInput::new(cx).placeholder("Your answer"));
        let subscriptions = vec![
            cx.subscribe_in(&title, window, |this, _, event: &TextInputEvent, window, cx| {
                if *event == TextInputEvent::Submit {
                    this.start_test_run(window, cx);
                }
            }),
            cx.subscribe_in(&text, window, |this, _, event: &TextInputEvent, window, cx| {
                if *event == TextInputEvent::Submit {
                    this.start_test_run(window, cx);
                }
            }),
            cx.subscribe_in(&answer, window, |this, _, event: &TextInputEvent, _, cx| {
                if *event == TextInputEvent::Submit {
                    this.send_answer(cx);
                }
            }),
            cx.observe(&title, |_, _, cx| cx.notify()),
        ];
        Self {
            title,
            text,
            answer,
            shown: None,
            starting: false,
            log: LogState::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Stops following a run (another pipeline was opened).
    pub(super) fn forget_run(&mut self) {
        self.shown = None;
        self.starting = false;
        self.log = LogState::default();
    }
}

impl EditorView {
    /// The run on show, once its model has it.
    pub(crate) fn shown_run(&self, cx: &App) -> Option<Arc<Run>> {
        self.test.shown.as_ref().and_then(|shown| shown.model.read(cx).run().cloned())
    }

    /// The draft put another run on show (or none): follow its model.
    pub(super) fn follow_run(&mut self, cx: &mut Context<Self>) {
        let wanted = self.parts.draft.read(cx).run().cloned();
        if wanted.as_ref() == self.test.shown.as_ref().map(|shown| &shown.id) {
            return;
        }
        self.test.log = LogState::default();
        self.test.shown = wanted.map(|id| {
            let model = hub::run(&id, cx);
            let subscriptions = vec![
                cx.observe(&model, |this, _, cx| {
                    this.refresh_stats(cx);
                    cx.notify();
                }),
                cx.subscribe(&model, |this, _, event: &RunLogEvent, cx| this.log_appended(event, cx)),
            ];
            Shown { id, model, stats: None, stats_for: None, stats_task: None, _subscriptions: subscriptions }
        });
        self.refresh_stats(cx);
        cx.notify();
    }

    /// Asks for the run's numbers again when the run changed.
    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.test.shown.as_mut() else { return };
        let Some(run) = shown.model.read(cx).run().cloned() else { return };
        if shown.stats_for.as_ref().is_some_and(|known| Arc::ptr_eq(known, &run)) {
            return;
        }
        shown.stats_for = Some(run);
        let asked = hub::queries(cx).run_stats(&shown.id, cx);
        let id = shown.id.clone();
        shown.stats_task = Some(cx.spawn(async move |this, cx| {
            let Ok(stats) = asked.await else { return };
            this.update(cx, |this, cx| {
                if let Some(shown) = this.test.shown.as_mut().filter(|shown| shown.id == id) {
                    shown.stats = Some(stats);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The header's "Test run": starts one when the item has a title, else asks for one.
    pub(super) fn test_run_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(DockTab::TestRun, cx);
        let running = self.shown_run(cx).is_some_and(|run| run.state.is_open());
        if running {
            return;
        }
        if self.test.shown.is_some() {
            self.parts.draft.update(cx, |draft, cx| draft.show_run(None, cx));
        }
        if self.test.title.read(cx).text().trim().is_empty() {
            self.test.title.read(cx).focus(window);
        } else {
            self.start_test_run(window, cx);
        }
    }

    /// Saves the draft and starts a test run on the item in the fields.
    pub(crate) fn start_test_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.test.title.read(cx).text().trim().to_string();
        if title.is_empty() {
            self.test.title.read(cx).focus(window);
            return;
        }
        if self.test.starting {
            return;
        }
        let text = self.test.text.read(cx).text().trim().to_string();
        self.keep_mine(cx);
        self.save_now(cx);
        self.test.starting = true;
        self.open_tab(DockTab::TestRun, cx);
        let started = hub::commands(cx).test_run(&self.pipeline, RunInput { title, text }, cx);
        let draft = self.parts.draft.clone();
        cx.spawn(async move |this, cx| {
            let run = started.await;
            if let Some(run) = &run {
                draft.update(cx, |draft, cx| draft.show_run(Some(run.id.clone()), cx)).ok();
            }
            this.update(cx, |this, cx| {
                this.test.starting = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Stops the run on show.
    pub(crate) fn stop_test_run(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = &self.test.shown {
            hub::commands(cx).cancel_run(&shown.id, cx);
        }
    }

    fn resume_test_run(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = &self.test.shown {
            hub::commands(cx).resume_run(&shown.id, cx);
        }
    }

    /// Back to the item's fields for another run.
    fn new_test_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.parts.draft.update(cx, |draft, cx| draft.show_run(None, cx));
        self.test.title.read(cx).focus(window);
    }

    /// Sends the answer field to the agent that asked.
    fn send_answer(&mut self, cx: &mut Context<Self>) {
        let text = self.test.answer.read(cx).text().trim().to_string();
        let (Some(shown), Some(run)) = (&self.test.shown, self.shown_run(cx)) else { return };
        if text.is_empty() {
            return;
        }
        let node = asking(&run).map(|(node, _)| node);
        hub::commands(cx).answer_run(&shown.id, node.as_ref(), &text, cx);
        self.test.answer.update(cx, |answer, cx| answer.set_text("", cx));
    }

    /// Goes to the review this run waits on, in the Inbox.
    fn open_review(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = &self.test.shown else { return };
        let inbox = hub::inbox(cx);
        let review = inbox.read(cx).reviews().iter().find(|review| review.run == shown.id).map(|review| review.id.clone());
        hub::navigate(Route::Inbox { item: review.map(InboxFocus::Review) }, cx);
    }

    // ------------------------------------------------------------ rendering

    pub(super) fn render_test_run(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let run = self.shown_run(cx);
        let body = match &run {
            Some(run) => self.render_run(run, window, cx),
            None => self.render_form(cx),
        };
        col()
            .flex_1()
            .min_h_0()
            .child(self.render_strip(cx))
            .child(
                div()
                    .id("test-run-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(SPACE_5))
                    .pt(px(SPACE_2_5))
                    .pb(px(SPACE_3))
                    .child(body),
            )
            .into_any_element()
    }

    /// The five numbers: queued items, agents running, spawned, waiting, spend.
    fn render_strip(&self, cx: &App) -> AnyElement {
        let graph = self.parts.draft.read(cx).graph();
        let limits = graph.map(|g| g.limits.clone()).unwrap_or_default();
        let source = graph.and_then(|g| {
            g.nodes.iter().find_map(|node| match &node.kind {
                NodeKind::Source(source) => Some(source.source),
                _ => None,
            })
        });
        let depth = graph.and_then(|g| {
            g.nodes
                .iter()
                .filter_map(|node| match &node.kind {
                    NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => {
                        agent.spawn.as_ref().filter(|s| s.enabled).map(|s| s.max_depth)
                    }
                    _ => None,
                })
                .max()
        });
        let stats = self.test.shown.as_ref().and_then(|shown| shown.stats.clone()).unwrap_or(RunStats {
            agents_max: limits.max_parallel,
            budget_usd: limits.budget_usd,
            ..RunStats::default()
        });
        let queued_label = match source {
            Some(SourceKind::Jira) => "Queued in Jira",
            Some(SourceKind::Sentry) => "Queued in Sentry",
            Some(SourceKind::Issue) => "Queued on the board",
            _ => "Queued",
        };
        let cells = [
            (queued_label, base::fmt::count(self.queued(cx).into()), "issues".to_string()),
            ("Agents running", stats.agents_running.to_string(), format!("of {} max", stats.agents_max)),
            ("Spawned", stats.spawned.to_string(), depth.map_or_else(|| "none allowed".to_string(), |d| format!("depth ≤{d}"))),
            ("Waiting for you", stats.waiting_variants.to_string(), "variants".to_string()),
            (
                "Spend this run",
                base::fmt::cost(stats.cost_usd),
                stats.budget_usd.map_or_else(|| "no limit".to_string(), |b| format!("of {}", base::fmt::cost(b))),
            ),
        ];
        row()
            .flex_none()
            .items_start()
            .px(px(SPACE_5))
            .py(px(SPACE_3))
            .border_b_1()
            .border_color(LINE)
            .children(cells.into_iter().enumerate().map(|(ix, (label, value, sub))| {
                col()
                    .flex_1()
                    .min_w_0()
                    .when(ix > 0, |d| d.pl(px(SPACE_4)).border_l_1().border_color(LINE))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(label))
                    .child(
                        row()
                            .items_baseline()
                            .gap(px(SPACE_1))
                            .child(div().type_style(TITLE_SM).tabular().child(value))
                            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(sub)),
                    )
            }))
            .probe("test-run-strip")
            .into_any_element()
    }

    /// The item to run the draft on, Run, and what a test run does.
    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let empty = self.test.title.read(cx).text().trim().is_empty();
        col()
            .gap(px(SPACE_2))
            .child(div().type_style(BODY_STRONG).child("Try the draft on one item"))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(div().w(relative(0.36)).flex_none().child(text_field("test-run-title-field", &self.test.title).small()).probe("test-run-title"))
                    .child(div().flex_1().min_w_0().child(text_field("test-run-text-field", &self.test.text).small()).probe("test-run-text"))
                    .child(
                        Button::new("test-run-start", if self.test.starting { "Starting…" } else { "Run" })
                            .secondary()
                            .icon(IconName::Play)
                            .disabled(empty || self.test.starting)
                            .on_click(cx.listener(|this, _, window, cx| this.start_test_run(window, cx)))
                            .probe("test-run-start"),
                    ),
            )
            .child(
                div()
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child("A test run uses real agents in throwaway worktrees. Merges and pull requests are only simulated."),
            )
            .into_any_element()
    }

    /// The run on show: its line, what it waits for or how it ended, and its timeline.
    fn render_run(&self, run: &Arc<Run>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = base::time::now_ms();
        let end = run.finished.unwrap_or(now).max(run.started);
        let secs = ((end - run.started) / 1000).max(0) as u64;
        let mut facts = vec![run.title.clone()];
        facts.push(base::fmt::duration(secs));
        facts.push(base::fmt::cost(run.cost_usd));
        let open = run.state.is_open();
        let control = if open {
            Button::new("test-run-stop", "Stop")
                .ghost()
                .small()
                .icon(IconName::Pause)
                .on_click(cx.listener(|this, _, _, cx| this.stop_test_run(cx)))
                .probe("test-run-stop")
                .into_any_element()
        } else {
            Button::new("test-run-new", "New test run")
                .ghost()
                .small()
                .icon(IconName::Play)
                .on_click(cx.listener(|this, _, window, cx| this.new_test_run(window, cx)))
                .probe("test-run-new")
                .into_any_element()
        };
        let legend = row()
            .flex_none()
            .gap(px(SPACE_3))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(row().gap(px(SPACE_1)).child(StatusDot::new(Status::Running).small()).child("agent"))
            .child(row().gap(px(SPACE_1)).child(StatusDot::new(Status::Spawn).small()).child("spawned"))
            .child(row().gap(px(SPACE_1)).child(StatusDot::new(Status::Waiting).small()).child("waiting for you"));
        let head = row()
            .gap(px(SPACE_1_5))
            .type_style(LABEL)
            .child(div().flex_none().type_style(LABEL_STRONG).child(format!("Test run #{}", run.number.max(1))))
            .child(div().flex_1().min_w_0().text_color(TEXT_MUTED).child(truncate_lh(format!("· {}", facts.join(" · ")), LABEL.line)))
            .child(state_word(run.state))
            .child(legend)
            .child(control);
        let rows = timeline_rows(run, now);
        let total = (end - run.started).max(0) as u64;
        let timeline = if rows.is_empty() {
            div().pt(px(SPACE_3)).type_style(LABEL).text_color(TEXT_MUTED).child(if open { "Starting the first nodes…" } else { "No node ran." }).into_any_element()
        } else {
            div().mt(px(SPACE_1)).child(Timeline::new(rows).ticks(ticks(total))).into_any_element()
        };
        col()
            .gap(px(SPACE_1))
            .child(head)
            .children(self.render_waiting(run, window, cx))
            .child(timeline)
            .into_any_element()
    }

    /// What the run waits for, or why it stopped.
    fn render_waiting(&self, run: &Run, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let banner = match run.state {
            RunState::WaitingAnswer => {
                let question = asking(run).map(|(_, question)| question).unwrap_or_default();
                Banner::new()
                    .tone(BannerTone::Warning)
                    .child(icon(IconName::Chat, 16., WARNING))
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(SPACE_1_5))
                            .child(div().type_style(BODY_STRONG).child(if question.is_empty() { "An agent is waiting for your answer".to_string() } else { question }))
                            .child(
                                row()
                                    .gap(px(SPACE_2))
                                    .child(div().flex_1().min_w_0().child(text_field("test-run-answer-field", &self.test.answer).small()).probe("test-run-answer"))
                                    .child(
                                        Button::new("test-run-answer-send", "Answer")
                                            .secondary()
                                            .small()
                                            .on_click(cx.listener(|this, _, _, cx| this.send_answer(cx)))
                                            .probe("test-run-answer-send"),
                                    ),
                            ),
                    )
            }
            RunState::WaitingReview => Banner::new()
                .tone(BannerTone::Warning)
                .child(icon(IconName::Inbox, 16., WARNING))
                .child(div().flex_1().min_w_0().child("Variants are waiting for you to pick one in the Inbox"))
                .child(Link::new("test-run-inbox", "Open in Inbox").on_click(cx.listener(|this, _, _, cx| this.open_review(cx)))),
            RunState::Failed | RunState::Interrupted => {
                let why = match (run.state, &run.error) {
                    (_, Some(error)) => error.clone(),
                    (RunState::Interrupted, None) => "The app closed while it was running.".to_string(),
                    _ => "It stopped with an error.".to_string(),
                };
                Banner::new()
                    .tone(BannerTone::Danger)
                    .child(icon(IconName::Alert, 16., DANGER))
                    .child(div().flex_1().min_w_0().child(why))
                    .child(
                        Button::new("test-run-resume", "Resume")
                            .secondary()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.resume_test_run(cx)))
                            .probe("test-run-resume"),
                    )
            }
            _ => return None,
        };
        Some(div().pt(px(SPACE_1_5)).child(banner).into_any_element())
    }
}

/// The state of a run in one or two words, coloured by meaning.
fn state_word(state: RunState) -> AnyElement {
    let (word, color) = match state {
        RunState::Queued => ("queued", TEXT_MUTED),
        RunState::Running => ("running", INFO),
        RunState::WaitingReview | RunState::WaitingAnswer => ("waiting for you", WARNING),
        RunState::Done => ("passed", SUCCESS),
        RunState::Failed => ("failed", DANGER),
        RunState::Cancelled => ("stopped", TEXT_MUTED),
        RunState::Interrupted => ("interrupted", DANGER),
    };
    div()
        .flex_none()
        .mr(px(SPACE_2))
        .type_style(LABEL)
        .text_color(color)
        .child(word)
        .probe_value("test-run-state", word)
        .into_any_element()
}

/// The node waiting for an answer and its question.
fn asking(run: &Run) -> Option<(hub::model::NodeId, String)> {
    run.nodes
        .iter()
        .rev()
        .find(|n| n.status == NodeStatus::Waiting && n.question.is_some())
        .map(|n| (n.node.clone(), n.question.clone().unwrap_or_default()))
}

/// One timeline row per node, in the order the nodes first ran: variants share a row
/// ("Fixer ×3"), a spawned agent says who made it.
pub(crate) fn timeline_rows(run: &Run, now: i64) -> Vec<TimelineRow> {
    let end = run.finished.unwrap_or(now).max(run.started);
    let total = (end - run.started).max(0) as u64;
    let mut order: Vec<&hub::model::NodeId> = Vec::new();
    for instance in &run.nodes {
        if instance.started.is_some() && !order.contains(&&instance.node) {
            order.push(&instance.node);
        }
    }
    order
        .into_iter()
        .map(|node| {
            let group: Vec<_> = run.nodes.iter().filter(|n| &n.node == node && n.started.is_some()).collect();
            let first = group[0];
            let variants = group.iter().map(|n| n.variant).collect::<std::collections::BTreeSet<_>>().len();
            let label = if variants > 1 { format!("{} ×{variants}", first.name) } else { first.name.clone() };
            let spawned = first.kind == SpanKind::Spawned;
            let meta = match (&first.spawned_by, spawned) {
                (Some(by), true) => {
                    let by = run.nodes.iter().find(|n| &n.node == by).map_or_else(|| by.to_string(), |n| n.name.clone());
                    format!("by {by}")
                }
                _ => group.iter().rev().map(|n| n.summary.as_str()).find(|s| !s.is_empty()).map(|s| base::fmt::truncate_chars(s, META_CHARS)).unwrap_or_default(),
            };
            let waiting = group.iter().any(|n| n.kind == SpanKind::Waiting || n.status == NodeStatus::Waiting);
            let tone = if waiting {
                BarTone::Waiting
            } else if spawned {
                BarTone::Spawn
            } else {
                BarTone::Agent
            };
            let start = group.iter().filter_map(|n| n.started).min().unwrap_or(run.started);
            let stop = group
                .iter()
                .map(|n| match (n.finished, n.started) {
                    (Some(finished), _) => finished,
                    (None, _) if matches!(n.status, NodeStatus::Running | NodeStatus::Waiting) => end,
                    (None, started) => started.unwrap_or(start),
                })
                .max()
                .unwrap_or(start);
            let offset = |at: i64| (at - run.started).clamp(0, total as i64) as u64;
            TimelineRow::from_ms(label, meta, offset(start), offset(stop), total, tone)
        })
        .collect()
}

/// The axis of a run that took `total_ms`: a few round steps and the end.
fn ticks(total_ms: u64) -> Vec<Tick> {
    let secs = total_ms / 1000;
    let step = TICK_STEPS.iter().copied().find(|step| secs / step <= MAX_TICKS).unwrap_or(secs.max(1));
    time_ticks(total_ms, step * 1000)
        .into_iter()
        .map(|(at, ms)| Tick::new(at, SharedString::from(base::fmt::duration(ms / 1000))))
        .collect()
}
