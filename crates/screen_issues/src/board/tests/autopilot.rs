//! The autopilot dialog: a new schedule autopilot, editing one, and the row actions.

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use hub::api::{AutopilotId, AutopilotTrigger, RunnableRef, Schedule};

use super::{autopilots, click, drawn, open, recorded};
use crate::board::IssuesView;
use crate::board::autopilot_form::{self, AutopilotForm, Repeat, Unit, parse_budget, parse_time, schedule_of};

fn form(view: &Entity<IssuesView>, cx: &mut VisualTestContext) -> Entity<AutopilotForm> {
    view.read_with(cx, |view, _| view.autopilot_form.clone()).expect("the dialog is open")
}

/// How many autopilots the project under test (Payments) has in the fixture's world.
fn payments_autopilots(api: &hub::fixtures::FixtureApi) -> usize {
    api.with_world(|w| w.autopilots.iter().filter(|a| a.project.as_str() == "payments").count())
}

fn dialog_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| ui::overlays(window, cx).is_open(autopilot_form::DIALOG, cx))
}

#[test]
fn schedules_come_from_the_fields() {
    assert_eq!(parse_time("02:00"), Some((2, 0)));
    assert_eq!(parse_time(" 9:30 "), Some((9, 30)));
    assert_eq!(parse_time("7"), Some((7, 0)));
    assert_eq!(parse_time("24:00"), None);
    assert_eq!(parse_time("9:60"), None);
    assert_eq!(schedule_of(Repeat::Daily, "02:00", 3, "", Unit::Hours), Ok(Schedule::Day { hour: 2, minute: 0 }));
    assert_eq!(
        schedule_of(Repeat::Weekly, "09:00", 0, "", Unit::Hours),
        Ok(Schedule::Week { weekday: 0, hour: 9, minute: 0 })
    );
    assert_eq!(schedule_of(Repeat::Interval, "", 0, "3", Unit::Hours), Ok(Schedule::Minutes { minutes: 180 }));
    assert_eq!(schedule_of(Repeat::Interval, "", 0, "15", Unit::Minutes), Ok(Schedule::Minutes { minutes: 15 }));
    assert!(schedule_of(Repeat::Interval, "", 0, "2", Unit::Minutes).is_err(), "too often");
    assert!(schedule_of(Repeat::Interval, "", 0, "x", Unit::Hours).is_err());
    assert!(schedule_of(Repeat::Daily, "noon", 0, "", Unit::Hours).is_err());
    assert_eq!(parse_budget(""), Ok(None));
    assert_eq!(parse_budget("$0.50"), Ok(Some(0.5)));
    assert_eq!(parse_budget("-1"), Err(()));
}

#[gpui::test]
fn new_autopilot_saves_a_weekly_schedule(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    autopilots(&view, cx);
    click(cx, "issues-autopilot-new");
    assert!(dialog_open(cx));
    assert!(!cx.update(|window, cx| ui::overlays(window, cx).is_open(crate::board::autopilots::BUTTON, cx)), "the popover closed");
    cx.simulate_input("Monday sweep");
    let form = form(&view, cx);
    form.read_with(cx, |form, cx| assert_eq!(form.name.read(cx).text(), "Monday sweep", "the name has focus"));

    click(cx, autopilot_form::TARGET);
    click(cx, "autopilot-target-pipeline:pay-triage");
    click(cx, "autopilot-repeat-1");
    form.update(cx, |form, cx| {
        form.time.update(cx, |input, cx| input.set_text("07:30", cx));
        form.budget.update(cx, |input, cx| input.set_text("0.75", cx));
    });
    click(cx, autopilot_form::SAVE);

    let sent = recorded(&api);
    assert!(sent.iter().any(|c| c == "save_autopilot Monday sweep"), "{sent:?}");
    let saved = api.with_world(|w| w.autopilots.iter().find(|a| a.name == "Monday sweep").cloned()).expect("saved");
    assert_eq!(saved.trigger, AutopilotTrigger::Schedule { schedule: Schedule::Week { weekday: 0, hour: 7, minute: 30 } });
    assert_eq!(saved.target, RunnableRef::pipeline("pay-triage"));
    assert_eq!(saved.budget_usd, Some(0.75));
    assert!(saved.enabled);
    assert!(!dialog_open(cx), "the dialog closed");
}

#[gpui::test]
fn a_missing_target_or_bad_time_is_refused_inline(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    cx.update(|window, cx| view.update(cx, |view, cx| view.open_autopilot_form(None, window, cx)));
    cx.run_until_parked();
    cx.simulate_input("Sweep");
    click(cx, autopilot_form::SAVE);
    let form = form(&view, cx);
    form.read_with(cx, |form, _| assert_eq!(form.error.as_ref().map(|e| e.as_ref()), Some("Pick what it runs.")));
    form.update(cx, |form, cx| {
        form.target = Some(RunnableRef::agent("fixer"));
        form.time.update(cx, |input, cx| input.set_text("25:00", cx));
    });
    click(cx, autopilot_form::SAVE);
    form.read_with(cx, |form, _| assert!(form.error.as_ref().is_some_and(|e| e.contains("HH:MM"))));
    assert!(drawn(cx, "autopilot-error"));
    assert!(!recorded(&api).iter().any(|c| c.starts_with("save_autopilot")));
    assert!(dialog_open(cx), "the dialog stays open");
}

#[gpui::test]
fn a_row_opens_the_autopilot_for_editing_and_keeps_its_id(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    autopilots(&view, cx);
    click(cx, "issues-autopilot-0");
    let form = form(&view, cx);
    form.read_with(cx, |form, cx| {
        assert_eq!(form.name.read(cx).text(), "Nightly e2e triage");
        assert_eq!(form.time.read(cx).text(), "02:00");
        assert_eq!(form.repeat, Repeat::Daily);
        assert_eq!(form.target, Some(RunnableRef::pipeline("pay-triage")));
    });
    click(cx, "autopilot-repeat-2");
    form.update(cx, |form, cx| form.every.update(cx, |input, cx| input.set_text("6", cx)));
    click(cx, autopilot_form::SAVE);
    let saved = api.with_world(|w| w.autopilots.iter().find(|a| a.id == AutopilotId::new("ap-nightly-e2e")).cloned());
    let saved = saved.expect("the same autopilot");
    assert_eq!(saved.trigger, AutopilotTrigger::Schedule { schedule: Schedule::Minutes { minutes: 360 } });
    assert_eq!(saved.last_result, "filed 2 issues", "what the engine recorded stays");
    assert_eq!(payments_autopilots(&api), 3, "edited in place, none added");
}

#[gpui::test]
fn run_now_and_delete_show_on_hover(cx: &mut TestAppContext) {
    let (api, view, cx) = open(cx, None);
    autopilots(&view, cx);
    assert!(!drawn(cx, "issues-autopilot-run-0"));
    let name: &'static str = "issues-autopilot-0";
    let bounds = cx.debug_bounds(name).expect("the row is drawn");
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    cx.run_until_parked();
    click(cx, "issues-autopilot-run-0");
    assert!(recorded(&api).iter().any(|c| c == "run_autopilot ap-nightly-e2e"), "{:?}", recorded(&api));
    assert!(view.read_with(cx, |view, _| view.autopilot_form.is_none()), "the button does not open the row");

    click(cx, "issues-autopilot-delete-0");
    assert!(recorded(&api).iter().any(|c| c == "delete_autopilot ap-nightly-e2e"));
    assert_eq!(payments_autopilots(&api), 2);
}
