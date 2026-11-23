//! Small shared types of the Home dashboard: the period switch, row kinds and link targets.

use gpui::{Hsla, SharedString};
use ui::theme::*;

/// What kind of worker a "Working now" row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A pipeline run on an issue.
    Pipeline,
    /// A chat session (Build, Plan or a custom agent).
    Chat,
}

pub struct KindLooks {
    pub icon: &'static str,
    pub label: &'static str,
    pub bg: Hsla,
    pub fg: Hsla,
}

pub fn looks(kind: Kind) -> KindLooks {
    match kind {
        Kind::Pipeline => KindLooks {
            icon: "pipelines",
            label: "Pipeline",
            bg: INFO_SOFT,
            fg: INFO_TEXT,
        },
        Kind::Chat => KindLooks {
            icon: "message",
            label: "Chat",
            bg: SURFACE_HOVER,
            fg: TEXT_SECONDARY,
        },
    }
}

/// Legend of the "Working now" card: kind, singular and plural label.
pub const LEGEND: [(Kind, &str, &str); 2] = [
    (Kind::Pipeline, "pipeline", "pipelines"),
    (Kind::Chat, "chat", "chats"),
];

/// `1 chat`, `2 chats`.
pub fn count_label(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Period shown by the KPI strip and "Spend by agent".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Range {
    #[default]
    Today,
    Week,
    Month,
    Quarter,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Today, Range::Week, Range::Month, Range::Quarter];

    /// Segmented-control label.
    pub fn label(self) -> &'static str {
        match self {
            Range::Today => "Today",
            Range::Week => "7 days",
            Range::Month => "30 days",
            Range::Quarter => "90 days",
        }
    }

    /// Lowercase label used in sentences ("Spend by agent · last 7 days").
    pub fn sentence(self) -> &'static str {
        match self {
            Range::Today => "today",
            Range::Week => "last 7 days",
            Range::Month => "last 30 days",
            Range::Quarter => "last 90 days",
        }
    }

    /// Local calendar days covered, today included.
    pub fn days(self) -> i64 {
        match self {
            Range::Today => 1,
            Range::Week => 7,
            Range::Month => 30,
            Range::Quarter => 90,
        }
    }

    /// First second of the period: local midnight `days() - 1` days before today.
    pub fn start(self, now: u64, offset: i64) -> u64 {
        day_start(now, offset, self.days() - 1)
    }
}

/// Unix time of the local midnight `back` days before the day of `now`.
pub fn day_start(now: u64, offset: i64, back: i64) -> u64 {
    let today = ui::time::date_of(now, offset).days;
    ((today - back) * 86_400 - offset).max(0) as u64
}

/// Where a click on a row leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// The issue board with this issue open.
    Issue(SharedString),
    /// A chat session (by id).
    Chat(SharedString),
    /// An Inbox report (by id).
    Inbox(SharedString),
}

/// `$1.23`.
pub fn money(v: f64) -> String {
    format!("${v:.2}")
}
