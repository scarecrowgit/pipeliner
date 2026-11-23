//! Rows of "Working now" and "Needs you", built from pipeline runs, chat sessions
//! (published by the Chat screen through `ui::nav`) and unread Inbox reports.

use agent_tools::inbox::{self, InboxItem};
use gpui::SharedString;
use ui::nav::ChatSessionInfo;

use crate::data::{Kind, Target, looks, money};
use crate::stats::{RunSummary, chat_running, chat_waiting, stage_agent, unread};

#[derive(Clone, Debug, PartialEq)]
pub enum RowState {
    /// Running; text says what (stage), progress as a fraction when known.
    Running(SharedString, Option<f32>),
    /// Waiting for the user; text says for what.
    Waiting(SharedString),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveRow {
    pub kind: Kind,
    pub name: SharedString,
    pub sub: SharedString,
    pub what: SharedString,
    pub reference: SharedString,
    pub runtime: SharedString,
    pub elapsed: SharedString,
    pub cost: SharedString,
    pub state: RowState,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NeedRow {
    pub title: SharedString,
    pub who: SharedString,
    pub reference: SharedString,
    /// When it started waiting (unix seconds), for sorting and the age label.
    pub since: u64,
    pub age: SharedString,
    pub action: SharedString,
    pub target: Target,
}

/// "Working now": runs with a worker or waiting for approval, then running / waiting chats.
pub fn live_rows(
    runs: &[RunSummary],
    chats: &[ChatSessionInfo],
    host: &SharedString,
    now: u64,
) -> Vec<LiveRow> {
    let mut rows: Vec<LiveRow> = runs
        .iter()
        .filter(|r| r.working() || r.waiting())
        .map(|r| run_row(r, host, now))
        .collect();
    rows.extend(
        chats
            .iter()
            .filter(|c| chat_running(c) || chat_waiting(c))
            .map(|c| chat_row(c, host, now)),
    );
    rows
}

fn run_row(run: &RunSummary, host: &SharedString, now: u64) -> LiveRow {
    let stage = run.current_stage();
    let stage_label = stage.map_or("Starting", |s| s.name.as_str());
    let agent = stage.map_or_else(|| "Pipeline".to_string(), stage_agent);
    let state = if run.waiting() {
        RowState::Waiting("awaits your approval".into())
    } else {
        RowState::Running(
            format!("{stage_label} · {}/{}", run.stages_done(), run.stages.len()).into(),
            Some(run.progress()),
        )
    };
    LiveRow {
        kind: Kind::Pipeline,
        name: agent.into(),
        sub: format!("{} · {stage_label}", looks(Kind::Pipeline).label).into(),
        what: run.title.clone().into(),
        reference: run.key.clone().into(),
        runtime: host.clone(),
        elapsed: age(run.since(), now),
        cost: money(run.cost_usd).into(),
        state,
        target: Target::Issue(run.key.clone().into()),
    }
}

fn chat_row(chat: &ChatSessionInfo, host: &SharedString, now: u64) -> LiveRow {
    LiveRow {
        kind: Kind::Chat,
        name: chat.agent.clone(),
        sub: looks(Kind::Chat).label.into(),
        what: chat.title.clone(),
        reference: chat.workdir.clone(),
        runtime: host.clone(),
        elapsed: age(chat.updated, now),
        cost: money(chat.cost_usd).into(),
        state: if chat_waiting(chat) {
            RowState::Waiting("asks for permission".into())
        } else {
            RowState::Running("running".into(), None)
        },
        target: Target::Chat(chat.id.clone()),
    }
}

/// "Needs you": approval steps waiting, chats asking for permission and unread reports, each
/// group oldest first.
pub fn need_rows(
    runs: &[RunSummary],
    chats: &[ChatSessionInfo],
    inbox: &[InboxItem],
    now: u64,
) -> Vec<NeedRow> {
    let mut approvals: Vec<NeedRow> = runs
        .iter()
        .filter(|r| r.waiting())
        .map(|r| NeedRow {
            title: match &r.review_of {
                Some(what) => format!("Review {what} for {}", r.key),
                None => format!("Review {}", r.key),
            }
            .into(),
            who: if r.pipeline_name.is_empty() {
                "Pipeline".into()
            } else {
                format!("Pipeline · {}", r.pipeline_name).into()
            },
            reference: r.key.clone().into(),
            since: r.since(),
            age: age(r.since(), now),
            action: "Review".into(),
            target: Target::Issue(r.key.clone().into()),
        })
        .collect();
    let mut answers: Vec<NeedRow> = chats
        .iter()
        .filter(|c| chat_waiting(c))
        .map(|c| NeedRow {
            title: format!("Answer {}", c.title).into(),
            who: format!("{} in chat", c.agent).into(),
            reference: c.workdir.clone(),
            since: c.updated,
            age: age(c.updated, now),
            action: "Answer".into(),
            target: Target::Chat(c.id.clone()),
        })
        .collect();
    let mut reports: Vec<NeedRow> = unread(inbox)
        .map(|i| NeedRow {
            title: i.title.clone().into(),
            who: format!("{} · {}", i.agent, i.outcome.label()).into(),
            reference: report_reference(i).into(),
            since: i.created,
            age: age(i.created, now),
            action: "Review".into(),
            target: Target::Inbox(i.id.clone().into()),
        })
        .collect();
    for group in [&mut approvals, &mut answers, &mut reports] {
        group.sort_by_key(|r| r.since);
    }
    approvals.extend(answers);
    approvals.extend(reports);
    approvals
}

/// Short reference of a report: its first pull request (`owner/repo#12`), else the
/// repository part of its folder label.
fn report_reference(item: &InboxItem) -> String {
    match item.prs().next() {
        Some(pr) => inbox::short_label(&pr.url, pr.kind),
        None => item
            .wd_label
            .split(" · ")
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Compact age like `12m` / `3h` / `2d`.
pub fn age(since: u64, now: u64) -> SharedString {
    if since == 0 {
        return "—".into();
    }
    let secs = now.saturating_sub(since);
    let text = match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    };
    text.into()
}

/// This machine's name, the runtime of every pipeline worker and chat.
pub fn hostname() -> SharedString {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "local".to_string())
        .into()
}
