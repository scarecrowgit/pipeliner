//! The header: title, badges, the more menu's commands, the dialogs, and the share popover's
//! local exports.

use gpui::TestAppContext;
use hub::Route;

use super::{FIX, calls, click, open, shown};

#[gpui::test]
fn title_and_badges_come_from_the_session(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    assert_eq!(t.view.read_with(cx, |v, cx| v.title(cx)), "Fix double charge on retry");
    assert!(shown(cx, "chat-share") && shown(cx, "chat-new-session") && shown(cx, "chat-more"));
}

#[gpui::test]
fn the_share_popover_exports_through_the_api(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    click(cx, "chat-share");
    assert!(shown(cx, "chat-share-workspace"), "hosted sharing is drawn");
    click(cx, "chat-share-tool-output");
    assert!(!t.view.read_with(cx, |v, _| v.share.tool_output));
    click(cx, "chat-share-markdown");
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|dir| Some(dir.join("fix.md")));
    cx.run_until_parked();
    let exported = calls(&t.api, "export_session");
    assert_eq!(exported.len(), 1);
    assert!(exported[0].starts_with(&format!("{FIX} Markdown ")) && exported[0].ends_with("fix.md"));
}

#[gpui::test]
fn the_more_menu_pins_archives_renames_and_deletes(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    click(cx, "chat-more");
    click(cx, "chat-more-pin");
    assert_eq!(calls(&t.api, "pin_session"), [format!("{FIX} false")]);

    click(cx, "chat-more");
    click(cx, "chat-more-archive");
    assert_eq!(calls(&t.api, "archive_session"), [format!("{FIX} true")]);
    let reason = t.composer.read_with(cx, |c, _| c.disabled_reason().map(|r| r.to_string()));
    assert_eq!(reason.as_deref(), Some("Archived chat · restore it to reply"));

    click(cx, "chat-more");
    click(cx, "chat-more-rename");
    let input = t.view.read_with(cx, |v, _| v.rename.as_ref().map(|(input, _)| input.clone()));
    let Some(input) = input else { panic!("the rename dialog is open") };
    input.update(cx, |i, cx| i.set_text("Retry double charge", cx));
    click(cx, "chat-rename-save");
    assert_eq!(calls(&t.api, "rename_session"), [format!("{FIX} Retry double charge")]);

    click(cx, "chat-more");
    click(cx, "chat-more-delete");
    click(cx, "chat-delete-confirm");
    assert_eq!(calls(&t.api, "delete_session"), [FIX]);
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(route, Route::Chats, "a deleted chat leaves the screen");
}
