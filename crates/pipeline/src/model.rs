//! Persistent data: issues on the board and pipeline runs (one run per issue attempt).
//! Stored as JSON under `$XDG_DATA_HOME/harness/` (`issues.json`, `runs/<id>.json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::def::{PipelineDef, StepKind};

pub fn data_dir() -> PathBuf {
    agent_tools::inbox::data_dir()
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

// ── Issues ──────────────────────────────────────────────────────────────

/// Board column of an issue; the pipeline moves it along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Backlog,
    Triage,
    Design,
    /// Mockup changes wait for the user's approval.
    Approval,
    Build,
    /// A pull request is open.
    Review,
    Done,
    Failed,
}

impl IssueStatus {
    pub const ALL: [IssueStatus; 8] = [
        IssueStatus::Backlog,
        IssueStatus::Triage,
        IssueStatus::Design,
        IssueStatus::Approval,
        IssueStatus::Build,
        IssueStatus::Review,
        IssueStatus::Done,
        IssueStatus::Failed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            IssueStatus::Backlog => "Backlog",
            IssueStatus::Triage => "Triage",
            IssueStatus::Design => "Design",
            IssueStatus::Approval => "Needs approval",
            IssueStatus::Build => "In progress",
            IssueStatus::Review => "In review",
            IssueStatus::Done => "Done",
            IssueStatus::Failed => "Failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// `HAR-12`.
    pub key: String,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub body: String,
    /// `Feature`, `Bug`, `Chore`.
    #[serde(default = "default_kind")]
    pub kind: String,
    pub created: u64,
    pub status: IssueStatus,
    /// Pipeline id the issue runs through.
    #[serde(default)]
    pub pipeline: String,
    /// Latest pipeline run.
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    /// Comments in the order they were posted.
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// A comment on an issue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    /// Unique within the issue, increasing.
    pub id: u32,
    pub author: String,
    pub created: u64,
    pub text: String,
    /// Names `@mentioned` in the text (resolved when posted).
    #[serde(default)]
    pub mentions: Vec<String>,
}

/// Who posts comments from this machine: `$USER`, else "You".
pub fn comment_author() -> String {
    std::env::var("USER")
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "You".into())
}

fn default_kind() -> String {
    "Feature".into()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IssueFile {
    next: u32,
    items: Vec<Issue>,
}

pub fn issues_path() -> PathBuf {
    data_dir().join("issues.json")
}

fn read_issues(path: &Path) -> IssueFile {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// All issues, newest first.
pub fn load_issues() -> Vec<Issue> {
    let mut items = read_issues(&issues_path()).items;
    items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.key.cmp(&a.key)));
    items
}

/// Create `HAR-<n>` and save it.
pub fn create_issue(title: &str, body: &str, kind: &str, pipeline: &str) -> std::io::Result<Issue> {
    create_issue_in(&issues_path(), title, body, kind, pipeline)
}

pub fn create_issue_in(
    path: &Path,
    title: &str,
    body: &str,
    kind: &str,
    pipeline: &str,
) -> std::io::Result<Issue> {
    let mut f = read_issues(path);
    f.next = f.next.max(f.items.len() as u32) + 1;
    let issue = Issue {
        key: format!("HAR-{}", f.next),
        title: title.trim().to_string(),
        body: body.trim().to_string(),
        kind: if kind.trim().is_empty() { default_kind() } else { kind.trim().to_string() },
        created: now(),
        status: IssueStatus::Backlog,
        pipeline: pipeline.to_string(),
        run: None,
        pr_url: None,
        comments: Vec::new(),
    };
    f.items.push(issue.clone());
    write_json(path, &f)?;
    Ok(issue)
}

/// Change one issue in place. `false` when there is no such issue.
pub fn update_issue(key: &str, f: impl FnOnce(&mut Issue)) -> std::io::Result<bool> {
    update_issue_in(&issues_path(), key, f)
}

pub fn update_issue_in(path: &Path, key: &str, f: impl FnOnce(&mut Issue)) -> std::io::Result<bool> {
    let mut file = read_issues(path);
    let Some(issue) = file.items.iter_mut().find(|i| i.key == key) else {
        return Ok(false);
    };
    f(issue);
    write_json(path, &file)?;
    Ok(true)
}

/// Append a comment to issue `key` in the file at `path`. `None` when there is no such
/// issue or the text is empty.
pub fn add_comment_in(
    path: &Path,
    key: &str,
    author: &str,
    text: &str,
    mentions: Vec<String>,
) -> std::io::Result<Option<Comment>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut added = None;
    update_issue_in(path, key, |issue| {
        let c = Comment {
            id: issue.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1,
            author: author.to_string(),
            created: now(),
            text: text.to_string(),
            mentions,
        };
        issue.comments.push(c.clone());
        added = Some(c);
    })?;
    Ok(added)
}

/// Remove comment `id` from issue `key` in the file at `path`.
pub fn delete_comment_in(path: &Path, key: &str, id: u32) -> std::io::Result<bool> {
    update_issue_in(path, key, |issue| issue.comments.retain(|c| c.id != id))
}

pub fn delete_issue(key: &str) -> std::io::Result<()> {
    let path = issues_path();
    let mut file = read_issues(&path);
    file.items.retain(|i| i.key != key);
    write_json(&path, &file)
}

// ── Runs ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    /// Waiting for the user.
    Waiting,
    Done,
    Skipped,
    Failed,
}

/// The state of one pipeline step in a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// Step id in the pipeline.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `StepKind::as_str` (`agent`, `approval`…); free text for runs of older versions.
    pub kind: String,
    pub status: StageStatus,
    #[serde(default)]
    pub started: u64,
    #[serde(default)]
    pub finished: u64,
    /// One line on what happened.
    #[serde(default)]
    pub summary: String,
}

impl Stage {
    pub fn step_kind(&self) -> Option<StepKind> {
        StepKind::parse(&self.kind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    WaitingApproval,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    /// Pipeline step (worktree, render, push…).
    Step,
    /// Agent reply text.
    Text,
    /// Agent tool call.
    Tool,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub at: u64,
    /// `pipeline`, a step name, or a sub-agent (`Coder 2`).
    pub who: String,
    pub kind: LogKind,
    pub text: String,
}

/// A file rendered for review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Render {
    /// Render step that made it.
    #[serde(default)]
    pub step: String,
    /// File stem, e.g. `issue-board`.
    pub screen: String,
    /// PNG of the file before the change (`None` for a new file).
    pub before: Option<PathBuf>,
    pub after: PathBuf,
}

/// A task an orchestrating step gave a sub-agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoderTask {
    pub title: String,
    pub status: StageStatus,
    #[serde(default)]
    pub summary: String,
    /// Step that delegated it.
    #[serde(default)]
    pub step: String,
}

/// What one agent of a run used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentUsage {
    /// Step name or sub-agent (`Coder 2`).
    pub who: String,
    /// `provider/model`.
    pub model: String,
    pub tokens: u64,
    /// USD; 0 when the model's price is unknown.
    pub cost_usd: f64,
}

/// A decision step's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub step: String,
    pub yes: bool,
    pub reason: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A workdir as the run uses it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunWorkdir {
    pub id: String,
    pub name: String,
    /// Where steps work (the worktree, or the folder itself).
    pub path: PathBuf,
    /// The run's branch (worktrees).
    #[serde(default)]
    pub branch: Option<String>,
    /// The folder the worktree came from.
    pub repo: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub issue: String,
    pub created: u64,
    pub status: RunStatus,
    /// Pipeline id.
    #[serde(default)]
    pub pipeline: String,
    #[serde(default)]
    pub pipeline_name: String,
    /// The pipeline as it was when the run started (edits apply to new runs).
    #[serde(default)]
    pub def: Option<PipelineDef>,
    /// One per step, in order.
    pub stages: Vec<Stage>,
    /// Index of the step to run next.
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub workdirs: Vec<RunWorkdir>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    /// Step id → what the step reported (agent summaries, command output tails).
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    #[serde(default)]
    pub renders: Vec<Render>,
    /// Changes the user asked for at approvals, oldest first.
    #[serde(default)]
    pub feedback: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<CoderTask>,
    /// An orchestrating step verified its work (builds/tests pass).
    #[serde(default)]
    pub verified: bool,
    /// What it ran to verify.
    #[serde(default)]
    pub checks: String,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub usage: Vec<AgentUsage>,
    #[serde(default)]
    pub log: Vec<LogLine>,
}

impl Run {
    pub fn new(issue: &str, def: &PipelineDef) -> Self {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            id: format!("run{}-{:04}", t.as_millis(), t.subsec_nanos() % 10_000),
            issue: issue.to_string(),
            created: now(),
            status: RunStatus::Running,
            pipeline: def.id.clone(),
            pipeline_name: def.name.clone(),
            def: Some(def.clone()),
            stages: def
                .steps
                .iter()
                .map(|s| Stage {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    kind: s.kind.as_str().to_string(),
                    status: StageStatus::Pending,
                    started: 0,
                    finished: 0,
                    summary: String::new(),
                })
                .collect(),
            cursor: 0,
            workdirs: Vec::new(),
            decisions: Vec::new(),
            notes: BTreeMap::new(),
            renders: Vec::new(),
            feedback: Vec::new(),
            tasks: Vec::new(),
            verified: false,
            checks: String::new(),
            pr_url: None,
            error: None,
            tokens: 0,
            cost_usd: 0.,
            usage: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id == id)
    }

    /// The step running or waiting now, else the next one to run.
    pub fn current_stage(&self) -> Option<&Stage> {
        self.stages
            .iter()
            .find(|s| matches!(s.status, StageStatus::Running | StageStatus::Waiting))
            .or_else(|| self.stages.get(self.cursor))
    }

    pub fn set_stage(&mut self, idx: usize, status: StageStatus, summary: impl Into<String>) {
        let Some(s) = self.stages.get_mut(idx) else { return };
        if status == StageStatus::Running {
            s.started = now();
            s.finished = 0;
        }
        if matches!(status, StageStatus::Done | StageStatus::Skipped | StageStatus::Failed) {
            s.finished = now();
        }
        s.status = status;
        let summary = summary.into();
        if !summary.is_empty() || status == StageStatus::Running {
            s.summary = summary;
        }
    }

    pub fn workdir(&self, id: &str) -> Option<&RunWorkdir> {
        self.workdirs.iter().find(|w| w.id == id)
    }

    /// Run directory (renders, artifacts).
    pub fn dir(&self) -> PathBuf {
        runs_dir().join(&self.id)
    }

    pub fn save(&self) -> std::io::Result<()> {
        write_json(&runs_dir().join(format!("{}.json", self.id)), self)
    }

    /// Runs of older versions had fixed stages without ids or names.
    pub(crate) fn upgrade(&mut self) {
        for s in self.stages.iter_mut() {
            if s.id.is_empty() {
                s.id = s.kind.clone();
            }
            if s.name.is_empty() {
                let mut c = s.kind.replace('_', " ");
                if let Some(f) = c.get_mut(..1) {
                    f.make_ascii_uppercase();
                }
                s.name = c;
            }
        }
        if self.pipeline_name.is_empty() {
            self.pipeline_name = "Mockup → GPUI build".into();
        }
        // The id is the file name, not stored in the pipeline itself.
        if let Some(d) = self.def.as_mut()
            && d.id.is_empty()
        {
            d.id = self.pipeline.clone();
        }
    }
}

pub fn runs_dir() -> PathBuf {
    data_dir().join("runs")
}

pub fn load_run(id: &str) -> Option<Run> {
    if id.contains('/') || id.contains("..") {
        return None;
    }
    let b = std::fs::read(runs_dir().join(format!("{id}.json"))).ok()?;
    let mut run: Run = serde_json::from_slice(&b).ok()?;
    run.upgrade();
    Some(run)
}

/// `har-12-add-an-export-button` style slug for branches.
pub fn slug(key: &str, title: &str) -> String {
    let mut s: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(40).collect();
    let s = s.trim_end_matches('-');
    let key = key.to_lowercase();
    if s.is_empty() { key } else { format!("{key}-{s}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_keys_increase() {
        let dir = std::env::temp_dir().join(format!("harness-issues-{}", std::process::id()));
        let path = dir.join("issues.json");
        let a = create_issue_in(&path, "First", "", "", "p").unwrap();
        let b = create_issue_in(&path, "Second", "body", "Bug", "p").unwrap();
        assert_eq!((a.key.as_str(), b.key.as_str()), ("HAR-1", "HAR-2"));
        assert_eq!(b.kind, "Bug");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn comments_roundtrip() {
        let dir = std::env::temp_dir().join(format!("harness-comments-{}", std::process::id()));
        let path = dir.join("issues.json");
        let issue = create_issue_in(&path, "First", "", "", "p").unwrap();
        assert!(add_comment_in(&path, &issue.key, "me", "   ", vec![]).unwrap().is_none());
        assert!(add_comment_in(&path, "HAR-99", "me", "hi", vec![]).unwrap().is_none());
        let a = add_comment_in(&path, &issue.key, "me", " hi @Build ", vec!["Build".into()])
            .unwrap()
            .unwrap();
        let b = add_comment_in(&path, &issue.key, "me", "second", vec![]).unwrap().unwrap();
        assert_eq!((a.id, b.id, a.text.as_str()), (1, 2, "hi @Build"));
        let back = read_issues(&path).items.remove(0);
        assert_eq!(back.comments, vec![a, b.clone()]);
        assert!(delete_comment_in(&path, &issue.key, 1).unwrap());
        assert_eq!(read_issues(&path).items[0].comments, vec![b]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn old_issues_have_no_comments() {
        let v = r#"{"key":"HAR-1","title":"t","created":1,"status":"backlog"}"#;
        let i: Issue = serde_json::from_str(v).unwrap();
        assert!(i.comments.is_empty());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("HAR-3", "Add an Export button!"), "har-3-add-an-export-button");
        assert_eq!(slug("HAR-3", "Кнопка"), "har-3");
    }

    #[test]
    fn run_roundtrip() {
        let def = crate::config::legacy_default();
        let mut r = Run::new("HAR-1", &def);
        r.set_stage(0, StageStatus::Done, "needs mockup");
        let v = serde_json::to_string(&r).unwrap();
        let mut back: Run = serde_json::from_str(&v).unwrap();
        back.upgrade();
        assert_eq!(back, r);
        assert_eq!(back.stage("triage").unwrap().summary, "needs mockup");
    }

    #[test]
    fn old_runs_upgrade() {
        let v = r#"{"id":"r","issue":"HAR-1","created":1,"status":"done","stages":[{"kind":"pull_request","status":"done"}],"needs_design":true}"#;
        let mut r: Run = serde_json::from_str(v).unwrap();
        r.upgrade();
        assert_eq!(r.stages[0].id, "pull_request");
        assert_eq!(r.stages[0].name, "Pull request");
    }
}
