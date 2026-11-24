//! The sidebar's data follows hub events; collapse is saved; row actions call commands.

use gpui::TestAppContext;
use hub::Route;
use hub::api::{Event, SessionId, SessionState};

use super::setup;

#[gpui::test]
fn the_inbox_count_and_a_chats_state_follow_hub_events(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert_eq!(t.shown().inbox_waiting, 4, "the mockup's four waiting items");

    let inbox = t.cx.update(|_, cx| hub::inbox(cx));
    let first = t.cx.update(|_, cx| inbox.read(cx).waiting_items()[0].id.clone());
    t.api.emit(Event::InboxRemoved { id: first });
    t.cx.run_until_parked();
    assert_eq!(t.shown().inbox_waiting, 3);

    let id = SessionId::new("chat-ledger");
    let state = |t: &mut super::Setup| t.shown().chats.iter().find(|(chat, _)| chat == &id).map(|(_, state)| *state);
    assert_ne!(state(&mut t), Some(SessionState::Running));
    let mut session = t.api.with_world(|world| world.session(&id).cloned()).expect("the ledger chat");
    session.state = SessionState::Running;
    t.api.emit(Event::SessionChanged { session });
    t.cx.run_until_parked();
    assert_eq!(state(&mut t), Some(SessionState::Running));
}

#[gpui::test]
fn collapse_toggles_and_is_saved_through_the_hub(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert!(!t.shown().collapsed);
    assert!(t.drawn("sidebar"));

    t.click("sidebar-collapse");
    assert!(t.shown().collapsed);
    assert!(t.drawn("rail"));
    assert!(t.api.with_world(|world| world.settings.sidebar_collapsed), "saved in the settings");
    assert_eq!(t.recorded("set_settings").len(), 1);

    // The rail still navigates, and expands again from its logo.
    t.click("sidebar-nav-inbox");
    assert_eq!(t.route(), Route::Inbox { item: None });
    t.click("sidebar-expand");
    assert!(!t.shown().collapsed);
    assert!(!t.api.with_world(|world| world.settings.sidebar_collapsed));
}

#[gpui::test]
fn chat_row_actions_call_the_commands(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let row = "sidebar-chat-chat-refund-timeout";
    assert!(!t.drawn(&format!("{row}-pin")), "actions show on hover only");
    let listed = |t: &mut super::Setup| t.shown().chats.iter().any(|(id, _)| id.as_str() == "chat-refund-timeout");

    t.hover(row);
    t.click(&format!("{row}-pin"));
    assert_eq!(t.recorded("pin_session"), ["chat-refund-timeout true"]);
    assert_eq!(t.route(), Route::Home, "the action does not open the chat");

    t.hover(row);
    t.click(&format!("{row}-archive"));
    assert_eq!(t.recorded("archive_session"), ["chat-refund-timeout true"]);
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).has_toast(cx)), "archiving offers Undo");
    assert!(!listed(&mut t), "an archived chat leaves the sidebar");

    t.click("toast-undo");
    assert_eq!(t.recorded("archive_session"), ["chat-refund-timeout true", "chat-refund-timeout false"]);
    assert!(listed(&mut t));

    t.click("sidebar-new-chat");
    assert_eq!(t.route(), Route::NewChat);
}
