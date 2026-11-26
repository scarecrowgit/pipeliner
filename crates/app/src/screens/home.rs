//! Home: what needs you, what runs now, and how runs and spend went (analytics).
//! Design: design/screens/home.html. Everything here is derived from the store; with no
//! tasks at all the page is a getting-started empty state.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Datelike as _, Days, Local, Timelike as _, Utc};
use gpui::{Animation, AnimationExt as _, Hsla, Stateful};
use harness_core::{ChatMode, Event as CoreEvent, Run as CoreRun, Task as CoreTask, TaskKind};

use super::prelude::*;

/// Range options of the segmented control, in days.
const RANGES: [usize; 3] = [7, 30, 90];

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// A task that waits for the user.
struct Waiting {
    id: Id,
    title: String,
    sub: String,
    since: DateTime<Utc>,
    chat: Option<Id>,
}

/// A task that is running right now.
struct Running {
    id: Id,
    title: String,
    sub: String,
    since: DateTime<Utc>,
    cost: f64,
}

struct RunRow {
    number: u64,
    title: String,
    pipeline: String,
    pipeline_id: Id,
    status: Status,
    pr_url: Option<String>,
    cost: f64,
    at: DateTime<Utc>,
}

struct EventRow {
    seq: i64,
    ts: DateTime<Utc>,
    kind: String,
    text: String,
    target: Option<Route>,
}

#[derive(Default)]
struct HomeData {
    /// The store has no tasks and no runs: show the getting-started page.
    empty: bool,
    days: usize,
    /// Spend per local day, oldest first (`days` entries, last is today).
    per_day: Vec<f64>,
    day_labels: Vec<String>,
    spend: f64,
    prev_spend: f64,
    today_spend: f64,
    month_spend: f64,
    runs: usize,
    prev_runs: usize,
    /// Share of finished runs (done or failed) that are done.
    success_pct: Option<u32>,
    waiting: Vec<Waiting>,
    running: Vec<Running>,
    median_decide: Option<i64>,
    prev_median: Option<i64>,
    agent_done: usize,
    agent_failed: usize,
    by_pipeline: Vec<(String, f64)>,
    by_model: Vec<(String, f64)>,
    by_actor: Vec<(String, f64)>,
    recent: Vec<RunRow>,
    events: Vec<EventRow>,
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// `anthropic/claude-sonnet-4-5-20250929` -> `claude-sonnet-4-5`.
fn short_model(m: &str) -> String {
    let m = m.rsplit('/').next().unwrap_or(m);
    let m = m.split('@').next().unwrap_or(m);
    let dated = m.len() > 9 && {
        let (head, tail) = m.split_at(m.len() - 9);
        !head.is_empty() && tail.starts_with('-') && tail[1..].bytes().all(|b| b.is_ascii_digit())
    };
    let m = if dated { &m[..m.len() - 9] } else { m };
    if m.is_empty() {
        "unknown".to_string()
    } else {
        m.to_string()
    }
}

/// Sorts descending by value and drops zeros.
fn ranked(map: HashMap<String, f64>) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = map.into_iter().filter(|(_, c)| *c > 0.0).collect();
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    v
}

impl HomeData {
    fn query(store: &Store, days: usize) -> HomeData {
        let tasks = store.tasks();
        let runs = store.runs();
        if tasks.is_empty() && runs.is_empty() {
            return HomeData {
                empty: true,
                days,
                ..Default::default()
            };
        }
        let pipelines: HashMap<Id, String> = store
            .pipelines()
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect();
        let chats: HashMap<Id, String> =
            store.chats().into_iter().map(|c| (c.id, c.title)).collect();
        let run_map: HashMap<&str, &CoreRun> = runs.iter().map(|r| (r.id.as_str(), r)).collect();
        let task_map: HashMap<&str, &CoreTask> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        let today = Local::now().date_naive();
        let start = today - Days::new(days as u64 - 1);
        let prev_start = start - Days::new(days as u64);
        let day_of = |t: DateTime<Utc>| t.with_timezone(&Local).date_naive();
        let in_range = |d: chrono::NaiveDate| d >= start && d <= today;
        let in_prev = |d: chrono::NaiveDate| d >= prev_start && d < start;

        let mut d = HomeData {
            days,
            per_day: vec![0.0; days],
            ..Default::default()
        };
        d.day_labels = (0..days)
            .map(|i| {
                (start + Days::new(i as u64))
                    .format("%a %b %-d")
                    .to_string()
            })
            .collect();

        let origin = |t: &CoreTask| -> String {
            if let Some(r) = t.run_id.as_deref().and_then(|id| run_map.get(id)) {
                pipelines
                    .get(&r.pipeline_id)
                    .cloned()
                    .unwrap_or_else(|| "Deleted pipeline".into())
            } else if t.kind == TaskKind::Chat || t.chat_id.is_some() {
                "Chats".into()
            } else {
                "Other".into()
            }
        };
        let context = |t: &CoreTask| -> String {
            if let Some(r) = t.run_id.as_deref().and_then(|id| run_map.get(id)) {
                format!("Run #{} · {}", r.number, r.title)
            } else if let Some(c) = t.chat_id.as_deref().and_then(|id| chats.get(id)) {
                format!("Chat · {c}")
            } else {
                t.kind.label().to_string()
            }
        };

        let (mut by_pipeline, mut by_model, mut by_actor): (
            HashMap<String, f64>,
            HashMap<String, f64>,
            HashMap<String, f64>,
        ) = Default::default();
        for t in &tasks {
            let when = t.started_at.unwrap_or(t.created_at);
            let day = day_of(when);
            let now = Local::now();
            let ld = when.with_timezone(&Local);
            if ld.year() == now.year() && ld.month() == now.month() {
                d.month_spend += t.cost_usd;
            }
            if day == today {
                d.today_spend += t.cost_usd;
            }
            if in_range(day) {
                d.per_day[(day - start).num_days() as usize] += t.cost_usd;
                d.spend += t.cost_usd;
                *by_pipeline.entry(origin(t)).or_default() += t.cost_usd;
                if !t.model.is_empty() {
                    *by_model.entry(short_model(&t.model)).or_default() += t.cost_usd;
                }
                if !t.actor.is_empty() {
                    *by_actor.entry(t.actor.clone()).or_default() += t.cost_usd;
                }
                if t.kind != TaskKind::Chat && t.actor != "you" {
                    match t.status {
                        Status::Done => d.agent_done += 1,
                        Status::Failed => d.agent_failed += 1,
                        _ => {}
                    }
                }
            } else if in_prev(day) {
                d.prev_spend += t.cost_usd;
            }
        }
        d.by_pipeline = ranked(by_pipeline);
        d.by_model = ranked(by_model);
        d.by_actor = ranked(by_actor);

        let (mut done, mut failed) = (0usize, 0usize);
        for r in &runs {
            let day = day_of(r.created_at);
            if in_range(day) {
                d.runs += 1;
                match r.status {
                    Status::Done => done += 1,
                    Status::Failed => failed += 1,
                    _ => {}
                }
            } else if in_prev(day) {
                d.prev_runs += 1;
            }
        }
        d.success_pct = (done * 100).checked_div(done + failed).map(|p| p as u32);

        let (mut cur, mut prev) = (vec![], vec![]);
        for dec in store.decisions() {
            let Some(gate) = task_map.get(dec.task_id.as_str()) else {
                continue;
            };
            let secs = (dec.created_at - gate.created_at).num_seconds().max(0);
            let day = day_of(dec.created_at);
            if in_range(day) {
                cur.push(secs);
            } else if in_prev(day) {
                prev.push(secs);
            }
        }
        d.median_decide = median(cur);
        d.prev_median = median(prev);

        let mut waiting: Vec<&CoreTask> = tasks
            .iter()
            .filter(|t| t.status == Status::Waiting)
            .collect();
        waiting.sort_by_key(|t| t.created_at);
        d.waiting = waiting
            .into_iter()
            .map(|t| Waiting {
                id: t.id.clone(),
                title: t.title.clone(),
                sub: if t.actor.is_empty() {
                    context(t)
                } else {
                    format!("{} · {}", context(t), t.actor)
                },
                since: t.created_at,
                chat: t.chat_id.clone(),
            })
            .collect();
        let mut running: Vec<&CoreTask> = tasks
            .iter()
            .filter(|t| t.status == Status::Running)
            .collect();
        running.sort_by_key(|t| t.started_at.unwrap_or(t.created_at));
        d.running = running
            .into_iter()
            .map(|t| Running {
                id: t.id.clone(),
                title: t.title.clone(),
                sub: if t.actor.is_empty() {
                    context(t)
                } else {
                    format!("{} · {}", t.actor, context(t))
                },
                since: t.started_at.unwrap_or(t.created_at),
                cost: t.cost_usd,
            })
            .collect();

        let mut recent: Vec<&CoreRun> = runs.iter().collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        d.recent = recent
            .into_iter()
            .take(8)
            .map(|r| RunRow {
                number: r.number,
                title: r.title.clone(),
                pipeline: pipelines
                    .get(&r.pipeline_id)
                    .cloned()
                    .unwrap_or_else(|| "Deleted pipeline".into()),
                pipeline_id: r.pipeline_id.clone(),
                status: r.status,
                pr_url: r.pr_url.clone(),
                cost: r.cost_usd,
                at: r.created_at,
            })
            .collect();

        d.events = store
            .events(14)
            .into_iter()
            .map(|e: CoreEvent| {
                let target = if let Some(t) = e.task_id.clone() {
                    Some(Route::Task { task: t })
                } else if let Some(r) = e.run_id.as_deref().and_then(|id| run_map.get(id)) {
                    Some(Route::Pipelines {
                        pipeline: Some(r.pipeline_id.clone()),
                    })
                } else if e.kind.starts_with("chat.") {
                    Some(Route::Chat { chat: None })
                } else if e.kind.starts_with("pipeline.") {
                    Some(Route::Pipelines { pipeline: None })
                } else {
                    None
                };
                EventRow {
                    seq: e.seq,
                    ts: e.ts,
                    kind: e.kind,
                    text: e.text,
                    target,
                }
            })
            .collect();
        d
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// `4m 12s`; hours fall back to the shared short format.
fn fmt_precise(secs: i64) -> String {
    if secs < 60 {
        format!("{}s", secs.max(0))
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        ui::fmt_duration(secs)
    }
}

/// `↑ 12%` / `↓ 8%` against the previous period, `None` when there is no baseline.
fn delta(cur: f64, prev: f64) -> Option<String> {
    if prev <= 0.0 {
        return None;
    }
    let pct = (cur - prev) / prev * 100.0;
    if pct.abs() < 1.0 {
        return Some("→ 0%".to_string());
    }
    Some(format!(
        "{} {:.0}%",
        if pct > 0.0 { "↑" } else { "↓" },
        pct.abs()
    ))
}

fn greeting() -> &'static str {
    match Local::now().hour() {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Working late",
    }
}

/// Icon and tint for an event kind.
fn event_look(kind: &str) -> (AppIcon, Hsla) {
    match kind {
        "run.failed" | "worktree.setup_failed" => (AppIcon::CircleX, theme::DANGER),
        "run.finished" => (AppIcon::CircleCheck, theme::SUCCESS),
        "run.cancelled" => (AppIcon::CircleX, theme::TEXT_MUTED),
        "run.sent_back" => (AppIcon::RotateCcw, theme::WARNING),
        "gate.waiting" | "plan.proposed" => (AppIcon::Inbox, theme::WARNING),
        "decision" => (AppIcon::Check, theme::SUCCESS),
        "task.spawned" => (AppIcon::GitFork, theme::SPAWN),
        "task.retry" => (AppIcon::RotateCcw, theme::INFO),
        "task.steer" => (AppIcon::Send, theme::ACCENT_TEXT),
        "pr.opened" | "pr.merged" => (AppIcon::GitPullRequest, theme::SUCCESS),
        "app.updated" => (AppIcon::Download, theme::TEXT_MUTED),
        k if k.starts_with("run.") => (AppIcon::Play, theme::INFO),
        k if k.starts_with("task.") => (AppIcon::Zap, theme::INFO),
        k if k.starts_with("chat.") => (AppIcon::Chat, theme::ACCENT_TEXT),
        k if k.starts_with("pipeline.") => (AppIcon::Workflow, theme::ACCENT_TEXT),
        k if k.starts_with("worktree.") => (AppIcon::GitBranch, theme::TEXT_MUTED),
        _ => (AppIcon::Activity, theme::TEXT_MUTED),
    }
}

/// Fade-in used by list rows when they first appear.
fn fade_id(id: String) -> ElementId {
    ElementId::Name(id.into())
}

fn fade_in() -> Animation {
    Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out)
}

/// A bordered section with a header bar.
fn section(title: &'static str, icon: AppIcon, meta: Option<String>) -> Div {
    panel().flex_1().min_w(px(300.)).child(
        div()
            .px(theme::S4)
            .py(theme::S3)
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                SectionHeader::new(title)
                    .icon(icon)
                    .when_some(meta, |h, m| h.meta(m)),
            ),
    )
}

fn compact_empty(
    id: &'static str,
    icon: AppIcon,
    title: &'static str,
    text: &'static str,
) -> impl IntoElement {
    EmptyState::new(icon, title, text).compact().id(id)
}

fn new_chat(cx: &mut App) {
    let task = state::run_engine(cx, |e| e.new_chat(ChatMode::Direct));
    cx.spawn(async move |cx| {
        if let Some(chat) = task.await {
            let _ = cx.update(|cx| {
                state::navigate(
                    Route::Chat {
                        chat: Some(chat.id),
                    },
                    cx,
                )
            });
        }
    })
    .detach();
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

pub struct HomeScreen {
    data: Option<HomeData>,
    days: usize,
    daily_budget: f64,
    monthly_budget: f64,
    configured: bool,
    load: Option<Task<()>>,
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
}

impl HomeScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                // Streaming tokens only touch messages; nothing here depends on them.
                let relevant = changes.iter().any(|c| {
                    !matches!(
                        c,
                        Change::Message { .. }
                            | Change::Variant { .. }
                            | Change::Artifact
                            | Change::Chat(_)
                    )
                });
                if relevant {
                    this.reload(cx);
                }
            }
        });
        // Elapsed times of running tasks tick every second while Home is open; ages of
        // waiting items and events refresh every 30 seconds.
        let ticker = cx.spawn(async move |this, cx| {
            let mut n = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                n = n.wrapping_add(1);
                let alive = this.update(cx, |this, cx| {
                    let visible = state::route(cx) == Route::Home;
                    let running = this.data.as_ref().is_some_and(|d| !d.running.is_empty());
                    if visible && (running || n.is_multiple_of(30)) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut this = HomeScreen {
            data: None,
            days: 30,
            daily_budget: 0.0,
            monthly_budget: 0.0,
            configured: true,
            load: None,
            _ticker: ticker,
            _subs: vec![sub],
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let cfg = state::engine(cx).config();
        self.daily_budget = cfg.daily_budget_usd;
        self.monthly_budget = cfg.monthly_budget_usd;
        self.configured = cfg.is_configured();
        let days = self.days;
        self.load = Some(load(
            cx,
            move |s| HomeData::query(s, days),
            |this, d, _| this.data = Some(d),
        ));
    }

    fn set_days(&mut self, days: usize, cx: &mut Context<Self>) {
        if self.days != days {
            self.days = days;
            self.reload(cx);
        }
    }

    // ----- pieces -----

    fn tile(id: &'static str, label: &'static str, last: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex_1()
            .min_w(px(150.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(theme::S6)
            .py(theme::S4)
            .when(!last, |d| d.border_r_1().border_color(theme::LINE))
            .child(div().text_label().fg_muted().child(label))
    }

    fn value_row(value: String, color: Hsla, sub: Option<(String, Hsla)>) -> Div {
        div()
            .flex()
            .items_baseline()
            .gap(theme::S2)
            .child(
                div()
                    .text_stat_sm()
                    .text_color(color)
                    .whitespace_nowrap()
                    .child(value),
            )
            .when_some(sub, |d, (s, c)| {
                d.child(div().text_label().text_color(c).ellipsis().child(s))
            })
    }

    fn stats(&self, d: &HomeData) -> impl IntoElement {
        let span = format!("{}d", d.days);
        let spend_sub = delta(d.spend, d.prev_spend).map(|s| {
            let up = s.starts_with('↑');
            (
                format!("{s} vs prev {span}"),
                if up {
                    theme::WARNING
                } else {
                    theme::TEXT_MUTED
                },
            )
        });
        let spend =
            Self::tile("home-stat-spend", "Spend", false)
                .child(Self::value_row(
                    ui::fmt_usd(d.spend),
                    theme::TEXT,
                    spend_sub,
                ))
                .when(d.per_day.iter().filter(|v| **v > 0.0).count() >= 2, |t| {
                    t.child(div().pt(theme::S1).child(
                        Sparkline::new("home-spend-spark", d.per_day.clone()).height(px(22.)),
                    ))
                });

        let runs_sub = match (d.success_pct, delta(d.runs as f64, d.prev_runs as f64)) {
            (Some(p), _) => Some((format!("{p}% succeeded"), theme::TEXT_MUTED)),
            (None, Some(s)) => Some((format!("{s} vs prev {span}"), theme::TEXT_MUTED)),
            _ => None,
        };
        let runs = Self::tile("home-stat-runs", "Runs", false).child(Self::value_row(
            d.runs.to_string(),
            theme::TEXT,
            runs_sub,
        ));

        let n = d.waiting.len();
        let oldest = d.waiting.first().map(|w| {
            (
                format!("oldest {}", ui::fmt_ago(w.since)),
                theme::TEXT_MUTED,
            )
        });
        let waiting = Self::tile("home-stat-waiting", "Waiting for you", false)
            .when(n > 0, |t| {
                t.cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .on_click(|_, _, cx| state::navigate(Route::Review { task: None }, cx))
            })
            .child(Self::value_row(
                n.to_string(),
                if n > 0 { theme::WARNING } else { theme::TEXT },
                oldest,
            ));

        let (median_v, median_sub) = match d.median_decide {
            Some(m) => (
                fmt_precise(m),
                d.prev_median
                    .map(|p| (format!("was {}", fmt_precise(p)), theme::TEXT_MUTED)),
            ),
            None => (
                "—".to_string(),
                Some(("no decisions yet".to_string(), theme::TEXT_MUTED)),
            ),
        };
        let decide = Self::tile("home-stat-decide", "Median time to decide", false)
            .child(Self::value_row(median_v, theme::TEXT, median_sub));

        let done_sub =
            (d.agent_failed > 0).then(|| (format!("{} failed", d.agent_failed), theme::DANGER));
        let done = Self::tile("home-stat-done", "Tasks done by agents", true).child(
            Self::value_row(d.agent_done.to_string(), theme::TEXT, done_sub),
        );

        div()
            .flex()
            .flex_wrap()
            .border_b_1()
            .border_color(theme::LINE)
            .child(spend)
            .child(runs)
            .child(waiting)
            .child(decide)
            .child(done)
    }

    fn needs_you(d: &HomeData) -> Div {
        let n = d.waiting.len();
        let sec = section(
            "Needs you",
            AppIcon::Inbox,
            (n > 0).then(|| format!("{n} waiting")),
        );
        if n == 0 {
            return sec.min_w(px(420.)).child(compact_empty(
                "home-wait-empty",
                AppIcon::CircleCheck,
                "All clear",
                "Nothing is waiting for you right now.",
            ));
        }
        sec.min_w(px(420.))
            .children(d.waiting.iter().take(8).map(|w| {
                let id = w.id.clone();
                let chat = w.chat.clone();
                div()
                    .id(SharedString::from(format!("home-wait-{}", w.id)))
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .min_h(px(56.))
                    .px(theme::S4)
                    .py(theme::S2)
                    .border_b_1()
                    .border_color(theme::LINE)
                    .child(StatusDot::new(Status::Waiting))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_body_strong().ellipsis().child(w.title.clone()))
                            .child(
                                div()
                                    .text_label()
                                    .fg_muted()
                                    .ellipsis()
                                    .child(w.sub.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .whitespace_nowrap()
                            .child(ui::fmt_ago(w.since)),
                    )
                    .when_some(chat, |d, c| {
                        d.child(
                            Button::new(
                                SharedString::from(format!("home-wait-chat-{}", w.id)),
                                "Chat",
                            )
                            .secondary()
                            .small()
                            .icon(AppIcon::Chat)
                            .on_click(move |_, _, cx| {
                                state::navigate(
                                    Route::Chat {
                                        chat: Some(c.clone()),
                                    },
                                    cx,
                                )
                            }),
                        )
                    })
                    .child(
                        Button::new(
                            SharedString::from(format!("home-wait-review-{}", w.id)),
                            "Review",
                        )
                        .primary()
                        .small()
                        .on_click(move |_, _, cx| {
                            state::navigate(
                                Route::Review {
                                    task: Some(id.clone()),
                                },
                                cx,
                            )
                        }),
                    )
                    .with_animation(
                        fade_id(format!("home-wait-in-{}", w.id)),
                        fade_in(),
                        |d, t| d.opacity(t),
                    )
            }))
            .when(n > 8, |s| {
                s.child(
                    div()
                        .id("home-wait-more")
                        .px(theme::S4)
                        .py(theme::S3)
                        .text_label()
                        .fg_accent()
                        .cursor_pointer()
                        .child(format!("Review queue · {} more", n - 8))
                        .on_click(|_, _, cx| state::navigate(Route::Review { task: None }, cx)),
                )
            })
    }

    fn running_now(d: &HomeData) -> Div {
        let n = d.running.len();
        let sec = section(
            "Running now",
            AppIcon::Activity,
            (n > 0).then(|| format!("{n} task{}", if n == 1 { "" } else { "s" })),
        );
        if n == 0 {
            return sec.child(compact_empty(
                "home-run-empty",
                AppIcon::Activity,
                "Nothing running",
                "Tasks that are working right now show up here.",
            ));
        }
        sec.children(d.running.iter().take(8).enumerate().map(|(i, r)| {
            let id = r.id.clone();
            div()
                .id(SharedString::from(format!("home-run-{}", r.id)))
                .flex()
                .items_center()
                .gap(theme::S3)
                .min_h(px(48.))
                .px(theme::S4)
                .py(theme::S2)
                .border_b_1()
                .border_color(theme::LINE)
                .cursor_pointer()
                .hover(|s| s.bg(theme::SURFACE_RAISED))
                .child(StatusDot::new(Status::Running).id(("home-run-dot", i)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_body_strong().ellipsis().child(r.title.clone()))
                        .child(
                            div()
                                .text_label()
                                .fg_muted()
                                .ellipsis()
                                .child(r.sub.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .child(
                            div()
                                .text_mono()
                                .fg_secondary()
                                .child(ui::fmt_duration((Utc::now() - r.since).num_seconds())),
                        )
                        .child(div().text_mono_sm().fg_muted().child(ui::fmt_usd(r.cost))),
                )
                .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
        }))
    }

    fn spend_chart(&self, d: &HomeData) -> Div {
        let daily = self.daily_budget;
        let monthly = self.monthly_budget;
        let sec = section(
            "Spend per day",
            AppIcon::Activity,
            Some(format!("last {} days", d.days)),
        )
        .flex_none()
        .w_full();
        if d.spend <= 0.0 {
            return sec.child(compact_empty(
                "home-spend-empty",
                AppIcon::Activity,
                "No spend in this period",
                "Cost is recorded per task as models run.",
            ));
        }
        let over: Vec<bool> = d
            .per_day
            .iter()
            .map(|v| daily > 0.0 && *v > daily)
            .collect();
        let tips: Vec<SharedString> = d
            .per_day
            .iter()
            .zip(&d.day_labels)
            .map(|(v, l)| SharedString::from(format!("{l} · {}", ui::fmt_usd(*v))))
            .collect();
        let mid = d
            .day_labels
            .get(d.days / 2)
            .map(|l| l.split_once(' ').map(|x| x.1).unwrap_or(l).to_string())
            .unwrap_or_default();
        let first = d
            .day_labels
            .first()
            .map(|l| l.split_once(' ').map(|x| x.1).unwrap_or(l).to_string())
            .unwrap_or_default();
        let budget_row = |id: &'static str, label: &'static str, spent: f64, cap: f64| {
            div()
                .flex_1()
                .min_w(px(220.))
                .flex()
                .flex_col()
                .gap(theme::S1)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_label()
                        .child(div().fg_muted().child(label))
                        .child(div().text_mono().fg_secondary().child(format!(
                            "{} of {}",
                            ui::fmt_usd(spent),
                            ui::fmt_usd(cap)
                        ))),
                )
                .child(ProgressBar::new(id, spent / cap).budget())
        };
        sec.child(
            div()
                .p(theme::S4)
                .flex()
                .flex_col()
                .gap(theme::S4)
                .child(
                    BarChart::new("home-spend-chart", d.per_day.clone())
                        .height(px(150.))
                        .highlight(over)
                        .tooltips(tips)
                        .limit(
                            (daily > 0.0).then_some(daily),
                            format!("limit {}", ui::fmt_usd(daily)),
                        )
                        .x_labels(first, mid, "today")
                        .format(|v| {
                            if v < 10.0 {
                                format!("${v:.1}")
                            } else {
                                format!("${v:.0}")
                            }
                        }),
                )
                .when(daily > 0.0 || monthly > 0.0, |c| {
                    c.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(theme::S6)
                            .when(daily > 0.0, |r| {
                                r.child(budget_row(
                                    "home-budget-day",
                                    "Today",
                                    d.today_spend,
                                    daily,
                                ))
                            })
                            .when(monthly > 0.0, |r| {
                                r.child(budget_row(
                                    "home-budget-month",
                                    "This month",
                                    d.month_spend,
                                    monthly,
                                ))
                            }),
                    )
                }),
        )
    }

    fn breakdown(
        id: &'static str,
        title: &'static str,
        icon: AppIcon,
        items: &[(String, f64)],
        d: &HomeData,
    ) -> Div {
        let sec = section(title, icon, Some(format!("{}d", d.days)));
        if items.is_empty() {
            return sec.child(compact_empty(
                "home-bd-empty",
                icon,
                "No spend yet",
                "Cost per task shows up here.",
            ));
        }
        let max = items
            .iter()
            .map(|i| i.1)
            .fold(0.0, f64::max)
            .max(f64::EPSILON);
        let total: f64 = items.iter().map(|i| i.1).sum();
        sec.child(
            div()
                .p(theme::S4)
                .flex()
                .flex_col()
                .children(items.iter().take(6).enumerate().map(|(i, (name, cost))| {
                    MeterRow::new((id, i), name.clone(), cost / max, ui::fmt_usd(*cost))
                }))
                .child(
                    div()
                        .pt(theme::S2)
                        .text_label()
                        .fg_muted()
                        .child(format!("{} total", ui::fmt_usd(total))),
                ),
        )
    }

    fn recent_runs(d: &HomeData) -> Div {
        let sec = section("Recent runs", AppIcon::Play, None);
        if d.recent.is_empty() {
            return sec.child(compact_empty(
                "home-runs-empty",
                AppIcon::Play,
                "No runs yet",
                "Start a pipeline and its runs are listed here.",
            ));
        }
        sec.children(d.recent.iter().enumerate().map(|(i, r)| {
            let pid = r.pipeline_id.clone();
            let url = r.pr_url.clone();
            div()
                .flex()
                .items_center()
                .gap(theme::S3)
                .min_h(px(44.))
                .px(theme::S4)
                .border_b_1()
                .border_color(theme::LINE)
                .child(
                    div()
                        .id(("home-recent", i))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(theme::S3)
                        .py(theme::S2)
                        .cursor_pointer()
                        .child(StatusDot::new(r.status).id(("home-recent-dot", i)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_body_strong()
                                        .ellipsis()
                                        .child(format!("#{} {}", r.number, r.title)),
                                )
                                .child(div().text_label().fg_muted().ellipsis().child(format!(
                                    "{} · {} · {} ago",
                                    r.pipeline,
                                    r.status.label(),
                                    ui::fmt_ago(r.at)
                                ))),
                        )
                        .on_click(move |_, _, cx| {
                            state::navigate(
                                Route::Pipelines {
                                    pipeline: Some(pid.clone()),
                                },
                                cx,
                            )
                        }),
                )
                .when_some(url, |d, u| {
                    d.child(
                        div()
                            .id(("home-recent-pr", i))
                            .flex()
                            .items_center()
                            .gap(theme::S1)
                            .text_label()
                            .fg_accent()
                            .cursor_pointer()
                            .child(ui::icon_sized(AppIcon::GitPullRequest, theme::ICON_SM))
                            .child("PR")
                            .on_click(move |_, _, cx| cx.open_url(&u)),
                    )
                })
                .child(
                    div()
                        .w(px(56.))
                        .text_right()
                        .text_mono()
                        .fg_secondary()
                        .child(ui::fmt_usd(r.cost)),
                )
        }))
    }

    fn activity(d: &HomeData) -> Div {
        let sec = section("Activity", AppIcon::Clock, None);
        if d.events.is_empty() {
            return sec.child(compact_empty(
                "home-act-empty",
                AppIcon::Clock,
                "No activity yet",
                "Runs, decisions and pull requests are logged here.",
            ));
        }
        let today = Local::now().date_naive();
        sec.children(d.events.iter().map(|e| {
            let (icon, tint) = event_look(&e.kind);
            let local = e.ts.with_timezone(&Local);
            let when = if local.date_naive() == today {
                local.format("%H:%M").to_string()
            } else {
                local.format("%b %-d").to_string()
            };
            let target = e.target.clone();
            div()
                .id(SharedString::from(format!("home-ev-{}", e.seq)))
                .flex()
                .items_start()
                .gap(theme::S3)
                .px(theme::S4)
                .py(theme::S2)
                .border_b_1()
                .border_color(theme::LINE_SUBTLE)
                .when(target.is_some(), |d| {
                    d.cursor_pointer().hover(|s| s.bg(theme::SURFACE_RAISED))
                })
                .child(
                    div()
                        .w(px(44.))
                        .flex_none()
                        .text_mono()
                        .fg_muted()
                        .child(when),
                )
                .child(
                    div()
                        .flex_none()
                        .pt(px(3.))
                        .text_color(tint)
                        .child(ui::icon_sized(icon, theme::ICON_SM)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_body()
                        .fg_secondary()
                        .child(e.text.clone()),
                )
                .when_some(target, |d, r| {
                    d.on_click(move |_, _, cx| state::navigate(r.clone(), cx))
                })
                .with_animation(
                    fade_id(format!("home-ev-in-{}", e.seq)),
                    fade_in(),
                    |d, t| d.opacity(t),
                )
        }))
    }

    fn getting_started(&self) -> impl IntoElement {
        let mut actions = div().flex().flex_wrap().justify_center().gap(theme::S2);
        if !self.configured {
            actions = actions.child(
                Button::new("home-empty-settings", "Set up a provider")
                    .primary()
                    .icon(AppIcon::Sliders)
                    .on_click(|_, _, cx| state::navigate(Route::Settings, cx)),
            );
        }
        let chat = Button::new("home-empty-chat", "New chat")
            .icon(AppIcon::Chat)
            .on_click(|_, _, cx| new_chat(cx));
        let pipeline = Button::new("home-empty-pipeline", "New pipeline")
            .icon(AppIcon::Plus)
            .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx));
        let (chat, pipeline) = if self.configured {
            (chat.primary(), pipeline)
        } else {
            (chat, pipeline)
        };
        let text = if self.configured {
            "Start a chat or create a pipeline. Runs, spend and the things that need you will show up here."
        } else {
            "Connect a model provider in Settings first, then start a chat or create a pipeline. Runs, spend and the things that need you will show up here."
        };
        body().child(
            EmptyState::new(AppIcon::Home, "Welcome to Pipeliner", text)
                .id("home-getting-started")
                .action(actions.child(chat).child(pipeline)),
        )
    }
}

impl Render for HomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Local::now();
        let running = self.data.as_ref().map(|d| d.running.len()).unwrap_or(0);
        let this = cx.entity().downgrade();
        let selected = RANGES.iter().position(|d| *d == self.days).unwrap_or(1);
        let header = PageHeader::new("Home")
            .subtitle(format!("{} · {}", greeting(), now.format("%A, %b %-d")))
            .when(running > 0, |h| {
                h.meta(Badge::new(format!("{running} running")).info().dot())
            })
            .action(segmented(
                "home-range",
                &["7d", "30d", "90d"],
                selected,
                move |ix, _, cx| {
                    let _ = this.update(cx, |t, cx| t.set_days(RANGES[ix], cx));
                },
            ))
            .action(
                Button::new("home-new-run", "New run")
                    .primary()
                    .icon(AppIcon::Play)
                    .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
            );

        let content = match &self.data {
            None => body()
                .child(skeleton_rows("home-loading", 4))
                .into_any_element(),
            Some(d) if d.empty => self.getting_started().into_any_element(),
            Some(d) => body()
                .p_0()
                .gap_0()
                .child(self.stats(d))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::S4)
                        .p(theme::S6)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(theme::S4)
                                .child(Self::needs_you(d))
                                .child(Self::running_now(d)),
                        )
                        .child(self.spend_chart(d))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(theme::S4)
                                .child(Self::breakdown(
                                    "home-bd-pipeline",
                                    "Spend by pipeline",
                                    AppIcon::Workflow,
                                    &d.by_pipeline,
                                    d,
                                ))
                                .child(Self::breakdown(
                                    "home-bd-model",
                                    "Spend by model",
                                    AppIcon::Sparkles,
                                    &d.by_model,
                                    d,
                                ))
                                .child(Self::breakdown(
                                    "home-bd-actor",
                                    "Spend by agent",
                                    AppIcon::Bot,
                                    &d.by_actor,
                                    d,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(theme::S4)
                                .child(Self::recent_runs(d))
                                .child(Self::activity(d)),
                        ),
                )
                .into_any_element(),
        };

        screen().child(header).child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(
            short_model("anthropic/claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(short_model("gpt-5"), "gpt-5");
        assert_eq!(short_model(""), "unknown");
    }

    #[test]
    fn median_picks_the_middle() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![30, 10, 20]), Some(20));
    }

    #[test]
    fn delta_needs_a_baseline() {
        assert_eq!(delta(5.0, 0.0), None);
        assert_eq!(delta(15.0, 10.0).as_deref(), Some("↑ 50%"));
        assert_eq!(delta(5.0, 10.0).as_deref(), Some("↓ 50%"));
    }
}
