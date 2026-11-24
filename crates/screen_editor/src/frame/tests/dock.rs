//! The dock: the Problems tab, test runs and their log.

use gpui::TestAppContext;
use hub::api::{Event, RunLogLine};
use hub::model::{LogLevel, NodeId};

use super::{PAY, calls, click, edit, open, shown, wait_check};
use crate::Selection;
use crate::frame::DockTab;

#[gpui::test]
async fn clicking_a_problem_selects_its_node(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    click(cx, "dock-tabs-2");
    t.view.read_with(cx, |v, _| assert_eq!(v.dock_tab(), DockTab::Problems));
    click(cx, "problem-0");
    t.view.read_with(cx, |v, cx| {
        assert_eq!(v.draft().read(cx).selection(), &Selection::Node(NodeId::new("jira")));
    });
}

#[gpui::test]
async fn the_dock_folds_to_its_tabs(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert!(shown(cx, "test-run-new"), "the last test run is on show");
    click(cx, "dock-collapse");
    t.view.read_with(cx, |v, _| assert!(v.dock.collapsed));
    click(cx, "dock-tabs-1");
    t.view.read_with(cx, |v, _| {
        assert!(!v.dock.collapsed, "a tab unfolds the dock");
        assert_eq!(v.dock_tab(), DockTab::Log);
    });
}

/// Types a title and starts a test run from the tab.
fn start(t: &super::Open, cx: &mut gpui::VisualTestContext) {
    if shown(cx, "test-run-new") {
        // The last test run is on show: back to the item's fields first.
        click(cx, "test-run-new");
    }
    let title = t.view.read_with(cx, |v, _| v.test.title.clone());
    title.update(cx, |input, cx| input.set_text("Refund webhook times out", cx));
    cx.run_until_parked();
    click(cx, "test-run-start");
}

#[gpui::test]
async fn a_test_run_starts_on_the_saved_draft_and_its_log_arrives(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Rename", |g| g.name = "Unsaved yet".into());
    start(&t, cx);
    let names = t.api.recorded_names();
    let save = names.iter().position(|n| *n == "save_pipeline").expect("the draft was saved");
    let run = names.iter().position(|n| *n == "test_run").expect("a test run started");
    assert!(save < run, "saved before the run: {names:?}");
    assert_eq!(calls(&t.api, "test_run"), [format!("{PAY} Refund webhook times out")]);
    let id = t.view.read_with(cx, |v, cx| v.draft().read(cx).run().cloned()).expect("the run is on show");
    assert!(shown(cx, "test-run-stop"));

    let line = |seq: u64, text: &str| RunLogLine {
        seq,
        at: super::NOW,
        node: Some(NodeId::new("fixer")),
        variant: 0,
        level: LogLevel::Info,
        text: text.to_string(),
    };
    t.api.emit(Event::RunLog { run: id.clone(), line: line(1, "worktree ready") });
    t.api.emit(Event::RunLog { run: id.clone(), line: line(2, "read src/refunds.ts") });
    cx.run_until_parked();
    click(cx, "dock-tabs-1");
    t.view.read_with(cx, |v, cx| {
        let lines: Vec<String> = v.log_lines(cx).iter().map(|l| l.text.clone()).collect();
        assert_eq!(lines, ["worktree ready", "read src/refunds.ts"]);
        assert_eq!(v.log_count(cx), 2);
    });
}

#[gpui::test]
async fn stop_cancels_the_run_on_show(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    start(&t, cx);
    let id = t.view.read_with(cx, |v, cx| v.draft().read(cx).run().cloned()).expect("the run is on show");
    click(cx, "test-run-stop");
    assert_eq!(calls(&t.api, "cancel_run"), [id.to_string()]);
    assert!(shown(cx, "test-run-new"), "a stopped run offers another");
}

#[gpui::test]
async fn the_header_button_asks_for_a_title_first(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    click(cx, "editor-test-run");
    assert!(calls(&t.api, "test_run").is_empty());
    let focused = t.view.update_in(cx, |v, window, cx| v.test.title.read(cx).is_focused(window));
    assert!(focused, "the title field takes focus");
}

#[gpui::test]
async fn the_last_test_run_is_on_show_after_a_restart(cx: &mut TestAppContext) {
    // The fixture's PAY triage has "Test run #42" from yesterday.
    let (t, cx) = open(cx);
    let shown_run = t.view.read_with(cx, |v, cx| v.draft().read(cx).run().cloned());
    assert_eq!(shown_run.as_ref().map(|id| id.as_str()), Some("run-test-42"), "the last test run is on show");
    assert!(shown(cx, "test-run-new"), "a finished run offers a new one");
    // Leaving it for a new run does not bring it back.
    click(cx, "test-run-new");
    cx.run_until_parked();
    assert!(t.view.read_with(cx, |v, cx| v.draft().read(cx).run().is_none()));
    assert!(shown(cx, "test-run-start"));
}

#[gpui::test]
async fn a_log_line_can_be_copied_and_a_long_one_wraps(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    start(&t, cx);
    let id = t.view.read_with(cx, |v, cx| v.draft().read(cx).run().cloned()).expect("the run is on show");
    let long = format!("bash {}", "cargo test --workspace --all-features ".repeat(8));
    let texts = ["worktree ready", "read src/refunds.ts", long.as_str()];
    for (ix, text) in texts.iter().enumerate() {
        let line = RunLogLine {
            seq: ix as u64 + 1,
            at: super::NOW,
            node: Some(NodeId::new("fixer")),
            variant: 0,
            level: if ix == 2 { LogLevel::Tool } else { LogLevel::Info },
            text: text.to_string(),
        };
        t.api.emit(Event::RunLog { run: id.clone(), line });
    }
    cx.run_until_parked();
    click(cx, "dock-tabs-1");
    assert_eq!(super::narrow::drag_and_copy(cx, "log-text-1").as_deref(), Some("read src/refunds.ts"));
    // The long line wraps inside the dock instead of running past it.
    let Some(editor) = cx.debug_bounds("editor") else { panic!("the editor was not laid out") };
    let Some(text) = cx.debug_bounds("log-text-2") else { panic!("the long line was not laid out") };
    assert!(text.right() <= editor.right() - gpui::px(ui::theme::PANEL_WIDTH), "{text:?}");
    assert!(text.size.height > gpui::px(ui::theme::LABEL.line), "{text:?}");
    assert_eq!(super::narrow::drag_and_copy(cx, "log-text-2"), Some(long));
}

#[gpui::test]
async fn an_empty_log_says_so(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert!(shown(cx, "test-run-new"), "the last test run is on show");
    click(cx, "dock-tabs-1");
    assert_eq!(t.view.read_with(cx, |v, cx| v.log_count(cx)), 0);
    assert!(shown(cx, "log-empty"), "not a blank list");
}
