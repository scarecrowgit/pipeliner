//! Answering an agent's question and raising a run's budget from the Inbox.

use gpui::{TestAppContext, VisualTestContext};
use hub::InboxFocus;
use hub::api::{InboxId, Outcome, RunId, RunState};
use hub::fixtures::World;

use super::{Open, calls, click, open_with, rows, shown};

/// The Auditor's question, asked by the Reproducer of run-1427, which waits for the answer.
fn asking(world: &mut World) {
    if let Some(item) = world
        .inbox
        .iter_mut()
        .find(|item| item.id.as_str() == "inbox-1435")
    {
        item.run = Some(RunId::new("run-1427"));
    }
    if let Some(run) = world
        .runs
        .iter_mut()
        .find(|run| run.id.as_str() == "run-1427")
    {
        run.state = RunState::WaitingAnswer;
        if let Some(node) = run.nodes.iter_mut().find(|n| n.node.as_str() == "repro") {
            node.question = Some("Which log sinks are in scope?".to_string());
        }
    }
}

/// Run-1431 stopped at its budget; its item says the run is blocked.
fn over_budget(world: &mut World) {
    if let Some(run) = world
        .runs
        .iter_mut()
        .find(|run| run.id.as_str() == "run-1431")
    {
        run.state = RunState::WaitingAnswer;
    }
    let mut item = world.inbox[0].clone();
    item.id = InboxId::new("inbox-budget");
    item.kind = hub::api::InboxKind::Failure;
    item.outcome = Some(Outcome::Blocked);
    item.title = "Saved cards for guest checkout stopped".to_string();
    item.summary = String::new();
    item.run = Some(RunId::new("run-1431"));
    item.read = false;
    item.created = super::NOW;
    world.inbox.push(item);
}

fn item(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Item(InboxId::new(id)))
}

fn answer_text(t: &Open, cx: &mut VisualTestContext) -> String {
    t.view.read_with(cx, |v, cx| {
        v.item_run
            .as_ref()
            .map(|r| r.answer.read(cx).text().to_string())
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn a_typed_answer_goes_to_the_asking_agent(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-1435"), asking);
    assert!(shown(cx, "item-answer-field"));
    assert!(shown(cx, "item-answer"), "the link to the issue stays");
    // Empty answers are refused.
    click(cx, "item-answer-send");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert!(calls(&t.api, "answer_run").is_empty());

    click(cx, "item-answer-field");
    // J and K are letters here, not the queue's keys.
    cx.simulate_input("just stdout and Datadog");
    cx.run_until_parked();
    assert_eq!(answer_text(&t, cx), "just stdout and Datadog");
    assert_eq!(super::focus(&t, cx), item("inbox-1435"));
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(
        calls(&t.api, "answer_run"),
        ["run-1427 repro just stdout and Datadog"]
    );

    // The run moved on: the item no longer waits, and the detail says so.
    assert!(
        !rows(&t, cx).contains(&"inbox-1435".to_string()),
        "{:?}",
        rows(&t, cx)
    );
    assert!(shown(cx, "item-answered"));
}

#[gpui::test]
async fn a_question_without_a_waiting_run_keeps_only_the_link(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-1435"), |_| {});
    assert!(shown(cx, "item-answer"));
    let has_run = t.view.read_with(cx, |v, _| v.item_run.is_some());
    assert!(!has_run);
}

#[gpui::test]
async fn a_run_at_its_budget_can_be_raised(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-budget"), over_budget);
    assert!(shown(cx, "item-budget"));
    assert!(rows(&t, cx).contains(&"inbox-budget".to_string()));
    let stats = t.view.read_with(cx, |v, _| {
        v.item_run
            .as_ref()
            .and_then(|r| r.stats.as_ref())
            .and_then(|s| s.budget_usd)
    });
    assert_eq!(
        stats,
        Some(3.0),
        "the limit is shown next to what was spent"
    );

    click(cx, "item-budget-amount");
    cx.simulate_keystrokes("end backspace backspace backspace backspace");
    cx.simulate_input("abc");
    cx.run_until_parked();
    click(cx, "item-raise-budget");
    assert!(calls(&t.api, "raise_run_budget").is_empty(), "not a number");

    click(cx, "item-budget-amount");
    cx.simulate_keystrokes("end backspace backspace backspace");
    cx.simulate_input("2.50");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "raise_run_budget"), ["run-1431 2.5"]);
    assert!(
        !rows(&t, cx).contains(&"inbox-budget".to_string()),
        "the run went on"
    );
}

#[gpui::test]
async fn the_raise_button_uses_the_default_amount(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-budget"), over_budget);
    click(cx, "item-raise-budget");
    assert_eq!(calls(&t.api, "raise_run_budget"), ["run-1431 1"]);
}
