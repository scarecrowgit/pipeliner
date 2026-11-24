//! What the runner asks of the world, as small traits: the engine implements them over its
//! store, model client and catalog; tests use fakes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent::{AgentEnv, AgentOutcome, AgentSpec, Finish, FinishTool, Input, StopCause};
use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use base::process::Cmd;
use model::{
    AgentEvent, BlobId, IssueKey, IssueStatus, Link, ModelRef, NodeId, Outcome, RunId, Runnable, RunnableRef,
    SessionId, ToolAccess, ToolGrants, Usage,
};
use serde_json::json;

/// The stored session a pipeline agent works in.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionOpen {
    pub title: String,
    pub runnable: RunnableRef,
    pub model: ModelRef,
    pub run: RunId,
    pub node: NodeId,
    pub variant: u32,
    pub issue: Option<IssueKey>,
    /// The folder it works in, when it has one.
    pub workdir: Option<PathBuf>,
    /// The checkout a worktree was cut from.
    pub source: Option<PathBuf>,
    pub branch: Option<String>,
}

/// Running agent sessions: what a node's agent, and a router's question, run on.
pub trait AgentSessions: Send + Sync {
    /// Creates the session and returns its id.
    fn open(&self, open: &SessionOpen) -> Result<SessionId>;
    /// What the session and every session below it spent so far.
    fn spent(&self, session: &SessionId) -> f64;
    /// Whether the session holds a conversation to continue.
    fn has_conversation(&self, session: &SessionId) -> bool;
    /// What the session spent outside any run: turns it took after its step, such as answers
    /// to comments that mentioned it. A step's own budget leaves them out.
    fn spent_outside_runs(&self, _session: &SessionId) -> f64 {
        0.0
    }
    /// Runs the agent loop until it stops, after taking an agent slot (the wait for one ends
    /// with a cancelled outcome when `cancel` fires). Every event is handed to `on_event`
    /// with the session it belongs to.
    fn run(
        &self,
        spec: &AgentSpec,
        env: &AgentEnv,
        input: Input,
        on_event: &mut (dyn FnMut(&SessionId, &AgentEvent) + Send),
        cancel: &CancelToken,
    ) -> AgentOutcome;
    /// The library agent or orchestrator behind a reference.
    fn runnable(&self, reference: &RunnableRef) -> Option<Runnable>;
    /// The model used when neither the node nor its library agent names one.
    fn default_model(&self) -> ModelRef;
}

/// A router rule's question.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub run: RunId,
    pub node: NodeId,
    pub issue: Option<IssueKey>,
    pub model: ModelRef,
    pub question: String,
    /// What the item says: its title, text and issue fields.
    pub item: String,
    /// The folder the model may read; `None` leaves it with no tools.
    pub workdir: Option<PathBuf>,
    pub max_steps: u32,
}

/// What the model said, before it is read as yes or no.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    /// The answer as given; the router accepts only `yes` or `no`.
    pub answer: String,
    pub reason: String,
    pub cost_usd: f64,
    pub usage: Usage,
    pub session: Option<SessionId>,
}

/// Asks a model a yes/no question with read-only tools.
pub trait Questions: Send + Sync {
    fn ask(&self, question: &Question, cancel: &CancelToken) -> Result<Answer>;
}

/// The tool a router question finishes with.
pub const ANSWER_TOOL: &str = "answer";

/// [`Questions`] over [`AgentSessions`]: a short read-only session that finishes with
/// [`ANSWER_TOOL`].
pub struct SessionQuestions(pub Arc<dyn AgentSessions>);

impl Questions for SessionQuestions {
    fn ask(&self, q: &Question, cancel: &CancelToken) -> Result<Answer> {
        let sessions = &self.0;
        let model = if q.model.is_empty() { sessions.default_model() } else { q.model.clone() };
        if model.is_empty() {
            bail!("The router question has no model and none is configured");
        }
        let runnable = RunnableRef::pipeline(q.run.as_str());
        let open = SessionOpen {
            title: format!("Question: {}", base::fmt::truncate_chars(base::fmt::first_line(&q.question), 60)),
            runnable: runnable.clone(),
            model: model.clone(),
            run: q.run.clone(),
            node: q.node.clone(),
            variant: 0,
            issue: q.issue.clone(),
            workdir: q.workdir.clone(),
            source: None,
            branch: None,
        };
        let session = sessions.open(&open)?;
        let spec = AgentSpec {
            name: "Router".into(),
            runnable,
            role: "You decide one yes/no question about a work item for a pipeline. Read what you need \
                   (you cannot change anything), then call `answer` with `yes` or `no` and one sentence of reason."
                .into(),
            model,
            reasoning: Default::default(),
            tools: ToolGrants { access: ToolAccess::ReadOnly, ..Default::default() },
            skills: Default::default(),
            spawn: Default::default(),
            templates: Vec::new(),
            budget_usd: None,
            max_steps: if q.max_steps == 0 { 12 } else { q.max_steps },
            finish: Finish::Tool(FinishTool {
                name: ANSWER_TOOL.into(),
                description: "Give the answer: `yes` or `no`, and why in one sentence.".into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string", "enum": ["yes", "no"] },
                        "reason": { "type": "string" }
                    },
                    "required": ["answer", "reason"]
                }),
            }),
            limits: Default::default(),
        };
        let env = AgentEnv {
            session: session.clone(),
            workdir: q.workdir.clone(),
            read_only: true,
            run: Some(q.run.clone()),
            node: Some(q.node.clone()),
            ..Default::default()
        };
        let input = Input::Item(format!("{}\n\n## The item\n\n{}", q.question.trim(), q.item.trim()));
        let outcome = sessions.run(&spec, &env, input, &mut |_, _| {}, cancel);
        match &outcome.stop {
            StopCause::Finished => {}
            StopCause::Cancelled => bail!("cancelled"),
            StopCause::Failed { error } => bail!("the model could not answer: {error}"),
            other => bail!("the model stopped without answering ({other:?})"),
        }
        let payload = outcome.payload.ok_or_else(|| anyhow!("the model stopped without calling `{ANSWER_TOOL}`"))?;
        let text = |key: &str| payload.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        Ok(Answer {
            answer: text("answer"),
            reason: text("reason"),
            cost_usd: outcome.totals.cost_usd,
            usage: outcome.totals.usage,
            session: Some(session),
        })
    }
}

/// A rendered picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Turns an HTML file into a PNG.
pub trait Renderer: Send + Sync {
    fn render(&self, html: &Path, cancel: &CancelToken) -> Result<Picture>;
}

/// Screenshots with headless Firefox at the size the mockup names in a `render size: WxH`
/// comment (1440×960 otherwise).
pub struct Firefox {
    /// Where profiles and screenshots are made; removed after each render.
    pub scratch: PathBuf,
}

const RENDER_TIMEOUT: Duration = Duration::from_secs(90);

impl Renderer for Firefox {
    fn render(&self, html: &Path, cancel: &CancelToken) -> Result<Picture> {
        if !on_path("firefox") {
            bail!("Rendering needs Firefox on the PATH for headless screenshots; install it or remove the render node");
        }
        let text = std::fs::read_to_string(html).map_err(|e| anyhow!("reading {}: {e}", html.display()))?;
        let (width, height) = render_size(&text);
        let dir = self.scratch.join(format!("render-{}", base::id::new_id()));
        let profile = dir.join("profile");
        std::fs::create_dir_all(&profile)?;
        let png = dir.join("shot.png");
        let out = Cmd::new("firefox")
            .args(["--headless", "--no-remote", "-profile"])
            .arg(&profile)
            .arg(format!("--window-size={width},{height}"))
            .arg("--screenshot")
            .arg(&png)
            .arg(format!("file://{}", html.display()))
            // A library path set for the app breaks Firefox's bundled NSS.
            .env_remove("LD_LIBRARY_PATH")
            .timeout(RENDER_TIMEOUT)
            .run(cancel);
        let bytes = std::fs::read(&png);
        let _ = std::fs::remove_dir_all(&dir);
        let out = out?;
        match bytes {
            Ok(png) if !png.is_empty() => Ok(Picture { png, width, height }),
            _ => bail!("Firefox made no screenshot of {}: {}", html.display(), tail(&out.combined(), 400)),
        }
    }
}

/// `render size: 1440x960` from a mockup's head comment; 1440×960 without one.
pub fn render_size(html: &str) -> (u32, u32) {
    html.find("render size:")
        .and_then(|i| {
            let rest = html[i + 12..].trim_start();
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == 'x')).unwrap_or(rest.len());
            let (w, h) = rest[..end].split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .filter(|&(w, h): &(u32, u32)| (200..=4000).contains(&w) && (200..=4000).contains(&h))
        .unwrap_or((1440, 960))
}

fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path)
        .any(|dir| std::fs::metadata(dir.join(program)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// The last `max` characters of `text`.
pub(crate) fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let skip = count - max;
    format!("…{}", text.chars().skip(skip).collect::<String>())
}

/// A report for the Inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxPost {
    pub run: RunId,
    pub title: String,
    /// Markdown.
    pub summary: String,
    pub outcome: Outcome,
    /// "Mockup → GPUI build · Pull request".
    pub source: String,
    pub issue: Option<IssueKey>,
    pub session: Option<SessionId>,
    pub links: Vec<Link>,
}

/// The app's own records: the Inbox and the blob store.
pub trait Records: Send + Sync {
    fn post(&self, post: &InboxPost) -> Result<()>;
    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId>;
}

/// The issue tracker.
pub trait Tracker: Send + Sync {
    /// Moves the issue and comments on it, as the pipeline.
    fn update_issue(&self, key: &IssueKey, status: Option<IssueStatus>, comment: &str) -> Result<()>;
    /// Notes a pull request opened for the issue.
    fn pr_opened(&self, key: &IssueKey, url: &str) -> Result<()>;
}
