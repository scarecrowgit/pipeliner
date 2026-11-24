//! The session's workdir chip, its popover and the switch dialog over the fixture backend.

use gpui::{TestAppContext, VisualTestContext};
use hub::Route;
use hub::api::WorkMode;
use hub::fixtures::FixtureApi;
use hub::model::SessionId;

use super::{CHIP, WorkdirChip, switch};
use crate::new_chat::tests::{click, drawn, open};

/// The chat that works in the `fix/PAY-1423` worktree.
fn fix_chat(api: &FixtureApi) -> SessionId {
    api.with_world(|world| {
        world
            .sessions
            .iter()
            .find(|s| s.parent.is_none() && s.workdir.as_ref().and_then(|w| w.branch.as_deref()) == Some("fix/PAY-1423"))
            .map(|s| s.id.clone())
    })
    .expect("the fixture has the fix/PAY-1423 chat")
}

fn open_chip(cx: &mut TestAppContext) -> (std::sync::Arc<FixtureApi>, gpui::Entity<WorkdirChip>, &mut VisualTestContext) {
    let probe = FixtureApi::new(hub::fixtures::World::acme(crate::new_chat::tests::NOW));
    let id = fix_chat(&probe);
    open(cx, Route::Home, move |window, cx| WorkdirChip::new(Some(id), window, cx))
}

fn overlay_open(cx: &mut VisualTestContext, id: &'static str) -> bool {
    cx.update(|window, cx| ui::overlays(window, cx).is_open(id, cx))
}

#[gpui::test]
fn the_chip_names_the_sessions_folder_and_branch(cx: &mut TestAppContext) {
    let (_api, chip, cx) = open_chip(cx);
    chip.read_with(cx, |chip, cx| {
        assert!(chip.session().is_some());
        assert_eq!(chip.label(cx), "payments-api · fix/PAY-1423");
        assert!(chip.choice().is_none(), "a session's chip holds no draft");
    });
    assert!(drawn(cx, CHIP));
}

#[gpui::test]
fn the_popovers_switch_action_opens_the_dialog(cx: &mut TestAppContext) {
    let (_api, _chip, cx) = open_chip(cx);
    click(cx, CHIP);
    assert!(overlay_open(cx, CHIP), "the popover opens from the chip");
    assert!(drawn(cx, "workdir-popover-current"), "the current workdir is listed");
    assert!(drawn(cx, "workdir-popover-copy"));
    click(cx, "workdir-popover-switch");
    assert!(!overlay_open(cx, CHIP));
    assert!(overlay_open(cx, switch::DIALOG), "the switch dialog is open");
    assert!(drawn(cx, switch::CONFIRM));
}

#[gpui::test]
fn the_dialog_confirm_moves_the_session(cx: &mut TestAppContext) {
    let (api, _chip, cx) = open_chip(cx);
    let id = fix_chat(&api);
    click(cx, CHIP);
    click(cx, "workdir-popover-folder-0");
    assert!(overlay_open(cx, switch::DIALOG), "picking a folder opens the dialog on it");
    assert!(api.recorded_names().is_empty(), "nothing moves before the confirm");

    click(cx, "switch-workdir-mode-read-only");
    click(cx, switch::CONFIRM);
    assert!(!overlay_open(cx, switch::DIALOG), "the dialog closes");
    let call = api.recorded().into_iter().find(|c| c.name == "set_session_workdir").expect("the command was called");
    assert!(call.detail.starts_with(id.as_str()));
    let moved = api.with_world(|world| world.session(&id).and_then(|s| s.workdir.clone())).expect("a workdir");
    assert!(call.detail.contains(&moved.path.display().to_string()));
    assert_eq!(moved.mode, WorkMode::ReadOnly);
    assert!(!moved.path.ends_with("fix-PAY-1423"), "the session works elsewhere now");
}

#[gpui::test]
fn the_dialog_waits_for_a_folder(cx: &mut TestAppContext) {
    let (api, _chip, cx) = open_chip(cx);
    click(cx, CHIP);
    click(cx, "workdir-popover-switch");
    click(cx, switch::CONFIRM);
    assert!(overlay_open(cx, switch::DIALOG), "with no folder chosen the confirm does nothing");
    assert!(!api.recorded_names().contains(&"set_session_workdir"));
}

#[gpui::test]
fn another_view_can_open_the_popover(cx: &mut TestAppContext) {
    let (_api, chip, cx) = open_chip(cx);
    chip.update_in(cx, |chip, window, cx| chip.open(window, cx));
    cx.run_until_parked();
    assert!(overlay_open(cx, CHIP), "open() works from outside the chip's own click");
    assert!(drawn(cx, "workdir-popover-switch"));
}
