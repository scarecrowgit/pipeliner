//! Mock data for the app sidebar (ported from `reference/app_sidebar.html`).

use gpui::Hsla;
use ui::Status;
use ui::nav::Screen;
use ui::theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Orchestrator,
    Pipeline,
    Agent,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Orchestrator => "router",
            Kind::Pipeline => "pipelines",
            Kind::Agent => "agent",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Orchestrator => "Orchestrator",
            Kind::Pipeline => "Pipeline",
            Kind::Agent => "Agent",
        }
    }

    /// Rail tile colors `(bg, fg)`.
    pub fn tile(self) -> (Hsla, Hsla) {
        match self {
            Kind::Orchestrator => (ACCENT_SOFT, ACCENT_TEXT),
            Kind::Pipeline => (INFO_SOFT, INFO_TEXT),
            Kind::Agent => (SURFACE_HOVER, TEXT_SECONDARY),
        }
    }
}

pub struct Runner {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub status: Status,
    /// Created at runtime by the Payments lead orchestrator (spawn look).
    pub created: bool,
}

impl Runner {
    pub fn aria(&self) -> String {
        let what = if self.created {
            "created by Payments lead"
        } else {
            self.kind.label()
        };
        format!("{} · {}", self.name, what)
    }

    /// Where a click on this entity goes: the editor only knows PAY triage.
    pub fn target(&self) -> Screen {
        if self.id == "pay-triage" {
            Screen::PipelineEditor
        } else {
            Screen::Agents
        }
    }
}

/// Rail dot color (`DOT` map in the mockup: draft is a filled grey dot there).
pub fn rail_dot_color(status: Status) -> Hsla {
    match status {
        Status::Draft => TEXT_DISABLED,
        other => other.color(),
    }
}

pub const RUNNERS: &[Runner] = &[
    Runner {
        id: "payments-lead",
        name: "Payments lead",
        kind: Kind::Orchestrator,
        status: Status::Running,
        created: false,
    },
    Runner {
        id: "pay-triage",
        name: "PAY triage",
        kind: Kind::Pipeline,
        status: Status::Live,
        created: false,
    },
    Runner {
        id: "build",
        name: "Build",
        kind: Kind::Agent,
        status: Status::Live,
        created: false,
    },
    Runner {
        id: "fixer",
        name: "Fixer",
        kind: Kind::Agent,
        status: Status::Running,
        created: false,
    },
    Runner {
        id: "codex",
        name: "Codex",
        kind: Kind::Agent,
        status: Status::Draft,
        created: false,
    },
    Runner {
        id: "auditor",
        name: "Auditor",
        kind: Kind::Agent,
        status: Status::Waiting,
        created: false,
    },
    Runner {
        id: "nightly-audit",
        name: "Nightly audit",
        kind: Kind::Pipeline,
        status: Status::Live,
        created: false,
    },
    Runner {
        id: "migration-checker",
        name: "Migration checker",
        kind: Kind::Agent,
        status: Status::Spawn,
        created: true,
    },
];

/// The compact rail shows the first seven entities.
pub const RAIL_RUNNERS: usize = 7;

pub struct NavEntry {
    pub screen: Screen,
    pub label: &'static str,
    /// Rail tooltip.
    pub aria: &'static str,
    pub icon: &'static str,
    pub count: Option<u32>,
}

pub const NAV: &[NavEntry] = &[
    NavEntry {
        screen: Screen::Inbox,
        label: "Inbox",
        aria: "Inbox · 4 need you",
        icon: "inbox",
        count: Some(4),
    },
    NavEntry {
        screen: Screen::Issues,
        label: "Issues",
        aria: "Issues",
        icon: "check",
        count: None,
    },
    NavEntry {
        screen: Screen::Chat,
        label: "Chat",
        aria: "Chat",
        icon: "message",
        count: None,
    },
    NavEntry {
        screen: Screen::Agents,
        label: "Agents",
        aria: "Agents",
        icon: "agent",
        count: None,
    },
];

pub const ALL_SUMMARY: &str = "All 14 · 2 orchestrators · 3 pipelines";
pub const WORKSPACE: &str = "Acme · Payments";
pub const NEW_ENTITY: &str = "New agent, orchestrator or pipeline";
pub const AGENTS_BUSY: &str = "4 of 8";
pub const SPEND_TODAY: &str = "$3.81 of $10";
pub const SPEND_FRACTION: f32 = 381. / 1000.;
