//! Tests of the engine against a scripted model, temp directories and temp git repos. No
//! network, no real model, nothing outside the temp directory.

mod cancel;
mod chat;
mod other;
mod permission;
mod pipelines;
mod pipelines_more;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::Limits;
use async_channel::Receiver;
use base::paths::Paths;
use base::time::FixedClock;
use llm::ScriptedLlm;
use model::{AgentEvent, Event, Session, SessionId, SessionState, WorkMode};
use tempfile::TempDir;

use crate::api::{ChatApi, NewSession, UserMessage, WorkdirChoice};
use crate::{Api, Engine, EngineOptions};

/// 23 November 2026, noon UTC.
pub(crate) const NOW: i64 = 1_795_435_200_000;
/// Longest a test waits for something to happen.
const PATIENCE: Duration = Duration::from_secs(20);

pub(crate) struct Fx {
    pub dir: TempDir,
    pub paths: Paths,
    pub llm: Arc<ScriptedLlm>,
    pub clock: Arc<FixedClock>,
    pub engine: Engine,
    pub options: EngineOptions,
    events: Receiver<Event>,
    /// Every event received so far.
    pub log: Mutex<Vec<Event>>,
}

pub(crate) fn options() -> EngineOptions {
    EngineOptions {
        limits: Limits {
            retry_base: Duration::from_millis(1),
            retry_max: Duration::from_millis(4),
            ..Default::default()
        },
        ..Default::default()
    }
}

impl Fx {
    pub fn new() -> Self {
        Self::with(options())
    }

    /// An engine with these options.
    pub fn with(options: EngineOptions) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonical temp dir");
        let paths = Paths::under(&root.join("app"));
        let llm = Arc::new(ScriptedLlm::new());
        let clock = Arc::new(FixedClock::new(NOW));
        let engine = Engine::open_with(paths.clone(), llm.clone(), clock.clone(), options.clone()).expect("engine");
        let events = engine.subscribe();
        Self { dir, paths, llm, clock, engine, options, events, log: Mutex::new(Vec::new()) }
    }

    /// A second engine over the same directory, as after a restart, with a fresh model.
    pub fn reopen(&self) -> (Engine, Arc<ScriptedLlm>) {
        let llm = Arc::new(ScriptedLlm::new());
        let engine = Engine::open_with(self.paths.clone(), llm.clone(), self.clock.clone(), self.options.clone()).expect("engine");
        (engine, llm)
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().expect("canonical temp dir")
    }

    /// A git repo on `main` with one commit holding `files`.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let repo = self.root().join(name);
        std::fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        for (rel, text) in files {
            let path = repo.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    /// A chat working in `folder` as it is.
    pub fn chat_in(&self, folder: &Path) -> Session {
        let workdir = WorkdirChoice { path: folder.to_path_buf(), mode: WorkMode::InPlace, base: None, branch: None };
        self.engine.new_session(new_session(Some(workdir))).expect("new session")
    }

    pub fn say(&self, id: &SessionId, text: &str) {
        self.engine.send(id, UserMessage { text: text.into(), images: Vec::new() }).expect("send");
    }

    /// Receives events until one matches; returns it. Everything received lands in `log`.
    pub fn wait_for(&self, what: &str, matches: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    self.log.lock().expect("log").push(event.clone());
                    if matches(&event) {
                        return event;
                    }
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => panic!("no {what} within {PATIENCE:?}; got {:#?}", self.log.lock().expect("log")),
            }
        }
    }

    /// Waits until no turn runs in `id`, then takes in the events published so far.
    pub fn settle(&self, id: &SessionId) -> Session {
        let deadline = Instant::now() + PATIENCE;
        while self.engine.inner.live.is_running(id) {
            assert!(Instant::now() < deadline, "the turn of {id} did not end; events: {:#?}", self.log.lock().expect("log"));
            std::thread::sleep(Duration::from_millis(2));
        }
        self.drain();
        self.engine.inner.session(id).expect("session")
    }

    /// Takes in every event published so far.
    pub fn drain(&self) {
        while let Ok(event) = self.events.try_recv() {
            self.log.lock().expect("log").push(event);
        }
    }

    /// The agent events of one session, in order.
    pub fn agent_events(&self, id: &SessionId) -> Vec<AgentEvent> {
        self.log
            .lock()
            .expect("log")
            .iter()
            .filter_map(|e| match e {
                Event::Agent { session, event } if session == id => Some(event.clone()),
                _ => None,
            })
            .collect()
    }

    /// The states the session's header went through, without repeats.
    pub fn states(&self, id: &SessionId) -> Vec<SessionState> {
        let mut out: Vec<SessionState> = Vec::new();
        for event in self.log.lock().expect("log").iter() {
            if let Event::SessionChanged { session } = event
                && &session.id == id
                && out.last() != Some(&session.state)
            {
                out.push(session.state);
            }
        }
        out
    }
}

pub(crate) fn new_session(workdir: Option<WorkdirChoice>) -> NewSession {
    NewSession {
        runnable: None,
        mode: Default::default(),
        model: None,
        workdir,
        forced_skills: Vec::new(),
        issue: None,
    }
}

/// Runs git with an isolated config; panics when it fails; returns stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("start git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}
