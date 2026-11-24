//! The runner against temp git repos, with fakes for sessions, questions, rendering, the
//! Inbox, the tracker and `gh`.

mod actions;
mod agents;
mod pattern;
mod router;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use ::agent::{AgentEnv, AgentOutcome, AgentSpec, Input, StopCause, Totals};
use anyhow::Result;
use base::cancel::CancelToken;
use model::{
    AgentEvent, BlobId, GraphDef, Issue, IssueKey, IssueStatus, ModelRef, NodeDef, NodeKind, PipelineId, Runnable,
    RunnableRef, RunId, SessionId, WorkdirDef,
};
use serde_json::json;
use tempfile::TempDir;
use vcs::pr::{Gh, GhOutput};

use super::{
    AgentSessions, Answer, InboxPost, Picture, Question, Questions, Records, Renderer, Runner, RunnerConfig, Services,
    SessionOpen, Tracker,
};
use crate::api::{InstanceKey, Item, Job, JobNote, JobResult, JobSink, NodeOutput, NodeRunner, Upstream};

type Script = Box<dyn FnOnce(&AgentEnv, &Input) -> AgentOutcome + Send>;

#[derive(Default)]
pub(super) struct FakeSessions {
    scripts: Mutex<VecDeque<Script>>,
    pub runs: Mutex<Vec<(AgentSpec, AgentEnv, Input)>>,
    pub opened: Mutex<Vec<SessionOpen>>,
}

impl FakeSessions {
    pub fn then(&self, script: impl FnOnce(&AgentEnv, &Input) -> AgentOutcome + Send + 'static) {
        self.scripts.lock().unwrap().push_back(Box::new(script));
    }
}

impl AgentSessions for FakeSessions {
    fn open(&self, open: &SessionOpen) -> Result<SessionId> {
        let mut opened = self.opened.lock().unwrap();
        opened.push(open.clone());
        Ok(SessionId::new(format!("s{}", opened.len())))
    }

    fn spent(&self, _: &SessionId) -> f64 {
        0.0
    }

    fn run(
        &self,
        spec: &AgentSpec,
        env: &AgentEnv,
        input: Input,
        on_event: &mut (dyn FnMut(&SessionId, &AgentEvent) + Send),
        _: &CancelToken,
    ) -> AgentOutcome {
        self.runs.lock().unwrap().push((spec.clone(), env.clone(), input.clone()));
        on_event(&env.session, &AgentEvent::Usage { usage: Default::default(), cost_usd: 0.25, context_tokens: 0, context_window: 0 });
        let script = self.scripts.lock().unwrap().pop_front().expect("a scripted agent run");
        script(env, &input)
    }

    fn runnable(&self, _: &RunnableRef) -> Option<Runnable> {
        None
    }

    fn default_model(&self) -> ModelRef {
        ModelRef::parse("scripted/scripted-model")
    }
}

/// An agent that ends by calling `step_done`.
pub(super) fn finished(summary: &str) -> AgentOutcome {
    AgentOutcome {
        stop: StopCause::Finished,
        text: String::new(),
        payload: Some(json!({ "summary": summary })),
        totals: Totals { cost_usd: 0.25, ..Default::default() },
        files: Vec::new(),
    }
}

#[derive(Default)]
pub(super) struct FakeQuestions {
    pub answers: Mutex<VecDeque<&'static str>>,
    pub asked: Mutex<Vec<Question>>,
}

impl Questions for FakeQuestions {
    fn ask(&self, question: &Question, _: &CancelToken) -> Result<Answer> {
        self.asked.lock().unwrap().push(question.clone());
        let answer = self.answers.lock().unwrap().pop_front().expect("a scripted answer");
        Ok(Answer { answer: answer.into(), reason: "because".into(), cost_usd: 0.01, ..Default::default() })
    }
}

/// Renders a file as its own bytes.
pub(super) struct FakeRenderer;

impl Renderer for FakeRenderer {
    fn render(&self, html: &Path, _: &CancelToken) -> Result<Picture> {
        Ok(Picture { png: std::fs::read(html)?, width: 800, height: 600 })
    }
}

#[derive(Default)]
pub(super) struct FakeRecords {
    pub posts: Mutex<Vec<InboxPost>>,
    pub blobs: Mutex<Vec<Vec<u8>>>,
}

impl Records for FakeRecords {
    fn post(&self, post: &InboxPost) -> Result<()> {
        self.posts.lock().unwrap().push(post.clone());
        Ok(())
    }

    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        let mut blobs = self.blobs.lock().unwrap();
        blobs.push(bytes.to_vec());
        Ok(BlobId::new(format!("blob{}", blobs.len())))
    }
}

#[derive(Default)]
pub(super) struct FakeTracker {
    pub updates: Mutex<Vec<(IssueKey, Option<IssueStatus>, String)>>,
    pub prs: Mutex<Vec<String>>,
}

impl Tracker for FakeTracker {
    fn update_issue(&self, key: &IssueKey, status: Option<IssueStatus>, comment: &str) -> Result<()> {
        self.updates.lock().unwrap().push((key.clone(), status, comment.to_string()));
        Ok(())
    }

    fn pr_opened(&self, _: &IssueKey, url: &str) -> Result<()> {
        self.prs.lock().unwrap().push(url.to_string());
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct FakeGh {
    pub calls: Mutex<Vec<Vec<String>>>,
}

pub(super) const PR_URL: &str = "https://github.com/acme/app/pull/7";

impl Gh for FakeGh {
    fn run(&self, _: Option<&Path>, args: &[String], _: Option<&str>, _: &CancelToken) -> Result<GhOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = if args.get(1).map(String::as_str) == Some("list") { "[]".to_string() } else { format!("{PR_URL}\n") };
        Ok(GhOutput { success: true, stdout, stderr: String::new() })
    }
}

#[derive(Default)]
pub(super) struct Notes(pub Mutex<Vec<JobNote>>);

impl JobSink for Notes {
    fn note(&self, note: JobNote) {
        self.0.lock().unwrap().push(note);
    }
}

pub(super) struct Fx {
    pub dir: TempDir,
    pub sessions: Arc<FakeSessions>,
    pub questions: Arc<FakeQuestions>,
    pub records: Arc<FakeRecords>,
    pub tracker: Arc<FakeTracker>,
    pub gh: Arc<FakeGh>,
}

impl Fx {
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temp dir"),
            sessions: Arc::default(),
            questions: Arc::default(),
            records: Arc::default(),
            tracker: Arc::default(),
            gh: Arc::default(),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().expect("canonical")
    }

    pub fn worktrees(&self) -> PathBuf {
        self.root().join("worktrees")
    }

    /// A repo on `main` with one commit holding `files`.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let repo = self.root().join(name);
        std::fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        for (rel, text) in files {
            write(&repo, rel, text);
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    pub fn runner(&self, workdirs: Vec<WorkdirDef>) -> Runner {
        let graph = GraphDef {
            schema: 1,
            id: PipelineId::new("demo"),
            name: "Demo".into(),
            description: String::new(),
            limits: Default::default(),
            workdirs,
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        let services = Services {
            sessions: self.sessions.clone(),
            questions: self.questions.clone(),
            renderer: Arc::new(FakeRenderer),
            records: self.records.clone(),
            tracker: self.tracker.clone(),
            gh: self.gh.clone(),
        };
        Runner::new(&graph, services, RunnerConfig::new(self.worktrees(), self.root().join("state")))
    }
}

pub(super) fn workdir(id: &str, path: &Path, prefix: &str) -> WorkdirDef {
    WorkdirDef {
        id: id.into(),
        name: id.to_uppercase(),
        path: path.to_path_buf(),
        worktree: true,
        base_branch: "main".into(),
        branch_prefix: prefix.into(),
        ..Default::default()
    }
}

pub(super) fn issue() -> Issue {
    Issue {
        key: IssueKey::new("HAR-7"),
        project: Default::default(),
        title: "Fix the retry".into(),
        body: "It retries twice.".into(),
        status: Default::default(),
        priority: Default::default(),
        kind: Default::default(),
        owner: String::new(),
        repo: None,
        created: 0,
        updated: 0,
        filed_by: None,
        runner: None,
        triggers: Default::default(),
        run: None,
        run_count: 0,
        pr_url: None,
        images: Vec::new(),
    }
}

pub(super) fn job(id: &str, kind: NodeKind, variant: u32, attempt: u32) -> Job {
    Job {
        run: RunId::new("run1"),
        key: InstanceKey { node: id.into(), variant, attempt },
        node: NodeDef { id: id.into(), name: id.to_uppercase(), pos: Default::default(), kind },
        attached: Vec::new(),
        item: Item { title: "Fix the retry".into(), text: "It retries twice.".into(), issue: Some(issue()), source: Default::default() },
        upstream: Vec::new(),
        retry: None,
        answer: None,
        resume_session: None,
        budget_left_usd: None,
        test: false,
    }
}

/// What an earlier instance left, as a job's upstream.
pub(super) fn upstream(node: &str, variant: u32, output: &NodeOutput) -> Upstream {
    Upstream { key: InstanceKey { node: node.into(), variant, attempt: 1 }, name: node.to_uppercase(), output: output.clone() }
}

pub(super) fn run(runner: &Runner, job: &Job) -> (JobResult, Vec<JobNote>) {
    let notes = Notes::default();
    let result = runner.run(job, &notes, &CancelToken::new());
    (result, notes.0.into_inner().unwrap())
}

pub(super) fn output(result: &JobResult) -> &NodeOutput {
    match result {
        JobResult::Done { output, .. }
        | JobResult::Ask { output, .. }
        | JobResult::OverBudget { output }
        | JobResult::Failed { output, .. }
        | JobResult::Cancelled { output } => output,
    }
}

/// Runs git with an isolated config; panics when it fails; returns stdout.
pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("start git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(super) fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}
