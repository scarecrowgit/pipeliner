//! The new-issue dialog and the autopilots popover.

use gpui::{Entity, TestAppContext, VisualTestContext};
use hub::Route;
use hub::api::{IssueStatus, IssueType, Priority, RunnableRef};
use hub::model::ProjectId;

use super::{autopilots, click, drawn, new_issue, open, recorded, route};
use crate::board::IssuesView;
use crate::board::new_issue::{NewIssueForm, StartWhen};

fn form(view: &Entity<IssuesView>, cx: &mut VisualTestContext) -> Entity<NewIssueForm> {
    view.read_with(cx, |view, _| view.form.clone()).expect("the dialog is open")
}

#[gpui::test]
fn typing_reaches_the_title_and_create_files_the_issue(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, Some("PAY-1440"));
    new_issue(&view, cx);
    assert!(drawn(cx, "new-issue-create"));
    assert!(!drawn(cx, "new-issue-create-start"), "nothing attached, nothing to start");
    cx.simulate_input("Saved card shows the wrong last 4");
    let form = form(&view, cx);
    form.read_with(cx, |form, cx| assert_eq!(form.title.read(cx).text(), "Saved card shows the wrong last 4"));

    form.update(cx, |form, cx| {
        form.priority = Priority::High;
        form.kind = IssueType::Bug;
        cx.notify();
    });
    let new = form.read_with(cx, |form, cx| form.build(cx)).expect("a valid issue");
    assert_eq!(new.project, ProjectId::new("payments"));
    assert_eq!(new.title, "Saved card shows the wrong last 4");
    assert_eq!((new.status, new.priority, new.kind), (IssueStatus::Backlog, Priority::High, IssueType::Bug));
    assert_eq!(new.owner, "You");
    assert_eq!(new.repo.as_deref(), Some("payments-api"));

    click(cx, "new-issue-create");
    let sent = recorded(&api);
    assert!(sent.iter().any(|c| c == "create_issue Saved card shows the wrong last 4"), "{sent:?}");
    assert!(!sent.iter().any(|c| c.starts_with("start_issue")));
    let Route::Issues { issue: Some(created) } = route(cx) else { panic!("the new issue is open") };
    assert!(view.read_with(cx, |view, cx| view.issues.read(cx).issue(&created).is_some()));
    let open = cx.update(|window, cx| ui::overlays(window, cx).is_open(crate::board::new_issue::DIALOG, cx));
    assert!(!open, "the dialog closed");
}

#[gpui::test]
fn an_empty_title_is_refused_inline(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    click(cx, "new-issue-create");
    let form = form(&view, cx);
    form.read_with(cx, |form, _| assert_eq!(form.error.as_ref().map(|e| e.as_ref()), Some("Give the issue a title.")));
    assert!(!recorded(&api).iter().any(|c| c.starts_with("create_issue")));
    assert!(drawn(cx, "new-issue-create"), "the dialog stays open");
    cx.simulate_input("x");
    form.read_with(cx, |form, _| assert!(form.error.is_none(), "typing clears the message"));
}

#[gpui::test]
fn create_and_start_starts_the_attached_runner(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    click(cx, "issues-add-todo");
    cx.simulate_input("Bump the ledger client");
    let form = form(&view, cx);
    form.update(cx, |form, cx| {
        form.runner = Some(RunnableRef::pipeline("pay-triage"));
        cx.notify();
    });
    let new = form.read_with(cx, |form, cx| form.build(cx)).expect("a valid issue");
    assert_eq!(new.status, IssueStatus::Todo, "the column's + files into that column");
    assert_eq!(new.runner, Some(RunnableRef::pipeline("pay-triage")));
    assert_eq!(new.triggers, StartWhen::RightAway.triggers());
    assert!(!new.triggers.on_assign, "the start is the explicit one");

    // Cmd-Enter runs the primary action, which starts right away.
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    let sent = recorded(&api);
    let created = sent.iter().position(|c| c == "create_issue Bump the ledger client").expect("created");
    let started = sent.iter().position(|c| c.starts_with("start_issue")).expect("started");
    assert!(created < started, "{sent:?}");
}

#[gpui::test]
fn create_more_keeps_the_dialog_for_the_next_one(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    new_issue(&view, cx);
    let form = form(&view, cx);
    form.update(cx, |form, cx| {
        form.create_more = true;
        form.start = StartWhen::OnTodo;
        cx.notify();
    });
    cx.simulate_input("First");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(recorded(&api).iter().any(|c| c == "create_issue First"));
    assert!(drawn(cx, "new-issue-create"), "still open");
    form.read_with(cx, |form, cx| assert_eq!(form.title.read(cx).text(), ""));
    cx.simulate_input("Second");
    form.read_with(cx, |form, cx| assert_eq!(form.title.read(cx).text(), "Second", "the title has focus again"));
}

#[gpui::test]
fn the_autopilots_popover_lists_and_switches_them(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    autopilots(&view, cx);
    assert!(drawn(cx, "issues-autopilot-0"));
    assert!(drawn(cx, "issues-autopilot-new"), "drawn as coming soon");
    click(cx, "issues-autopilot-toggle-0");
    let sent = recorded(&api);
    assert!(sent.iter().any(|c| c == "save_autopilot Nightly e2e triage"), "{sent:?}");
    let enabled = api.with_world(|w| w.autopilots.iter().find(|a| a.name == "Nightly e2e triage").map(|a| a.enabled));
    assert_eq!(enabled, Some(false));
}
