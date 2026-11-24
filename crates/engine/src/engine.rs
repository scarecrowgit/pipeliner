//! [`Engine`]: what it holds, how it starts (recovery included) and stops, and the helpers
//! every part of the API shares.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent::Limits;
use anyhow::{Context as _, Result, anyhow};
use base::paths::Paths;
use base::time::{Clock, SystemClock};
use catalog::Catalog;
use llm::LlmClient;
use model::{Event, Session, SessionId, SessionState};
use store::Store;
use store::import_v1::{ImportOptions, ImportReport};

use crate::api::Api;
use crate::bus::{Bus, lock};
use crate::gate::Asks;
use crate::live::{Live, Slots};
use crate::mcp::McpHub;

/// The error of every method whose work is not connected yet.
pub const NOT_WIRED: &str = "Pipelines are not wired yet";

/// Marker of a session whose turn is in progress; found again after a crash.
pub(crate) fn running_marker(id: &SessionId) -> String {
    format!("running:{id}")
}

/// How long [`crate::api::OpsApi::shutdown`] waits for turns to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// What an engine runs with besides its paths, model client and clock.
#[derive(Clone, Default)]
pub struct EngineOptions {
    /// Patience and sizes of the agent loop; tests shrink the retry pauses. The compaction
    /// threshold is taken from the settings.
    pub limits: Limits,
    /// How pipelines run `gh`; the installed one when `None`.
    pub gh: Option<Arc<dyn vcs::pr::Gh + Send + Sync>>,
    /// How render actions take screenshots; headless Firefox when `None`.
    pub renderer: Option<Arc<dyn pipeline::runners::Renderer>>,
    /// Values pipelines must never let out of a job (the configured provider keys).
    pub secrets: Vec<String>,
}

impl std::fmt::Debug for EngineOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineOptions")
            .field("limits", &self.limits)
            .field("gh", &self.gh.as_ref().map(|_| "custom"))
            .field("renderer", &self.renderer.as_ref().map(|_| "custom"))
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// The engine: implements [`Api`] over the domain crates. Cheap to clone; every clone is the
/// same engine.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

/// Everything the engine shares between its threads.
pub(crate) struct Inner {
    pub paths: Paths,
    pub store: Store,
    pub llm: Arc<dyn LlmClient>,
    pub clock: Arc<dyn Clock>,
    pub bus: Bus,
    /// The catalog without any project, reloaded after every write.
    pub catalog: RwLock<Arc<Catalog>>,
    pub live: Live,
    pub asks: Asks,
    pub slots: Slots,
    pub mcp: McpHub,
    pub limits: Limits,
    /// Open pipeline runs and their drivers.
    pub runs: crate::pipelines::Runs,
    /// `gh` for pull requests.
    pub gh: Arc<dyn vcs::pr::Gh + Send + Sync>,
    /// Screenshots for render actions.
    pub renderer: Arc<dyn pipeline::runners::Renderer>,
    /// Values redacted from everything a pipeline job lets out.
    pub secrets: Vec<String>,
    /// Serialises "is a run open on this issue?" and starting one.
    pub issue_starts: Mutex<()>,
    /// Tests: snapshot writes fail.
    #[cfg(test)]
    pub fail_snapshots: AtomicBool,
    /// Serialises read-modify-write of session headers.
    headers: Mutex<()>,
    /// Turn threads, joined on shutdown.
    pub threads: Mutex<Vec<JoinHandle<()>>>,
    shut_down: AtomicBool,
}

impl Engine {
    /// Opens the engine over `paths` with the given model client and clock: opens the store,
    /// loads the catalog, and marks sessions that were running when the last process ended as
    /// interrupted.
    pub fn open(paths: Paths, llm: Arc<dyn LlmClient>, clock: Arc<dyn Clock>) -> Result<Engine> {
        Self::open_with(paths, llm, clock, EngineOptions::default())
    }

    /// [`Engine::open`] with options.
    pub fn open_with(paths: Paths, llm: Arc<dyn LlmClient>, clock: Arc<dyn Clock>, options: EngineOptions) -> Result<Engine> {
        let store = Store::open(&paths).context("opening the store")?;
        let catalog = Catalog::load(&paths, None);
        let renderer = options.renderer.clone().unwrap_or_else(|| {
            Arc::new(pipeline::runners::Firefox { scratch: paths.v2_dir().join("renders") })
        });
        let gh = options.gh.clone().unwrap_or_else(|| Arc::new(vcs::pr::Cli));
        let inner = Inner {
            runs: Default::default(),
            secrets: options.secrets.clone(),
            issue_starts: Mutex::new(()),
            #[cfg(test)]
            fail_snapshots: AtomicBool::new(false),
            gh,
            renderer,
            mcp: McpHub::new(paths.clone()),
            paths,
            store,
            llm,
            clock,
            bus: Bus::default(),
            catalog: RwLock::new(Arc::new(catalog)),
            live: Live::default(),
            asks: Asks::default(),
            slots: Slots::default(),
            limits: options.limits,
            headers: Mutex::new(()),
            threads: Mutex::new(Vec::new()),
            shut_down: AtomicBool::new(false),
        };
        let engine = Engine { inner: Arc::new(inner) };
        engine.inner.keep_queues()?;
        engine.inner.recover()?;
        crate::pipelines::recover(&engine.inner);
        Ok(engine)
    }

    /// The engine the app and the CLI run with: providers from `config.toml` and the
    /// environment, and the wall clock.
    pub fn open_configured(paths: Paths) -> Result<Engine> {
        let config = llm::Config::load(&paths)?;
        let secrets = config.providers.iter().filter_map(|p| p.api_key.clone()).filter(|k| !k.trim().is_empty()).collect();
        let options = EngineOptions { secrets, ..Default::default() };
        Self::open_with(paths, Arc::new(llm::Providers::new(config)), Arc::new(SystemClock), options)
    }

    /// Imports the old app's data into the store (see [`store::import_v1`]). A dry run reports
    /// what would be imported and writes nothing.
    pub fn import_v1(&self, dry_run: bool) -> Result<ImportReport> {
        let inner = &self.inner;
        let report = store::import_v1::import_v1(&inner.store, &inner.paths, &ImportOptions { dry_run })?;
        if !dry_run {
            inner.publish(Event::SpendChanged);
            inner.publish(Event::SettingsChanged);
        }
        Ok(report)
    }

    /// The paths the engine works in.
    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }
}

impl Api for Engine {
    fn subscribe(&self) -> async_channel::Receiver<Event> {
        self.inner.bus.subscribe()
    }
}

impl Inner {
    pub fn publish(&self, event: Event) {
        self.bus.publish(event);
    }

    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// The catalog without a project.
    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog.read().map_or_else(|e| e.into_inner().clone(), |c| c.clone())
    }

    /// The catalog as a session in `folder` sees it: the project's own agents included.
    pub fn catalog_for(&self, folder: Option<&Path>) -> Arc<Catalog> {
        match folder {
            Some(folder) => Arc::new(Catalog::load(&self.paths, Some(folder))),
            None => self.catalog(),
        }
    }

    /// Re-reads the catalog from disk and tells subscribers.
    pub fn reload_catalog(&self) {
        let fresh = Arc::new(Catalog::load(&self.paths, None));
        match self.catalog.write() {
            Ok(mut catalog) => *catalog = fresh,
            Err(poisoned) => *poisoned.into_inner() = fresh,
        }
        self.publish(Event::CatalogChanged);
    }

    pub fn session(&self, id: &SessionId) -> Result<Session> {
        self.store.session(id)?.ok_or_else(|| anyhow!("There is no chat {id}"))
    }

    /// Changes a session's header and publishes it. `None` when the session is gone.
    pub fn update_session(&self, id: &SessionId, change: impl FnOnce(&mut Session)) -> Result<Option<Session>> {
        let _guard = lock(&self.headers);
        let Some(mut session) = self.store.session(id)? else { return Ok(None) };
        change(&mut session);
        self.store.upsert_session(&session)?;
        self.publish(Event::SessionChanged { session: session.clone() });
        Ok(Some(session))
    }

    /// Sets a session's state, logging nothing when the session is gone.
    pub fn set_state(&self, id: &SessionId, state: SessionState) {
        let now = self.now();
        if let Err(error) = self.update_session(id, |s| {
            s.state = state;
            s.updated = s.updated.max(now);
        }) {
            self.notice_error(&format!("Could not save the chat's state: {error:#}"));
        }
    }

    /// Shows an error as a toast.
    pub fn notice_error(&self, text: &str) {
        self.publish(Event::Notice { level: model::NoticeLevel::Error, text: text.to_string() });
    }

    /// Saves every queue change to the store and puts back the queues kept before the last
    /// process ended; they are delivered with the session's next message.
    fn keep_queues(self: &Arc<Self>) -> Result<()> {
        let weak = Arc::downgrade(self);
        self.live.save_queues_with(Box::new(move |id, queue| {
            let Some(inner) = weak.upgrade() else { return };
            let queue: Vec<store::QueuedMessage> =
                queue.iter().map(|m| store::QueuedMessage { text: m.text.clone(), images: m.images.clone() }).collect();
            if let Err(error) = inner.store.set_queued_messages(id, &queue) {
                inner.notice_error(&format!("Could not keep the queued messages: {error:#}"));
            }
        }));
        for (id, queue) in self.store.queued_messages()? {
            let messages = queue.into_iter().map(|m| crate::api::UserMessage { text: m.text, images: m.images }).collect();
            self.live.restore(&id, messages);
        }
        Ok(())
    }

    /// Sessions that were running when the last process ended become failed, so they can be
    /// continued with retry.
    fn recover(&self) -> Result<()> {
        for state in [SessionState::Running, SessionState::Waiting] {
            let filter = store::SessionFilter { state: Some(state), ..Default::default() };
            for session in self.store.sessions(&filter)? {
                let marker = running_marker(&session.id);
                if state == SessionState::Running || self.store.marker(&marker)?.is_some() {
                    self.interrupted(&session.id);
                }
            }
        }
        Ok(())
    }

    /// Marks a session as interrupted: failed, and continued by retry.
    pub fn interrupted(&self, id: &SessionId) {
        self.set_state(id, SessionState::Failed);
        let _ = self.store.remove_marker(&running_marker(id));
    }

    pub fn is_shut_down(&self) -> bool {
        self.shut_down.load(Ordering::SeqCst)
    }

    /// Cancels every turn, waits a little for the threads, stops the MCP servers.
    pub fn shutdown(&self) {
        if self.shut_down.swap(true, Ordering::SeqCst) {
            return;
        }
        // Runs first: their agents become interrupted instances that resume later.
        self.runs.stop_all();
        self.live.cancel_all();
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        let threads = std::mem::take(&mut *lock(&self.threads));
        for handle in threads {
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        for id in self.live.running() {
            self.interrupted(&id);
        }
        self.mcp.shutdown();
    }
}
