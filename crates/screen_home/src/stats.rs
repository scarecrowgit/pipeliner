//! Aggregation behind the dashboard: KPIs, spend per day and spend by agent, computed
//! from pipeline runs, chat sessions and Inbox reports. Pure functions, unit-tested.

use std::collections::HashMap;

use agent_tools::inbox::InboxItem;
use pipeline::{AgentUsage, Run, RunStatus, Stage, StageStatus, StepKind};
use ui::nav::{ChatSessionInfo, ChatStatus};

use crate::data::{Kind, day_start};

/// What Home needs of an issue's latest run (no log, no renders).
#[derive(Clone, Debug, PartialEq)]
pub struct RunSummary {
    pub key: String,
    pub title: String,
    pub status: RunStatus,
    /// A worker is running it now.
    pub active: bool,
    pub created: u64,
    pub stages: Vec<Stage>,
    pub cost_usd: f64,
    pub usage: Vec<AgentUsage>,
    pub pr_url: Option<String>,
    pub pipeline_name: String,
    /// Names of the sub-agents the run's steps lead (`Coder`), for grouping usage.
    pub delegates: Vec<String>,
    /// What a waiting approval step reviews: the name of the step it sends changes back
    /// to (`Mockup`), else its own name.
    pub review_of: Option<String>,
}

impl RunSummary {
    pub fn new(run: &Run, title: &str, active: bool) -> Self {
        Self {
            key: run.issue.clone(),
            title: title.to_string(),
            status: run.status,
            active,
            created: run.created,
            stages: run.stages.clone(),
            cost_usd: run.cost_usd,
            usage: run.usage.clone(),
            pr_url: run.pr_url.clone(),
            pipeline_name: run.pipeline_name.clone(),
            delegates: run
                .def
                .iter()
                .flat_map(|d| d.steps.iter())
                .filter_map(|s| s.delegate.as_ref().map(|d| d.name.clone()))
                .collect(),
            review_of: review_of(run),
        }
    }

    /// Agents are working on it.
    pub fn working(&self) -> bool {
        self.active || self.status == RunStatus::Running
    }

    /// An approval step waits for the user.
    pub fn waiting(&self) -> bool {
        !self.working() && self.status == RunStatus::WaitingApproval
    }

    /// Stage being worked on or waited for; else the next pending one.
    pub fn current_stage(&self) -> Option<&Stage> {
        self.stages
            .iter()
            .find(|s| matches!(s.status, StageStatus::Running | StageStatus::Waiting))
            .or_else(|| {
                self.stages
                    .iter()
                    .find(|s| s.status == StageStatus::Pending)
            })
    }

    /// Stages finished (done or skipped).
    pub fn stages_done(&self) -> usize {
        self.stages
            .iter()
            .filter(|s| matches!(s.status, StageStatus::Done | StageStatus::Skipped))
            .count()
    }

    /// Fraction of the pipeline's stages finished.
    pub fn progress(&self) -> f32 {
        self.stages_done() as f32 / self.stages.len().max(1) as f32
    }

    /// Last time a stage started or finished (the run's creation at least).
    fn last_change(&self) -> u64 {
        self.stages
            .iter()
            .flat_map(|s| [s.started, s.finished])
            .fold(self.created, u64::max)
    }

    /// Since when the current stage runs (or the run waits).
    pub fn since(&self) -> u64 {
        match self.current_stage() {
            Some(s) if s.started > 0 && s.status != StageStatus::Pending => s.started,
            _ => self.last_change(),
        }
    }

    /// When the pull request was opened, if there is one.
    pub fn shipped_at(&self) -> Option<u64> {
        self.pr_url.as_ref()?;
        let finished = self
            .stages
            .iter()
            .find(|s| s.step_kind() == Some(StepKind::PullRequest))
            .map_or(0, |s| s.finished);
        Some(if finished > 0 { finished } else { self.created })
    }

    pub fn finished(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
        ) && !self.active
    }
}

/// The step a waiting approval reviews (see [`RunSummary::review_of`]).
fn review_of(run: &Run) -> Option<String> {
    let stage = run.stages.iter().find(|s| s.status == StageStatus::Waiting)?;
    let back = run
        .def
        .as_ref()
        .and_then(|d| d.step(&stage.id))
        .and_then(|step| run.stage(&step.back_to));
    Some(back.unwrap_or(stage).name.clone())
}

/// Who works on a stage: the step itself for model steps, you for approvals, else the
/// pipeline.
pub fn stage_agent(stage: &Stage) -> String {
    match stage.step_kind() {
        Some(k) if k.uses_model() => stage.name.clone(),
        Some(StepKind::Approval) => "You".into(),
        _ => "Pipeline".into(),
    }
}

/// Group label of a pipeline agent: a numbered sub-agent (`Coder 2`, with `Coder` one of
/// `delegates`) → `Coders`; anything else as it is.
pub fn role_of(who: &str, delegates: &[String]) -> String {
    let sub = delegates.iter().find(|d| {
        who.strip_prefix(d.as_str())
            .is_some_and(|rest| rest.trim().chars().all(|c| c.is_ascii_digit()))
    });
    match sub {
        Some(d) => format!("{d}s"),
        None => who.to_string(),
    }
}

/// Inbox reports waiting for the user.
pub fn unread(inbox: &[InboxItem]) -> impl Iterator<Item = &InboxItem> {
    inbox.iter().filter(|i| !i.read && !i.archived)
}

pub fn chat_running(chat: &ChatSessionInfo) -> bool {
    chat.status == ChatStatus::Running
}

pub fn chat_waiting(chat: &ChatSessionInfo) -> bool {
    chat.status == ChatStatus::Waiting
}

// ── KPIs ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kpis {
    pub working_runs: usize,
    pub working_chats: usize,
    /// Runs waiting for approval + chats waiting for permission + unread reports.
    pub waiting: usize,
    /// When the oldest waiting item started waiting.
    pub oldest_wait: Option<u64>,
    /// Runs created in the period.
    pub runs: usize,
    /// …of which finished, and of those, done.
    pub finished: usize,
    pub done: usize,
    pub spend_runs: f64,
    pub spend_chats: f64,
    /// Pull requests opened in the period.
    pub shipped: usize,
}

impl Kpis {
    pub fn working(&self) -> usize {
        self.working_runs + self.working_chats
    }

    pub fn spend(&self) -> f64 {
        self.spend_runs + self.spend_chats
    }

    /// Share of finished runs that ended done, in percent.
    pub fn done_pct(&self) -> Option<u32> {
        (self.finished > 0).then(|| (self.done as f64 * 100. / self.finished as f64).round() as u32)
    }
}

/// KPIs for the period starting at `start` (unix seconds).
pub fn kpis(
    runs: &[RunSummary],
    chats: &[ChatSessionInfo],
    inbox: &[InboxItem],
    start: u64,
) -> Kpis {
    let mut k = Kpis {
        working_runs: runs.iter().filter(|r| r.working()).count(),
        working_chats: chats.iter().filter(|c| chat_running(c)).count(),
        ..Kpis::default()
    };
    let waits = runs
        .iter()
        .filter(|r| r.waiting())
        .map(RunSummary::since)
        .chain(chats.iter().filter(|c| chat_waiting(c)).map(|c| c.updated))
        .chain(unread(inbox).map(|i| i.created));
    for t in waits {
        k.waiting += 1;
        if t > 0 {
            k.oldest_wait = Some(k.oldest_wait.map_or(t, |o| o.min(t)));
        }
    }
    for r in runs.iter().filter(|r| r.created >= start) {
        k.runs += 1;
        k.spend_runs += r.cost_usd;
        if r.finished() {
            k.finished += 1;
            if r.status == RunStatus::Done {
                k.done += 1;
            }
        }
    }
    k.spend_chats = chats
        .iter()
        .filter(|c| c.updated >= start)
        .map(|c| c.cost_usd)
        .sum();
    k.shipped = runs
        .iter()
        .filter_map(RunSummary::shipped_at)
        .filter(|&t| t >= start)
        .count();
    k
}

// ── Spend per day ───────────────────────────────────────────────────────

pub const DAYS_SHOWN: usize = 7;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DaySpend {
    /// Local midnight, unix seconds.
    pub start: u64,
    pub runs: f64,
    pub chats: f64,
}

impl DaySpend {
    pub fn total(&self) -> f64 {
        self.runs + self.chats
    }
}

/// Spend of the last 7 local days, oldest first (the last is today): runs by the day they
/// were created, chats by the day of their last activity.
pub fn daily_spend(
    runs: &[RunSummary],
    chats: &[ChatSessionInfo],
    now: u64,
    offset: i64,
) -> [DaySpend; DAYS_SHOWN] {
    let mut days = [DaySpend::default(); DAYS_SHOWN];
    for (i, d) in days.iter_mut().enumerate() {
        d.start = day_start(now, offset, (DAYS_SHOWN - 1 - i) as i64);
    }
    let slot = |t: u64| days.iter().rposition(|d| t >= d.start);
    let run_slots: Vec<_> = runs.iter().map(|r| (slot(r.created), r.cost_usd)).collect();
    let chat_slots: Vec<_> = chats
        .iter()
        .map(|c| (slot(c.updated), c.cost_usd))
        .collect();
    for (i, cost) in run_slots {
        if let Some(i) = i {
            days[i].runs += cost;
        }
    }
    for (i, cost) in chat_slots {
        if let Some(i) = i {
            days[i].chats += cost;
        }
    }
    days
}

/// Round `v` up to 1, 2 or 5 × 10ⁿ for the top of a chart axis (`0` stays `0`).
pub fn nice_ceil(v: f64) -> f64 {
    if v <= 0. || !v.is_finite() {
        return 0.;
    }
    let mag = 10f64.powf(v.log10().floor());
    [1., 2., 5., 10.]
        .iter()
        .map(|m| m * mag)
        .find(|&step| step >= v * (1. - 1e-9))
        .unwrap_or(10. * mag)
}

// ── Spend by agent ──────────────────────────────────────────────────────

/// Rows shown before the rest is folded into "Others".
pub const AGENT_ROWS: usize = 7;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSpend {
    pub name: String,
    /// `None` for the folded "Others" row.
    pub kind: Option<Kind>,
    pub cost: f64,
}

/// Spend since `start` by pipeline agent (from each run's usage) and chat agent, most
/// expensive first; zero rows dropped, the tail folded into `Others · N`.
pub fn spend_by_agent(
    runs: &[RunSummary],
    chats: &[ChatSessionInfo],
    start: u64,
) -> Vec<AgentSpend> {
    let mut by: HashMap<(Kind, String), f64> = HashMap::new();
    for r in runs.iter().filter(|r| r.created >= start) {
        let mut counted = 0.;
        for u in &r.usage {
            *by.entry((Kind::Pipeline, role_of(&u.who, &r.delegates))).or_default() += u.cost_usd;
            counted += u.cost_usd;
        }
        // Cost not broken down by agent (older runs).
        let rest = r.cost_usd - counted;
        if rest > 0.005 {
            *by.entry((Kind::Pipeline, "Pipeline".into())).or_default() += rest;
        }
    }
    for c in chats.iter().filter(|c| c.updated >= start) {
        *by.entry((Kind::Chat, c.agent.to_string())).or_default() += c.cost_usd;
    }
    let mut rows: Vec<AgentSpend> = by
        .into_iter()
        .filter(|(_, cost)| *cost >= 0.005)
        .map(|((kind, name), cost)| AgentSpend {
            name,
            kind: Some(kind),
            cost,
        })
        .collect();
    rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.name.cmp(&b.name)));
    if rows.len() > AGENT_ROWS {
        let tail = rows.split_off(AGENT_ROWS - 1);
        rows.push(AgentSpend {
            name: format!("Others · {}", tail.len()),
            kind: None,
            cost: tail.iter().map(|r| r.cost).sum(),
        });
    }
    rows
}
