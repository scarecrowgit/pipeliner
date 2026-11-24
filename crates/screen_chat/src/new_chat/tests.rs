//! The new-chat page and the chats list in a test window over the fixture backend.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, Modifiers, Render, TestAppContext, VisualTestContext, Window};
use hub::api::{WorkMode, WorkdirChoice};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, Route};
use ui::OverlayHost;

use super::{NewChatView, TYPING_REST};
use super::chats::{ChatsView, Group};
use crate::workdir::common::Blocker;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// A window over the Acme world with `build`'s view as the content of an overlay host.
pub(crate) fn open<V: Render + 'static>(
    cx: &mut TestAppContext,
    route: Route,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
) -> (Arc<FixtureApi>, Entity<V>, &mut VisualTestContext) {
    open_with(cx, route, |_| {}, build)
}

/// [`open`] over a world changed by `change` first.
pub(crate) fn open_with<V: Render + 'static>(
    cx: &mut TestAppContext,
    route: Route,
    change: impl FnOnce(&mut World),
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
) -> (Arc<FixtureApi>, Entity<V>, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let mut world = World::acme(NOW);
    change(&mut world);
    let api = Arc::new(FixtureApi::new(world));
    cx.update(|cx| {
        Hub::new_fixture(api.clone(), cx);
        hub::navigate(route, cx);
    });
    let slot: Rc<RefCell<Option<Entity<V>>>> = Rc::default();
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| build(window, cx));
        *put.borrow_mut() = Some(view.clone());
        OverlayHost::new(window, cx).with_content(view)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (api, view, cx)
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    cx.run_until_parked();
    let Some(bounds) = cx.debug_bounds(selector) else { panic!("`{selector}` was not laid out") };
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// Whether `selector` is drawn.
pub(crate) fn drawn(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(selector).is_some()
}

pub(crate) fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| hub::router(cx).read(cx).current().clone())
}

fn new_chat(cx: &mut TestAppContext) -> (Arc<FixtureApi>, Entity<NewChatView>, &mut VisualTestContext) {
    open(cx, Route::NewChat, NewChatView::new)
}

fn path_of(api: &FixtureApi, repo: &str) -> PathBuf {
    api.with_world(|world| world.workdirs.iter().find(|entry| entry.repo == repo).map(|e| e.path.clone()))
        .unwrap_or_else(|| panic!("no recent folder {repo}"))
}

#[gpui::test]
fn choosing_a_folder_loads_its_facts_and_drops_stale_ones(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    view.read_with(cx, |view, _| assert_eq!(view.recent.len(), 3, "the recent folders are listed"));

    click(cx, "new-chat-folder-row-0");
    view.read_with(cx, |view, _| {
        let facts = view.facts().repo().expect("payments-api is a repo");
        assert_eq!(facts.branch.as_deref(), Some("main"));
        assert_eq!(view.base.as_deref(), Some("main"));
        assert_eq!(view.worktrees.len(), 3, "its worktrees are listed");
        assert!(!view.branches.is_empty());
    });

    // Two picks in a row: only the answer about the second folder counts.
    let (payments, infra) = (path_of(&api, "payments-api"), path_of(&api, "infra"));
    view.update(cx, |view, cx| {
        view.select_folder(payments.clone(), cx);
        view.select_folder(infra.clone(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.facts().path(), Some(infra.as_path()));
        assert!(!view.facts().is_loading());
        assert!(view.facts().repo().is_none(), "the facts of payments-api came late and were dropped");
        assert!(view.worktrees.is_empty());
    });

    view.update(cx, |view, cx| {
        view.select_folder(infra.clone(), cx);
        view.select_folder(payments.clone(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.facts().path(), Some(payments.as_path()));
        assert!(view.facts().repo().is_some());
    });
}

#[gpui::test]
fn each_work_mode_makes_its_choice(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    let payments = path_of(&api, "payments-api");
    click(cx, "new-chat-folder-row-0");

    let session = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.new_session(cx));
    let chip = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.chip().read(cx).choice().cloned());

    let worktree = WorkdirChoice {
        path: payments.clone(),
        mode: WorkMode::Worktree,
        base: Some("main".into()),
        branch: Some("chat/new-chat".into()),
    };
    let new = session(cx).expect("nothing blocks a worktree of a repo");
    assert_eq!(new.workdir.as_ref(), Some(&worktree));
    assert_eq!(chip(cx).as_ref(), Some(&worktree), "the chip holds the same choice");

    click(cx, "new-chat-mode-in-place");
    let in_place = WorkdirChoice { path: payments.clone(), mode: WorkMode::InPlace, base: None, branch: None };
    assert_eq!(session(cx).and_then(|new| new.workdir), Some(in_place.clone()));
    assert_eq!(chip(cx), Some(in_place));

    click(cx, "new-chat-mode-read-only");
    let read_only = WorkdirChoice { path: payments, mode: WorkMode::ReadOnly, base: None, branch: None };
    assert_eq!(session(cx).and_then(|new| new.workdir), Some(read_only));
    view.read_with(cx, |view, cx| {
        assert_eq!(view.mode(), WorkMode::ReadOnly);
        assert_eq!(view.chip().read(cx).label(cx), "payments-api · main");
    });
}

#[gpui::test]
fn worktree_mode_on_a_folder_outside_git_blocks_the_start(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    click(cx, "new-chat-folder-row-2");
    view.read_with(cx, |view, cx| {
        assert_eq!(view.mode(), WorkMode::Worktree);
        assert_eq!(view.blocker(cx), Some(Blocker::NotARepo));
        assert!(Blocker::NotARepo.message().is_some_and(|m| m.starts_with("Not a git repository")));
        assert!(!view.can_start(cx));
        assert!(view.new_session(cx).is_none());
    });
    click(cx, "new-chat-start");
    assert!(!api.recorded_names().contains(&"new_session"), "a blocked start creates nothing");

    click(cx, "new-chat-mode-in-place");
    view.read_with(cx, |view, cx| assert!(view.can_start(cx), "in place works outside git"));
}

#[gpui::test]
fn start_creates_the_session_and_opens_it(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    click(cx, "new-chat-folder-row-0");
    click(cx, "new-chat-start");

    assert!(api.recorded_names().contains(&"new_session"), "the session was created");
    assert!(!api.recorded_names().contains(&"send"), "nothing is sent: the chat's composer takes over");
    let Route::Chat { session } = route(cx) else { panic!("the new chat is open") };
    let workdir = api.with_world(|world| world.session(&session).and_then(|s| s.workdir.clone()));
    let workdir = workdir.expect("the session works in the folder");
    assert_eq!(workdir.mode, WorkMode::Worktree);
    assert_eq!(workdir.branch.as_deref(), Some("chat/new-chat"));
    view.read_with(cx, |view, _| assert!(!view.starting));
}

#[gpui::test]
fn the_search_has_focus_and_its_keys_move_and_start(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    let focused = cx.update(|window, cx| view.read(cx).search.read(cx).is_focused(window));
    assert!(focused, "the folder search has focus on open");
    let path = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.facts().path().map(PathBuf::from));

    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(path(cx), Some(path_of(&api, "payments-api")), "down picks the first row");
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(path(cx), Some(path_of(&api, "checkout-web")));
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert_eq!(path(cx), Some(path_of(&api, "payments-api")));

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(api.recorded_names().contains(&"new_session"), "enter starts");
    assert!(matches!(route(cx), Route::Chat { .. }));
}

#[gpui::test]
fn the_panel_names_the_folder_and_the_list_counts_worktrees(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    click(cx, "new-chat-folder-row-0");
    for line in ["new-chat-panel-name", "new-chat-panel-path"] {
        let Some(bounds) = cx.debug_bounds(line) else { panic!("{line} is drawn") };
        assert!(f32::from(bounds.size.width) > 60. && f32::from(bounds.size.height) > 10., "{line} has room: {bounds:?}");
    }
    let mut extra = api.with_world(|world| world.worktrees[0].clone());
    extra.path.push("extra");
    view.update(cx, |view, cx| {
        view.worktrees.push(extra);
        cx.notify();
    });
    let listing = view.read_with(cx, |view, cx| view.listing(cx));
    assert_eq!((listing.worktrees.len(), listing.worktrees_total), (3, 4), "three shown, \"3 of 4\"");

    let mut local = view.read_with(cx, |view, cx| view.runtime(cx)).expect("a local runtime");
    assert_eq!(super::picker::runtime_line(&local), "local · online · 3 agents");
    local.slots_busy = 1;
    assert_eq!(super::picker::runtime_line(&local), "local · online · 1 agent");
}

#[gpui::test]
fn runtimes_that_are_not_available_cannot_be_picked(cx: &mut TestAppContext) {
    let (_api, view, cx) = new_chat(cx);
    let picked = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.runtime(cx).map(|rt| rt.name));
    assert_eq!(picked(cx).as_deref(), Some("mac-mini"));
    click(cx, "new-chat-runtime-cloud");
    click(cx, "new-chat-runtime-remote");
    assert_eq!(picked(cx).as_deref(), Some("mac-mini"));
    view.update(cx, |view, cx| view.set_runtime("cloud", cx));
    assert_eq!(picked(cx).as_deref(), Some("mac-mini"));
}

#[gpui::test]
fn a_choice_set_on_the_chip_is_the_pages_folder(cx: &mut TestAppContext) {
    let (api, view, cx) = new_chat(cx);
    let infra = path_of(&api, "infra");
    let chip = view.read_with(cx, |view, _| view.chip().clone());
    chip.update(cx, |chip, cx| {
        let choice = WorkdirChoice { path: infra.clone(), mode: WorkMode::ReadOnly, base: None, branch: None };
        chip.set_choice(Some(choice), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.facts().path(), Some(infra.as_path()));
        assert_eq!(view.mode(), WorkMode::ReadOnly);
    });
}

#[gpui::test]
fn the_chats_list_groups_filters_and_opens(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, Route::Chats, ChatsView::new);
    view.update(cx, |view, cx| view.set_now(NOW, cx));
    cx.run_until_parked();

    let rows = view.read_with(cx, |view, cx| view.rows(cx));
    assert!(rows.len() > 3);
    let order = |group: Group| match group {
        Group::Pinned => 0,
        Group::Today => 1,
        Group::Earlier => 2,
    };
    assert!(rows.windows(2).all(|pair| order(pair[0].0) <= order(pair[1].0)), "pinned, then today, then earlier");
    assert!(rows.iter().all(|(group, session)| (*group == Group::Pinned) == session.pinned));
    assert!(rows.iter().any(|(group, _)| *group == Group::Today));
    assert!(rows.iter().any(|(group, _)| *group == Group::Earlier));
    assert!(drawn(cx, "chats-filter"));

    let filter = view.read_with(cx, |view, _| view.filter().clone());
    filter.update(cx, |input, cx| input.insert_text("checkout", cx));
    cx.run_until_parked();
    let filtered = view.read_with(cx, |view, cx| view.rows(cx));
    assert!(!filtered.is_empty() && filtered.len() < rows.len());
    assert!(filtered.iter().all(|(_, session)| {
        let folder = session.workdir.as_ref().map(|w| w.repo.clone()).unwrap_or_default();
        format!("{} {folder}", session.title).to_lowercase().contains("checkout")
    }));

    let first = filtered[0].1.id.clone();
    click(cx, "chats-row-0");
    assert_eq!(route(cx), Route::Chat { session: first });
}

/// A recent folder deep under /tmp, whose path is longer than the column.
fn long_entry() -> hub::api::WorkdirEntry {
    hub::api::WorkdirEntry {
        path: "/tmp/claude-1000/-mnt-storage-projects-pipeliner/b931af7e-9892-484b-aef8-4c0284814418/scratchpad/cli-smoke"
            .into(),
        repo: "cli-smoke".into(),
        branch: Some("main".into()),
        host: hub::api::RepoHost::Local,
        is_worktree: false,
        status: "no remote".into(),
        owner: None,
        sessions: 1,
    }
}

#[gpui::test]
fn keys_typed_into_the_window_filter_the_folders_and_enter_starts(cx: &mut TestAppContext) {
    let (api, view, cx) =
        open_with(cx, Route::NewChat, |world| world.workdirs.insert(0, long_entry()), NewChatView::new);
    let Some(field) = cx.debug_bounds("new-chat-folder") else { panic!("the folder search is drawn") };
    let width = f32::from(field.size.width);
    assert!((600. ..=720.).contains(&width), "a long path does not widen the column: {width}");

    // No call on the field: the keys go wherever the window's focus is.
    cx.simulate_input("pay");
    cx.run_until_parked();
    let rows = view.read_with(cx, |view, cx| {
        view.listing(cx).rows().iter().map(|entry| entry.repo.clone()).collect::<Vec<_>>()
    });
    assert_eq!(rows, ["payments-api"], "typing filtered the list");

    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(api.recorded_names().contains(&"new_session"), "enter starts");
    let Route::Chat { session } = route(cx) else { panic!("the new chat is open") };
    let path = api.with_world(|world| world.session(&session).and_then(|s| s.workdir.clone())).map(|w| w.path);
    assert!(path.is_some_and(|path| path.to_string_lossy().contains("payments-api")));
}

#[gpui::test]
fn a_typed_path_is_offered_once_it_was_looked_at(cx: &mut TestAppContext) {
    let (_api, view, cx) = new_chat(cx);
    cx.simulate_keystrokes("ctrl-o");
    cx.run_until_parked();
    let text = view.read_with(cx, |view, cx| view.search.read(cx).text().to_string());
    assert_eq!(text, "~/", "open folder starts a path in the search");

    let typed = "/srv/elsewhere/payments-api-fork";
    view.update_in(cx, |view, window, cx| {
        view.search.update(cx, |input, cx| input.set_text("", cx));
        view.search.read(cx).focus(window);
    });
    cx.simulate_input(typed);
    cx.executor().advance_clock(TYPING_REST * 2);
    cx.run_until_parked();
    assert!(drawn(cx, "new-chat-folder-path"), "the typed folder is a row");
    view.read_with(cx, |view, _| {
        let info = view.typed_folder(std::path::Path::new(typed)).expect("the folder was looked at");
        assert!(info.facts.is_some());
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_eq!(view.facts().path(), Some(std::path::Path::new(typed))));
}
