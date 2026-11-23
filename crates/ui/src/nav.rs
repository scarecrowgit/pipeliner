//! App-wide navigation: which screen is shown. Any screen can call [`navigate`].

use std::path::{Path, PathBuf};

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
    /// The sidebar shows only icons (56px) instead of the wide column. Saved on disk.
    pub sidebar_collapsed: bool,
    /// Issue to open on the board next time it renders (one-shot).
    pub issue_focus: Option<SharedString>,
    /// Pipeline to show in the editor next time it renders (one-shot).
    pub pipeline_focus: Option<SharedString>,
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
        sidebar_collapsed: sidebar_state_path()
            .is_some_and(|p| load_sidebar_collapsed(&p)),
        issue_focus: None,
        pipeline_focus: None,
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

// ── Sidebar mode ────────────────────────────────────────────────────────

/// `$XDG_DATA_HOME/harness/sidebar` (absolute `XDG_DATA_HOME` only), else
/// `~/.local/share/harness/sidebar`.
fn sidebar_state_path() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".local").join("share"))
        })?;
    Some(data.join("harness").join("sidebar"))
}

fn serialize_sidebar_collapsed(collapsed: bool) -> &'static str {
    if collapsed { "collapsed" } else { "expanded" }
}

/// `collapsed` → true; anything else (unknown content) → false.
fn parse_sidebar_collapsed(text: &str) -> bool {
    text.trim() == "collapsed"
}

/// Read the saved mode from `path`; a missing or unreadable file means expanded.
fn load_sidebar_collapsed(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| parse_sidebar_collapsed(&t))
}

/// Write the mode to a temp file next to `path`, then rename it into place.
fn save_sidebar_collapsed(path: &Path, collapsed: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serialize_sidebar_collapsed(collapsed))?;
    std::fs::rename(&tmp, path)
}

pub fn sidebar_collapsed(cx: &App) -> bool {
    nav_entity(cx).is_some_and(|n| n.read(cx).sidebar_collapsed)
}

/// Switch the sidebar mode, re-render observers and save the choice (best effort).
pub fn set_sidebar_collapsed(collapsed: bool, cx: &mut App) {
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            if n.sidebar_collapsed != collapsed {
                n.sidebar_collapsed = collapsed;
                if let Some(path) = sidebar_state_path() {
                    let _ = save_sidebar_collapsed(&path, collapsed);
                }
                cx.notify();
            }
        });
    }
}

pub fn toggle_sidebar(cx: &mut App) {
    let collapsed = sidebar_collapsed(cx);
    set_sidebar_collapsed(!collapsed, cx);
}

/// Show the issue board with issue `key` open.
pub fn open_issue(key: impl Into<SharedString>, cx: &mut App) {
    let key = key.into();
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.screen = Screen::Issues;
            n.issue_focus = Some(key);
            cx.notify();
        });
    }
}

/// Issue board: take the pending issue to open, if any.
pub fn take_issue_focus(cx: &mut App) -> Option<SharedString> {
    let nav = nav_entity(cx)?;
    nav.update(cx, |n, _| n.issue_focus.take())
}

/// Show the pipeline editor with pipeline `id` open.
pub fn open_pipeline(id: impl Into<SharedString>, cx: &mut App) {
    let id = id.into();
    if let Some(nav) = nav_entity(cx) {
        nav.update(cx, |n, cx| {
            n.screen = Screen::PipelineEditor;
            n.pipeline_focus = Some(id);
            cx.notify();
        });
    }
}

/// Pipeline editor: take the pending pipeline to open, if any.
pub fn take_pipeline_focus(cx: &mut App) -> Option<SharedString> {
    let nav = nav_entity(cx)?;
    nav.update(cx, |n, _| n.pipeline_focus.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("harness-nav-test-{}-{name}", std::process::id()))
            .join("harness")
            .join("sidebar")
    }

    #[test]
    fn sidebar_mode_round_trip() {
        let path = temp_path("round-trip");
        save_sidebar_collapsed(&path, true).unwrap();
        assert!(load_sidebar_collapsed(&path));
        save_sidebar_collapsed(&path, false).unwrap();
        assert!(!load_sidebar_collapsed(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn missing_file_is_expanded() {
        assert!(!load_sidebar_collapsed(&temp_path("missing")));
    }

    #[test]
    fn garbage_is_expanded() {
        let path = temp_path("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "wat\u{0}\n").unwrap();
        assert!(!load_sidebar_collapsed(&path));
        assert!(!parse_sidebar_collapsed(""));
        assert!(parse_sidebar_collapsed("collapsed\n"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }
}
