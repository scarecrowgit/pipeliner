//! An in-memory [`Api`] for previews and tests: the Acme Payments world of the mockups.
//!
//! [`FixtureApi`] serves a believable data set, records every command it receives, applies the
//! simple ones to its own state and emits the matching events, and replays scripted
//! [`AgentEvent`] streams so a chat preview shows streaming. Named scenarios start each screen's
//! preview from the state of its mockup:
//!
//! ```ignore
//! let api = Arc::new(FixtureApi::scenario("chat-session").unwrap_or_default());
//! let hub = Hub::new(api.clone(), cx);
//! if let Some((route, _)) = fixtures::start_route("chat-session") { hub::navigate(route, cx) }
//! api.start_scripts(); // streams, for "chat-streaming"
//! ```

mod api_admin;
mod api_chat;
mod api_work;
mod data_catalog;
mod data_inbox;
mod data_issues;
mod data_ops;
mod data_runs;
mod data_sessions;
mod data_transcript;
pub mod script;
mod search;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::api::{
    AgentEvent, Api, Autopilot, ChildDir, Event, GraphDef, HomeStats, InboxItem, Issue, IssueEvent, IssueKey, McpServerInfo,
    Millis, Need, Period, PermissionRequest, PermissionRule, PipelineId, PipelineVersion, Project, RepoFacts, Review,
    Run, RunId, RunLogLine, Runnable, RunnableActivity, RunnableRef, Runtime, Session, SessionId, Settings,
    SkillInContext, Skill, SpendBucket, SpendByRunnable, SpendToday, StoredMessage, TodoItem, ToolDisplay, FileChange,
    UserMessage, WorkdirEntry, Working,
};
use crate::router::{InboxFocus, Overlay, Route};

/// A minute in milliseconds, for times relative to the world's clock.
pub const MINUTE: Millis = 60_000;
/// An hour in milliseconds.
pub const HOUR: Millis = 60 * MINUTE;
/// A day in milliseconds.
pub const DAY: Millis = 24 * HOUR;

/// Every named scenario, for previews and the test that loads them all.
pub const SCENARIOS: [&str; 12] = [
    "home",
    "sidebar",
    "chat-session",
    "chat-streaming",
    "chat-history",
    "new-chat",
    "inbox-review",
    "issue-board",
    "issue-comments",
    "agents",
    "pipeline-editor",
    "empty",
];

/// Where a scenario's preview starts, and which overlay is open.
pub fn start_route(name: &str) -> Option<(Route, Option<Overlay>)> {
    let route = match name {
        "home" | "sidebar" | "empty" => Route::Home,
        "chat-session" | "chat-streaming" => Route::Chat { session: SessionId::new(data_sessions::FIX_1423) },
        "chat-history" => return Some((Route::Chat { session: SessionId::new(data_sessions::FIX_1423) }, Some(Overlay::Palette))),
        "new-chat" => Route::NewChat,
        "inbox-review" => Route::Inbox { item: Some(InboxFocus::Review(data_inbox::REVIEW_1423.into())) },
        "issue-board" => Route::Issues { issue: Some(IssueKey::new("PAY-1440")) },
        "issue-comments" => Route::Issues { issue: Some(IssueKey::new("PAY-1427")) },
        "agents" => Route::Agents { runnable: Some(RunnableRef::orchestrator("payments-lead")) },
        "pipeline-editor" => Route::Editor { pipeline: PipelineId::new("pay-triage"), node: Some("fixer".into()) },
        _ => return None,
    };
    Some((route, None))
}

/// How scripted streams are played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Every event at once, on the calling thread: for tests.
    Instant,
    /// One event per interval, from a helper thread: for previews.
    Every(Duration),
}

/// A command the fixture received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    /// The [`Api`] method: `pin_session`.
    pub name: &'static str,
    /// Its arguments, as text.
    pub detail: String,
}

/// What a session's context panel shows besides the session itself.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    pub todos: Vec<TodoItem>,
    pub files: Vec<FileChange>,
    pub skills: Vec<SkillInContext>,
    pub mcp: Vec<McpServerInfo>,
    pub repo: Option<RepoFacts>,
    pub queued: Vec<UserMessage>,
}

/// Home's numbers.
#[derive(Clone, Debug, Default)]
pub struct OpsData {
    pub stats: HashMap<Period, HomeStats>,
    pub needs: Vec<Need>,
    pub working: Vec<Working>,
    pub spend_today: SpendToday,
    pub buckets: HashMap<Period, Vec<SpendBucket>>,
    pub by_runnable: HashMap<Period, Vec<SpendByRunnable>>,
}

/// The fixture's whole state. Public so tests and previews can adjust it.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub now: Millis,
    pub sessions: Vec<Session>,
    pub extras: HashMap<SessionId, Extras>,
    pub messages: HashMap<SessionId, Vec<StoredMessage>>,
    /// What finished tool calls showed live, by session and tool-use id.
    pub tool_displays: HashMap<SessionId, HashMap<String, ToolDisplay>>,
    pub permissions: Vec<PermissionRequest>,
    pub rules: Vec<PermissionRule>,
    pub projects: Vec<Project>,
    pub settings: Settings,
    pub issues: Vec<Issue>,
    pub issue_events: HashMap<IssueKey, Vec<IssueEvent>>,
    /// The highest seq deleted from each feed, so it is not handed out again.
    pub deleted_seqs: HashMap<IssueKey, u64>,
    pub autopilots: Vec<Autopilot>,
    pub pipelines: Vec<GraphDef>,
    pub versions: HashMap<PipelineId, Vec<PipelineVersion>>,
    pub runs: Vec<Run>,
    pub logs: HashMap<RunId, Vec<RunLogLine>>,
    pub reviews: Vec<Review>,
    pub inbox: Vec<InboxItem>,
    pub runnables: Vec<Runnable>,
    pub activity: Vec<(RunnableRef, RunnableActivity)>,
    pub skills: Vec<Skill>,
    pub ops: OpsData,
    pub runtimes: Vec<Runtime>,
    pub workdirs: Vec<WorkdirEntry>,
    pub worktrees: Vec<WorkdirEntry>,
    /// Folders path completion finds, anywhere: `list_dir` returns the children of a folder.
    pub dirs: Vec<ChildDir>,
    pub next_id: u64,
}

impl World {
    /// The Acme Payments world of the mockups, with times relative to `now`.
    pub fn acme(now: Millis) -> Self {
        let mut world = World { now, next_id: 1, ..World::default() };
        data_issues::fill(&mut world);
        data_sessions::fill(&mut world);
        data_transcript::fill(&mut world);
        data_runs::fill(&mut world);
        data_inbox::fill(&mut world);
        data_catalog::fill(&mut world);
        data_ops::fill(&mut world);
        data_sessions::dirs(&mut world);
        // Headers count what is stored, as the engine's do.
        for session in &mut world.sessions {
            session.message_count = world.messages.get(&session.id).map_or(0, |messages| messages.len() as u32);
        }
        world
    }

    /// A fresh install: one project, nothing else.
    pub fn empty(now: Millis) -> Self {
        let mut world = World { now, next_id: 1, ..World::default() };
        data_issues::projects(&mut world);
        world.projects.truncate(1);
        world.settings.current_project = world.projects.first().map(|project| project.id.clone());
        data_ops::runtimes(&mut world);
        world
    }

    /// A fresh id with a prefix: `chat-7`.
    pub fn fresh_id(&mut self, prefix: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("{prefix}-{id}")
    }

    /// A session by id.
    pub fn session(&self, id: &SessionId) -> Option<&Session> {
        self.sessions.iter().find(|session| &session.id == id)
    }

    /// A session to change, or an error naming it.
    pub fn session_mut(&mut self, id: &SessionId) -> Result<&mut Session> {
        match self.sessions.iter_mut().find(|session| &session.id == id) {
            Some(session) => Ok(session),
            None => bail!("no chat {id}"),
        }
    }

    /// An issue to change, or an error naming it.
    pub fn issue_mut(&mut self, key: &IssueKey) -> Result<&mut Issue> {
        match self.issues.iter_mut().find(|issue| &issue.key == key) {
            Some(issue) => Ok(issue),
            None => bail!("no issue {key}"),
        }
    }

    /// A runnable to change, or an error naming it.
    pub fn runnable_mut(&mut self, reference: &RunnableRef) -> Result<&mut Runnable> {
        match self.runnables.iter_mut().find(|runnable| &runnable.reference == reference) {
            Some(runnable) => Ok(runnable),
            None => bail!("no agent or pipeline {reference}"),
        }
    }
}

type Subscribers = Arc<Mutex<Vec<async_channel::Sender<Event>>>>;

/// The fixture backend. Cheap to share behind an `Arc`.
pub struct FixtureApi {
    world: Mutex<World>,
    subscribers: Subscribers,
    recorded: Mutex<Vec<Recorded>>,
    failing: Mutex<HashSet<String>>,
    pace: Mutex<Pace>,
    /// Events [`FixtureApi::start_scripts`] plays.
    scripts: Mutex<Vec<Event>>,
}

impl Default for FixtureApi {
    fn default() -> Self {
        Self::new(World::acme(base::time::now_ms()))
    }
}

impl FixtureApi {
    /// A fixture over `world`, playing scripts instantly.
    pub fn new(world: World) -> Self {
        Self {
            world: Mutex::new(world),
            subscribers: Arc::default(),
            recorded: Mutex::default(),
            failing: Mutex::default(),
            pace: Mutex::new(Pace::Instant),
            scripts: Mutex::default(),
        }
    }

    /// The state of a mockup, by name (see [`SCENARIOS`]).
    pub fn scenario(name: &str) -> Option<Self> {
        let now = base::time::now_ms();
        let api = match name {
            "empty" => Self::new(World::empty(now)),
            "chat-streaming" => {
                let api = Self::new(World::acme(now));
                api.set_pace(Pace::Every(Duration::from_millis(60)));
                let session = SessionId::new(data_sessions::FIX_1423);
                let events = api.with_world(|world| {
                    // The script is built on a copy: the preview loads the transcript as it is
                    // before the reply, then watches the reply stream in.
                    let events = script::after_permission(&mut world.clone(), &session);
                    world.permissions.retain(|request| request.session != session);
                    if let Ok(chat) = world.session_mut(&session) {
                        chat.state = crate::api::SessionState::Running;
                    }
                    events
                });
                lock(&api.scripts).extend(events);
                api
            }
            _ if SCENARIOS.contains(&name) => Self::new(World::acme(now)),
            _ => return None,
        };
        Some(api)
    }

    /// Reads or changes the state directly. Emits nothing.
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut lock(&self.world))
    }

    /// Sets how scripted streams are played.
    pub fn set_pace(&self, pace: Pace) {
        *lock(&self.pace) = pace;
    }

    /// Every command received so far, oldest first.
    pub fn recorded(&self) -> Vec<Recorded> {
        lock(&self.recorded).clone()
    }

    /// The names of the commands received so far.
    pub fn recorded_names(&self) -> Vec<&'static str> {
        lock(&self.recorded).iter().map(|command| command.name).collect()
    }

    /// Makes the next call of `command` (an [`Api`] method name) fail. Commands and
    /// `search_sessions` honour it.
    pub fn fail_next(&self, command: &str) {
        lock(&self.failing).insert(command.to_string());
    }

    /// The app's directories in this world: home is `/home/me`, the rest derived from it as
    /// the engine would. Nothing is created on disk.
    pub fn paths(&self) -> base::paths::Paths {
        base::paths::Paths::resolve(|name| (name == "HOME").then(|| data_sessions::HOME.to_string()))
    }

    /// Subscribers whose receiver is still alive: a dropped hub no longer counts.
    pub fn subscriber_count(&self) -> usize {
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|sender| !sender.is_closed());
        subscribers.len()
    }

    /// Sends an event to every subscriber.
    pub fn emit(&self, event: Event) {
        send_all(&self.subscribers, event);
    }

    /// Sends events to every subscriber, in order.
    pub fn emit_all(&self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.emit(event);
        }
    }

    /// Plays agent events of a session at the current [`Pace`].
    pub fn play(&self, session: &SessionId, events: Vec<AgentEvent>) {
        let events: Vec<Event> =
            events.into_iter().map(|event| Event::Agent { session: session.clone(), event }).collect();
        self.play_events(events);
    }

    /// Plays any events at the current [`Pace`].
    pub fn play_events(&self, events: Vec<Event>) {
        let pace = *lock(&self.pace);
        match pace {
            Pace::Instant => self.emit_all(events),
            Pace::Every(interval) => {
                let subscribers = self.subscribers.clone();
                std::thread::spawn(move || {
                    for event in events {
                        std::thread::sleep(interval);
                        send_all(&subscribers, event);
                    }
                });
            }
        }
    }

    /// Plays the scenario's scripted streams (the reply streaming into "chat-streaming").
    pub fn start_scripts(&self) {
        let events = std::mem::take(&mut *lock(&self.scripts));
        self.play_events(events);
    }

    /// Records a command and fails it if a test asked for that.
    fn record(&self, name: &'static str, detail: impl Into<String>) -> Result<()> {
        lock(&self.recorded).push(Recorded { name, detail: detail.into() });
        if lock(&self.failing).remove(name) {
            bail!("the fixture was told to fail {name}");
        }
        Ok(())
    }

    /// Fails a query if a test asked for that; queries are not recorded.
    fn fail_if_told(&self, name: &str) -> Result<()> {
        if lock(&self.failing).remove(name) {
            bail!("the fixture was told to fail {name}");
        }
        Ok(())
    }

    fn world(&self) -> MutexGuard<'_, World> {
        lock(&self.world)
    }

    fn blob_dir(&self) -> PathBuf {
        std::env::temp_dir().join("harness-fixture-blobs")
    }
}

impl Api for FixtureApi {
    fn subscribe(&self) -> async_channel::Receiver<Event> {
        let (sender, receiver) = async_channel::unbounded();
        lock(&self.subscribers).push(sender);
        receiver
    }
}

fn send_all(subscribers: &Subscribers, event: Event) {
    lock(subscribers).retain(|sender| sender.try_send(event.clone()).is_ok());
}

/// Locks a mutex; a panic elsewhere while holding it leaves data that is still usable here.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
