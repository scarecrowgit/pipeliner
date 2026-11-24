//! Opening what the list shows: a double-click on a row (or on the panel's head, or on a
//! runnable named in the panel) does what Enter does; controls in the head do not; dialogs
//! confirm with Cmd/Ctrl-Enter; long names, a narrow list and load errors stay readable.

use std::path::PathBuf;

use gpui::{TestAppContext, px, size};
use hub::Route;
use hub::api::{LoadProblem, PipelineId};
use hub::model::RunnableRef;

use super::{bounds, calls, click, double_click, is_open, lead, open, open_with, route, selected, shown};
use crate::dialogs::Form;
use crate::edit::EDIT_DIALOG;
use crate::list::Columns;

/// The reference of the open edit dialog, if one is open.
fn editing(t: &super::Open, cx: &mut gpui::VisualTestContext) -> Option<RunnableRef> {
    t.view.read_with(cx, |v, _| match v.form.as_ref() {
        Some(Form::Edit(form)) => Some(form.reference.clone()),
        _ => None,
    })
}

#[gpui::test]
fn a_double_click_on_a_pipeline_row_opens_the_editor(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    click(cx, "agents-row-pipeline:nightly-audit");
    assert_eq!(route(cx), Route::Agents { runnable: Some(RunnableRef::pipeline("nightly-audit")) }, "one click only selects");

    double_click(cx, "agents-row-pipeline:pay-triage");
    assert_eq!(route(cx), Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
    assert_eq!(selected(&t, cx), Some(RunnableRef::pipeline("pay-triage")), "the row is selected too");
}

#[gpui::test]
fn a_double_click_on_an_agent_row_opens_its_edit_dialog(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    double_click(cx, "agents-row-agent:fixer");
    assert!(is_open(cx, EDIT_DIALOG), "the edit dialog is open");
    assert_eq!(editing(&t, cx), Some(RunnableRef::agent("fixer")));
    assert_eq!(route(cx), Route::Agents { runnable: Some(RunnableRef::agent("fixer")) }, "it stays on the screen");
    cx.simulate_keystrokes("escape");
    assert!(!is_open(cx, EDIT_DIALOG), "escape closes the dialog");
}

#[gpui::test]
fn enter_on_a_selected_agent_opens_its_edit_dialog(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    click(cx, "agents-row-agent:reproducer");
    cx.simulate_keystrokes("enter");
    assert_eq!(editing(&t, cx), Some(RunnableRef::agent("reproducer")));
    assert!(is_open(cx, EDIT_DIALOG));
    cx.simulate_keystrokes("escape");
    assert!(!is_open(cx, EDIT_DIALOG), "escape closes the dialog");
}

#[gpui::test]
fn a_double_click_on_the_panel_head_opens_but_not_on_its_controls(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    double_click(cx, "agents-more-orchestrator:payments-lead");
    assert!(!is_open(cx, EDIT_DIALOG), "a double-click on the more button does not open the editor");
    cx.simulate_keystrokes("escape");
    assert!(!is_open(cx, "agents-more-orchestrator:payments-lead"), "escape closes the menu");

    double_click(cx, "agents-head");
    assert_eq!(editing(&t, cx), Some(lead()), "the head's name opens the edit dialog");
    cx.simulate_keystrokes("escape");
    assert!(!is_open(cx, EDIT_DIALOG), "escape closes the dialog");

    click(cx, "agents-row-pipeline:pay-triage");
    double_click(cx, "agents-head");
    assert_eq!(route(cx), Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
}

#[gpui::test]
fn rename_import_and_export_confirm_with_cmd_enter(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    let more = "agents-more-orchestrator:payments-lead";

    click(cx, more);
    click(cx, "agents-more-rename");
    cx.simulate_input("Payments captain");
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(calls(&t.api, "rename_runnable"), ["orchestrator:payments-lead Payments captain"]);
    assert!(!is_open(cx, "agents-rename-dialog"));

    click(cx, "agents-new");
    click(cx, "agents-new-import");
    cx.simulate_input("~/agents/reviewer.yaml");
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(calls(&t.api, "import_runnable"), ["/home/me/agents/reviewer.yaml"]);
    assert!(!is_open(cx, "agents-import-dialog"));

    click(cx, "agents-row-agent:fixer");
    click(cx, "agents-more-agent:fixer");
    click(cx, "agents-more-export");
    cx.simulate_input("~/Downloads");
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(calls(&t.api, "export_runnable"), ["agent:fixer /home/me/Downloads"]);
    assert!(!is_open(cx, "agents-export-dialog"));
}

#[gpui::test]
fn a_long_name_leaves_its_kind_in_the_name_column(cx: &mut TestAppContext) {
    let long = "Payments lead who also owns refunds, chargebacks, dispute evidence and the quarterly reconciliation report for every acquiring bank";
    let (_t, cx) = open_with(cx, Some(lead()), |world| {
        if let Some(r) = world.runnables.iter_mut().find(|r| r.reference == lead()) {
            r.name = long.to_string();
        }
    });
    let row = bounds(cx, "agents-row-orchestrator:payments-lead");
    let kind = bounds(cx, "agents-kind-orchestrator:payments-lead");
    // State, Activity and Used from take 438px at the right of the row.
    assert!(f32::from(kind.right()) <= f32::from(row.right()) - 438., "the kind label stays left of State: {kind:?} in {row:?}");

    let head = bounds(cx, "agents-head");
    let more = bounds(cx, "agents-more-orchestrator:payments-lead");
    assert!(head.right() <= more.left(), "the head's name leaves room for the more button");
    assert!(more.right() <= px(1440.), "the more button stays in the window");
}

#[gpui::test]
fn a_narrow_list_leaves_out_columns(cx: &mut TestAppContext) {
    assert_eq!(Columns::for_width(1020.), Columns { state: true, activity: true, uses: true });
    assert_eq!(Columns::for_width(600.), Columns { state: true, activity: true, uses: false });
    assert_eq!(Columns::for_width(420.), Columns { state: true, activity: false, uses: false });
    assert_eq!(Columns::for_width(300.), Columns { state: false, activity: false, uses: false });

    // A 1000px window with the 232px sidebar open: the screen is 768 wide, the list 348.
    let (t, cx) = open(cx, Some(lead()));
    cx.simulate_resize(size(px(768.), px(1200.)));
    cx.run_until_parked();
    let width = t.view.read_with(cx, |v, _| v.list_w.get());
    assert!((width - 348.).abs() < 1., "the list measured its width: {width}");
    let row = bounds(cx, "agents-row-agent:fixer");
    let kind = bounds(cx, "agents-kind-agent:fixer");
    assert!(kind.right() <= row.right(), "the name keeps its room: {kind:?} in {row:?}");
    let badges = cx.update(|window, cx| t.view.update(cx, |v, cx| v.header_badges(window, cx).len()));
    assert_eq!(badges, 0, "the header's line is left out so \"New\" stays in the window");
    assert!(bounds(cx, "agents-new").right() <= px(768.));
}

#[gpui::test]
fn a_load_error_wraps_and_can_be_selected(cx: &mut TestAppContext) {
    let message = "expected a mapping at line 3, column 7, while parsing the `tools` block of an agent definition: found a sequence of 4 items instead. \
        The library reads every file under agents/, orchestrators/ and pipelines/ and skips the ones it cannot parse; \
        this one declares `tools:` as a list of names where a mapping with `access`, `allow` and `deny` is expected.";
    let (t, cx) = open(cx, None);
    t.view.update(cx, |v, cx| {
        v.problems_override = Some(vec![LoadProblem {
            path: PathBuf::from("/home/me/.config/harness/library/agents/broken.yaml"),
            message: message.into(),
        }]);
        cx.notify();
    });
    assert!(shown(cx, "agents-problems"));
    let notice = bounds(cx, "agents-problems");
    let text = bounds(cx, "agents-problem-message-0");
    assert!(text.right() <= notice.right(), "the message stays inside the notice");
    assert!(text.size.height > px(20.), "the long message wraps instead of being cut: {text:?}");

    let from = gpui::point(text.left() + px(1.), text.top() + px(8.));
    let to = gpui::point(text.right() - px(4.), text.top() + px(8.));
    cx.simulate_mouse_down(from, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(to, Some(gpui::MouseButton::Left), gpui::Modifiers::none());
    cx.simulate_mouse_up(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();
    let view = t.view.read_with(cx, |v, _| v.texts.get("problem-message-0").cloned()).expect("the message is a text view");
    let picked = view.read_with(cx, |md, _| md.selected_text()).unwrap_or_default();
    assert!(picked.starts_with("expected a mapping"), "a drag selects the message: {picked:?}");
}

#[gpui::test]
fn the_edit_dialog_closes_on_escape_and_saves_from_its_fields(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(RunnableRef::agent("fixer")));
    cx.simulate_keystrokes("e");
    assert!(is_open(cx, EDIT_DIALOG));
    cx.simulate_keystrokes("escape");
    assert!(!is_open(cx, EDIT_DIALOG), "escape in the first field closes the dialog");
    assert!(calls(&t.api, "save_runnable").is_empty());

    cx.simulate_keystrokes("e");
    cx.simulate_keystrokes("end");
    cx.simulate_input(" Quickly.");
    cx.simulate_keystrokes("enter");
    assert!(!is_open(cx, EDIT_DIALOG), "enter in a line saves and closes");
    assert_eq!(calls(&t.api, "save_runnable"), ["agent:fixer"]);

    cx.simulate_keystrokes("e");
    let role = t.view.read_with(cx, |v, _| match v.form.as_ref() {
        Some(Form::Edit(form)) => form.role.clone(),
        _ => panic!("the edit form is open"),
    });
    cx.update(|window, cx| role.read(cx).focus(window));
    cx.simulate_keystrokes("ctrl-enter");
    assert!(!is_open(cx, EDIT_DIALOG), "cmd-enter in the instructions saves and closes");
    assert_eq!(calls(&t.api, "save_runnable").len(), 2);
}

#[gpui::test]
fn an_empty_name_keeps_the_rename_dialog_open(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    click(cx, "agents-more-orchestrator:payments-lead");
    click(cx, "agents-more-rename");
    cx.simulate_keystrokes("backspace");
    cx.simulate_keystrokes("ctrl-enter");
    assert!(is_open(cx, "agents-rename-dialog"), "nothing to rename to: the dialog stays");
    cx.simulate_keystrokes("enter");
    assert!(is_open(cx, "agents-rename-dialog"));
    assert!(calls(&t.api, "rename_runnable").is_empty());
}

#[gpui::test]
fn another_selection_shows_its_panel_from_the_head(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, Some(lead()));
    cx.simulate_resize(size(px(1440.), px(700.)));
    cx.run_until_parked();
    t.view.update(cx, |v, cx| {
        v.panel_scroll.set_offset(gpui::point(px(0.), px(-600.)));
        cx.notify();
    });
    cx.run_until_parked();
    click(cx, "agents-row-agent:fixer");
    let offset = t.view.read_with(cx, |v, _| v.panel_scroll.offset());
    assert_eq!(offset.y, px(0.), "the panel of the new selection starts at its head");
    click(cx, "agents-more-agent:fixer");
    assert!(shown(cx, "agents-more-menu"), "its more button can be pressed");
}
