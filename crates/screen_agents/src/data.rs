//! Mock data for the Agents & pipelines screen (port of the mockup's `SEED`, `T`, `S`, `USE`).

use gpui::Hsla;
use ui::Status;
use ui::theme::*;

/// What an entry is (`T` in the mockup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Orchestrator,
    Pipeline,
    Agent,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Orchestrator => "Orchestrator",
            Kind::Pipeline => "Pipeline",
            Kind::Agent => "Agent",
        }
    }
}

/// Run state (`S` in the mockup): drives the status dot and status-text color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Idle,
    Live,
    Waiting,
    Ended,
}

impl RunState {
    pub fn dot(self) -> Status {
        match self {
            RunState::Running => Status::Running,
            RunState::Idle => Status::Draft,
            RunState::Live | RunState::Ended => Status::Live,
            RunState::Waiting => Status::Waiting,
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            RunState::Running => INFO,
            RunState::Idle | RunState::Ended => TEXT_MUTED,
            RunState::Live => SUCCESS,
            RunState::Waiting => WARNING,
        }
    }
}

/// Places an entry can be used from (`USE` in the mockup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Chat,
    Issues,
    Pipelines,
    Autopilots,
}

impl Use {
    pub const ALL: [Use; 4] = [Use::Chat, Use::Issues, Use::Pipelines, Use::Autopilots];

    pub fn label(self) -> &'static str {
        match self {
            Use::Chat => "Chat",
            Use::Issues => "Issues",
            Use::Pipelines => "Inside pipelines",
            Use::Autopilots => "Autopilots",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Grid filter (the tab bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Orchestrators,
    Agents,
    Pipelines,
    Created,
}

impl Filter {
    pub const TABS: [Filter; 5] = [
        Filter::All,
        Filter::Orchestrators,
        Filter::Agents,
        Filter::Pipelines,
        Filter::Created,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Orchestrators => "Orchestrators",
            Filter::Agents => "Agents",
            Filter::Pipelines => "Pipelines",
            Filter::Created => "Created by agents",
        }
    }

    pub fn matches(self, e: &Entry) -> bool {
        let native = |k: Kind| e.kind == k && e.created_by.is_none();
        match self {
            Filter::All => true,
            Filter::Orchestrators => native(Kind::Orchestrator),
            Filter::Agents => native(Kind::Agent),
            Filter::Pipelines => native(Kind::Pipeline),
            Filter::Created => e.created_by.is_some(),
        }
    }
}

/// One agent, orchestrator or pipeline. Fields that the screen edits are plain owned values.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: &'static str,
    pub kind: Kind,
    pub name: &'static str,
    /// Id of the orchestrator/agent that created this one (cleared by "Keep as a permanent agent").
    pub created_by: Option<&'static str>,
    pub for_issue: &'static str,
    pub state: RunState,
    pub status_text: String,
    pub meta: &'static str,
    pub desc: &'static str,
    pub model: Option<&'static str>,
    pub runtime: Option<&'static str>,
    /// Team (orchestrators) or the agents inside (pipelines).
    pub team: Vec<&'static str>,
    pub can_create: bool,
    pub max_alive: i32,
    pub depth: i32,
    pub input: &'static str,
    pub output: &'static str,
    /// Tool permissions (the "Tools" chips on agents).
    pub tools: &'static [&'static str],
    /// Loaded skills in insertion order (`skills[id]` in the mockup), edited from the panel.
    pub skills: Vec<(&'static str, SkillMode)>,
    /// Indexed by [`Use::index`].
    pub uses: [bool; 4],
}

impl Entry {
    pub fn uses(&self, u: Use) -> bool {
        self.uses[u.index()]
    }
}

/// Base entry with every optional field empty; the seed overrides what it needs.
#[allow(clippy::too_many_arguments)]
fn base(
    id: &'static str,
    kind: Kind,
    name: &'static str,
    state: RunState,
    status_text: &str,
    meta: &'static str,
    desc: &'static str,
    uses: [bool; 4],
) -> Entry {
    Entry {
        id,
        kind,
        name,
        created_by: None,
        for_issue: "",
        state,
        status_text: status_text.to_string(),
        meta,
        desc,
        model: None,
        runtime: None,
        team: Vec::new(),
        can_create: false,
        max_alive: 1,
        depth: 2,
        input: "",
        output: "",
        tools: &[],
        skills: Vec::new(),
        uses,
    }
}

#[allow(clippy::too_many_arguments)]
fn agent(
    id: &'static str,
    name: &'static str,
    state: RunState,
    status_text: &str,
    meta: &'static str,
    desc: &'static str,
    model: &'static str,
    runtime: &'static str,
    tools: &'static [&'static str],
    uses: [bool; 4],
) -> Entry {
    Entry {
        model: Some(model),
        runtime: Some(runtime),
        tools,
        ..base(id, Kind::Agent, name, state, status_text, meta, desc, uses)
    }
}

fn created(mut e: Entry, by: &'static str, issue: &'static str) -> Entry {
    e.created_by = Some(by);
    e.for_issue = issue;
    e
}

pub fn seed() -> Vec<Entry> {
    use RunState::*;
    const T: bool = true;
    const F: bool = false;
    let mut list = vec![
        Entry {
            model: Some("opus-5.5"),
            runtime: Some("cloud"),
            team: vec!["fixer", "codex", "reproducer", "pay-triage"],
            can_create: true,
            max_alive: 3,
            ..base(
                "payments-lead",
                Kind::Orchestrator,
                "Payments lead",
                Running,
                "running · PAY-1436",
                "14 runs today · $1.12",
                "Splits payments work across the team and creates helpers when nobody fits.",
                [T, T, T, F],
            )
        },
        Entry {
            model: Some("sonnet-5.5"),
            runtime: Some("cloud"),
            team: vec!["nightly-audit", "release-notes", "analyst"],
            ..base(
                "release-captain",
                Kind::Orchestrator,
                "Release captain",
                Idle,
                "idle",
                "1 run today · $0.09",
                "Runs the release checklist: audit, notes, tag. Never writes code.",
                [T, T, F, T],
            )
        },
        Entry {
            team: vec!["fixer", "reproducer", "analyst"],
            input: "a message or a Jira issue",
            output: "variants to review, then a PR or a spec",
            ..base(
                "pay-triage",
                Kind::Pipeline,
                "PAY triage",
                Live,
                "v14 live",
                "37 queued · 3 agents inside",
                "Router › Fixer · Reproducer · Analyst › Review. Returns variants for a person to pick.",
                [T, T, T, T],
            )
        },
        Entry {
            team: vec!["auditor"],
            input: "a scope or nothing",
            output: "a report and issues for each finding",
            ..base(
                "nightly-audit",
                Kind::Pipeline,
                "Nightly audit",
                Live,
                "next run 02:00",
                "autopilot · 2 agents inside",
                "Auditor ×2 › Report. Checks log redaction and dependency licences.",
                [T, T, T, T],
            )
        },
        Entry {
            team: vec!["analyst"],
            input: "a tag range",
            output: "notes.md to review",
            ..base(
                "release-notes",
                Kind::Pipeline,
                "Release notes",
                Idle,
                "draft v1",
                "not deployed",
                "Collects merged PRs and drafts notes for review.",
                [F, T, T, F],
            )
        },
        agent(
            "build",
            "Build",
            Live,
            "ready",
            "Claude Code · mac-mini",
            "General coding agent with full tools. The default in chat.",
            "opus-5.5",
            "mac-mini",
            &["repo.write", "bash", "test.run"],
            [T, T, T, F],
        ),
        agent(
            "fixer",
            "Fixer",
            Running,
            "running · PAY-1423",
            "Claude Code · opus-5.5",
            "Fixes bugs and writes the failing test first. May spawn Test writer.",
            "opus-5.5",
            "cloud",
            &["repo.write", "test.run", "git.bisect"],
            [T, T, T, F],
        ),
        agent(
            "reproducer",
            "Reproducer",
            Idle,
            "idle",
            "Claude Code · sonnet-5.5",
            "Finds a minimal reproduction and attaches logs. Never edits code.",
            "sonnet-5.5",
            "cloud",
            &["repo.read", "logs.search"],
            [T, T, T, F],
        ),
        agent(
            "analyst",
            "Analyst",
            Idle,
            "idle",
            "sonnet-5.5",
            "Turns stories into specs with acceptance criteria.",
            "sonnet-5.5",
            "cloud",
            &["repo.read", "docs.write"],
            [T, T, T, F],
        ),
        agent(
            "codex",
            "Codex",
            Idle,
            "1 queued",
            "Codex CLI · gpt-5.2 · laptop",
            "Dependency bumps and mechanical refactors.",
            "gpt-5.2",
            "laptop",
            &["repo.write", "bash"],
            [T, T, T, F],
        ),
        agent(
            "auditor",
            "Auditor",
            Waiting,
            "asks you · PAY-1435",
            "OpenCode · opus-5.5",
            "Read-only security checks. Reports, never fixes.",
            "opus-5.5",
            "cloud",
            &["repo.read", "logs.search"],
            [T, T, T, T],
        ),
        created(
            agent(
                "migration-checker",
                "Migration checker",
                Running,
                "alive · PAY-1438",
                "$0.08 so far",
                "Reads the gateway SDK v9 diff and lists breaking changes for Codex.",
                "haiku-5.5",
                "cloud",
                &["repo.read"],
                [T, F, F, F],
            ),
            "payments-lead",
            "PAY-1438",
        ),
        created(
            agent(
                "sdk-diff-reader",
                "SDK diff reader",
                Ended,
                "ended · $0.03",
                "lived 6 min",
                "Compared webhook payloads between SDK versions.",
                "haiku-5.5",
                "cloud",
                &["repo.read"],
                [F, F, F, F],
            ),
            "payments-lead",
            "PAY-1419",
        ),
        created(
            agent(
                "test-writer",
                "Test writer",
                Ended,
                "ended · ×2",
                "$0.04",
                "Writes one failing test that pins a bug.",
                "haiku-5.5",
                "cloud",
                &["test.run"],
                [F, F, T, F],
            ),
            "fixer",
            "PAY-1423",
        ),
    ];
    for e in &mut list {
        e.skills = seed_skills(e.id);
    }
    list
}

/// Entries in the "New" menu.
pub struct NewKind {
    pub kind: Kind,
    pub name: &'static str,
    pub desc: &'static str,
}

pub const NEW_KINDS: [NewKind; 3] = [
    NewKind {
        kind: Kind::Agent,
        name: "Agent",
        desc: "One model, its tools and skills",
    },
    NewKind {
        kind: Kind::Orchestrator,
        name: "Orchestrator",
        desc: "Runs a team and can create agents",
    },
    NewKind {
        kind: Kind::Pipeline,
        name: "Pipeline",
        desc: "Fixed graph of agents and files",
    },
];

// ── Skills (`SKILLLIB` / `SKDEF` in the mockup) ─────────────────────────

/// How a skill is loaded: always in context, or only when the task needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillMode {
    Forced,
    Auto,
}

/// One skill of the shared catalog.
pub struct SkillInfo {
    pub name: &'static str,
    pub desc: &'static str,
    /// Size in thousands of tokens, as printed ("3.2" → "3.2k").
    pub tok: &'static str,
}

pub const SKILL_LIB: [SkillInfo; 6] = [
    SkillInfo {
        name: "payments-retries",
        desc: "Idempotency keys, retry budgets, gateway timeouts",
        tok: "3.2",
    },
    SkillInfo {
        name: "write-failing-test",
        desc: "Pin a bug with a test before touching code",
        tok: "1.8",
    },
    SkillInfo {
        name: "pci-logging",
        desc: "What must never reach logs, traces or error reports",
        tok: "2.4",
    },
    SkillInfo {
        name: "adr-writer",
        desc: "Write an ADR in the team format",
        tok: "1.1",
    },
    SkillInfo {
        name: "jira-hygiene",
        desc: "Titles, labels and links for Jira issues",
        tok: "0.9",
    },
    SkillInfo {
        name: "release-checklist",
        desc: "Audit, notes and tag, in that order",
        tok: "1.6",
    },
];

pub fn skill_info(name: &str) -> Option<&'static SkillInfo> {
    SKILL_LIB.iter().find(|s| s.name == name)
}

/// Initial skills per entry id.
fn seed_skills(id: &str) -> Vec<(&'static str, SkillMode)> {
    use SkillMode::*;
    match id {
        "payments-lead" => vec![("payments-retries", Forced), ("jira-hygiene", Auto)],
        "release-captain" => vec![("release-checklist", Forced), ("adr-writer", Auto)],
        "pay-triage" => vec![
            ("payments-retries", Forced),
            ("write-failing-test", Auto),
            ("jira-hygiene", Auto),
        ],
        "nightly-audit" => vec![("pci-logging", Forced)],
        "release-notes" => vec![("release-checklist", Auto)],
        "build" => vec![("write-failing-test", Auto)],
        "fixer" => vec![("write-failing-test", Forced), ("payments-retries", Auto)],
        "reproducer" => vec![("payments-retries", Auto)],
        "analyst" => vec![("adr-writer", Auto), ("jira-hygiene", Auto)],
        "auditor" => vec![("pci-logging", Forced)],
        _ => Vec::new(),
    }
}
