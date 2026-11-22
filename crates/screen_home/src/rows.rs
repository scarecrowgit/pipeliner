//! Rows of "Working now" and "Needs you": the mockup's mock rows with the chat stand-ins
//! replaced by real chat sessions published by the Chat screen (`ui::nav`).

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::SharedString;
use ui::nav::{ChatSessionInfo, ChatStatus};

use crate::data::{self, Kind, MockState, NEEDS, Target};

#[derive(Clone, Debug, PartialEq)]
pub enum RowState {
    /// Running; progress in percent when known (real chats have none).
    Running(Option<u8>),
    /// Waiting for the user; text says for what.
    Waiting(SharedString),
}

#[derive(Clone, Debug)]
pub struct LiveRow {
    pub kind: Kind,
    pub name: SharedString,
    /// Kind label, or "created by <agent>".
    pub sub: SharedString,
    pub what: SharedString,
    pub reference: SharedString,
    pub runtime: SharedString,
    pub elapsed: SharedString,
    pub cost: SharedString,
    pub state: RowState,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub struct NeedRow {
    pub title: SharedString,
    pub who: SharedString,
    pub reference: SharedString,
    pub age: SharedString,
    pub action: SharedString,
    pub target: Target,
}

/// Chats worth showing on Home: running or waiting for the user.
pub fn active_chats(sessions: &[ChatSessionInfo]) -> Vec<ChatSessionInfo> {
    sessions
        .iter()
        .filter(|s| matches!(s.status, ChatStatus::Running | ChatStatus::Waiting))
        .cloned()
        .collect()
}

/// "Working now" rows. With no real chats (e.g. in the standalone preview) the mock
/// "Build · Chat" row stays; otherwise it is replaced by one row per active chat.
pub fn live_rows(chats: &[ChatSessionInfo], host: &SharedString, now: u64) -> Vec<LiveRow> {
    let mut rows = Vec::new();
    for mock in data::live() {
        if mock.chat_slot && !chats.is_empty() {
            rows.extend(chats.iter().map(|chat| chat_row(chat, host, now)));
            continue;
        }
        let kind_label = data::looks(mock.kind).label;
        rows.push(LiveRow {
            kind: mock.kind,
            name: mock.name.into(),
            sub: match mock.by {
                Some(by) => format!("created by {by}").into(),
                None => kind_label.into(),
            },
            what: mock.what.into(),
            reference: mock.reference.into(),
            runtime: mock.runtime.into(),
            elapsed: mock.elapsed.into(),
            cost: mock.cost.into(),
            state: match mock.state {
                MockState::Running(pct) => RowState::Running(Some(pct)),
                MockState::Waiting(text) => RowState::Waiting(text.into()),
            },
            target: mock.target,
        });
    }
    rows
}

fn chat_row(chat: &ChatSessionInfo, host: &SharedString, now: u64) -> LiveRow {
    LiveRow {
        kind: Kind::Agent,
        name: chat.agent.clone(),
        sub: data::looks(Kind::Agent).label.into(),
        what: format!("Chat · {}", chat.title).into(),
        reference: chat.workdir.clone(),
        runtime: host.clone(),
        elapsed: age(chat.updated, now),
        cost: format!("${:.2}", chat.cost_usd).into(),
        state: if chat.status == ChatStatus::Waiting {
            RowState::Waiting("asks for permission".into())
        } else {
            RowState::Running(None)
        },
        target: Target::Chat(chat.id.clone()),
    }
}

/// "Needs you" rows: the mock permission request is replaced by the real waiting chats.
pub fn need_rows(chats: &[ChatSessionInfo], now: u64) -> Vec<NeedRow> {
    let mut rows = Vec::new();
    for mock in &NEEDS {
        if mock.chat_slot && !chats.is_empty() {
            rows.extend(
                chats
                    .iter()
                    .filter(|c| c.status == ChatStatus::Waiting)
                    .map(|chat| NeedRow {
                        title: format!("Answer {}", chat.title).into(),
                        who: format!("{} in chat", chat.agent).into(),
                        reference: chat.workdir.clone(),
                        age: age(chat.updated, now),
                        action: "Answer".into(),
                        target: Target::Chat(chat.id.clone()),
                    }),
            );
            continue;
        }
        rows.push(NeedRow {
            title: mock.title.into(),
            who: mock.who.into(),
            reference: mock.reference.into(),
            age: mock.age.into(),
            action: mock.action.into(),
            target: Target::Screen(mock.target),
        });
    }
    rows
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compact age like the mockup's `12m` / `3h` / `2d`.
pub fn age(updated: u64, now: u64) -> SharedString {
    if updated == 0 {
        return "—".into();
    }
    let secs = now.saturating_sub(updated);
    let text = match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    };
    text.into()
}

/// This machine's name for the "runtime" column of real chats.
pub fn hostname() -> SharedString {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "local".to_string())
        .into()
}
