//! The agent, model and mode pickers: the current value, and the command each choice sends.

use gpui::TestAppContext;

use super::{FIX, calls, click, focus_composer, open, shown};
use crate::session::pickers::{agent_choices, current_label};

#[gpui::test]
fn the_agent_picker_marks_the_current_agent_and_switches(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    assert_eq!(t.view.read_with(cx, current_label), "Build");

    click(cx, "chat-agent-picker");
    assert!(shown(cx, "chat-mode-build") && shown(cx, "chat-mode-plan"));
    assert!(shown(cx, "chat-agent-picker-claude-code"), "external harnesses are listed");
    click(cx, "chat-mode-plan");
    assert_eq!(calls(&t.api, "set_session_runnable"), [format!("{FIX} agent:build Plan")]);
    assert_eq!(t.view.read_with(cx, current_label), "Plan");

    let (index, current) = t.view.read_with(cx, |v, cx| {
        let choices = agent_choices(v.catalog.read(cx), v.header_session(cx));
        let lead = choices.iter().find(|c| c.label == "Payments lead").map(|c| c.id.to_string());
        let current: Vec<String> = choices.iter().filter(|c| c.current).map(|c| c.label.to_string()).collect();
        (lead, current)
    });
    assert_eq!(current, ["Plan"]);
    let Some(lead) = index else { panic!("Payments lead is offered") };
    click(cx, "chat-agent-picker");
    click(cx, &lead);
    let last = calls(&t.api, "set_session_runnable").pop().unwrap_or_default();
    assert_eq!(last, format!("{FIX} orchestrator:payments-lead Plan"));
    assert_eq!(t.view.read_with(cx, current_label), "Payments lead");
}

#[gpui::test]
fn the_model_picker_lists_providers_and_switches(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    click(cx, "chat-model-picker");
    assert!(shown(cx, "chat-model-picker-1"), "the models arrived");
    click(cx, "chat-model-picker-1");
    assert_eq!(calls(&t.api, "set_session_model"), [format!("{FIX} anthropic/sonnet-5.5")]);
    let model = t.view.read_with(cx, |v, cx| v.header_session(cx).map(|s| s.model.short().to_string()));
    assert_eq!(model.as_deref(), Some("sonnet-5.5"));
}

#[gpui::test]
fn tab_in_the_composer_switches_build_and_plan(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    focus_composer(&t, cx);
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "set_session_runnable"), [format!("{FIX} agent:build Plan")]);
    click(cx, "chat-mode-toggle");
    assert_eq!(calls(&t.api, "set_session_runnable").pop(), Some(format!("{FIX} agent:build Build")));
}

#[gpui::test]
fn tab_in_the_agent_picker_switches_build_and_plan_as_its_footer_says(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    click(cx, "chat-agent-picker");
    assert!(shown(cx, "chat-mode-plan"));
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "set_session_runnable"), [format!("{FIX} agent:build Plan")]);
    let open = cx.update(|window, cx| ui::overlays(window, cx).open_count(cx));
    assert_eq!(open, 1, "the picker stays open on the new choice");
}
