//! The hub: owns the model entities, runs the event pump, and is reachable from anywhere in the
//! app through a global.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use base::paths::Paths;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task, WeakEntity};

use crate::api::{Api, ReviewId, RunId, SessionId, Target};
use crate::commands::Commands;
use crate::models::{
    CatalogModel, ChatIndex, InboxModel, IssuesModel, OpsModel, PermissionsModel, ReviewModel, RunModel, RunsModel,
    SearchModel, SessionModel,
};
use crate::notice::HubNotice;
use crate::queries::Queries;
use crate::router::{Route, Router};

/// Notices kept for views that open after one was emitted.
const RECENT_NOTICES: usize = 20;

/// The app's hub. Replacing it (a second [`Hub::new`]) drops the old one and stops its pump.
pub(crate) struct GlobalHub(pub(crate) Entity<Hub>);

impl Global for GlobalHub {}

/// The bridge between the engine and the views. Created once at start with [`Hub::new`].
/// Emits [`HubNotice`]s for toasts.
pub struct Hub {
    pub(crate) api: Arc<dyn Api>,
    pub(crate) commands: Commands,
    pub(crate) router: Entity<Router>,
    pub(crate) chat_index: Entity<ChatIndex>,
    pub(crate) search: Entity<SearchModel>,
    pub(crate) issues: Entity<IssuesModel>,
    pub(crate) runs: Entity<RunsModel>,
    pub(crate) inbox: Entity<InboxModel>,
    pub(crate) catalog: Entity<CatalogModel>,
    pub(crate) ops: Entity<OpsModel>,
    pub(crate) permissions: Entity<PermissionsModel>,
    pub(crate) sessions: HashMap<SessionId, WeakEntity<SessionModel>>,
    pub(crate) run_models: HashMap<RunId, WeakEntity<RunModel>>,
    pub(crate) reviews: HashMap<ReviewId, WeakEntity<ReviewModel>>,
    notices: VecDeque<HubNotice>,
    paths: Paths,
    _pump: Task<()>,
    _router: Subscription,
}

impl EventEmitter<HubNotice> for Hub {}

impl Hub {
    /// Creates the hub, subscribes to the engine's events and makes it reachable with
    /// [`hub`]. Call once, before any view asks for a model.
    pub fn new(api: Arc<dyn Api>, cx: &mut App) -> Entity<Hub> {
        let hub = cx.new(|cx: &mut Context<Hub>| {
            let router = cx.new(|_| Router::new(Route::Home));
            let events = api.subscribe();
            let pump = cx.spawn(async move |this, cx| crate::pump::run(events, this, cx).await);
            let router_subscription = cx.observe(&router, |hub: &mut Hub, router, cx| {
                let open = match router.read(cx).current() {
                    Route::Chat { session } => Some(session.clone()),
                    _ => None,
                };
                hub.chat_index.update(cx, |index, cx| index.set_open(open, cx));
            });
            Hub {
                commands: Commands::new(api.clone(), cx.weak_entity(), cx.background_executor()),
                router,
                chat_index: cx.new(|_| ChatIndex::new(api.clone())),
                search: cx.new(|_| SearchModel::new(api.clone())),
                issues: cx.new(|_| IssuesModel::new(api.clone())),
                runs: cx.new(|_| RunsModel::new(api.clone())),
                inbox: cx.new(|_| InboxModel::new(api.clone())),
                catalog: cx.new(|_| CatalogModel::new(api.clone())),
                ops: cx.new(|_| OpsModel::new(api.clone())),
                permissions: cx.new(|_| PermissionsModel::new(api.clone())),
                sessions: HashMap::new(),
                run_models: HashMap::new(),
                reviews: HashMap::new(),
                notices: VecDeque::new(),
                paths: Paths::from_env(),
                api,
                _pump: pump,
                _router: router_subscription,
            }
        });
        cx.set_global(GlobalHub(hub.clone()));
        hub
    }

    /// The engine behind the hub, for wiring that needs it directly (the binary's quit path).
    pub fn api(&self) -> &Arc<dyn Api> {
        &self.api
    }

    /// The command handle views call.
    pub fn commands(&self) -> &Commands {
        &self.commands
    }

    /// A handle for one-off reads.
    pub fn queries(&self) -> Queries {
        Queries::new(self.api.clone())
    }

    /// A hub whose paths are the fixture world's (home `/home/me`), for previews and tests.
    pub fn new_fixture(api: Arc<crate::fixtures::FixtureApi>, cx: &mut App) -> Entity<Hub> {
        let paths = api.paths();
        Self::new_with_paths(api, paths, cx)
    }

    /// [`Hub::new`] with the app's resolved paths, so views show paths the way the engine
    /// sees them.
    pub fn new_with_paths(api: Arc<dyn Api>, paths: Paths, cx: &mut App) -> Entity<Hub> {
        let hub = Self::new(api, cx);
        hub.update(cx, |hub, cx| hub.set_paths(paths, cx));
        hub
    }

    /// Replaces the paths views use for display (they default to [`Paths::from_env`]).
    pub fn set_paths(&mut self, paths: Paths, cx: &mut Context<Self>) {
        if self.paths != paths {
            self.paths = paths;
            cx.notify();
        }
    }

    /// The app's directories as the engine resolved them.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The user's home directory: where `~` points in displayed paths and where pickers start.
    pub fn home(&self) -> &Path {
        &self.paths.home
    }

    /// Where the app is.
    pub fn router(&self) -> &Entity<Router> {
        &self.router
    }

    /// The latest notices, oldest first.
    pub fn notices(&self) -> impl Iterator<Item = &HubNotice> {
        self.notices.iter()
    }

    /// Shows a notice: emitted to subscribers and kept among the recent ones.
    pub fn notify_user(&mut self, notice: HubNotice, cx: &mut Context<Self>) {
        self.notices.push_back(notice.clone());
        if self.notices.len() > RECENT_NOTICES {
            self.notices.pop_front();
        }
        cx.emit(notice);
    }

    /// Open session models (alive ones only).
    pub fn open_sessions(&self) -> usize {
        self.sessions.values().filter(|weak| weak.upgrade().is_some()).count()
    }

    fn session_model(&mut self, id: &SessionId, cx: &mut Context<Self>) -> Entity<SessionModel> {
        self.sessions.retain(|_, weak| weak.upgrade().is_some());
        if let Some(model) = self.sessions.get(id).and_then(WeakEntity::upgrade) {
            return model;
        }
        let api = self.api.clone();
        let model = cx.new(|_| SessionModel::new(id.clone(), api));
        self.sessions.insert(id.clone(), model.downgrade());
        model
    }

    fn run_model(&mut self, id: &RunId, cx: &mut Context<Self>) -> Entity<RunModel> {
        self.run_models.retain(|_, weak| weak.upgrade().is_some());
        if let Some(model) = self.run_models.get(id).and_then(WeakEntity::upgrade) {
            return model;
        }
        let api = self.api.clone();
        let model = cx.new(|_| RunModel::new(id.clone(), api));
        self.run_models.insert(id.clone(), model.downgrade());
        model
    }

    fn review_model(&mut self, id: &ReviewId, cx: &mut Context<Self>) -> Entity<ReviewModel> {
        self.reviews.retain(|_, weak| weak.upgrade().is_some());
        if let Some(model) = self.reviews.get(id).and_then(WeakEntity::upgrade) {
            return model;
        }
        let api = self.api.clone();
        let model = cx.new(|_| ReviewModel::new(id.clone(), api));
        self.reviews.insert(id.clone(), model.downgrade());
        model
    }

    /// Goes where a [`Target`] leads. A run is looked up first (from memory, else the engine).
    pub fn open_target(&mut self, target: Target, cx: &mut Context<Self>) {
        let Target::Run(id) = &target else {
            if let Some(route) = Route::for_target(&target, None) {
                self.router.update(cx, |router, cx| router.navigate(route, cx));
            }
            return;
        };
        if let Some(run) = self.runs.read(cx).find(id).cloned() {
            if let Some(route) = Route::for_target(&target, Some(&run)) {
                self.router.update(cx, |router, cx| router.navigate(route, cx));
            }
            return;
        }
        let api = self.api.clone();
        let id = id.clone();
        let work = cx.background_spawn(async move { api.run(&id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |hub, cx| match result {
                Ok(Some(run)) => {
                    if let Some(route) = Route::for_target(&target, Some(&run)) {
                        hub.router.update(cx, |router, cx| router.navigate(route, cx));
                    }
                }
                Ok(None) => hub.notify_user(
                    HubNotice::new(crate::api::NoticeLevel::Warning, "This run no longer exists"),
                    cx,
                ),
                Err(error) => hub.notify_user(HubNotice::failed("open the run", &error), cx),
            })
            .ok();
        })
        .detach();
    }
}

/// The hub. Panics if [`Hub::new`] was not called: that is a wiring bug of the binary.
pub fn hub(cx: &App) -> Entity<Hub> {
    cx.global::<GlobalHub>().0.clone()
}

/// The command handle.
pub fn commands(cx: &App) -> Commands {
    hub(cx).read(cx).commands.clone()
}

/// The handle for one-off reads.
pub fn queries(cx: &App) -> Queries {
    hub(cx).read(cx).queries()
}

/// A path for showing to the user: `~/code/acme/payments-api` under the home directory.
pub fn display_path(path: &Path, cx: &App) -> String {
    hub(cx).read(cx).paths.display(path)
}

/// The user's home directory, as the hub's paths name it.
pub fn home(cx: &App) -> std::path::PathBuf {
    hub(cx).read(cx).paths.home.clone()
}

/// The router: the current route, history and overlays.
pub fn router(cx: &App) -> Entity<Router> {
    hub(cx).read(cx).router.clone()
}

/// Goes to `route`.
pub fn navigate(route: Route, cx: &mut App) {
    router(cx).update(cx, |router, cx| router.navigate(route, cx));
}

/// Goes where a Home row leads.
pub fn open(target: Target, cx: &mut App) {
    hub(cx).update(cx, |hub, cx| hub.open_target(target, cx));
}

macro_rules! singleton {
    ($(#[$doc:meta])* $name:ident, $model:ty) => {
        $(#[$doc])*
        pub fn $name(cx: &mut App) -> Entity<$model> {
            let model = hub(cx).read(cx).$name.clone();
            model.update(cx, |model, cx| model.ensure_loaded(cx));
            model
        }
    };
}

singleton!(
    /// The sidebar's chats, loaded on first use.
    chat_index, ChatIndex
);
singleton!(
    /// The palette's chat search.
    search, SearchModel
);
singleton!(
    /// Issues of the current project.
    issues, IssuesModel
);
singleton!(
    /// Latest and open runs.
    runs, RunsModel
);
singleton!(
    /// Inbox items and open reviews.
    inbox, InboxModel
);
singleton!(
    /// Runnables, skills and pipelines.
    catalog, CatalogModel
);
singleton!(
    /// Home, spend, runtimes, settings and projects.
    ops, OpsModel
);
singleton!(
    /// Pending permission requests across sessions.
    permissions, PermissionsModel
);

/// The model of one session, shared by every view that shows it and dropped with the last one.
pub fn session(id: &SessionId, cx: &mut App) -> Entity<SessionModel> {
    let model = hub(cx).update(cx, |hub, cx| hub.session_model(id, cx));
    model.update(cx, |model, cx| model.ensure_loaded(cx));
    model
}

/// The model of one run, shared and dropped like [`session`].
pub fn run(id: &RunId, cx: &mut App) -> Entity<RunModel> {
    let model = hub(cx).update(cx, |hub, cx| hub.run_model(id, cx));
    model.update(cx, |model, cx| model.ensure_loaded(cx));
    model
}

/// The model of one review, shared and dropped like [`session`].
pub fn review(id: &ReviewId, cx: &mut App) -> Entity<ReviewModel> {
    let model = hub(cx).update(cx, |hub, cx| hub.review_model(id, cx));
    model.update(cx, |model, cx| model.ensure_loaded(cx));
    model
}
