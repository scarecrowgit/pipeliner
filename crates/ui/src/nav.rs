//! App-wide navigation: which screen is shown. Any screen can call [`navigate`].

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Subscription, Window};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Screen {
    /// Dashboard: totals, what is working now, what needs the user, spend.
    Home,
    /// "Inbox" in the sidebar — the review queue (variants waiting for a person).
    Inbox,
    Issues,
    Chat,
    Agents,
    /// Pipeline editor (opened from a pipeline in the sidebar / agents list).
    PipelineEditor,
}

pub struct Nav {
    pub screen: Screen,
    /// Entry selected on the Agents screen / in the sidebar entity list (by display name,
    /// e.g. "Payments lead"). Shared so both sides stay in sync.
    pub agent: Option<SharedString>,
    /// Chat sessions published by the Chat screen (newest first) for the sidebar.
    pub chat_sessions: Vec<ChatSessionInfo>,
    /// Id of the session open in the Chat screen.
    pub chat_current: Option<SharedString>,
    /// One-shot request from the sidebar to the Chat screen (see [`take_chat_request`]).
    pub chat_request: Option<ChatRequest>,
    /// Number of saved (not archived) chats, for "All chats · N".
    pub chat_total: usize,
    /// The chat history picker (⌘K) is open over the current screen.
    pub history_open: bool,
    /// Bumped when saved chats change outside the Chat screen (pin, archive, delete in
    /// the history picker) so it reloads its list.
    pub chats_version: u64,
    /// Bumped when an agent posts to (or the user changes) the Inbox.
    pub inbox_version: u64,
    /// Items waiting in the Inbox (sidebar badge), published by the Inbox screen.
    pub inbox_count: usize,
    /// Inbox item to select next time the Inbox screen renders (one-shot).
    pub inbox_focus: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatStatus {
    /// Nothing running.
    Idle,
    /// A model turn or tool is running.
    Running,
    /// Waiting for the user (tool permission).
    Waiting,
    /// The last turn failed.
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatSessionInfo {
    pub id: SharedString,
    pub title: SharedString,
    pub status: ChatStatus,
    pub pinned: bool,
    /// Last activity, unix seconds (0 = unknown).
    pub updated: u64,
    /// "Build", "Plan" or a custom agent name.
    pub agent: SharedString,
    /// Folder label, e.g. `pipeliner · main`.
    pub workdir: SharedString,
    pub cost_usd: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatRequest {
    Open(SharedString),
    /// Open a copy of a saved chat as a new session.
    Fork(SharedString),
    New,
}

struct GlobalNav(Entity<Nav>);
impl Global for GlobalNav {}

/// Install the nav entity (done by `ui::init`). Returns it so the root view can observe it.
pub fn install(initial: Screen, cx: &mut App) -> Entity<Nav> {
    let nav = cx.new(|_| Nav {
        screen: initial,
        agent: None,
        chat_sessions: Vec::new(),
        chat_current: None,
        chat_request: None,
        chat_total: 0,
        history_open: false,
        chats_version: 0,
        inbox_version: 0,
        inbox_count: 0,
        inbox_focus: None,
    });
    cx.set_global(GlobalNav(nav.clone()));
    nav
}

pub fn nav_entity(cx: &App) -> Option<Entity<Nav>> {
    cx.try_global::<GlobalNav>().map(|g| g.0.clone())
}

/// Switch the main area to `screen`. No-op in standalone previews without nav.
pub fn navigate(screen: Screen, cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.screen = screen;
            cx.notify();
        });
    }
}

pub fn current(cx: &App) -> Screen {
    nav_entity(cx).map(|n| n.read(cx).screen).unwrap_or(Screen::Inbox)
}

/// Show the Agents screen with `name` selected (sidebar → screen, or screen → sidebar).
/// Pass `navigate_to_screen = false` to only record the selection.
pub fn select_agent(name: impl Into<SharedString>, navigate_to_screen: bool, cx: &mut App) {
    let name = name.into();
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            if n.agent.as_ref() == Some(&name) && (!navigate_to_screen || n.screen == Screen::Agents) {
                return;
            }
            n.agent = Some(name);
            if navigate_to_screen {
                n.screen = Screen::Agents;
            }
            cx.notify();
        });
    }
}

pub fn selected_agent(cx: &App) -> Option<SharedString> {
    nav_entity(cx).and_then(|n| n.read(cx).agent.clone())
}

/// Call `f` whenever navigation lands on `screen` (and the nav changes while it is shown).
/// Typical use: move keyboard focus into the screen so its key bindings work immediately.
/// Store the returned subscription in the screen. `None` in previews without nav.
pub fn on_activate<V: 'static>(
    screen: Screen,
    window: &mut Window,
    cx: &mut Context<V>,
    f: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Option<Subscription> {
    let nav = nav_entity(cx)?;
    let mut last = nav.read(cx).screen;
    Some(cx.observe_in(&nav, window, move |this, nav, window, cx| {
        let now = nav.read(cx).screen;
        if now == screen && last != screen {
            f(this, window, cx);
        }
        last = now;
    }))
}

/// Chat screen → sidebar: the session list (pinned and recent), which one is open and
/// how many chats exist. No-op when unchanged.
pub fn set_chat_sessions(
    sessions: Vec<ChatSessionInfo>,
    current: Option<SharedString>,
    total: usize,
    cx: &mut App,
) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            if n.chat_sessions != sessions || n.chat_current != current || n.chat_total != total
            {
                n.chat_sessions = sessions;
                n.chat_current = current;
                n.chat_total = total;
                cx.notify();
            }
        });
    }
}

/// Show or hide the chat history picker (⌘K).
pub fn set_history_open(open: bool, cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            if n.history_open != open {
                n.history_open = open;
                cx.notify();
            }
        });
    }
}

pub fn history_open(cx: &App) -> bool {
    nav_entity(cx).is_some_and(|n| n.read(cx).history_open)
}

/// Saved chats changed on disk (pinned, archived, deleted): the Chat screen reloads.
pub fn chats_changed(cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.chats_version += 1;
            cx.notify();
        });
    }
}

/// Sidebar → Chat screen: show Chat and open (or create) a session.
pub fn request_chat(request: ChatRequest, cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.screen = Screen::Chat;
            n.chat_request = Some(request);
            cx.notify();
        });
    }
}

/// Chat screen: take the pending sidebar request, if any.
pub fn take_chat_request(cx: &mut App) -> Option<ChatRequest> {
    let nav = nav_entity(cx)?;
    nav.update(cx, |n, _| n.chat_request.take())
}

/// The Inbox changed on disk (a report was posted or edited).
pub fn inbox_changed(cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.inbox_version += 1;
            cx.notify();
        });
    }
}

/// Inbox screen → sidebar: how many items wait. No-op when unchanged.
pub fn set_inbox_count(count: usize, cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            if n.inbox_count != count {
                n.inbox_count = count;
                cx.notify();
            }
        });
    }
}

/// Show the Inbox with item `id` selected.
pub fn open_inbox_item(id: impl Into<SharedString>, cx: &mut App) {
    let id = id.into();
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.screen = Screen::Inbox;
            n.inbox_focus = Some(id);
            cx.notify();
        });
    }
}

/// Inbox screen: take the pending item to select, if any.
pub fn take_inbox_focus(cx: &mut App) -> Option<SharedString> {
    let nav = nav_entity(cx)?;
    nav.update(cx, |n, _| n.inbox_focus.take())
}
