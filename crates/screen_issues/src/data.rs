//! Mock data and domain types for the issue board (port of the mockup's `seed()`,
//! `COLS`, `RUNNERS`, `RT`, `RUN`, `PRIO` and `PEOPLE` tables).

use gpui::{Hsla, SharedString};
use ui::Status;
use ui::images::ImageAttachment;
use ui::theme::*;

/// Border of warning boxes (`#4a3f17` in the mockup; not a Graphite token).
pub const WARNING_LINE: Hsla = hex(0x4a3f17);

// ── Columns ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Backlog,
    Todo,
    Progress,
    Review,
    Done,
}

impl Column {
    pub const ALL: [Column; 5] = [
        Column::Backlog,
        Column::Todo,
        Column::Progress,
        Column::Review,
        Column::Done,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Column::Backlog => "Backlog",
            Column::Todo => "Todo",
            Column::Progress => "In progress",
            Column::Review => "In review",
            Column::Done => "Done",
        }
    }

    pub fn dot(self) -> Status {
        match self {
            Column::Backlog | Column::Todo => Status::Draft,
            Column::Progress => Status::Running,
            Column::Review => Status::Waiting,
            Column::Done => Status::Live,
        }
    }
}

// ── Priority ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            Priority::High => WARNING,
            Priority::Medium => TEXT_SECONDARY,
            Priority::Low => TEXT_MUTED,
        }
    }
}

// ── People ──────────────────────────────────────────────────────────────

pub fn person_name(initials: &str) -> Option<&'static str> {
    match initials {
        "AR" => Some("Alex Rivera"),
        "NK" => Some("Nina Kim"),
        "ME" => Some("You"),
        _ => None,
    }
}

// ── Runners (agents, orchestrators, pipelines) ──────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerKind {
    Pipeline,
    Agent,
    Orch,
}

impl RunnerKind {
    pub fn label(self) -> &'static str {
        match self {
            RunnerKind::Pipeline => "Pipeline",
            RunnerKind::Agent => "Agent",
            RunnerKind::Orch => "Orchestrator",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            RunnerKind::Pipeline => "pipelines",
            RunnerKind::Agent => "agent",
            RunnerKind::Orch => "router",
        }
    }

    /// Icon tile tone (bg/fg pair in `ui::icon_tile`).
    pub fn tone(self) -> ui::Tone {
        match self {
            RunnerKind::Pipeline => ui::Tone::Info,
            RunnerKind::Agent => ui::Tone::Neutral,
            RunnerKind::Orch => ui::Tone::Accent,
        }
    }

    /// Tile background / foreground.
    pub fn tile_colors(self) -> (Hsla, Hsla) {
        match self {
            RunnerKind::Pipeline => (INFO_SOFT, INFO_TEXT),
            RunnerKind::Agent => (SURFACE_HOVER, TEXT_SECONDARY),
            RunnerKind::Orch => (ACCENT_SOFT, ACCENT_TEXT),
        }
    }
}

pub struct Runner {
    pub id: &'static str,
    pub kind: RunnerKind,
    pub name: &'static str,
    pub desc: &'static str,
    pub load: &'static str,
    pub steps: &'static [&'static str],
}

pub static RUNNERS: [Runner; 8] = [
    Runner {
        id: "p-triage",
        kind: RunnerKind::Pipeline,
        name: "PAY triage",
        desc: "Router › Fixer · Reproducer · Analyst › Review",
        load: "v14 live",
        steps: &["Router", "Fixer ×3", "Review", "You"],
    },
    Runner {
        id: "p-audit",
        kind: RunnerKind::Pipeline,
        name: "Nightly audit",
        desc: "Auditor ×2 › Report · autopilot 02:00",
        load: "idle",
        steps: &["Auditor ×2", "Report", "You"],
    },
    Runner {
        id: "a-fixer",
        kind: RunnerKind::Agent,
        name: "Fixer",
        desc: "Claude Code · opus-5.5 · may spawn ≤2",
        load: "1 running",
        steps: &["Fixer", "PR", "You"],
    },
    Runner {
        id: "a-repro",
        kind: RunnerKind::Agent,
        name: "Reproducer",
        desc: "Claude Code · sonnet-5.5 · cloud",
        load: "free",
        steps: &["Reproducer", "You"],
    },
    Runner {
        id: "a-codex",
        kind: RunnerKind::Agent,
        name: "Codex",
        desc: "Codex CLI · gpt-5.2 · runtime laptop",
        load: "1 queued",
        steps: &["Codex", "PR", "You"],
    },
    Runner {
        id: "a-auditor",
        kind: RunnerKind::Agent,
        name: "Auditor",
        desc: "OpenCode · opus-5.5 · read-only",
        load: "blocked",
        steps: &["Auditor", "Report", "You"],
    },
    Runner {
        id: "o-lead",
        kind: RunnerKind::Orch,
        name: "Payments lead",
        desc: "Runs Fixer, Codex, Reproducer, PAY triage · creates helpers",
        load: "1 running",
        steps: &["Payments lead", "splits", "Fixer + Codex", "You"],
    },
    Runner {
        id: "o-release",
        kind: RunnerKind::Orch,
        name: "Release captain",
        desc: "Runs Nightly audit, Release notes, Analyst",
        load: "idle",
        steps: &["Release captain", "Nightly audit", "You"],
    },
];

/// Index into [`RUNNERS`].
pub type RunnerId = usize;

pub fn runner(id: RunnerId) -> &'static Runner {
    &RUNNERS[id]
}

fn runner_by_key(key: &str) -> RunnerId {
    RUNNERS
        .iter()
        .position(|r| r.id == key)
        .expect("known runner id")
}

/// Case-insensitive lookup by display name (for `@mentions`).
pub fn runner_by_name(name: &str) -> Option<RunnerId> {
    RUNNERS
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case(name))
}

// ── Runs ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Waiting,
    Blocked,
    Done,
    Queued,
}

impl RunState {
    pub fn dot(self) -> Status {
        match self {
            RunState::Running => Status::Running,
            RunState::Waiting | RunState::Blocked => Status::Waiting,
            RunState::Done => Status::Live,
            RunState::Queued => Status::Draft,
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            RunState::Running => INFO,
            RunState::Waiting | RunState::Blocked => WARNING,
            RunState::Done => SUCCESS,
            RunState::Queued => TEXT_MUTED,
        }
    }

    pub fn bg(self) -> Hsla {
        match self {
            RunState::Running => INFO_SOFT,
            RunState::Waiting | RunState::Blocked => WARNING_SOFT,
            RunState::Done | RunState::Queued => SURFACE,
        }
    }

    /// Waiting on a person (counts toward "N need you").
    pub fn needs_you(self) -> bool {
        matches!(self, RunState::Waiting | RunState::Blocked)
    }
}

#[derive(Clone, Debug)]
pub struct Run {
    pub state: RunState,
    pub text: SharedString,
    pub pct: u8,
}

impl Run {
    pub fn new(state: RunState, text: impl Into<SharedString>) -> Self {
        Self {
            state,
            text: text.into(),
            pct: 0,
        }
    }

    pub fn running(text: impl Into<SharedString>, pct: u8) -> Self {
        Self {
            state: RunState::Running,
            text: text.into(),
            pct,
        }
    }
}

// ── Activity ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    /// `who` holds initials ("AR", "ME").
    Person,
    Agent,
    Pipeline,
}

impl Actor {
    pub fn of(kind: RunnerKind) -> Self {
        match kind {
            RunnerKind::Pipeline => Actor::Pipeline,
            RunnerKind::Agent | RunnerKind::Orch => Actor::Agent,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Activity {
    pub who: SharedString,
    pub actor: Actor,
    pub when: SharedString,
    pub text: SharedString,
    /// Author is a spawned helper (spawn-colored tile and name).
    pub spawn: bool,
    /// Text is a spawn note (spawn-colored text).
    pub spawn_note: bool,
    /// Question blocking the run (warning box).
    pub blocker: bool,
    /// Images attached to the comment.
    pub images: Vec<ImageAttachment>,
}

impl Activity {
    pub fn person(who: &str, when: &str, text: impl Into<SharedString>) -> Self {
        Self::new(who, Actor::Person, when, text)
    }

    pub fn agent(who: &str, when: &str, text: impl Into<SharedString>) -> Self {
        Self::new(who, Actor::Agent, when, text)
    }

    pub fn pipeline(who: &str, when: &str, text: impl Into<SharedString>) -> Self {
        Self::new(who, Actor::Pipeline, when, text)
    }

    pub fn new(who: &str, actor: Actor, when: &str, text: impl Into<SharedString>) -> Self {
        Self {
            who: SharedString::from(who.to_string()),
            actor,
            when: SharedString::from(when.to_string()),
            text: text.into(),
            spawn: false,
            spawn_note: false,
            blocker: false,
            images: Vec::new(),
        }
    }

    fn spawned(mut self) -> Self {
        self.spawn = true;
        self
    }

    fn spawn_note(mut self) -> Self {
        self.spawn_note = true;
        self
    }

    fn blocker(mut self) -> Self {
        self.blocker = true;
        self
    }
}

// ── Issues ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Assign,
    Comment,
    Mention,
}

impl Trigger {
    pub const ALL: [Trigger; 3] = [Trigger::Assign, Trigger::Comment, Trigger::Mention];

    pub fn label(self) -> &'static str {
        match self {
            Trigger::Assign => "On assign",
            Trigger::Comment => "On comment",
            Trigger::Mention => "On @mention",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub key: SharedString,
    pub title: SharedString,
    pub status: Column,
    pub label: SharedString,
    pub prio: Priority,
    pub person: Option<&'static str>,
    pub runner: Option<RunnerId>,
    pub run: Option<Run>,
    /// Agent that filed this issue.
    pub created_by: Option<SharedString>,
    pub desc: SharedString,
    /// Images attached to the description.
    pub images: Vec<ImageAttachment>,
    pub activity: Vec<Activity>,
    /// "Start when" toggles, indexed like [`Trigger::ALL`].
    pub triggers: [bool; 3],
}

impl Issue {
    #[allow(clippy::too_many_arguments)]
    fn new(
        key: &str,
        title: &str,
        status: Column,
        label: &str,
        prio: Priority,
        person: &'static str,
        runner: Option<&str>,
        run: Option<Run>,
        desc: &str,
        activity: Vec<Activity>,
    ) -> Self {
        Self {
            key: SharedString::from(key.to_string()),
            title: SharedString::from(title.to_string()),
            status,
            label: SharedString::from(label.to_string()),
            prio,
            person: Some(person),
            runner: runner.map(runner_by_key),
            run,
            created_by: None,
            desc: SharedString::from(desc.to_string()),
            images: Vec::new(),
            activity,
            triggers: [true; 3],
        }
    }

    /// A fresh "Untitled issue" (the mockup's `addIssue`).
    pub fn untitled(key: String, status: Column) -> Self {
        Self::new(
            &key,
            "Untitled issue",
            status,
            "Task",
            Priority::Medium,
            "ME",
            None,
            None,
            "Describe the outcome. Attach an agent or a pipeline to have it picked up.",
            vec![Activity::person("ME", "now", "Created.")],
        )
    }

    pub fn runner(&self) -> Option<&'static Runner> {
        self.runner.map(runner)
    }

    pub fn is_running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|r| r.state == RunState::Running)
    }

    pub fn needs_you(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.state.needs_you())
    }

    /// No run yet, or only queued (can be started / picked up).
    pub fn startable(&self) -> bool {
        self.run
            .as_ref()
            .is_none_or(|r| r.state == RunState::Queued)
    }
}

pub fn seed() -> Vec<Issue> {
    use Column::*;
    use Priority::*;
    vec![
        Issue::new(
            "PAY-1440",
            "Flaky checkout e2e on Safari",
            Backlog,
            "Bug",
            Low,
            "AR",
            None,
            None,
            "checkout.spec fails about 1 in 8 runs on Safari 19. Looks like a race on the 3-D Secure iframe.",
            vec![Activity::person("AR", "1d", "Filed from the nightly e2e report.")],
        ),
        Issue {
            created_by: Some("Analyst".into()),
            ..Issue::new(
                "PAY-1442",
                "Add receipts to refund emails",
                Backlog,
                "Story",
                Medium,
                "ME",
                None,
                None,
                "Refund emails have no receipt. Raised while specifying PAY-1431.",
                vec![
                    Activity::agent(
                        "Analyst",
                        "3h",
                        "Created this issue while writing the spec for PAY-1431. Not in scope there.",
                    )
                    .spawned(),
                ],
            )
        },
        Issue::new(
            "PAY-1436",
            "Retry budget per merchant",
            Progress,
            "Story",
            Medium,
            "NK",
            Some("o-lead"),
            Some(Run::running("split in 2 · Analyst spec, Fixer code", 30)),
            "Cap automatic retries per merchant per hour so one bad integration can’t flood the gateway.",
            vec![
                Activity::person("NK", "5h", "Let’s spec this before anyone writes code."),
                Activity::agent(
                    "Payments lead",
                    "30m",
                    "Split in two: Analyst writes the spec, then Fixer implements. Created Migration checker to watch the SDK v9 bump in parallel.",
                ),
                Activity::agent(
                    "Payments lead",
                    "29m",
                    "Created Migration checker · alive until PAY-1438 closes.",
                )
                .spawned()
                .spawn_note(),
            ],
        ),
        Issue::new(
            "PAY-1438",
            "Bump gateway SDK to v9",
            Todo,
            "Chore",
            Low,
            "AR",
            Some("a-codex"),
            Some(Run::new(RunState::Queued, "starts in 2 min · runtime laptop")),
            "v8 is deprecated in January. Changelog lists two breaking changes.",
            vec![
                Activity::person("AR", "20m", "Assigned to Codex."),
                Activity::agent("Codex", "19m", "Queued behind 1 run on runtime laptop."),
            ],
        ),
        Issue::new(
            "PAY-1427",
            "Refund webhook times out",
            Progress,
            "Bug",
            Medium,
            "ME",
            Some("p-triage"),
            Some(Run::running("Reproducer running · Log digger spawned", 64)),
            "Refunds with many line items hit the 10s gateway timeout.",
            vec![
                Activity::pipeline("PAY triage", "12m", "Picked up by rule 2 → Reproducer."),
                Activity::agent("Reproducer", "9m", "Spawned Log digger to search the last 24h of logs.")
                    .spawn_note(),
            ],
        ),
        Issue::new(
            "PAY-1435",
            "Audit PCI log redaction",
            Progress,
            "Chore",
            High,
            "AR",
            Some("a-auditor"),
            Some(Run::new(RunState::Blocked, "asks: which log sinks are in scope?")),
            "Check that card numbers never reach logs, traces or error reports.",
            vec![
                Activity::agent(
                    "Auditor",
                    "40m",
                    "Found 3 sinks: Datadog, Sentry, S3 archive. Sentry token is expired, so I can’t read it. Which sinks are in scope?",
                )
                .blocker(),
            ],
        ),
        Issue::new(
            "PAY-1423",
            "Double charge on retry",
            Review,
            "Bug",
            High,
            "ME",
            Some("p-triage"),
            Some(Run::new(RunState::Waiting, "3 variants to review")),
            "Customers are charged twice when the gateway times out and we retry.",
            vec![
                Activity::pipeline("PAY triage", "2h", "Picked up by rule 1 → Fixer ×3."),
                Activity::agent("Fixer", "1h", "Spawned Test writer ×2 · depth 1 of 2.").spawn_note(),
                Activity::agent(
                    "Fixer",
                    "40m",
                    "3 variants ready. A passes all tests and is the smallest safe change.",
                ),
            ],
        ),
        Issue::new(
            "PAY-1431",
            "Saved cards for guest checkout",
            Review,
            "Story",
            Low,
            "NK",
            Some("p-triage"),
            Some(Run::new(RunState::Waiting, "2 specs to review")),
            "Let guests save a card for next time without a full account.",
            vec![Activity::agent(
                "Analyst",
                "3h",
                "Wrote two specs: tokenised card with email link, or a light account.",
            )],
        ),
        Issue::new(
            "PAY-1419",
            "Webhook signature check",
            Done,
            "Bug",
            High,
            "AR",
            Some("a-fixer"),
            Some(Run::new(RunState::Done, "PR #2280 merged")),
            "Reject webhooks with a missing or stale signature.",
            vec![
                Activity::agent("Fixer", "1d", "Opened PR #2280."),
                Activity::person("AR", "1d", "Merged."),
            ],
        ),
        Issue::new(
            "PAY-1417",
            "Update payout docs",
            Done,
            "Docs",
            Low,
            "AR",
            None,
            None,
            "Docs still describe weekly payouts.",
            vec![Activity::person("AR", "2d", "Done by hand.")],
        ),
    ]
}

// ── Filters ─────────────────────────────────────────────────────────────

/// "Show issues run by" chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Orch,
    Agent,
    Pipeline,
    Unattached,
}

impl Filter {
    pub const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Orch,
        Filter::Agent,
        Filter::Pipeline,
        Filter::Unattached,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Orch => "Orchestrators",
            Filter::Agent => "Agents",
            Filter::Pipeline => "Pipelines",
            Filter::Unattached => "Unattached",
        }
    }

    pub fn matches(self, issue: &Issue) -> bool {
        let kind = issue.runner().map(|r| r.kind);
        match self {
            Filter::All => true,
            Filter::Unattached => kind.is_none(),
            Filter::Orch => kind == Some(RunnerKind::Orch),
            Filter::Agent => kind == Some(RunnerKind::Agent),
            Filter::Pipeline => kind == Some(RunnerKind::Pipeline),
        }
    }
}
