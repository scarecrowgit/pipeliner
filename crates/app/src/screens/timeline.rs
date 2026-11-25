//! Timeline: tasks as bars over a day, grouped by run (or chat).
//! Design: design/screens/timeline.html. Each row is a run or a chat; each task is a bar
//! from `started_at` to `finished_at` (or now while it is active). The lane scrolls
//! horizontally under a fixed label column; a one-second tick grows live bars and moves the
//! "now" line only while the screen is visible and something is running.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Days, Local, NaiveDate, TimeZone as _, Utc};
use gpui::{
    ease_in_out, linear_color_stop, linear_gradient, point, pulsating_between, relative, Animation,
    AnimationExt as _, Hsla, ScrollHandle,
};
use gpui_component::tooltip::Tooltip;
use harness_core::{Task as CoreTask, TaskKind};

use super::prelude::*;

/// Height of every row (labels and lane share it so they line up).
const ROW_PX: f32 = 36.0;
/// Height of the hour-tick header.
const HEAD_PX: f32 = 32.0;
/// Width of the fixed label column.
const LABEL_W: f32 = 290.0;
const DAY_SECS: f64 = 86_400.0;

/// Zoom levels: label, pixels per hour, minutes between tick labels, minutes between grid lines.
const ZOOMS: [(&str, f32, i64, i64); 3] = [
    ("Day", 40.0, 60, 60),
    ("Hour", 140.0, 60, 30),
    ("15 min", 480.0, 15, 15),
];

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// One task bar.
struct Bar {
    id: Id,
    title: String,
    actor: String,
    model: String,
    kind: TaskKind,
    status: Status,
    depth: usize,
    start: DateTime<Utc>,
    /// `None` while the task is active: the bar grows with the clock.
    end: Option<DateTime<Utc>>,
    cost: f64,
    tokens: u64,
    /// Queued work laid out after "now" (an estimate, not a record).
    planned: bool,
}

enum Owner {
    Run(Id),
    Chat(Id),
    Other,
}

/// One row group: a run, a chat, or the rest.
struct Group {
    key: String,
    owner: Owner,
    title: String,
    status: Option<Status>,
    bars: Vec<Bar>,
    cost: f64,
}

#[derive(Default)]
struct DayData {
    groups: Vec<Group>,
    /// Cumulative spend at the end of each hour (0 for hours that have not happened yet).
    cumulative: Vec<f64>,
    spend: f64,
    tasks: usize,
    running: usize,
    waiting: usize,
    /// Queued tasks shown as planned bars (today only).
    queued: usize,
    /// When the data was read; planned bars are shifted by the time since then.
    queried_at: DateTime<Utc>,
}

/// Local midnight of `day`, as UTC.
fn day_start(day: NaiveDate) -> DateTime<Utc> {
    let naive = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
        .with_timezone(&Utc)
}

/// Seconds of overlap between `[a0, a1]` and `[b0, b1]`.
fn overlap_secs(a0: DateTime<Utc>, a1: DateTime<Utc>, b0: DateTime<Utc>, b1: DateTime<Utc>) -> f64 {
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    ((hi - lo).num_milliseconds() as f64 / 1000.0).max(0.0)
}

impl DayData {
    fn query(store: &Store, day: NaiveDate) -> DayData {
        let ds = day_start(day);
        let de = day_start(day + Days::new(1));
        let now = Utc::now();
        let runs: HashMap<Id, harness_core::Run> = store
            .runs()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let chats: HashMap<Id, String> =
            store.chats().into_iter().map(|c| (c.id, c.title)).collect();

        let mut out = DayData {
            cumulative: vec![0.0; 24],
            queried_at: now,
            ..Default::default()
        };
        let mut hourly = [0.0f64; 24];
        let is_today = ds <= now && now < de;
        #[allow(clippy::type_complexity)]
        let mut groups: HashMap<
            String,
            (
                Group,
                Vec<(CoreTask, DateTime<Utc>, Option<DateTime<Utc>>)>,
                Vec<CoreTask>,
            ),
        > = HashMap::new();

        for t in store.tasks() {
            if t.status == Status::Queued {
                // Queued work has no real span: it is only planned on today's lane.
                if is_today {
                    out.queued += 1;
                    let (key, owner, title, status) = group_info(&t, &runs, &chats);
                    let g = groups.entry(key.clone()).or_insert_with(|| {
                        (
                            Group {
                                key,
                                owner,
                                title,
                                status,
                                bars: vec![],
                                cost: 0.0,
                            },
                            vec![],
                            vec![],
                        )
                    });
                    g.0.cost += t.cost_usd;
                    g.2.push(t);
                }
                continue;
            }
            let start = match t.started_at {
                Some(s) => s,
                None if matches!(
                    t.status,
                    Status::Waiting | Status::Running | Status::Done | Status::Failed
                ) =>
                {
                    t.created_at
                }
                None => continue,
            };
            let active = matches!(t.status, Status::Running | Status::Waiting);
            let end = if active {
                None
            } else {
                Some(t.finished_at.unwrap_or(start).max(start))
            };
            let eff_end = end.unwrap_or(now).max(start);
            if start >= de || eff_end < ds {
                continue;
            }

            // Spread the task's cost evenly over its span, bucketed by hour of this day.
            let span = (eff_end - start).num_milliseconds() as f64 / 1000.0;
            for (h, slot) in hourly.iter_mut().enumerate() {
                let b0 = ds + chrono::Duration::hours(h as i64);
                let b1 = b0 + chrono::Duration::hours(1);
                if span <= 0.0 {
                    if start >= b0 && start < b1 {
                        *slot += t.cost_usd;
                    }
                } else {
                    *slot += t.cost_usd * overlap_secs(start, eff_end, b0, b1) / span;
                }
            }

            out.tasks += 1;
            match t.status {
                Status::Running => out.running += 1,
                Status::Waiting => out.waiting += 1,
                _ => {}
            }

            let (key, owner, title, status) = group_info(&t, &runs, &chats);
            let g = groups.entry(key.clone()).or_insert_with(|| {
                (
                    Group {
                        key,
                        owner,
                        title,
                        status,
                        bars: vec![],
                        cost: 0.0,
                    },
                    vec![],
                    vec![],
                )
            });
            g.0.cost += t.cost_usd;
            g.1.push((t, start, None));
        }

        let mut cum = 0.0;
        for (h, v) in hourly.iter().enumerate() {
            cum += v;
            let started = ds + chrono::Duration::hours(h as i64) <= now;
            out.cumulative[h] = if started { cum } else { 0.0 };
        }
        out.spend = hourly.iter().sum();

        let mut list: Vec<Group> = groups
            .into_values()
            .map(|(mut g, mut tasks, mut queued)| {
                if !queued.is_empty() {
                    let durs: Vec<chrono::Duration> = tasks
                        .iter()
                        .filter(|(t, _, _)| matches!(t.status, Status::Done | Status::Failed))
                        .filter_map(|(t, _, _)| Some(t.finished_at? - t.started_at?))
                        .collect();
                    let est = estimate_duration(&durs);
                    queued.sort_by_key(|t| (t.priority, t.created_at));
                    let slots = plan_queue(now, est, de, queued.len());
                    for (t, (s, e)) in queued.into_iter().zip(slots) {
                        tasks.push((t, s, Some(e)));
                    }
                }
                g.bars = order_bars(tasks);
                g
            })
            .collect();
        let rank = |g: &Group| match g.owner {
            Owner::Run(_) => 0,
            Owner::Chat(_) => 1,
            Owner::Other => 2,
        };
        list.sort_by_key(|g| (rank(g), g.bars.first().map(|b| b.start)));
        out.groups = list;
        out
    }
}

/// Group key, owner, title and run status of the row a task belongs to.
fn group_info(
    t: &CoreTask,
    runs: &HashMap<Id, harness_core::Run>,
    chats: &HashMap<Id, String>,
) -> (String, Owner, String, Option<Status>) {
    if let Some(r) = t.run_id.as_deref().and_then(|id| runs.get(id)) {
        (
            format!("run-{}", r.id),
            Owner::Run(r.pipeline_id.clone()),
            format!("Run #{} · {}", r.number, r.title),
            Some(r.status),
        )
    } else if let Some(c) = t.chat_id.clone() {
        let title = chats.get(&c).cloned().unwrap_or_default();
        (
            format!("chat-{c}"),
            Owner::Chat(c),
            format!("Chat · {title}"),
            None,
        )
    } else {
        (
            "other".to_string(),
            Owner::Other,
            "Other tasks".to_string(),
            None,
        )
    }
}

/// Typical length of a task in a group: the mean of its finished tasks, clamped to
/// 2–30 minutes; 5 minutes when there is nothing to go on.
fn estimate_duration(finished: &[chrono::Duration]) -> chrono::Duration {
    if finished.is_empty() {
        return chrono::Duration::minutes(5);
    }
    let total: i64 = finished.iter().map(|d| d.num_milliseconds().max(0)).sum();
    let avg = chrono::Duration::milliseconds(total / finished.len() as i64);
    avg.clamp(chrono::Duration::minutes(2), chrono::Duration::minutes(30))
}

/// Lays `n` queued tasks out back-to-back from `now`, each `est` long, never past `day_end`.
fn plan_queue(
    now: DateTime<Utc>,
    est: chrono::Duration,
    day_end: DateTime<Utc>,
    n: usize,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut cur = now.min(day_end);
    (0..n)
        .map(|_| {
            let s = cur.min(day_end);
            let e = (s + est).min(day_end);
            cur = e;
            (s, e)
        })
        .collect()
}

/// Orders a group's tasks by start time with spawned tasks right under their parent.
/// The third element is the planned end of a queued task (`Some` makes a planned bar).
fn order_bars(mut tasks: Vec<(CoreTask, DateTime<Utc>, Option<DateTime<Utc>>)>) -> Vec<Bar> {
    tasks.sort_by_key(|(_, s, _)| *s);
    let ids: HashSet<Id> = tasks.iter().map(|(t, _, _)| t.id.clone()).collect();
    let mut children: HashMap<Option<Id>, Vec<usize>> = HashMap::new();
    for (i, (t, _, _)) in tasks.iter().enumerate() {
        let parent = t.parent_id.clone().filter(|p| ids.contains(p));
        children.entry(parent).or_default().push(i);
    }
    let mut order = vec![];
    let mut stack: Vec<(usize, usize)> = children
        .get(&None)
        .map(|v| v.iter().rev().map(|i| (*i, 0)).collect())
        .unwrap_or_default();
    while let Some((i, depth)) = stack.pop() {
        order.push((i, depth));
        if let Some(kids) = children.get(&Some(tasks[i].0.id.clone())) {
            stack.extend(kids.iter().rev().map(|k| (*k, depth + 1)));
        }
    }
    order
        .into_iter()
        .map(|(i, depth)| {
            let (t, start, planned_end) = &tasks[i];
            let active = matches!(t.status, Status::Running | Status::Waiting);
            Bar {
                id: t.id.clone(),
                title: t.title.clone(),
                actor: t.actor.clone(),
                model: t.model.clone(),
                kind: t.kind,
                status: t.status,
                depth,
                start: *start,
                end: if let Some(e) = planned_end {
                    Some(*e)
                } else if active {
                    None
                } else {
                    Some(t.finished_at.unwrap_or(*start).max(*start))
                },
                cost: t.cost_usd,
                tokens: t.tokens_in + t.tokens_out,
                planned: planned_end.is_some(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Look
// ---------------------------------------------------------------------------

/// Bar background, border and text colors for a task.
fn bar_colors(b: &Bar) -> (Hsla, Hsla, Hsla) {
    if b.planned {
        return (
            theme::alpha(theme::LINE_STRONG, 0.0),
            theme::LINE_STRONG,
            theme::TEXT_MUTED,
        );
    }
    match b.status {
        Status::Running if b.kind == TaskKind::Spawned => {
            (theme::SPAWN_BAR, theme::SPAWN, theme::ON_ACCENT)
        }
        Status::Running => (theme::ACCENT_STRONG, theme::ACCENT, theme::ON_ACCENT),
        Status::Waiting => (theme::WARNING_SOFT, theme::WARNING, theme::WARNING),
        Status::Failed => (theme::alpha(theme::DANGER, 0.6), theme::DANGER, theme::TEXT),
        Status::Cancelled | Status::Queued => {
            (theme::LINE_STRONG, theme::TEXT_DISABLED, theme::TEXT_MUTED)
        }
        Status::Done if b.kind == TaskKind::Spawned => (
            theme::alpha(theme::SPAWN_BAR, 0.45),
            theme::SPAWN_LINE,
            theme::TEXT_SECONDARY,
        ),
        Status::Done => (
            theme::alpha(theme::SUCCESS, 0.32),
            theme::alpha(theme::SUCCESS, 0.55),
            theme::TEXT_SECONDARY,
        ),
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Hover text of a bar.
/// `start`/`end` are the span as drawn (planned bars are shifted to stay after "now").
fn bar_tooltip(
    b: &Bar,
    now: DateTime<Utc>,
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
) -> SharedString {
    if b.planned {
        let who = [b.actor.as_str(), b.model.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        let mut lines = vec![b.title.clone()];
        if !who.is_empty() {
            lines.push(who);
        }
        let e = end.unwrap_or(start);
        lines.push(format!(
            "planned ~{} – {} (estimate)",
            start.with_timezone(&Local).format("%H:%M"),
            e.with_timezone(&Local).format("%H:%M")
        ));
        lines.push("Queued · not started".to_string());
        return lines.join("\n").into();
    }
    let s = start.with_timezone(&Local);
    let span = match end {
        Some(e) => format!(
            "{} – {}",
            s.format("%H:%M:%S"),
            e.with_timezone(&Local).format("%H:%M:%S")
        ),
        None => format!("{} – now", s.format("%H:%M:%S")),
    };
    let secs = (end.unwrap_or(now) - start).num_seconds();
    let who = [b.actor.as_str(), b.model.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ");
    let mut lines = vec![b.title.clone()];
    if !who.is_empty() {
        lines.push(who);
    }
    lines.push(format!("{span} ({})", ui::fmt_duration(secs)));
    lines.push(format!(
        "{} · {} · {} tokens",
        b.status.label(),
        ui::fmt_usd(b.cost),
        fmt_tokens(b.tokens)
    ));
    lines.join("\n").into()
}

fn stat_tile(
    label: &'static str,
    value: String,
    sub: Option<String>,
    color: Hsla,
    last: bool,
) -> Div {
    div()
        .flex_1()
        .min_w(px(150.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(theme::S6)
        .py(theme::S4)
        .when(!last, |d| d.border_r_1().border_color(theme::LINE))
        .child(div().text_label().fg_muted().child(label))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(theme::S2)
                .child(div().text_stat_sm().text_color(color).child(value))
                .when_some(sub, |d, s| d.child(div().text_label().fg_muted().child(s))),
        )
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

pub struct TimelineScreen {
    data: Option<DayData>,
    day: NaiveDate,
    zoom: usize,
    collapsed: HashSet<String>,
    daily_budget: f64,
    scroll: ScrollHandle,
    /// Scroll to "now" (today) or to the first bar once the lane has been laid out.
    pending_scroll: bool,
    load: Option<Task<()>>,
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
}

impl TimelineScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes.iter().any(|c| {
                    matches!(
                        c,
                        Change::Task { .. }
                            | Change::Run(_)
                            | Change::Chat(_)
                            | Change::Config
                            | Change::External
                    )
                }) {
                    this.reload(cx);
                }
            }
        });
        // Live bars and the "now" line: tick each second only while something runs on the
        // visible, current day; otherwise refresh every 30 seconds.
        let ticker = cx.spawn(async move |this, cx| {
            let mut n = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                n = n.wrapping_add(1);
                let alive = this.update(cx, |this, cx| {
                    let visible = state::route(cx) == Route::Timeline
                        && this.day == Local::now().date_naive();
                    let running = this.data.as_ref().is_some_and(|d| d.running > 0);
                    if visible && (running || n.is_multiple_of(30)) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut this = TimelineScreen {
            data: None,
            day: Local::now().date_naive(),
            zoom: 0,
            collapsed: HashSet::new(),
            daily_budget: 0.0,
            scroll: ScrollHandle::new(),
            pending_scroll: true,
            load: None,
            _ticker: ticker,
            _subs: vec![sub],
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.daily_budget = state::engine(cx).config().daily_budget_usd;
        let day = self.day;
        self.load = Some(load(
            cx,
            move |s| DayData::query(s, day),
            move |this, d, _| {
                if this.day == day {
                    this.data = Some(d);
                }
            },
        ));
    }

    fn set_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        if day == self.day {
            return;
        }
        self.day = day;
        self.data = None;
        self.pending_scroll = true;
        self.reload(cx);
        cx.notify();
    }

    fn shift_day(&mut self, delta: i64, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        let next = if delta < 0 {
            self.day - Days::new(1)
        } else {
            self.day + Days::new(1)
        };
        self.set_day(next.min(today), cx);
    }

    fn set_zoom(&mut self, zoom: usize, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.pending_scroll = true;
        cx.notify();
    }

    fn toggle(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_string());
        }
        cx.notify();
    }

    /// Applies a pending scroll once the lane has been laid out (needs its bounds).
    fn apply_scroll(&mut self, d: &DayData, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pending_scroll {
            return;
        }
        if self.scroll.bounds().size.width <= px(0.) {
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
            return;
        }
        let pxph = ZOOMS[self.zoom].1 as f64;
        let ds = day_start(self.day);
        let anchor_secs = if self.day == Local::now().date_naive() {
            (Utc::now() - ds).num_seconds() as f64
        } else {
            d.groups
                .iter()
                .filter_map(|g| g.bars.iter().map(|b| b.start).min())
                .min()
                .map(|s| (s - ds).num_seconds() as f64)
                .unwrap_or(8.0 * 3600.0)
        };
        let target = (anchor_secs / 3600.0 * pxph - 240.0).max(0.0) as f32;
        let max: f32 = self.scroll.max_offset().width.into();
        self.scroll.set_offset(point(px(-target.min(max)), px(0.)));
        self.pending_scroll = false;
    }

    // ----- pieces -----

    fn stats(&self, d: &DayData) -> impl IntoElement {
        let budget = self.daily_budget;
        let spend_sub = (budget > 0.0).then(|| format!("of {}", ui::fmt_usd(budget)));
        let spend_color = if budget > 0.0 && d.spend > budget {
            theme::WARNING
        } else {
            theme::TEXT
        };
        div()
            .flex()
            .flex_wrap()
            .border_b_1()
            .border_color(theme::LINE)
            .child(stat_tile(
                "Spend",
                ui::fmt_usd(d.spend),
                spend_sub,
                spend_color,
                false,
            ))
            .child(stat_tile(
                "Tasks",
                d.tasks.to_string(),
                Some(if d.queued > 0 {
                    format!("in {} rows · {} queued", d.groups.len(), d.queued)
                } else {
                    format!("in {} rows", d.groups.len())
                }),
                theme::TEXT,
                false,
            ))
            .child(stat_tile(
                "Running",
                d.running.to_string(),
                None,
                if d.running > 0 {
                    theme::INFO_TEXT
                } else {
                    theme::TEXT
                },
                false,
            ))
            .child(stat_tile(
                "Waiting for you",
                d.waiting.to_string(),
                None,
                if d.waiting > 0 {
                    theme::WARNING
                } else {
                    theme::TEXT
                },
                true,
            ))
    }

    /// Cumulative spend through the day, with the daily budget as a limit line.
    fn spend_chart(&self, d: &DayData) -> Div {
        let budget = self.daily_budget;
        let over: Vec<bool> = d
            .cumulative
            .iter()
            .map(|v| budget > 0.0 && *v > budget)
            .collect();
        let tips: Vec<SharedString> = d
            .cumulative
            .iter()
            .enumerate()
            .map(|(h, v)| SharedString::from(format!("by {:02}:00 · {}", h + 1, ui::fmt_usd(*v))))
            .collect();
        let meta = if budget > 0.0 {
            format!("{} of {}", ui::fmt_usd(d.spend), ui::fmt_usd(budget))
        } else {
            ui::fmt_usd(d.spend)
        };
        panel()
            .child(
                div()
                    .px(theme::S4)
                    .py(theme::S3)
                    .border_b_1()
                    .border_color(theme::LINE)
                    .child(
                        SectionHeader::new("Spend over the day")
                            .icon(AppIcon::Activity)
                            .meta(meta),
                    ),
            )
            .child(
                div().p(theme::S4).child(
                    BarChart::new("tl-spend-chart", d.cumulative.clone())
                        .height(px(72.))
                        .highlight(over)
                        .tooltips(tips)
                        .limit(
                            (budget > 0.0).then_some(budget),
                            format!("limit {}", ui::fmt_usd(budget)),
                        )
                        .x_labels("00:00", "12:00", "24:00")
                        .format(|v| {
                            if v < 10.0 {
                                format!("${v:.1}")
                            } else {
                                format!("${v:.0}")
                            }
                        }),
                ),
            )
    }

    /// The label column, the hour-tick header and every row.
    fn grid(&self, d: &DayData, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let (_, pxph, label_step, line_step) = ZOOMS[self.zoom];
        let width = pxph * 24.0;
        let ds = day_start(self.day);
        let now = Utc::now();
        let today = self.day == Local::now().date_naive();
        let de = day_start(self.day + Days::new(1));
        // Planned bars follow the clock between reloads so they stay after the now line.
        let shift = (now - d.queried_at).max(chrono::Duration::zero());
        // A bar's drawn span: planned bars are shifted, live bars end at now.
        let span_of = |b: &Bar| -> (DateTime<Utc>, Option<DateTime<Utc>>) {
            if b.planned {
                (
                    (b.start + shift).min(de),
                    b.end.map(|e| (e + shift).min(de)),
                )
            } else {
                (b.start, b.end)
            }
        };
        let x_of = |t: DateTime<Utc>| -> f32 {
            ((t - ds).num_milliseconds() as f64 / 1000.0).clamp(0.0, DAY_SECS) as f32 / 3600.0
                * pxph
        };

        // Left column header and lane header.
        let label_head = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(HEAD_PX))
            .px(theme::S3)
            .border_b_1()
            .border_color(theme::LINE)
            .text_mono_sm()
            .fg_muted()
            .child("Run · step")
            .child("cost");

        let mut lane_head = div()
            .relative()
            .h(px(HEAD_PX))
            .w(px(width))
            .border_b_1()
            .border_color(theme::LINE);
        let mut grid_lines = div().absolute().top_0().bottom_0().left_0().w(px(width));
        let mut m = 0i64;
        while m < 24 * 60 {
            let x = m as f32 / 60.0 * pxph;
            if m % label_step == 0 {
                let text = if label_step >= 60 && self.zoom == 0 {
                    format!("{:02}", m / 60)
                } else {
                    format!("{:02}:{:02}", m / 60, m % 60)
                };
                lane_head = lane_head.child(
                    div()
                        .absolute()
                        .left(px(x + 4.))
                        .top(px(8.))
                        .text_mono_sm()
                        .fg_muted()
                        .child(text),
                );
                lane_head = lane_head.child(
                    div()
                        .absolute()
                        .left(px(x))
                        .bottom_0()
                        .h(px(6.))
                        .w(px(1.))
                        .bg(theme::LINE_STRONG),
                );
            }
            if m % line_step == 0 {
                let strong = m % 60 == 0;
                grid_lines = grid_lines.child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(if strong {
                            theme::LINE
                        } else {
                            theme::LINE_SUBTLE
                        }),
                );
            }
            m += line_step.min(label_step);
        }
        if today {
            let x = x_of(now);
            lane_head = lane_head.child(
                div()
                    .absolute()
                    .left(px(x + 3.))
                    .top(px(8.))
                    .px(px(4.))
                    .rounded(theme::RADIUS_SM)
                    .bg(theme::ACCENT_SOFT)
                    .text_mono_sm()
                    .text_color(theme::ACCENT_TEXT)
                    .child(format!("now {}", now.with_timezone(&Local).format("%H:%M"))),
            );
        }

        let mut labels = div().flex().flex_col();
        let mut lanes = div().flex().flex_col().w(px(width));
        let mut tick_ix = 0usize;
        for g in &d.groups {
            let collapsed = self.collapsed.contains(&g.key);
            let key = g.key.clone();
            let start = g.bars.iter().map(|b| span_of(b).0).min();
            let end = g.bars.iter().map(|b| span_of(b).1.unwrap_or(now)).max();
            let tone = g
                .status
                .map(theme::status_color)
                .unwrap_or(theme::TEXT_MUTED);
            let nav = match &g.owner {
                Owner::Run(p) => Some(Route::Pipelines {
                    pipeline: Some(p.clone()),
                }),
                Owner::Chat(c) => Some(Route::Chat {
                    chat: Some(c.clone()),
                }),
                Owner::Other => None,
            };

            // Group row: collapse toggle, title, status, total cost.
            labels = labels.child(
                div()
                    .id(SharedString::from(format!("tl-group-{}", g.key)))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(ROW_PX))
                    .px(theme::S3)
                    .border_b_1()
                    .border_color(theme::LINE_SUBTLE)
                    .bg(theme::SURFACE_RAISED)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_HOVER))
                    .child(div().flex_none().fg_muted().child(ui::icon_sized(
                        if collapsed {
                            AppIcon::ChevronRight
                        } else {
                            AppIcon::ChevronDown
                        },
                        theme::ICON_SM,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_body_strong()
                            .truncate()
                            .child(g.title.clone()),
                    )
                    .when_some(g.status, |d, s| {
                        d.child(
                            div()
                                .text_label()
                                .text_color(theme::status_color(s))
                                .whitespace_nowrap()
                                .child(s.label()),
                        )
                    })
                    .child(
                        div()
                            .w(px(54.))
                            .flex_none()
                            .text_right()
                            .text_mono_sm()
                            .fg_secondary()
                            .child(ui::fmt_usd(g.cost)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&key, cx))),
            );
            let mut lane = div()
                .relative()
                .h(px(ROW_PX))
                .w_full()
                .border_b_1()
                .border_color(theme::LINE_SUBTLE)
                .bg(theme::SURFACE_RAISED);
            if let (Some(s), Some(e)) = (start, end) {
                let x0 = x_of(s);
                let w = (x_of(e) - x0).max(3.0);
                let id = SharedString::from(format!("tl-span-{}", g.key));
                lane = lane.child(
                    div()
                        .id(id)
                        .absolute()
                        .left(px(x0))
                        .top(px(ROW_PX / 2.0 - 3.0))
                        .w(px(w))
                        .h(px(4.))
                        .rounded(theme::RADIUS_FULL)
                        .bg(theme::alpha(tone, 0.4))
                        .when_some(nav, |d, r| {
                            d.cursor_pointer()
                                .on_click(move |_, _, cx| state::navigate(r.clone(), cx))
                        }),
                );
            }
            lanes = lanes.child(lane);
            if collapsed {
                continue;
            }

            for b in &g.bars {
                let i = tick_ix;
                tick_ix += 1;
                let id = b.id.clone();
                let task_id = b.id.clone();
                let spawned = b.kind == TaskKind::Spawned;
                let live = b.end.is_none();
                let (b_start, b_end) = span_of(b);
                let eff_end = b_end.unwrap_or(now).max(b_start);
                let secs = (eff_end - b_start).num_seconds();

                // Label cell.
                let sub = if live {
                    format!(
                        "{} · {}",
                        b.status.label().to_lowercase(),
                        ui::fmt_duration(secs)
                    )
                } else if b.planned {
                    "queued".to_string()
                } else {
                    String::new()
                };
                labels = labels.child(
                    div()
                        .id(("tl-label", i))
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .h(px(ROW_PX))
                        .pl(px(12.0 + b.depth as f32 * 18.0))
                        .pr(theme::S3)
                        .border_b_1()
                        .border_color(theme::LINE_SUBTLE)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::SURFACE_RAISED))
                        .child(StatusDot::new(b.status).id(("tl-dot", i)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_baseline()
                                .gap(theme::S2)
                                .child(
                                    div()
                                        .text_body()
                                        .text_color(if spawned {
                                            theme::SPAWN_TEXT
                                        } else {
                                            theme::TEXT
                                        })
                                        .truncate()
                                        .child(b.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(80.))
                                        .text_label()
                                        .fg_muted()
                                        .truncate()
                                        .child(if sub.is_empty() { b.actor.clone() } else { sub }),
                                ),
                        )
                        .child(
                            div()
                                .w(px(54.))
                                .flex_none()
                                .text_right()
                                .text_mono_sm()
                                .fg_muted()
                                .child(ui::fmt_usd(b.cost)),
                        )
                        .on_click({
                            let id = id.clone();
                            move |_, _, cx| state::open_task(id.clone(), cx)
                        }),
                );

                // Lane row with the bar.
                let (bg, border, fg) = bar_colors(b);
                let x0 = x_of(b_start);
                let w = (x_of(eff_end) - x0).max(4.0);
                let text = [b.title.as_str(), b.actor.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let text = format!("{text} · {}", ui::fmt_usd(b.cost));
                let tip = bar_tooltip(b, now, b_start, b_end);
                let waiting = b.status == Status::Waiting;
                let bar = div()
                    .id(("tl-bar", i))
                    .absolute()
                    .left(px(x0))
                    .top(px((ROW_PX - 22.0) / 2.0))
                    .w(px(w))
                    .h(px(22.))
                    .rounded(theme::RADIUS_SM)
                    .border_1()
                    .border_color(border)
                    .when(b.planned, |d| d.border_dashed())
                    .bg(bg)
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(move |_, _, cx| state::open_task(task_id.clone(), cx))
                    .when(live && b.status == Status::Running, |bar| {
                        let hi = theme::alpha(theme::TEXT, 0.22);
                        bar.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .w(relative(0.5))
                                .bg(linear_gradient(
                                    90.,
                                    linear_color_stop(theme::alpha(hi, 0.0), 0.),
                                    linear_color_stop(hi, 0.5),
                                ))
                                .with_animation(
                                    SharedString::from(format!("tl-shimmer-{}", b.id)),
                                    Animation::new(Duration::from_millis(1600))
                                        .repeat()
                                        .with_easing(ease_in_out),
                                    |d, t| d.left(relative(-0.5 + 1.5 * t)),
                                ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .px(px(6.))
                            .text_mono_sm()
                            .text_color(fg)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(text),
                    );
                let lane = div()
                    .relative()
                    .h(px(ROW_PX))
                    .w_full()
                    .border_b_1()
                    .border_color(theme::LINE_SUBTLE);
                lanes = lanes.child(if waiting {
                    lane.child(
                        bar.with_animation(
                            SharedString::from(format!("tl-pulse-{}", b.id)),
                            Animation::new(Duration::from_millis(1800))
                                .repeat()
                                .with_easing(pulsating_between(0.7, 1.0)),
                            |d, t| d.opacity(t),
                        ),
                    )
                } else {
                    lane.child(bar)
                });
            }
        }

        let mut rows_area = div().relative().w(px(width)).child(grid_lines).child(lanes);
        if today {
            let x = x_of(now);
            rows_area = rows_area.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(theme::ACCENT),
            );
        }

        let label_col = div()
            .flex_none()
            .w(px(LABEL_W))
            .border_r_1()
            .border_color(theme::LINE)
            .child(label_head)
            .child(labels);
        let lane_col = div()
            .id("tl-lane-scroll")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .child(div().w(px(width)).child(lane_head).child(rows_area));
        panel().child(div().flex().child(label_col).child(lane_col))
    }

    fn legend() -> Div {
        let item = |bg: Hsla, border: Hsla, dashed: bool, text: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .child(
                    div()
                        .w(px(18.))
                        .h(px(10.))
                        .rounded(theme::RADIUS_SM)
                        .border_1()
                        .when(dashed, |d| d.border_dashed())
                        .border_color(border)
                        .bg(bg),
                )
                .child(text)
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(theme::S4)
            .gap_y(theme::S2)
            .text_label()
            .fg_muted()
            .child(item(
                theme::alpha(theme::SUCCESS, 0.32),
                theme::alpha(theme::SUCCESS, 0.55),
                false,
                "done",
            ))
            .child(item(theme::ACCENT_STRONG, theme::ACCENT, false, "running"))
            .child(item(
                theme::WARNING_SOFT,
                theme::WARNING,
                false,
                "waiting for you",
            ))
            .child(item(
                theme::SPAWN_BAR,
                theme::SPAWN,
                false,
                "made by an agent",
            ))
            .child(item(
                theme::alpha(theme::DANGER, 0.6),
                theme::DANGER,
                false,
                "failed",
            ))
            .child(item(
                theme::alpha(theme::LINE_STRONG, 0.0),
                theme::LINE_STRONG,
                true,
                "planned",
            ))
            .child(
                div().child("Click a bar or row to open its thread. Click a run row to fold it."),
            )
    }
}

impl Render for TimelineScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let is_today = self.day == today;
        let label = if is_today {
            self.day.format("Today · %A, %b %-d").to_string()
        } else {
            self.day.format("%A, %b %-d").to_string()
        };
        let (running, waiting, queued) = self
            .data
            .as_ref()
            .map(|d| (d.running, d.waiting, d.queued))
            .unwrap_or((0, 0, 0));

        let header = PageHeader::new("Timeline")
            .subtitle(label)
            .when(running > 0, |h| {
                h.meta(Badge::new(format!("{running} running")).info().dot())
            })
            .when(waiting > 0, |h| {
                h.meta(
                    Badge::new(format!("{waiting} waiting for you"))
                        .warning()
                        .dot(),
                )
            })
            .when(queued > 0, |h| {
                h.meta(Badge::new(format!("{queued} queued")))
            })
            .action(
                IconButton::new("tl-prev", AppIcon::ChevronLeft)
                    .tooltip("Previous day")
                    .on_click(cx.listener(|this, _, _, cx| this.shift_day(-1, cx))),
            )
            .action(
                IconButton::new("tl-next", AppIcon::ChevronRight)
                    .tooltip("Next day")
                    .disabled(is_today)
                    .on_click(cx.listener(|this, _, _, cx| this.shift_day(1, cx))),
            )
            .action(
                Button::new("tl-today", "Today")
                    .secondary()
                    .disabled(is_today)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_day(Local::now().date_naive(), cx)),
                    ),
            )
            .when(is_today, |h| {
                h.action(
                    Button::new("tl-now", "Jump to now")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_scroll = true;
                            cx.notify();
                        })),
                )
            })
            .action({
                let this = cx.entity().downgrade();
                segmented(
                    "tl-zoom",
                    &ZOOMS.map(|z| z.0),
                    self.zoom,
                    move |ix, _, cx| {
                        let _ = this.update(cx, |t, cx| t.set_zoom(ix, cx));
                    },
                )
            });

        let empty_day = self.day.format("%A, %b %-d").to_string();
        let content = match self.data.take() {
            None => body()
                .child(skeleton_rows("timeline-loading", 4))
                .into_any_element(),
            Some(d) if d.groups.is_empty() => {
                let title = if is_today {
                    "Nothing ran today".to_string()
                } else {
                    format!("Nothing ran on {empty_day}")
                };
                let out = body()
                    .child(EmptyState::new(
                        AppIcon::Timeline,
                        title,
                        "Every step, chat task and sub-agent that runs shows up here as a bar, grouped by run.",
                    ))
                    .into_any_element();
                self.data = Some(d);
                out
            }
            Some(d) => {
                self.apply_scroll(&d, window, cx);
                let out = body()
                    .p_0()
                    .gap_0()
                    .child(self.stats(&d))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme::S4)
                            .p(theme::S6)
                            .child(self.spend_chart(&d))
                            .child(self.grid(&d, window, cx))
                            .child(Self::legend()),
                    )
                    .into_any_element();
                self.data = Some(d);
                out
            }
        };
        screen().child(header).child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_is_clamped_to_zero() {
        let t = Utc::now();
        let h = chrono::Duration::hours(1);
        assert_eq!(overlap_secs(t, t + h, t + h * 2, t + h * 3), 0.0);
        assert_eq!(overlap_secs(t, t + h * 2, t + h, t + h * 3), 3600.0);
    }

    #[test]
    fn plan_queue_is_back_to_back_from_now() {
        let now = Utc::now();
        let est = chrono::Duration::minutes(5);
        let end = now + chrono::Duration::hours(5);
        let slots = plan_queue(now, est, end, 3);
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0], (now, now + est));
        assert_eq!(slots[1], (now + est, now + est * 2));
        assert_eq!(slots[2], (now + est * 2, now + est * 3));
    }

    #[test]
    fn plan_queue_clamps_at_day_end() {
        let now = Utc::now();
        let est = chrono::Duration::minutes(10);
        let end = now + chrono::Duration::minutes(15);
        let slots = plan_queue(now, est, end, 4);
        assert_eq!(slots[0], (now, now + est));
        assert_eq!(slots[1], (now + est, end));
        assert_eq!(slots[2], (end, end));
        assert_eq!(slots[3], (end, end));
        assert!(plan_queue(now, est, end, 0).is_empty());
    }

    #[test]
    fn estimate_falls_back_and_clamps() {
        let m = chrono::Duration::minutes;
        assert_eq!(estimate_duration(&[]), m(5));
        assert_eq!(estimate_duration(&[chrono::Duration::seconds(10)]), m(2));
        assert_eq!(estimate_duration(&[m(120)]), m(30));
        assert_eq!(estimate_duration(&[m(4), m(8)]), m(6));
    }

    #[test]
    fn tokens_are_compact() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_300), "12.3k");
    }
}
