//! "Write with AI" in the new-issue dialog.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::{click, drawn, new_issue, open, recorded};
use crate::board::IssuesView;
use crate::board::new_issue::NewIssueForm;

fn form(view: &Entity<IssuesView>, cx: &mut VisualTestContext) -> Entity<NewIssueForm> {
    view.read_with(cx, |view, _| view.form.clone()).expect("the dialog is open")
}

fn drafts(api: &hub::fixtures::FixtureApi) -> usize {
    recorded(api).iter().filter(|c| c.starts_with("draft_description")).count()
}

fn body(form: &Entity<NewIssueForm>, cx: &mut VisualTestContext) -> String {
    form.read_with(cx, |form, cx| form.body.read(cx).text().to_string())
}

fn filed_body(api: &hub::fixtures::FixtureApi, title: &str) -> Option<String> {
    api.with_world(|w| w.issues.iter().find(|i| i.title == title).map(|i| i.body.clone()))
}

#[gpui::test]
fn a_draft_is_reviewed_kept_and_filed(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    assert!(drawn(cx, "new-issue-ai"));
    assert!(!drawn(cx, "new-issue-ai-draft"));
    cx.simulate_input("Saved card shows the wrong last 4");
    click(cx, "new-issue-ai");
    assert!(recorded(&api).iter().any(|c| c == "draft_description Saved card shows the wrong last 4"));
    assert!(drawn(cx, "new-issue-ai-draft"), "the draft is shown for review");
    assert!(drawn(cx, "new-issue-ai-keep") && drawn(cx, "new-issue-ai-undo") && drawn(cx, "new-issue-ai-retry"));

    click(cx, "new-issue-ai-keep");
    let form = form(&view, cx);
    let kept = body(&form, cx);
    assert!(kept.contains("### Done when"), "{kept}");
    // The caret is in the text area, which is editable now.
    cx.simulate_input("!");
    assert!(body(&form, cx).ends_with('!'));

    click(cx, "new-issue-create");
    let filed = filed_body(&api, "Saved card shows the wrong last 4").expect("filed");
    assert!(filed.contains("### Done when"), "{filed}");
}

#[gpui::test]
fn create_while_reviewing_files_the_draft(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    cx.simulate_input("Fix it");
    click(cx, "new-issue-ai");
    assert!(drawn(cx, "new-issue-ai-draft"));
    click(cx, "new-issue-create");
    let filed = filed_body(&api, "Fix it").expect("filed");
    assert!(filed.contains("### Done when"), "{filed}");
}

#[gpui::test]
fn undo_puts_the_note_back(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    cx.simulate_input("Fix it");
    let form = form(&view, cx);
    form.update(cx, |form, cx| form.body.update(cx, |area, cx| area.set_text("my note", cx)));
    click(cx, "new-issue-ai");
    assert!(drawn(cx, "new-issue-ai-draft"));
    assert_eq!(drafts(&api), 1);
    click(cx, "new-issue-ai-undo");
    assert_eq!(body(&form, cx), "my note");
    assert!(drawn(cx, "new-issue-ai"), "the footer is back");
    form.read_with(cx, |form, _| assert!(!form.ai_reviewing()));
}

#[gpui::test]
fn cmd_z_undoes_only_while_reviewing_and_ctrl_j_writes(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    let form = form(&view, cx);
    cx.simulate_input("Fix it");
    click(cx, "new-issue-body-frame");
    cx.simulate_input("a note");
    cx.simulate_keystrokes("ctrl-j");
    cx.run_until_parked();
    assert_eq!(drafts(&api), 1);
    assert!(drawn(cx, "new-issue-ai-draft"));
    form.read_with(cx, |form, _| assert!(form.ai_reviewing()));
    cx.simulate_keystrokes("ctrl-z");
    cx.run_until_parked();
    form.read_with(cx, |form, _| assert!(!form.ai_reviewing()));
    assert!(drawn(cx, "new-issue-ai"));
}

#[gpui::test]
fn a_second_press_while_writing_does_nothing(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    cx.simulate_input("Fix it");
    let form = form(&view, cx);
    form.update(cx, |form, cx| {
        form.ai_set_writing(true);
        cx.notify();
    });
    cx.simulate_keystrokes("ctrl-j");
    cx.run_until_parked();
    assert_eq!(drafts(&api), 0);
}

#[gpui::test]
fn nothing_to_write_from_is_said_in_place(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    click(cx, "new-issue-ai");
    assert_eq!(drafts(&api), 0);
    assert!(drawn(cx, "new-issue-ai-error"));
    let form = form(&view, cx);
    form.read_with(cx, |form, _| assert_eq!(form.ai_error(), Some("Write a title or a note first.")));
    assert!(!drawn(cx, "new-issue-ai-draft"));
}

#[gpui::test]
fn asking_for_changes_redrafts_from_the_draft(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    cx.simulate_input("Fix it");
    click(cx, "new-issue-ai");
    let form = form(&view, cx);
    let first = form.read_with(cx, |form, _| form.ai_draft().to_string());
    assert!(first.contains("### Steps to reproduce"));

    click(cx, "new-issue-ai-ask");
    cx.simulate_input("shorter");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(drafts(&api), 2);
    let second = form.read_with(cx, |form, _| form.ai_draft().to_string());
    assert!(!second.contains("### Steps to reproduce"), "{second}");
    assert!(second.contains("### Done when"));
    form.read_with(cx, |form, cx| assert_eq!(form.ai_ask_text(cx), "", "the field is cleared"));

    click(cx, "new-issue-ai-retry");
    assert_eq!(drafts(&api), 3);
    assert!(form.read_with(cx, |form, _| form.ai_draft().contains("### Steps to reproduce")));
}
