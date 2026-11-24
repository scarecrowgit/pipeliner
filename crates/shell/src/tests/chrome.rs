//! Navigation, the mounted screen and its cache, the placeholder, toasts and the keys.

use gpui::{TestAppContext, prelude::*};
use hub::api::{NoticeLevel, SessionId};
use hub::{HubNotice, Overlay, Route, Section};

use super::{Palette, setup};
use crate::PALETTE_LAYER;

#[gpui::test]
fn navigation_changes_the_active_item_and_the_mounted_screen(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert_eq!(t.route(), Route::Home);
    assert_eq!(t.shown().active, Some(Section::Home));
    assert_eq!(t.log.borrow().as_slice(), ["home Home"]);

    t.click("sidebar-nav-inbox");
    assert_eq!(t.route(), Route::Inbox { item: None });
    assert_eq!(t.shown().active, Some(Section::Inbox));
    assert_eq!(t.log.borrow().last().map(String::as_str), Some("inbox Inbox { item: None }"));

    t.click("sidebar-chat-chat-1423");
    let chat = Route::Chat { session: SessionId::new("chat-1423") };
    assert_eq!(t.route(), chat);
    let shown = t.shown();
    assert_eq!(shown.active, Some(Section::Chats));
    assert_eq!(shown.current_chat, Some(SessionId::new("chat-1423")));
    assert!(t.drawn("sidebar-chat-chat-1423-explore"), "the open chat's sub-agents are listed under it");

    // A breadcrumb-less screen gets its section label as the title.
    assert!(t.drawn("header"));
}

#[gpui::test]
fn a_screen_view_is_reused_when_returning_to_its_route(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let a = Route::Chat { session: SessionId::new("chat-1423") };
    let b = Route::Chat { session: SessionId::new("chat-ledger") };
    t.navigate(a.clone());
    let first = t.body().entity_id();
    t.navigate(Route::Home);
    let home = t.body().entity_id();
    t.navigate(b);
    assert_ne!(t.body().entity_id(), first, "another session gets its own view");
    t.navigate(a);
    assert_eq!(t.body().entity_id(), first, "the first session's view is kept");
    t.navigate(Route::Home);
    assert_eq!(t.body().entity_id(), home, "one Home view serves every visit");
    let shell = t.shell.clone();
    let mounts = t.cx.update(|_, cx| shell.read(cx).mounts());
    assert_eq!(mounts, 3, "home, chat A and chat B were each built once: {:?}", t.log.borrow());
}

#[gpui::test]
fn an_unregistered_section_shows_the_placeholder(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert!(!t.drawn("screen-placeholder"));
    t.click("sidebar-nav-issues");
    assert_eq!(t.route(), Route::Issues { issue: None });
    let shell = t.shell.clone();
    assert!(t.cx.update(|_, cx| shell.read(cx).showing_placeholder()));
    assert!(t.drawn("screen-placeholder"));
    assert_eq!(t.shown().active, Some(Section::Issues));
}

#[gpui::test]
fn a_notice_becomes_a_toast(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert!(!t.drawn("toast"));
    let hub = t.hub.clone();
    t.cx.update(|_, cx| {
        hub.update(cx, |hub, cx| hub.notify_user(HubNotice::new(NoticeLevel::Warning, "The laptop went offline"), cx))
    });
    t.cx.run_until_parked();
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).has_toast(cx)));
    assert!(t.drawn("toast"));

    // A failed command comes back the same way.
    t.cx.update(|window, cx| ui::overlays(window, cx).dismiss_toast(cx));
    t.api.fail_next("pin_session");
    let commands = t.cx.update(|_, cx| hub::commands(cx));
    t.cx.update(|_, cx| commands.pin_session(&SessionId::new("chat-ledger"), true, cx));
    t.cx.run_until_parked();
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).has_toast(cx)));
}

#[gpui::test]
fn keys_open_the_palette_start_a_chat_and_collapse(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let shell = t.shell.clone();
    let built = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = built.clone();
    t.cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_palette(
                move |_, cx| {
                    count.set(count.get() + 1);
                    cx.new(|_| Palette).into()
                },
                window,
                cx,
            )
        });
    });
    let router = t.cx.update(|_, cx| hub::router(cx));

    t.cx.simulate_keystrokes("ctrl-k");
    t.cx.run_until_parked();
    assert!(t.cx.update(|_, cx| router.read(cx).is_open(Overlay::Palette)));
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)));
    assert!(t.drawn("palette"), "the mounted palette view is drawn in the layer");
    assert_eq!(built.get(), 1);

    // Escape closes the layer; the router follows.
    t.cx.simulate_keystrokes("escape");
    t.cx.run_until_parked();
    assert!(!t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)));
    assert!(!t.cx.update(|_, cx| router.read(cx).is_open(Overlay::Palette)));

    // ⌘K opens it again after Escape, and closes it when pressed while it is open.
    t.cx.simulate_keystrokes("ctrl-k");
    t.cx.run_until_parked();
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)), "⌘K reopens");
    t.cx.simulate_keystrokes("ctrl-k");
    t.cx.run_until_parked();
    assert!(!t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)), "⌘K closes");
    assert_eq!(built.get(), 2);

    // The search field opens it too; navigating closes it.
    t.click("sidebar-search");
    assert!(t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)));
    assert_eq!(built.get(), 3, "each opening builds a fresh palette");
    t.cx.update(|_, cx| hub::navigate(Route::Chats, cx));
    t.cx.run_until_parked();
    assert!(!t.cx.update(|window, cx| ui::overlays(window, cx).is_open(PALETTE_LAYER, cx)));

    t.cx.simulate_keystrokes("ctrl-n");
    t.cx.run_until_parked();
    assert_eq!(t.route(), Route::NewChat);

    t.cx.simulate_keystrokes("ctrl-\\");
    t.cx.run_until_parked();
    assert!(t.shown().collapsed);
}

#[gpui::test]
fn f11_enters_and_leaves_full_screen_from_anywhere(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    // Keys reach a window only while it is the active one, as on the desktop.
    t.cx.update(|window, _| window.activate_window());
    t.cx.run_until_parked();
    let full = |t: &mut super::Setup| t.cx.update(|window, _| window.is_fullscreen());
    assert!(!full(&mut t));

    t.keys("f11");
    assert!(full(&mut t), "F11 with nothing focused");
    t.navigate(Route::Chats);
    t.keys("f11");
    assert!(!full(&mut t), "F11 again, on another screen");

    // With a text field focused: the project form's name field takes focus when it opens.
    t.click("sidebar-project");
    t.click("project-settings");
    assert!(t.open(crate::sidebar::projects::form::FORM_DIALOG));
    t.keys("f11");
    assert!(full(&mut t), "F11 while a text field has focus");
    assert!(t.open(crate::sidebar::projects::form::FORM_DIALOG), "the dialog stays open");
    t.keys("f11");
    assert!(!full(&mut t));
    // The field still takes text: F11 did not end up typed or steal focus.
    t.keys("ctrl-a");
    t.typed("Payments core");
    t.click("project-form-save");
    let name = t.api.with_world(|world| world.projects.iter().find(|p| p.id.as_str() == "payments").map(|p| p.name.clone()));
    assert_eq!(name.as_deref(), Some("Payments core"));
}
