//! The board, the list, filters and the keyboard.

use gpui::{AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hub::api::{Event, IssueStatus, NodeStatus, RunState};
use hub::model::{IssueKey, RunId};
use hub::Route;

use super::{click, drawn, open, open_with, recorded, route};
use crate::board::{IssuesView, Layout};
use crate::board::card::{StripTone, strip};
use crate::board::filters::RunsWith;

fn key(k: &str) -> IssueKey {
    IssueKey::new(k)
}

#[gpui::test]
fn columns_follow_the_mockup_with_counts(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, Some("PAY-1440"));
    view.read_with(cx, |view, cx| {
        let columns = view.columns(cx);
        let shape: Vec<(IssueStatus, usize)> = columns.iter().map(|c| (c.status, c.cards.len())).collect();
        assert_eq!(
            shape,
            vec![
                (IssueStatus::Backlog, 2),
                (IssueStatus::Todo, 1),
                (IssueStatus::InProgress, 3),
                (IssueStatus::InReview, 2),
                (IssueStatus::Done, 2),
            ]
        );
        assert_eq!(columns[3].needs_you, 2, "both reviews wait for a person");
        assert_eq!(view.issues.read(cx).open_count(), 8);
    });
    assert!(drawn(cx, "issues-col-in-review"));
    assert!(drawn(cx, "issue-card-PAY-1423"));
    let badges = cx.update(|window, cx| view.update(cx, |view, cx| view.header_badges(window, cx).len()));
    assert_eq!(badges, 1, "the open count");
}

#[gpui::test]
fn a_card_shows_its_run_and_follows_run_changes(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    let line = |cx: &mut gpui::VisualTestContext| {
        view.read_with(cx, |view, cx| {
            let issue = view.issues.read(cx).issue(&key("PAY-1427")).cloned().expect("PAY-1427");
            let run = view.runs.read(cx).latest_for(&key("PAY-1427")).cloned();
            strip(&issue, run.as_deref()).expect("a run line")
        })
    };
    let running = line(cx);
    assert_eq!(running.tone, StripTone::Running);
    assert_eq!(running.text, "Reproducer running");
    assert_eq!(running.spawned.as_deref(), Some("Log digger spawned"));
    assert!(running.progress.is_some());

    let mut run = api
        .with_world(|world| world.runs.iter().find(|run| run.id == RunId::new("run-1427")).cloned())
        .expect("run-1427");
    run.state = RunState::WaitingAnswer;
    if let Some(node) = run.nodes.iter_mut().find(|node| node.name == "Reproducer") {
        node.status = NodeStatus::Waiting;
        node.question = Some("which log sinks are in scope?".into());
    }
    api.emit(Event::RunChanged { run });
    cx.run_until_parked();
    let waiting = line(cx);
    assert_eq!(waiting.tone, StripTone::Waiting);
    assert_eq!(waiting.text, "asks: which log sinks are in scope?");
    view.read_with(cx, |view, cx| assert_eq!(view.columns(cx)[2].needs_you, 1, "the column now needs you"));
}

#[gpui::test]
fn clicking_a_card_opens_it_and_selects_it(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, Some("PAY-1440"));
    click(cx, "issue-card-PAY-1438");
    assert_eq!(route(cx), Route::Issues { issue: Some(key("PAY-1438")) });
    view.read_with(cx, |view, _| {
        assert_eq!(view.issue(), Some(&key("PAY-1438")));
        assert_eq!(view.cursor, Some(key("PAY-1438")));
        assert!(view.panel.is_some(), "the panel is open");
    });
}

#[gpui::test]
fn dropping_a_card_on_another_column_sends_the_status(cx: &mut TestAppContext) {
    let (api, _view, cx) = open(cx, None);
    cx.run_until_parked();
    let from = cx.debug_bounds("issue-card-PAY-1438").expect("the card").center();
    let to = cx.debug_bounds("issues-col-in-review").expect("the column").center();
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    let mid = gpui::point(from.x + gpui::px(40.), from.y + gpui::px(10.));
    cx.simulate_mouse_move(mid, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let sent = recorded(&api);
    assert!(
        sent.iter().any(|c| c.starts_with("update_issue PAY-1438") && c.contains("status: Some(InReview)")),
        "the status patch was sent: {sent:?}"
    );
    assert_ne!(route(cx), Route::Issues { issue: Some(key("PAY-1438")) }, "a drag is not a click");
}

#[gpui::test]
fn dropping_on_the_same_column_sends_nothing(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    view.update(cx, |view, cx| view.move_issue(&key("PAY-1438"), IssueStatus::Todo, cx));
    cx.run_until_parked();
    assert!(!recorded(&api).iter().any(|c| c.starts_with("update_issue")));
}

#[gpui::test]
fn filters_narrow_the_board_and_the_list(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, None);
    let board = |cx: &mut gpui::VisualTestContext| {
        view.read_with(cx, |view, cx| view.columns(cx).iter().map(|c| c.cards.len()).sum::<usize>())
    };
    let list = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |view, cx| view.rows(cx).len());
    assert_eq!((board(cx), list(cx)), (10, 10));

    click(cx, "issues-chip-pipelines");
    view.read_with(cx, |view, _| assert_eq!(view.filters.runs_with, RunsWith::Pipelines));
    assert_eq!((board(cx), list(cx)), (3, 3));

    view.update(cx, |view, cx| view.edit_filters(|f| f.runs_with = RunsWith::All, cx));
    view.update(cx, |view, cx| view.search.update(cx, |input, cx| input.set_text("refund", cx)));
    // `set_text` does not emit `Changed`: the filter follows typing, so set it as typing would.
    view.update(cx, |view, cx| view.edit_filters(|f| f.text = "refund".into(), cx));
    let keys = view.read_with(cx, |view, cx| view.rows(cx).iter().map(|i| i.key.to_string()).collect::<Vec<_>>());
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.contains(&"PAY-1427".to_string()) && keys.contains(&"PAY-1442".to_string()));
    assert_eq!(board(cx), 2);

    view.update(cx, |view, cx| {
        view.edit_filters(|f| f.text.clear(), cx);
        view.edit_filters(|f| f.toggle_owner("Nina Kim"), cx);
    });
    assert_eq!((board(cx), list(cx)), (2, 2));

    click(cx, "issues-view-list");
    assert_eq!(view.read_with(cx, |view, cx| view.layout(cx)), Layout::List);
    assert!(drawn(cx, "issues-row-PAY-1436"));
    assert!(!drawn(cx, "issues-row-PAY-1440"), "filtered out of the list too");
}

#[gpui::test]
fn the_layout_is_remembered_for_the_session(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, None);
    click(cx, "issues-view-list");
    let again = cx.update(|window, cx| cx.new(|cx| crate::IssuesView::new(None, window, cx)));
    assert_eq!(again.read_with(cx, |view, cx| view.layout(cx)), Layout::List);
    click(cx, "issues-view-board");
    assert_eq!(view.read_with(cx, |view, cx| view.layout(cx)), Layout::Board);
}

#[gpui::test]
fn arrows_walk_the_cards_and_enter_opens(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, Some("PAY-1440"));
    // The panel takes focus when it opens; a press on the board gives it back.
    cx.update(|window, cx| window.focus(&view.read(cx).focus));
    // Backlog lists PAY-1442 (Medium) above PAY-1440 (Low).
    cx.simulate_keystrokes("up");
    view.read_with(cx, |view, _| assert_eq!(view.cursor, Some(key("PAY-1442"))));
    cx.simulate_keystrokes("right");
    view.read_with(cx, |view, _| assert_eq!(view.cursor, Some(key("PAY-1438"))));
    cx.simulate_keystrokes("right down");
    view.read_with(cx, |view, _| assert_eq!(view.cursor, Some(key("PAY-1427")), "second card of In progress"));
    cx.simulate_keystrokes("enter");
    assert_eq!(route(cx), Route::Issues { issue: Some(key("PAY-1427")) });
    view.read_with(cx, |view, _| assert_eq!(view.issue(), Some(&key("PAY-1427"))));
}

#[gpui::test]
fn the_empty_project_offers_a_first_issue(cx: &mut TestAppContext) {
    let (_api, _view, cx) = open_with(cx, None, |world| world.issues.clear());
    assert!(drawn(cx, "issues-empty-new"));
    click(cx, "issues-empty-new");
    assert!(drawn(cx, "new-issue-create"));
}

#[gpui::test]
fn list_headers_sort_the_rows(cx: &mut TestAppContext) {
    let (_api, view, cx) = open(cx, None);
    view.update(cx, |view, cx| view.set_layout(Layout::List, cx));
    cx.run_until_parked();
    let keys = |view: &Entity<IssuesView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |view, cx| view.rows(cx).iter().map(|i| i.key.to_string()).collect::<Vec<_>>())
    };
    // Key is the first column: a click sorts by it, a second click turns the order round.
    click(cx, "issues-table-head-0");
    let up = keys(&view, cx);
    let mut sorted = up.clone();
    sorted.sort();
    assert_eq!(up, sorted);
    click(cx, "issues-table-head-0");
    let down = keys(&view, cx);
    sorted.reverse();
    assert_eq!(down, sorted);
}

#[gpui::test]
fn a_long_error_title_and_name_keep_the_card_small(cx: &mut TestAppContext) {
    let path: String = "/home/user/Pictures/designs/pipeliner-mockups/".repeat(5);
    let error = format!(
        "Merge mockup failed: `main` is checked out in {path} with changes the merge would overwrite; nothing was \
         merged (`git reset --keep --quiet 937a0a362fc0fea6262662a4c74670004693c29e` failed (exit 128): fatal: \
         Cannot do a keep reset in the middle of a merge.)"
    );
    let (_api, _view, cx) = open_with(cx, None, move |world| {
        if let Some(run) = world.runs.iter_mut().find(|run| run.id == RunId::new("run-1427")) {
            run.state = RunState::Failed;
            run.error = Some(error);
        }
        if let Some(issue) = world.issues.iter_mut().find(|issue| issue.key == key("PAY-1427")) {
            issue.title = "x".repeat(300);
            issue.filed_by = Some(hub::api::RunnableRef::agent(&"log-digger-".repeat(12)));
        }
    });
    let Some(card) = cx.debug_bounds("issue-card-PAY-1427") else { panic!("the card was not laid out") };
    assert!(f32::from(card.size.width) <= crate::board::card::CARD_WIDTH + 0.5, "{card:?}");
    assert!(f32::from(card.size.height) < 280., "title and run line are capped: {card:?}");
}

#[gpui::test]
fn the_filter_bar_stays_inside_a_narrow_window(cx: &mut TestAppContext) {
    let (_api, _view, cx) = open(cx, Some("PAY-1440"));
    // A 1000px window less the sidebar, with the panel open.
    cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(900.)));
    cx.run_until_parked();
    assert!(drawn(cx, "issues-chip-unattached"));
    let Some(bar) = cx.debug_bounds("issues-bar-filters") else { panic!("the filters are not laid out") };
    assert!(f32::from(bar.right()) <= 768. - ui::theme::SPACE_5 + 0.5, "the chips scroll inside the bar: {bar:?}");
    assert!(f32::from(bar.size.width) > 0.);
}

#[gpui::test]
fn the_list_keeps_the_columns_that_fit(cx: &mut TestAppContext) {
    use crate::board::list::Fit;
    assert_eq!(Fit::of(0., false), Fit::Full, "unmeasured, without the panel");
    assert_eq!(Fit::of(0., true), Fit::Compact, "unmeasured, beside the panel");
    assert_eq!(Fit::of(1168., true), Fit::Full);
    assert_eq!(Fit::of(728., false), Fit::Compact, "1000px window, sidebar open, no panel");
    assert_eq!(Fit::of(328., true), Fit::Title, "1000px window with the panel open");

    let (_api, view, cx) = open(cx, Some("PAY-1440"));
    view.update(cx, |view, cx| view.set_layout(Layout::List, cx));
    cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(900.)));
    cx.run_until_parked();
    let width = view.read_with(cx, |view, _| view.list_width.get());
    assert!((width - (768. - 400. - 2. * ui::theme::SPACE_5)).abs() < 2., "the list measured its room: {width}");
    assert!(drawn(cx, "issues-row-PAY-1440"), "the rows are drawn");
}
