//! The header and the properties: what they show, the patch each picker sends, the runner
//! picker, the title edited in place, Start and Stop.

use gpui::TestAppContext;

use super::{calls, click, open, shown};

#[gpui::test]
fn properties_show_the_issue_and_each_picker_sends_its_patch(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    for probe in ["issue-key", "issue-title", "issue-prop-status", "issue-prop-priority", "issue-prop-owner", "issue-prop-type"] {
        assert!(shown(cx, probe), "{probe} is drawn");
    }
    assert!(shown(cx, "issue-runner-card"), "PAY triage is attached");

    click(cx, "issue-prop-status");
    click(cx, "issue-prop-status-4");
    click(cx, "issue-prop-priority");
    click(cx, "issue-prop-priority-0");
    click(cx, "issue-prop-owner");
    click(cx, "issue-prop-owner-1");
    click(cx, "issue-prop-type");
    click(cx, "issue-prop-type-2");
    click(cx, "issue-trigger-1");
    let patches = calls(&t.api, "update_issue");
    assert_eq!(patches.len(), 5, "{patches:?}");
    assert!(patches[0].contains("status: Some(Done)"), "{}", patches[0]);
    assert!(patches[1].contains("priority: Some(High)"), "{}", patches[1]);
    assert!(patches[2].contains("owner: Some(\"Alex Rivera\")"), "{}", patches[2]);
    assert!(patches[3].contains("kind: Some(Chore)"), "{}", patches[3]);
    assert!(patches[4].contains("on_comment: true"), "{}", patches[4]);

    let status = t.panel.read_with(cx, |p, cx| p.current(cx).map(|i| i.status));
    assert_eq!(status, Some(hub::api::IssueStatus::Done), "the panel follows the change");
}

#[gpui::test]
fn the_runner_picker_lists_the_catalog_and_attaches_the_choice(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1440");
    assert!(!shown(cx, "issue-runner-card"), "nothing is attached");
    click(cx, "issue-prop-pipeline");
    assert!(shown(cx, "issue-runner-0"), "the picker lists runnables");
    cx.simulate_input("triage");
    cx.run_until_parked();
    let query = t.panel.read_with(cx, |p, cx| p.search.read(cx).text().to_string());
    assert_eq!(query, "triage", "typing goes to the search field");
    assert!(shown(cx, "issue-runner-0"));
    // `debug_bounds` keeps elements of earlier frames: count the rows from the picker's data.
    let rows = t.panel.read_with(cx, |p, cx| super::super::pickers::runner_choices(p.catalog.read(cx), "triage").len());
    assert_eq!(rows, 1, "only PAY triage matches");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "attach_runner"), ["PAY-1440 Some(RunnableRef(\"pipeline:pay-triage\"))"]);
    assert!(shown(cx, "issue-runner-card"), "the card replaces the attach button");

    click(cx, "issue-detach");
    assert_eq!(calls(&t.api, "attach_runner").len(), 2);
    let runner = t.panel.read_with(cx, |p, cx| p.current(cx).and_then(|i| i.runner.clone()));
    assert_eq!(runner, None, "detached");
}

#[gpui::test]
fn the_title_is_edited_in_place(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    click(cx, "issue-title");
    assert!(shown(cx, "issue-title-field"));
    cx.simulate_input("Refund webhook times out on big refunds");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let patches = calls(&t.api, "update_issue");
    assert_eq!(patches.len(), 1);
    assert!(patches[0].contains("title: Some(\"Refund webhook times out on big refunds\")"), "{}", patches[0]);
    assert!(shown(cx, "issue-title"), "the field gives way to the title");
}

#[gpui::test]
fn stop_cancels_the_open_run(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(!shown(cx, "issue-start"));
    click(cx, "issue-stop");
    assert_eq!(calls(&t.api, "cancel_run"), ["run-1427"]);
}

#[gpui::test]
fn start_starts_the_attached_runner(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1438");
    assert!(!shown(cx, "issue-stop"));
    click(cx, "issue-start");
    assert_eq!(calls(&t.api, "start_issue"), ["PAY-1438"]);
}

#[gpui::test]
fn delete_from_the_more_menu_asks_then_closes(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1440");
    click(cx, "issue-more");
    click(cx, "issue-more-delete");
    click(cx, "issue-delete-confirm");
    assert_eq!(calls(&t.api, "delete_issue"), ["PAY-1440"]);
    assert_eq!(t.closes.get(), 1);
}
