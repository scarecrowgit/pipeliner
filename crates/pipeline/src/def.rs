//! Pipeline definitions: named working folders and an ordered list of steps (actions).
//! Each pipeline is a TOML file in `~/.config/harness/pipelines/<id>.toml`; any number
//! can exist, and issues pick one.
//!
//! Step kinds:
//! - **agent** — a model works in a workdir with tools; with `delegate` it is an
//!   orchestrator that hands tasks to sub-agents instead of editing itself.
//! - **decision** — a model answers a yes/no question; "no" jumps to `on_no`.
//! - **render** — renders the HTML files changed in a workdir (before / after PNGs).
//! - **approval** — waits for you: approve to go on, or request changes to go back to
//!   `back_to` with your notes.
//! - **command** — a shell command in a workdir (tests, builds, scripts).
//! - **merge** — commits the run's branch of a worktree and merges it into its base.
//! - **pull_request** — commits, pushes and opens a (draft) PR; posts it to the Inbox.
//! - **report** — posts a summary of the run to the Inbox.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow};
use serde::{Deserialize, Serialize};

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// `~` and `~/x` → home.
pub fn expand(p: &str) -> PathBuf {
    let p = p.trim();
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("harness")
}

pub fn pipelines_dir() -> PathBuf {
    config_dir().join("pipelines")
}

/// A folder steps work in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkdirDef {
    /// Referenced by steps (`code`, `mockups`).
    pub id: String,
    pub name: String,
    /// The repository or folder.
    pub path: PathBuf,
    /// A fresh git worktree (own branch) per run; otherwise the folder is used as is.
    pub worktree: bool,
    /// Branch worktrees start from, PRs target and merges go into.
    pub base_branch: String,
    /// Run branches are `<prefix><issue-slug>`.
    pub branch_prefix: String,
    /// Run once in a new worktree before any step uses it (warm caches, install deps).
    pub setup: String,
    /// Environment for commands and agents' bash in this workdir.
    pub env: BTreeMap<String, String>,
}

impl Default for WorkdirDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            path: PathBuf::new(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: String::new(),
            setup: String::new(),
            env: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Agent,
    Decision,
    Render,
    Approval,
    Command,
    Merge,
    PullRequest,
    Report,
}

impl StepKind {
    pub const ALL: [StepKind; 8] = [
        StepKind::Agent,
        StepKind::Decision,
        StepKind::Render,
        StepKind::Approval,
        StepKind::Command,
        StepKind::Merge,
        StepKind::PullRequest,
        StepKind::Report,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Agent => "agent",
            StepKind::Decision => "decision",
            StepKind::Render => "render",
            StepKind::Approval => "approval",
            StepKind::Command => "command",
            StepKind::Merge => "merge",
            StepKind::PullRequest => "pull_request",
            StepKind::Report => "report",
        }
    }

    pub fn parse(s: &str) -> Option<StepKind> {
        StepKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            StepKind::Agent => "Agent",
            StepKind::Decision => "Decision",
            StepKind::Render => "Render",
            StepKind::Approval => "Approval",
            StepKind::Command => "Command",
            StepKind::Merge => "Merge",
            StepKind::PullRequest => "Pull request",
            StepKind::Report => "Report",
        }
    }

    /// One line for the "Add step" menu.
    pub fn about(self) -> &'static str {
        match self {
            StepKind::Agent => "A model works in a folder with tools; can lead sub-agents",
            StepKind::Decision => "A model answers yes/no; “no” jumps to another step",
            StepKind::Render => "Renders the HTML files a step changed, before and after",
            StepKind::Approval => "Waits for you: approve, or send back with notes",
            StepKind::Command => "Runs a shell command (tests, builds, scripts)",
            StepKind::Merge => "Merges the run's branch of a worktree into its base",
            StepKind::PullRequest => "Commits, pushes and opens a pull request",
            StepKind::Report => "Posts a summary of the run to the Inbox",
        }
    }

    /// Steps that run a model.
    pub fn uses_model(self) -> bool {
        matches!(self, StepKind::Agent | StepKind::Decision)
    }

    /// Steps that work in a workdir.
    pub fn uses_workdir(self) -> bool {
        !matches!(self, StepKind::Approval | StepKind::Report)
    }
}

/// What an agent may do in its workdir.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tools {
    /// read_file, list_dir, grep.
    ReadOnly,
    /// Read tools plus bash (no file edits): for orchestrators and reviewers.
    ReadAndRun,
    /// Everything: read, edit, write, bash.
    #[default]
    Full,
}

impl Tools {
    pub const ALL: [Tools; 3] = [Tools::ReadOnly, Tools::ReadAndRun, Tools::Full];

    pub fn label(self) -> &'static str {
        match self {
            Tools::ReadOnly => "Read only",
            Tools::ReadAndRun => "Read and run commands",
            Tools::Full => "Read, edit and run",
        }
    }

    pub fn names(self) -> Vec<&'static str> {
        match self {
            Tools::ReadOnly => vec!["read_file", "list_dir", "grep"],
            Tools::ReadAndRun => vec!["read_file", "list_dir", "grep", "bash"],
            Tools::Full => vec!["read_file", "list_dir", "grep", "edit_file", "write_file", "bash"],
        }
    }
}

/// Sub-agents an orchestrating agent step hands tasks to (`delegate` tool).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delegate {
    /// Shown in the log and on the board (`Coder`).
    pub name: String,
    pub model: String,
    pub tools: Tools,
    pub skills: Vec<String>,
    pub skills_on_demand: Vec<String>,
    pub max_steps: usize,
    pub instructions: String,
}

impl Default for Delegate {
    fn default() -> Self {
        Self {
            name: "Coder".into(),
            model: String::new(),
            tools: Tools::Full,
            skills: Vec::new(),
            skills_on_demand: Vec::new(),
            max_steps: 80,
            instructions: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepDef {
    /// Unique within the pipeline; jumps refer to it.
    pub id: String,
    pub name: String,
    pub kind: StepKind,
    /// Workdir id the step works in.
    pub workdir: String,

    // agent / decision
    pub model: String,
    pub instructions: String,
    pub tools: Tools,
    pub skills: Vec<String>,
    pub skills_on_demand: Vec<String>,
    pub max_steps: usize,
    pub delegate: Option<Delegate>,
    /// decision: the yes/no question.
    pub question: String,
    /// decision: step id to jump to on "no" (`end` finishes the run).
    pub on_no: String,

    // render
    /// Files to render when changed, e.g. `screens/*.html`.
    pub files: String,

    // approval
    /// Step id the run goes back to when you request changes.
    pub back_to: String,
    /// Skip the approval when the step before it rendered nothing.
    pub skip_if_nothing_to_review: bool,

    // command
    pub command: String,
    pub timeout_secs: u64,
    /// A failing command does not stop the run.
    pub allow_failure: bool,

    // pull request
    pub draft: bool,
    pub push: bool,
}

impl Default for StepDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: StepKind::Agent,
            workdir: String::new(),
            model: String::new(),
            instructions: String::new(),
            tools: Tools::Full,
            skills: Vec::new(),
            skills_on_demand: Vec::new(),
            max_steps: 60,
            delegate: None,
            question: String::new(),
            on_no: String::new(),
            files: "**/*.html".into(),
            back_to: String::new(),
            skip_if_nothing_to_review: true,
            command: String::new(),
            timeout_secs: 600,
            allow_failure: false,
            draft: true,
            push: true,
        }
    }
}

impl StepDef {
    /// A new step of `kind` with sensible defaults (`id` unique in `def`).
    pub fn new(kind: StepKind, def: &PipelineDef) -> Self {
        let id = def.unique_step_id(kind.as_str());
        let workdir = def.workdirs.first().map(|w| w.id.clone()).unwrap_or_default();
        let model = def
            .steps
            .iter()
            .find(|s| s.kind.uses_model() && !s.model.is_empty())
            .map(|s| s.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let mut s = StepDef {
            name: kind.label().to_string(),
            id,
            kind,
            workdir,
            model,
            ..StepDef::default()
        };
        match kind {
            StepKind::Decision => {
                s.tools = Tools::ReadOnly;
                s.max_steps = 12;
                s.question = "Is this step needed for the issue?".into();
                s.on_no = "end".into();
            }
            StepKind::Approval => {
                s.back_to = def
                    .steps
                    .iter()
                    .rev()
                    .find(|x| x.kind == StepKind::Agent)
                    .map(|x| x.id.clone())
                    .unwrap_or_default();
            }
            StepKind::Command => s.command = "echo ok".into(),
            _ => {}
        }
        s
    }
}

pub const DEFAULT_MODEL: &str = "nr/cc/claude-sonnet-5-5";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineDef {
    /// File stem; issues refer to it.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub description: String,
    pub workdirs: Vec<WorkdirDef>,
    pub steps: Vec<StepDef>,
}

impl Default for PipelineDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "New pipeline".into(),
            description: String::new(),
            workdirs: Vec::new(),
            steps: Vec::new(),
        }
    }
}

/// `mockup-gpui-build` style id from a name.
pub fn slugify(name: &str) -> String {
    let mut s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "pipeline".into() } else { s }
}

impl PipelineDef {
    pub fn workdir(&self, id: &str) -> Option<&WorkdirDef> {
        self.workdirs.iter().find(|w| w.id == id)
    }

    pub fn step(&self, id: &str) -> Option<&StepDef> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn step_index(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }

    pub fn unique_step_id(&self, base: &str) -> String {
        unique(base, |c| self.steps.iter().any(|s| s.id == c))
    }

    pub fn unique_workdir_id(&self, base: &str) -> String {
        unique(&slugify(base), |c| self.workdirs.iter().any(|w| w.id == c))
    }

    /// Problems that would stop a run (shown in the editor, checked before a run).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.steps.is_empty() {
            out.push("The pipeline has no steps.".into());
        }
        for w in &self.workdirs {
            if w.path.as_os_str().is_empty() {
                out.push(format!("Workdir `{}` has no folder.", w.name));
            } else if !w.path.is_dir() {
                out.push(format!("Workdir `{}`: {} does not exist.", w.name, w.path.display()));
            } else if w.worktree && !w.path.join(".git").exists() {
                out.push(format!(
                    "Workdir `{}` uses worktrees but {} is not a git repository.",
                    w.name,
                    w.path.display()
                ));
            }
        }
        for s in &self.steps {
            if s.kind.uses_workdir() && self.workdir(&s.workdir).is_none() {
                out.push(format!("Step `{}` has no workdir.", s.name));
            }
            if s.kind.uses_model() && !s.model.contains('/') {
                out.push(format!("Step `{}`: model must be `provider/model`.", s.name));
            }
            if s.kind == StepKind::Decision
                && !s.on_no.is_empty()
                && s.on_no != "end"
                && self.step(&s.on_no).is_none()
            {
                out.push(format!("Step `{}` jumps to a missing step `{}`.", s.name, s.on_no));
            }
            if s.kind == StepKind::Approval && !s.back_to.is_empty() && self.step(&s.back_to).is_none() {
                out.push(format!("Step `{}` sends back to a missing step `{}`.", s.name, s.back_to));
            }
            if s.kind == StepKind::Command && s.command.trim().is_empty() {
                out.push(format!("Step `{}` has no command.", s.name));
            }
        }
        out
    }

    pub fn path(&self) -> PathBuf {
        pipelines_dir().join(format!("{}.toml", self.id))
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if self.id.is_empty() || self.id.contains('/') || self.id.contains("..") {
            return Err(anyhow!("invalid pipeline id `{}`", self.id));
        }
        std::fs::create_dir_all(pipelines_dir())?;
        let mut text = String::from(
            "# Harness pipeline. Steps run top to bottom; see the editor for what each field does.\n\n",
        );
        text.push_str(&toml::to_string_pretty(self)?);
        std::fs::write(self.path(), text)?;
        Ok(())
    }

    pub fn from_toml(id: &str, text: &str) -> anyhow::Result<Self> {
        let mut def: PipelineDef = toml::from_str(text)?;
        def.id = id.to_string();
        Ok(def)
    }
}

fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..1000)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken(c))
        .unwrap_or_else(|| format!("{base}-x"))
}

/// All pipelines (sorted by name). On first use, writes the default pipeline (from the old
/// `pipeline.toml` when there is one). Unreadable files are reported, not dropped silently.
pub fn load_all() -> (Vec<PipelineDef>, Vec<String>) {
    let dir = pipelines_dir();
    let mut errors = Vec::new();
    if !dir.is_dir() {
        let def = crate::config::legacy_default();
        if let Err(e) = def.save() {
            errors.push(format!("Could not write {}: {e:#}", def.path().display()));
            return (vec![def], errors);
        }
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "toml") {
                continue;
            }
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&p)
                .context("read")
                .and_then(|t| PipelineDef::from_toml(&id, &t))
            {
                Ok(d) => out.push(d),
                Err(e) => errors.push(format!("{}: {e:#}", p.display())),
            }
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    (out, errors)
}

/// A free id for a pipeline named `name`.
pub fn new_pipeline_id(name: &str, existing: &[PipelineDef]) -> String {
    unique(&slugify(name), |c| {
        existing.iter().any(|p| p.id == c) || pipelines_dir().join(format!("{c}.toml")).exists()
    })
}

/// Display path with `~`.
pub fn tilde(p: &Path) -> String {
    agent_tools::workdir::display_path(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let mut def = crate::config::legacy_default();
        def.id = "x".into();
        let text = toml::to_string_pretty(&def).unwrap();
        let back = PipelineDef::from_toml("x", &text).unwrap();
        assert_eq!(back, def);
        assert!(back.steps.iter().any(|s| s.delegate.is_some()));
    }

    #[test]
    fn new_steps_get_unique_ids_and_defaults() {
        let def = crate::config::legacy_default();
        let s = StepDef::new(StepKind::Approval, &def);
        assert_ne!(s.id, "approval");
        assert!(!s.back_to.is_empty());
        let d = StepDef::new(StepKind::Decision, &def);
        assert_eq!(d.on_no, "end");
        assert_eq!(slugify("Mockup → GPUI build"), "mockup-gpui-build");
    }

    #[test]
    fn problems_found() {
        let mut def = PipelineDef { id: "p".into(), ..Default::default() };
        assert!(!def.problems().is_empty());
        def.workdirs.push(WorkdirDef { id: "w".into(), name: "W".into(), path: "/definitely/missing".into(), ..Default::default() });
        def.steps.push(StepDef { id: "a".into(), name: "A".into(), workdir: "w".into(), model: "p/m".into(), ..Default::default() });
        assert!(def.problems().iter().any(|p| p.contains("does not exist")));
    }
}
