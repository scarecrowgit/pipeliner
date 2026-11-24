//! The project switcher, the project form, the limits dialog, pipeline rows and the editor's
//! narrow-window rail.

use gpui::TestAppContext;
use hub::Route;
use hub::api::{PipelineId, ProjectId};

use super::{setup, setup_with_width};
use crate::sidebar::limits::{LIMITS_DIALOG, parse_amount};
use crate::sidebar::projects::form::{DELETE_DIALOG, FORM_DIALOG, clean_prefix, problem, suggest_prefix};
use crate::sidebar::next_run_label;
use crate::sidebar::projects::{ANCHOR, matches, state_word};

fn current(t: &super::Setup) -> Option<String> {
    t.settings().current_project.map(|id| id.to_string())
}

#[gpui::test]
fn the_switcher_switches_by_search_click_and_keys(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert_eq!(current(&t).as_deref(), Some("payments"));

    t.click("sidebar-project");
    assert!(t.open(ANCHOR));
    // "dbt" is found in the Data project's repo folders.
    t.typed("dbt");
    t.keys("enter");
    assert!(!t.open(ANCHOR), "choosing closes the switcher");
    assert_eq!(current(&t).as_deref(), Some("data"));

    t.click("sidebar-project");
    t.click("project-platform");
    assert_eq!(current(&t).as_deref(), Some("platform"));

    t.keys("ctrl-2");
    assert_eq!(current(&t).as_deref(), Some("checkout"), "⌘2 is the second project");
    t.keys("ctrl-1");
    assert_eq!(current(&t).as_deref(), Some("payments"));
}

#[gpui::test]
fn a_new_project_checks_its_folders_and_becomes_current(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.click("sidebar-project");
    t.click("project-new");
    assert!(t.open(FORM_DIALOG));

    t.typed("Billing service");
    t.click("project-form-prefix");
    t.keys("ctrl-a");
    t.typed("bil-1l");

    t.click("project-form-path");
    t.typed("/nowhere/at/all");
    t.keys("enter");
    assert!(!t.drawn("project-form-repo-0-remove"), "a missing folder is not added");
    t.keys("ctrl-a");
    t.typed("~/code/acme/ledger");
    t.keys("enter");
    assert!(t.drawn("project-form-repo-0-remove"));

    t.click("project-form-save");
    assert!(!t.open(FORM_DIALOG));
    assert_eq!(t.recorded("create_project"), ["Billing service"]);
    let project = t.api.with_world(|world| world.projects.iter().find(|p| p.name == "Billing service").cloned());
    let project = project.expect("the project was saved");
    assert_eq!(project.key_prefix, "BILL");
    assert_eq!(project.repos, [std::path::PathBuf::from("/home/me/code/acme/ledger")]);
    assert_eq!(t.settings().current_project, Some(project.id));
}

#[gpui::test]
fn project_settings_rename_and_delete_the_current_project(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.click("sidebar-project");
    t.click("project-settings");
    assert!(t.open(FORM_DIALOG));
    t.keys("ctrl-a");
    t.typed("Payments core");
    t.click("project-form-save");
    let name = t.api.with_world(|world| world.projects.iter().find(|p| p.id.as_str() == "payments").map(|p| p.name.clone()));
    assert_eq!(name.as_deref(), Some("Payments core"));

    t.click("sidebar-project");
    t.click("project-settings");
    t.click("project-form-delete");
    assert!(t.open(DELETE_DIALOG));
    t.click("project-delete-confirm");
    assert!(!t.open(DELETE_DIALOG) && !t.open(FORM_DIALOG));
    assert_eq!(t.recorded("delete_project"), ["payments"]);
    assert_ne!(t.settings().current_project, Some(ProjectId::new("payments")), "another project became current");
}

#[gpui::test]
fn the_limits_dialog_saves_the_limit_and_slots(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    assert_eq!(t.settings().daily_limit_usd, Some(10.0));
    let slots = t.settings().slots;

    t.click("sidebar-spend");
    t.click("spend-limits");
    assert!(t.open(LIMITS_DIALOG));
    t.click("limits-amount");
    t.keys("ctrl-a");
    t.typed("abc");
    t.click("limits-save");
    assert!(t.open(LIMITS_DIALOG), "a bad amount keeps the dialog open");
    assert!(t.recorded("set_daily_limit").is_empty() && t.recorded("set_slots").is_empty());

    t.click("limits-amount");
    t.keys("ctrl-a");
    t.typed("25");
    t.click("limits-slots-inc");
    t.click("limits-save");
    assert!(!t.open(LIMITS_DIALOG));
    assert_eq!(t.settings().daily_limit_usd, Some(25.0));
    assert_eq!(t.settings().slots, slots + 1);

    t.click("sidebar-spend");
    t.click("spend-limits");
    t.click("limits-no-limit");
    t.click("limits-save");
    assert_eq!(t.settings().daily_limit_usd, None);
}

#[gpui::test]
fn pipeline_rows_show_their_state_and_open_the_editor(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let pipelines = t.shown().pipelines;
    let meta = |id: &str| pipelines.iter().find(|(p, _)| p == id).map(|(_, m)| m.clone());
    assert_eq!(meta("pay-triage").as_deref(), Some("37 queued"));
    assert_eq!(meta("nightly-audit").as_deref(), Some("v3"), "a deployed pipeline at rest shows its version");
    assert_eq!(meta("release-notes").as_deref(), Some("draft"));

    t.click("sidebar-pipeline-nightly-audit");
    assert_eq!(t.route(), Route::Editor { pipeline: PipelineId::new("nightly-audit"), node: None });
}

#[gpui::test]
fn the_editor_starts_with_the_rail_in_a_narrow_window(cx: &mut TestAppContext) {
    let mut t = setup_with_width(cx, 1440.);
    let editor = Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None };
    t.navigate(editor.clone());
    assert!(t.shown().collapsed, "narrow window: the editor gets the rail");

    t.click("sidebar-expand");
    assert!(!t.shown().collapsed, "expanding by hand holds for the visit");
    t.navigate(Route::Editor { pipeline: PipelineId::new("nightly-audit"), node: None });
    assert!(!t.shown().collapsed, "another pipeline is the same visit");

    t.navigate(Route::Home);
    assert!(!t.shown().collapsed);
    t.navigate(editor.clone());
    assert!(t.shown().collapsed, "a new visit starts with the rail again");
    t.navigate(Route::Home);
    assert!(!t.shown().collapsed, "leaving restores the person's setting");
    assert!(t.recorded("set_settings").is_empty(), "visits never touch the saved setting");
}

#[gpui::test]
fn a_wide_window_keeps_the_sidebar_in_the_editor(cx: &mut TestAppContext) {
    let mut t = setup_with_width(cx, 1800.);
    t.navigate(Route::Editor { pipeline: PipelineId::new("pay-triage"), node: None });
    assert!(!t.shown().collapsed);
}

#[test]
fn prefixes_are_cleaned_suggested_and_checked() {
    assert_eq!(clean_prefix("pay-12x"), "PAYX");
    assert_eq!(clean_prefix("abcdefghij"), "ABCDEF");
    assert_eq!(suggest_prefix("Billing service"), "BS");
    assert_eq!(suggest_prefix("Payments"), "PAY");
    let others = vec![model::Project {
        id: ProjectId::new("payments"),
        name: "Payments".into(),
        key_prefix: "PAY".into(),
        repos: Vec::new(),
    }];
    assert!(problem("", "AB", &others).is_some());
    assert!(problem("Billing", "B", &others).is_some());
    assert!(problem("Billing", "PAY", &others).is_some());
    assert!(problem("Billing", "BILL", &others).is_none());
    assert!(matches(&others[0], "pay") && matches(&others[0], "") && !matches(&others[0], "zzz"));
}

#[test]
fn amounts_parse_with_or_without_a_dollar_sign() {
    assert_eq!(parse_amount("10"), Some(10.0));
    assert_eq!(parse_amount(" $12.50 "), Some(12.5));
    assert_eq!(parse_amount("7,5"), Some(7.5));
    assert_eq!(parse_amount("0"), None);
    assert_eq!(parse_amount("ten"), None);
}

#[gpui::test]
fn the_switcher_loads_live_counts_and_switches_through_the_engine(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let ops = t.cx.update(|_, cx| hub::ops(cx));
    let loaded = |t: &mut super::Setup| t.cx.update(|_, cx| ops.read(cx).counts_for(&ProjectId::new("payments")).is_some());
    assert!(!loaded(&mut t), "counts are read when the switcher first opens");
    t.click("sidebar-project");
    assert!(loaded(&mut t));
    t.click("project-checkout");
    assert_eq!(t.recorded("set_current_project"), ["checkout"]);
    assert!(t.recorded("set_settings").is_empty(), "switching does not rewrite the settings");
}

#[test]
fn project_states_and_next_runs_read_as_drawn() {
    assert_eq!(state_word(2), "2 running");
    assert_eq!(state_word(0), "idle");
    let hour = 60 * 60 * 1000;
    let now = 1_795_000_000_000;
    let soon = next_run_label(now, now + 3 * hour);
    assert_eq!(soon.len(), 5, "a clock time within a day: {soon}");
    assert!(soon.contains(':'));
    let later = next_run_label(now, now + 50 * hour);
    assert!(!later.contains(':'), "a weekday further out: {later}");
}
