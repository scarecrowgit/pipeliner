//! Mock data of the Home dashboard, 1:1 with the mockup's `LIVE`, `NEEDS`, `DAYS`, `BY`
//! and `KPI` tables (`reference/home_dashboard.html`).

use gpui::Hsla;
use ui::nav::Screen;
use ui::theme::*;

/// What kind of worker a "Working now" row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Orchestrator,
    Agent,
    Pipeline,
    /// Created by another agent (dashed spawn tile).
    Made,
}

pub struct KindLooks {
    pub icon: &'static str,
    pub label: &'static str,
    pub bg: Hsla,
    pub fg: Hsla,
}

pub fn looks(kind: Kind) -> KindLooks {
    match kind {
        Kind::Orchestrator => KindLooks {
            icon: "router",
            label: "Orchestrator",
            bg: ACCENT_SOFT,
            fg: ACCENT_TEXT,
        },
        Kind::Agent => KindLooks {
            icon: "agent",
            label: "Agent",
            bg: SURFACE_HOVER,
            fg: TEXT_SECONDARY,
        },
        Kind::Pipeline => KindLooks {
            icon: "pipelines",
            label: "Pipeline",
            bg: INFO_SOFT,
            fg: INFO_TEXT,
        },
        Kind::Made => KindLooks {
            icon: "spawn",
            label: "Created by an agent",
            bg: SPAWN_SOFT,
            fg: SPAWN_TEXT,
        },
    }
}

/// Legend of the "Working now" card: kind and its plural label.
pub const LEGEND: [(Kind, &str); 4] = [
    (Kind::Orchestrator, "orchestrator"),
    (Kind::Agent, "agents"),
    (Kind::Pipeline, "pipeline"),
    (Kind::Made, "created by agents"),
];

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

    pub fn kpis(self) -> &'static [Kpi; 5] {
        match self {
            Range::Today => &KPI_TODAY,
            Range::Week => &KPI_WEEK,
            Range::Month => &KPI_MONTH,
            Range::Quarter => &KPI_QUARTER,
        }
    }

    pub fn by_agent(self) -> &'static [(&'static str, Kind, f32); 7] {
        match self {
            Range::Today => &BY_TODAY,
            Range::Week => &BY_WEEK,
            Range::Month => &BY_MONTH,
            Range::Quarter => &BY_QUARTER,
        }
    }
}

/// Where a click on a row leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Agents screen with this entry selected.
    Agent(&'static str),
    Screen(Screen),
    /// A real chat session (by id).
    Chat(gpui::SharedString),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockState {
    /// Running, with progress in percent.
    Running(u8),
    /// Waiting for the user; text says for what.
    Waiting(&'static str),
}

pub struct LiveMock {
    pub kind: Kind,
    pub name: &'static str,
    /// For `Kind::Made`: the agent that created it.
    pub by: Option<&'static str>,
    pub what: &'static str,
    pub reference: &'static str,
    pub runtime: &'static str,
    pub elapsed: &'static str,
    pub cost: &'static str,
    pub state: MockState,
    pub target: Target,
    /// Stand-in for real chat sessions: replaced by them when the Chat screen publishes any.
    pub chat_slot: bool,
}

pub fn live() -> [LiveMock; 6] {
    use Kind::*;
    use MockState::*;
    let row = |kind, name, what, reference, elapsed, cost, state, target| LiveMock {
        kind,
        name,
        by: None,
        what,
        reference,
        runtime: "cloud",
        elapsed,
        cost,
        state,
        target,
        chat_slot: false,
    };
    [
        row(
            Orchestrator,
            "Payments lead",
            "Retry budget per merchant",
            "PAY-1436",
            "31m",
            "$0.44",
            Running(40),
            Target::Agent("Payments lead"),
        ),
        row(
            Pipeline,
            "PAY triage",
            "Refund webhook times out · Reproducer",
            "PAY-1427",
            "20m",
            "$0.12",
            Running(64),
            Target::Screen(Screen::PipelineEditor),
        ),
        LiveMock {
            by: Some("Payments lead"),
            ..row(
                Made,
                "Migration checker",
                "Read SDK v9 diff for Codex",
                "PAY-1438",
                "9m",
                "$0.08",
                Running(50),
                Target::Agent("Migration checker"),
            )
        },
        LiveMock {
            by: Some("Reproducer"),
            ..row(
                Made,
                "Log digger",
                "Search 24h of logs",
                "PAY-1427",
                "6m",
                "$0.02",
                Running(70),
                Target::Screen(Screen::PipelineEditor),
            )
        },
        LiveMock {
            runtime: "mac-mini",
            chat_slot: true,
            ..row(
                Agent,
                "Build",
                "Chat · Fix double charge on retry",
                "fix/PAY-1423",
                "12m",
                "$0.21",
                Waiting("asks to run bun test"),
                Target::Screen(Screen::Chat),
            )
        },
        row(
            Agent,
            "Auditor",
            "Audit PCI log redaction",
            "PAY-1435",
            "40m",
            "$0.15",
            Waiting("asks which sinks"),
            Target::Screen(Screen::Issues),
        ),
    ]
}

pub struct NeedMock {
    pub title: &'static str,
    pub who: &'static str,
    pub reference: &'static str,
    pub age: &'static str,
    pub action: &'static str,
    pub target: Screen,
    /// Stand-in for real waiting chats (see [`LiveMock::chat_slot`]).
    pub chat_slot: bool,
}

pub const NEEDS: [NeedMock; 4] = [
    NeedMock {
        title: "Allow bun test src/charges",
        who: "Build in chat",
        reference: "fix/PAY-1423",
        age: "12m",
        action: "Answer",
        target: Screen::Chat,
        chat_slot: true,
    },
    NeedMock {
        title: "Pick 1 of 3 variants",
        who: "PAY triage · Fixer ×3",
        reference: "PAY-1423",
        age: "40m",
        action: "Review",
        target: Screen::Inbox,
        chat_slot: false,
    },
    NeedMock {
        title: "Which log sinks are in scope?",
        who: "Auditor",
        reference: "PAY-1435",
        age: "40m",
        action: "Reply",
        target: Screen::Issues,
        chat_slot: false,
    },
    NeedMock {
        title: "Pick 1 of 2 specs",
        who: "PAY triage · Analyst ×2",
        reference: "PAY-1431",
        age: "3h",
        action: "Review",
        target: Screen::Inbox,
        chat_slot: false,
    },
];

/// Spend per day, last 7 days; the last one is today (so far).
pub const DAYS: [(&str, f32); 7] = [
    ("Mon", 5.2),
    ("Tue", 7.9),
    ("Wed", 6.4),
    ("Thu", 8.8),
    ("Fri", 4.1),
    ("Sat", 2.3),
    ("Sun", 3.81),
];

/// Daily spend limit (top of the chart).
pub const DAY_LIMIT: f32 = 10.0;

const BY_TODAY: [(&str, Kind, f32); 7] = [
    ("Payments lead", Kind::Orchestrator, 1.12),
    ("PAY triage", Kind::Pipeline, 0.94),
    ("Build", Kind::Agent, 0.62),
    ("Fixer", Kind::Agent, 0.41),
    ("Codex", Kind::Agent, 0.18),
    ("Auditor", Kind::Agent, 0.15),
    ("Others · 6", Kind::Agent, 0.39),
];
const BY_WEEK: [(&str, Kind, f32); 7] = [
    ("Payments lead", Kind::Orchestrator, 9.4),
    ("PAY triage", Kind::Pipeline, 8.1),
    ("Fixer", Kind::Agent, 6.2),
    ("Build", Kind::Agent, 5.3),
    ("Codex", Kind::Agent, 3.1),
    ("Nightly audit", Kind::Pipeline, 2.2),
    ("Others · 8", Kind::Agent, 4.21),
];
const BY_MONTH: [(&str, Kind, f32); 7] = [
    ("PAY triage", Kind::Pipeline, 41.2),
    ("Payments lead", Kind::Orchestrator, 33.8),
    ("Fixer", Kind::Agent, 27.5),
    ("Build", Kind::Agent, 22.9),
    ("Nightly audit", Kind::Pipeline, 9.6),
    ("Codex", Kind::Agent, 8.4),
    ("Others · 11", Kind::Agent, 17.3),
];
const BY_QUARTER: [(&str, Kind, f32); 7] = [
    ("Payments lead", Kind::Orchestrator, 184.2),
    ("PAY triage", Kind::Pipeline, 151.6),
    ("Build", Kind::Agent, 102.3),
    ("Fixer", Kind::Agent, 71.9),
    ("Codex", Kind::Agent, 29.4),
    ("Auditor", Kind::Agent, 24.1),
    ("Others · 6", Kind::Agent, 48.9),
];

/// Which KPI a value belongs to; the first two are computed from the rows shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KpiKind {
    WorkingNow,
    WaitingForYou,
    Other,
}

pub struct Kpi {
    pub kind: KpiKind,
    pub label: &'static str,
    pub value: &'static str,
    pub sub: &'static str,
}

const fn kpi(kind: KpiKind, label: &'static str, value: &'static str, sub: &'static str) -> Kpi {
    Kpi {
        kind,
        label,
        value,
        sub,
    }
}

const KPI_TODAY: [Kpi; 5] = [
    kpi(KpiKind::WorkingNow, "Working now", "6", "of 12 slots"),
    kpi(KpiKind::WaitingForYou, "Waiting for you", "4", "oldest 3h"),
    kpi(KpiKind::Other, "Runs", "61", "92% finished"),
    kpi(KpiKind::Other, "Spend", "$3.81", "of $10.00"),
    kpi(KpiKind::Other, "Shipped", "7", "PRs and specs"),
];
const KPI_WEEK: [Kpi; 5] = [
    kpi(KpiKind::WorkingNow, "Working now", "6", "of 12 slots"),
    kpi(KpiKind::WaitingForYou, "Waiting for you", "4", "oldest 3h"),
    kpi(KpiKind::Other, "Runs", "412", "89% finished"),
    kpi(KpiKind::Other, "Spend", "$38.51", "of $70.00"),
    kpi(KpiKind::Other, "Shipped", "43", "PRs and specs"),
];
const KPI_MONTH: [Kpi; 5] = [
    kpi(KpiKind::WorkingNow, "Working now", "6", "of 12 slots"),
    kpi(KpiKind::WaitingForYou, "Waiting for you", "4", "oldest 3h"),
    kpi(KpiKind::Other, "Runs", "1,730", "90% finished"),
    kpi(KpiKind::Other, "Spend", "$160.70", "of $300.00"),
    kpi(KpiKind::Other, "Shipped", "171", "PRs and specs"),
];
const KPI_QUARTER: [Kpi; 5] = [
    kpi(KpiKind::WorkingNow, "Working now", "6", "of 12 slots"),
    kpi(KpiKind::WaitingForYou, "Waiting for you", "4", "oldest 3h"),
    kpi(KpiKind::Other, "Runs", "4,212", "89% finished"),
    kpi(KpiKind::Other, "Spend", "$612.40", "of $900.00"),
    kpi(KpiKind::Other, "Shipped", "418", "PRs and specs"),
];

pub struct Runtime {
    pub name: &'static str,
    pub online: bool,
    pub usage: &'static str,
    pub used: u32,
    pub max: u32,
    pub note: &'static str,
}

pub const RUNTIMES: [Runtime; 3] = [
    Runtime {
        name: "mac-mini",
        online: true,
        usage: "3 of 4 slots",
        used: 3,
        max: 4,
        note: "Build, Fixer test runs · 61% CPU",
    },
    Runtime {
        name: "cloud",
        online: true,
        usage: "5 of 8 sandboxes",
        used: 5,
        max: 8,
        note: "$0.02 per sandbox-hour",
    },
    Runtime {
        name: "laptop",
        online: false,
        usage: "offline 2h",
        used: 0,
        max: 2,
        note: "Codex queue waits for it · 1 run",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn spend_cents(range: Range) -> i64 {
        let spend = range
            .kpis()
            .iter()
            .find(|k| k.label == "Spend")
            .expect("Spend KPI");
        let dollars: f64 = spend
            .value
            .trim_start_matches('$')
            .replace(',', "")
            .parse()
            .unwrap();
        (dollars * 100.0).round() as i64
    }

    #[test]
    fn by_agent_sums_to_spend_kpi() {
        for range in Range::ALL {
            let sum: f32 = range.by_agent().iter().map(|row| row.2).sum();
            assert_eq!(
                (f64::from(sum) * 100.0).round() as i64,
                spend_cents(range),
                "{range:?}"
            );
        }
    }

    #[test]
    fn quarter_first_row_is_max() {
        let rows = Range::Quarter.by_agent();
        assert!(rows.iter().all(|row| row.2 <= rows[0].2));
    }
}
