//! Mock data and pure model logic for the pipeline editor (port of the mockup's
//! `<script type="text/x-dc">`): node kinds, the scripted test run, canvas geometry.

use gpui::Hsla;
use ui::Status;
use ui::theme::*;

/// Node width in layer units.
pub const NODE_W: f32 = 180.;
/// Layer (unscaled canvas) size.
pub const LAYER_W: f32 = 1368.;
pub const LAYER_H: f32 = 750.;
pub const ZOOM_MIN: f32 = 0.4;
pub const ZOOM_MAX: f32 = 1.4;
/// Last step of the scripted run (`PROGRESS.len() - 1`).
pub const LAST_STEP: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    Source,
    Router,
    Orch,
    Pipe,
    Agent,
    Spawn,
    File,
    Gate,
    #[default]
    Action,
}

pub struct KindInfo {
    /// Icon name; `file` nodes use the `file` icon.
    pub icon: &'static str,
    pub tile_bg: Hsla,
    pub tile_fg: Hsla,
    pub label: &'static str,
}

impl Kind {
    pub fn info(self) -> KindInfo {
        let (icon, tile_bg, tile_fg, label) = match self {
            Kind::Source => ("source", INFO_SOFT, INFO_TEXT, "Source"),
            Kind::Router => ("filter", SURFACE_HOVER, TEXT_SECONDARY, "Router"),
            Kind::Orch => (
                "router",
                ACCENT_SOFT,
                ACCENT_TEXT,
                "Orchestrator · runs a team, can create agents",
            ),
            Kind::Pipe => (
                "pipelines",
                INFO_SOFT,
                INFO_TEXT,
                "Pipeline · called like an agent",
            ),
            Kind::Agent => ("agent", ACCENT_SOFT, ACCENT_TEXT, "Agent"),
            Kind::Spawn => (
                "spawn",
                SPAWN_SOFT,
                SPAWN_TEXT,
                "Sub-agent · created at runtime",
            ),
            Kind::File => ("file", SURFACE_HOVER, TEXT_SECONDARY, "File"),
            Kind::Gate => (
                "inbox",
                WARNING_SOFT,
                WARNING,
                "Review · waits for a person",
            ),
            Kind::Action => ("check", SURFACE_HOVER, TEXT_SECONDARY, "Action"),
        };
        KindInfo {
            icon,
            tile_bg,
            tile_fg,
            label,
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            Kind::Source => "source",
            Kind::Router => "router",
            Kind::Orch => "orch",
            Kind::Pipe => "pipe",
            Kind::Agent => "agent",
            Kind::Spawn => "spawn",
            Kind::File => "file",
            Kind::Gate => "gate",
            Kind::Action => "action",
        }
    }
}

/// Toolbar "Add" buttons: (kind, icon, label).
pub const ADD_BUTTONS: [(Kind, &str, &str); 9] = [
    (Kind::Source, "source", "Source"),
    (Kind::Router, "filter", "Router"),
    (Kind::Agent, "agent", "Agent"),
    (Kind::Orch, "router", "Orchestrator"),
    (Kind::Pipe, "pipelines", "Pipeline"),
    (Kind::Spawn, "spawn", "Sub-agent"),
    (Kind::File, "file", "File"),
    (Kind::Gate, "inbox", "Review"),
    (Kind::Action, "check", "Action"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileMode {
    #[default]
    Read,
    Write,
}

/// One node. Flat like the mockup's JS objects: only the fields of its kind matter.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub id: String,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub title: String,
    // source
    pub jql: String,
    pub every: String,
    pub claim: bool,
    // file
    pub glob: String,
    pub mode: FileMode,
    // agent / orchestrator
    pub model: String,
    pub variants: i32,
    pub tools: usize,
    pub can_spawn: bool,
    pub depth: i32,
    pub kids: i32,
    pub role: String,
    pub budget: i32,
    // spawn
    pub parent: Option<String>,
    pub max: i32,
    // orchestrator
    pub team: Vec<String>,
    pub can_create: bool,
    pub max_alive: i32,
    // pipeline
    pub reference: String,
    pub returns: String,
    // action
    pub meta: String,
    pub rows: Vec<(String, String)>,
}

impl Node {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn has_in(&self) -> bool {
        let k = self.kind();
        k != Kind::Source && !(k == Kind::File && self.mode == FileMode::Read)
    }

    pub fn has_out(&self) -> bool {
        matches!(
            self.kind(),
            Kind::Source | Kind::Agent | Kind::File | Kind::Orch | Kind::Pipe
        )
    }

    /// Whether spawn-port edges from this node are active.
    pub fn spawns(&self) -> bool {
        self.can_spawn || self.can_create
    }
}

/// `{ f, p, t }`: from node, output port (-1 = header port, else row index), to node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub port: i32,
    pub to: String,
}

fn node(id: &str, kind: Kind, x: f32, y: f32, title: &str) -> Node {
    Node {
        id: id.into(),
        kind,
        x,
        y,
        title: title.into(),
        ..Default::default()
    }
}

pub fn initial_nodes() -> Vec<Node> {
    vec![
        Node {
            jql: "project = PAY AND status = \"Ready for agent\" ORDER BY priority".into(),
            every: "2m".into(),
            claim: true,
            ..node("jira", Kind::Source, 24., 200., "Jira puller")
        },
        node("router", Kind::Router, 252., 170., "Router"),
        Node {
            glob: "docs/**, CODEOWNERS".into(),
            mode: FileMode::Read,
            ..node("repo", Kind::File, 252., 430., "Repo context")
        },
        Node {
            model: "opus-5.5".into(),
            variants: 3,
            tools: 4,
            can_spawn: true,
            depth: 2,
            kids: 3,
            role: "Reproduce the bug from the issue, write a failing test, then fix it. Keep the diff under 200 lines.".into(),
            budget: 31,
            ..node("fixer", Kind::Agent, 480., 140., "Fixer")
        },
        Node {
            model: "sonnet-5.5".into(),
            variants: 1,
            tools: 3,
            can_spawn: true,
            depth: 1,
            kids: 2,
            role: "Find a minimal reproduction and attach logs. Do not change code.".into(),
            budget: 12,
            ..node("repro", Kind::Agent, 480., 362., "Reproducer")
        },
        Node {
            model: "sonnet-5.5".into(),
            variants: 2,
            tools: 2,
            can_spawn: false,
            depth: 1,
            kids: 1,
            role: "Turn the story into a short spec with acceptance criteria and open questions.".into(),
            budget: 9,
            ..node("analyst", Kind::Agent, 480., 584., "Analyst")
        },
        Node {
            parent: Some("fixer".into()),
            max: 3,
            role: "Write one failing test that pins the bug. Return the test file only.".into(),
            ..node("testw", Kind::Spawn, 708., 16., "Test writer")
        },
        Node {
            parent: Some("repro".into()),
            max: 2,
            role: "Search the last 24h of logs for the request ids in the issue. Return matching lines.".into(),
            ..node("logd", Kind::Spawn, 708., 450., "Log digger")
        },
        Node {
            glob: "out/{key}/spec.md".into(),
            mode: FileMode::Write,
            ..node("spec", Kind::File, 708., 604., "spec.md")
        },
        node("review", Kind::Gate, 936., 300., "Review"),
        Node {
            meta: "fix/{key} → main".into(),
            rows: vec![("repo".into(), "payments-api".into())],
            ..node("pr", Kind::Action, 1164., 250., "Open PR")
        },
        Node {
            meta: "status → In review".into(),
            rows: vec![("comment".into(), "PR link".into())],
            ..node("jupd", Kind::Action, 1164., 430., "Update Jira")
        },
    ]
}

pub fn initial_edges() -> Vec<Edge> {
    [
        ("jira", -1, "router"),
        ("router", 0, "fixer"),
        ("router", 1, "repro"),
        ("router", 2, "analyst"),
        ("repo", -1, "fixer"),
        ("repo", -1, "repro"),
        ("repo", -1, "analyst"),
        ("fixer", 1, "testw"),
        ("repro", 1, "logd"),
        ("fixer", -1, "review"),
        ("repro", -1, "review"),
        ("analyst", -1, "spec"),
        ("spec", -1, "review"),
        ("review", 1, "pr"),
        ("review", 1, "jupd"),
    ]
    .into_iter()
    .map(|(f, p, t)| Edge {
        from: f.into(),
        port: p,
        to: t.into(),
    })
    .collect()
}

/// Defaults for a node added from the toolbar (`addKind` in the mockup).
pub fn new_node(kind: Kind, uid: usize, x: f32, y: f32) -> Node {
    let id = format!("{}{}", kind.id_prefix(), uid);
    let base = Node {
        id,
        kind,
        x,
        y,
        ..Default::default()
    };
    match kind {
        Kind::Source => Node {
            title: "New source".into(),
            jql: "project = KEY".into(),
            every: "5m".into(),
            claim: false,
            ..base
        },
        Kind::Router => Node { title: "Router".into(), ..base },
        Kind::Agent => Node {
            title: "New agent".into(),
            model: "sonnet-5.5".into(),
            variants: 1,
            tools: 2,
            depth: 1,
            kids: 1,
            ..base
        },
        Kind::Spawn => Node { title: "New sub-agent".into(), max: 1, ..base },
        Kind::Orch => Node {
            title: "Payments lead".into(),
            model: "opus-5.5".into(),
            team: vec!["Fixer".into(), "Codex".into(), "Reproducer".into()],
            can_create: true,
            max_alive: 3,
            role: "Split the issue, give each part to the cheapest teammate with the skill, create a helper only when nobody fits.".into(),
            ..base
        },
        Kind::Pipe => Node {
            title: "Nightly audit".into(),
            reference: "Nightly audit".into(),
            returns: "report".into(),
            ..base
        },
        Kind::File => Node { title: "Files".into(), glob: "path/**".into(), mode: FileMode::Read, ..base },
        Kind::Gate => Node { title: "Review".into(), ..base },
        Kind::Action => Node {
            title: "New action".into(),
            meta: "webhook".into(),
            rows: vec![("target".into(), "—".into())],
            ..base
        },
    }
}

// ── Scripted test run ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Idle,
    Running,
    Done,
    Waiting,
    Spawned,
}

impl RunStatus {
    pub fn dot(self) -> Status {
        match self {
            RunStatus::Idle => Status::Draft,
            RunStatus::Running => Status::Running,
            RunStatus::Done => Status::Live,
            RunStatus::Waiting => Status::Waiting,
            RunStatus::Spawned => Status::Spawn,
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            RunStatus::Idle => "",
            RunStatus::Running => "running",
            RunStatus::Done => "done",
            RunStatus::Waiting => "6 waiting",
            RunStatus::Spawned => "spawned",
        }
    }
    pub fn color(self) -> Hsla {
        match self {
            RunStatus::Idle => TEXT_MUTED,
            RunStatus::Running => INFO,
            RunStatus::Done => SUCCESS,
            RunStatus::Waiting => WARNING,
            RunStatus::Spawned => SPAWN_TEXT,
        }
    }
}

/// `ST[id][step]` from the mockup; nodes not in the script stay idle.
pub fn scripted_status(id: &str, step: usize) -> RunStatus {
    use RunStatus::*;
    let row: [RunStatus; 7] = match id {
        "jira" => [Idle, Running, Done, Done, Done, Done, Done],
        "repo" => [Idle, Done, Done, Done, Done, Done, Done],
        "router" => [Idle, Idle, Done, Done, Done, Done, Done],
        "fixer" | "repro" => [Idle, Idle, Running, Running, Running, Done, Done],
        "analyst" => [Idle, Idle, Running, Running, Done, Done, Done],
        "testw" | "logd" => [Idle, Idle, Idle, Spawned, Done, Done, Done],
        "spec" => [Idle, Idle, Idle, Idle, Done, Done, Done],
        "review" => [Idle, Idle, Idle, Idle, Idle, Waiting, Waiting],
        _ => return Idle,
    };
    row[step.min(LAST_STEP)]
}

pub const PROGRESS: [f32; 7] = [0., 6., 22., 46., 70., 100., 100.];
pub const SPEND: [&str; 7] = [
    "$0.00", "$0.02", "$0.21", "$0.48", "$0.77", "$0.94", "$0.94",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogTone {
    Normal,
    Spawn,
    Wait,
}

pub struct LogLine {
    pub t: &'static str,
    pub who: &'static str,
    pub msg: &'static str,
    pub tone: LogTone,
}

pub const LOG: [LogLine; 6] = [
    LogLine {
        t: "00:00",
        who: "jira",
        msg: "pulled 3 · PAY-1423 PAY-1427 PAY-1431",
        tone: LogTone::Normal,
    },
    LogLine {
        t: "00:02",
        who: "router",
        msg: "PAY-1423 → Fixer (rule 1) · PAY-1427 → Reproducer (rule 2) · PAY-1431 → Analyst (rule 3)",
        tone: LogTone::Normal,
    },
    LogLine {
        t: "00:19",
        who: "spawn",
        msg: "Fixer spawned Test writer ×2 · Reproducer spawned Log digger · depth 1 of 2",
        tone: LogTone::Spawn,
    },
    LogLine {
        t: "00:51",
        who: "analyst",
        msg: "wrote out/PAY-1431/spec.v1.md, spec.v2.md",
        tone: LogTone::Normal,
    },
    LogLine {
        t: "01:42",
        who: "review",
        msg: "6 variants waiting for you · 3 issues",
        tone: LogTone::Wait,
    },
    LogLine {
        t: "01:45",
        who: "run",
        msg: "done · $0.94 of $3.00 · 3 agents, 3 spawned",
        tone: LogTone::Normal,
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BarTone {
    Run,
    Spawn,
    Wait,
}

pub struct TimelineRow {
    pub name: &'static str,
    pub key: &'static str,
    /// Start / end in percent of the run.
    pub s: f32,
    pub e: f32,
    pub tone: BarTone,
}

pub const TIMELINE: [TimelineRow; 8] = [
    TimelineRow {
        name: "Jira puller",
        key: "3 issues",
        s: 0.,
        e: 6.,
        tone: BarTone::Run,
    },
    TimelineRow {
        name: "Router",
        key: "3 routed",
        s: 6.,
        e: 9.,
        tone: BarTone::Run,
    },
    TimelineRow {
        name: "Fixer ×3",
        key: "PAY-1423",
        s: 9.,
        e: 84.,
        tone: BarTone::Run,
    },
    TimelineRow {
        name: "Test writer ×2",
        key: "by Fixer",
        s: 24.,
        e: 58.,
        tone: BarTone::Spawn,
    },
    TimelineRow {
        name: "Reproducer",
        key: "PAY-1427",
        s: 9.,
        e: 80.,
        tone: BarTone::Run,
    },
    TimelineRow {
        name: "Log digger",
        key: "by Reproducer",
        s: 30.,
        e: 54.,
        tone: BarTone::Spawn,
    },
    TimelineRow {
        name: "Analyst ×2",
        key: "PAY-1431",
        s: 9.,
        e: 50.,
        tone: BarTone::Run,
    },
    TimelineRow {
        name: "Review",
        key: "6 variants",
        s: 86.,
        e: 100.,
        tone: BarTone::Wait,
    },
];

pub const TOOLS: [&str; 5] = [
    "repo.read",
    "repo.write",
    "test.run",
    "git.bisect",
    "logs.search",
];

/// Router rules: (field, op, value) chips.
pub const ROUTES: [&[(&str, &str, &str)]; 3] = [
    &[("type", "is", "Bug"), ("prio", "≥", "High")],
    &[("type", "is", "Bug")],
    &[("type", "is", "Story")],
];

// ── Select options ──────────────────────────────────────────────────────

/// Agent library: (value, label).
pub const LIBRARY: [(&str, &str); 6] = [
    ("Fixer", "Fixer · agent"),
    ("Reproducer", "Reproducer · agent"),
    ("Analyst", "Analyst · agent"),
    ("Codex", "Codex · agent"),
    ("Build", "Build · agent"),
    ("New agent", "New agent (only in this pipeline)"),
];
pub const MODELS: [&str; 3] = ["opus-5.5", "sonnet-5.5", "haiku-5.5"];
pub const POLL_EVERY: [&str; 4] = ["1m", "2m", "5m", "15m"];
pub const PIPELINES: [(&str, &str); 2] = [
    ("Nightly audit", "Nightly audit · v3 live"),
    ("Release notes", "Release notes · draft"),
];
pub const REVIEWERS: [&str; 2] = ["payments-oncall", "Only me"];
pub const NUDGE: [&str; 3] = ["1h", "4h", "1d"];

// ── Geometry ────────────────────────────────────────────────────────────

/// Node height for `rows` key/value rows: border + title 40 + meta 26 + rows + padding 4.
pub fn node_height(rows: usize) -> f32 {
    2. + 40. + 26. + 26. * rows as f32 + 4.
}

pub fn in_pt(n: &Node) -> (f32, f32) {
    (n.x, n.y + 21.)
}

pub fn out_pt(n: &Node, port: i32) -> (f32, f32) {
    if port >= 0 {
        (n.x + NODE_W, n.y + 1. + 66. + 26. * port as f32 + 13.)
    } else {
        (n.x + NODE_W, n.y + 21.)
    }
}

/// Cubic control points for the mockup's `bez()` curve.
pub fn bez_controls(a: (f32, f32), b: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let c = 40f32.max((b.0 - a.0).abs() / 2.);
    ((a.0 + c, a.1), (b.0 - c, b.1))
}
