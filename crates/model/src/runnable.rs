//! Runnables: agents, orchestrators and pipelines as one list. Anything here can take an issue,
//! answer in chat or sit inside a pipeline.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{IssueKey, ModelRef, RunId, RunnableRef, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnableKind {
    Agent,
    Orchestrator,
    Pipeline,
}

/// Where a runnable's definition comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum Origin {
    /// Ships with the app (Build, Plan, the self-editing pipeline).
    Builtin,
    /// `~/.config/harness/library/`.
    Library,
    /// `<repo>/.harness/`.
    Project { repo: PathBuf },
    /// A Claude Code agent file, read-only here.
    ClaudeCode { path: PathBuf },
    /// Created by another agent at run time.
    Spawned {
        by: RunnableRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
}

/// The places a runnable is offered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    pub chat: bool,
    pub issues: bool,
    pub pipelines: bool,
    pub autopilots: bool,
}

impl Default for Uses {
    fn default() -> Self {
        Self { chat: true, issues: true, pipelines: true, autopilots: false }
    }
}

/// The coarse tool levels the editor offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    /// read_file, list_dir, grep.
    ReadOnly,
    /// Read plus bash.
    ReadAndRun,
    /// Read, run, edit and write.
    #[default]
    Full,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrants {
    #[serde(default)]
    pub access: ToolAccess,
    /// Extra tools by name on top of the level, such as `mcp__github__create_pull_request`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Tools removed from the level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPolicy {
    /// In context from the first request, and passed to every agent this one creates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forced: Vec<String>,
    /// Offered by name; the model loads them when relevant. Empty means every installed skill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto: Vec<String>,
}

/// Whether and how far an agent may create its own agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnPolicy {
    #[serde(default)]
    pub enabled: bool,
    /// Created agents alive at the same time.
    #[serde(default = "default_alive")]
    pub alive_at_once: u32,
    /// Deeper than this asks a person first.
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    /// Created agents per run or per chat turn.
    #[serde(default = "default_per_run")]
    pub per_run: u32,
    /// Spending cap of each created agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

fn default_alive() -> u32 {
    3
}

fn default_depth() -> u32 {
    2
}

fn default_per_run() -> u32 {
    3
}

impl Default for SpawnPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            alive_at_once: default_alive(),
            max_depth: default_depth(),
            per_run: default_per_run(),
            budget_usd: None,
        }
    }
}

/// What makes an agent or an orchestrator: its role text and its limits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentDef {
    /// The system prompt body.
    pub role: String,
    #[serde(default)]
    pub model: ModelRef,
    /// Runtime name; only `local` runs today.
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub tools: ToolGrants,
    #[serde(default)]
    pub skills: SkillPolicy,
    #[serde(default)]
    pub spawn: SpawnPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Model turns before the run is stopped; 0 means the default.
    #[serde(default)]
    pub max_steps: u32,
    /// Orchestrators only: who it hands work to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub team: Vec<RunnableRef>,
    /// Orchestrators only: how it splits and routes, in plain words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing: String,
}

/// One row of the "Agents & pipelines" list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Runnable {
    pub reference: RunnableRef,
    pub kind: RunnableKind,
    pub name: String,
    pub description: String,
    pub origin: Origin,
    #[serde(default)]
    pub uses: Uses,
    /// Finishes what it runs, takes no new work.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub archived: bool,
    /// Present for agents and orchestrators. A pipeline's graph is loaded separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentDef>,
    /// The file the definition lives in, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

/// What a runnable is doing right now, for list rows and pickers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunnableState {
    /// Available, nothing running.
    #[default]
    Idle,
    /// The default chat agent: always there.
    Ready,
    Running {
        issue: Option<IssueKey>,
    },
    /// Asks a person something.
    Waiting {
        issue: Option<IssueKey>,
    },
    Queued {
        count: u32,
    },
    Paused,
    /// A pipeline with no deployed version.
    Draft {
        version: u32,
    },
    /// A pipeline with a deployed version.
    Live {
        version: u32,
    },
    /// A created agent that is still working.
    Alive {
        issue: Option<IssueKey>,
    },
    /// A created agent that finished.
    Ended {
        times: u32,
    },
}

/// Activity numbers shown next to a runnable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunnableActivity {
    pub state: RunnableState,
    pub runs_today: u32,
    pub cost_today_usd: f64,
    /// Pipelines: items waiting to start.
    #[serde(default)]
    pub queued: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// `~/.config/harness/skills`.
    User,
    /// `~/.claude/skills`.
    ClaudeCode,
    /// `<repo>/.harness/skills` or `<repo>/.claude/skills`.
    Project,
}

/// An installed skill: a folder with a `SKILL.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub source: SkillSource,
    /// Rough size of the full text in tokens, for "3.2k tokens".
    pub tokens: u64,
}
