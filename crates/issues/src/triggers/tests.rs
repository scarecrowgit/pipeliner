use model::{IssueStatus, RunnableRef};

use super::*;
use crate::testing::{agent, issue, runnable};
use model::RunnableKind;

fn comment(text: &str, mentions: &[&RunnableRef]) -> Happening {
    Happening::Comment { text: text.into(), mentions: mentions.iter().map(|r| (*r).clone()).collect() }
}

fn setup() -> (model::Issue, Vec<model::Runnable>) {
    let mut i = issue(IssueStatus::Todo);
    i.title = "Refund webhook times out".into();
    i.body = "Gateway timeout".into();
    let fixer = agent("Fixer");
    i.runner = Some(fixer.reference.clone());
    (i, vec![fixer, agent("Codex"), runnable(RunnableKind::Orchestrator, "Release captain")])
}

#[test]
fn attached_runner_starts_on_attach_and_creation() {
    let (i, rs) = setup();
    for (happening, cause) in [(Happening::Attached, "attached"), (Happening::Created, "issue created")] {
        let w = wake_intents(&i, &happening, &rs);
        assert!(w.refusals.is_empty());
        assert_eq!(w.intents.len(), 1);
        let intent = &w.intents[0];
        assert_eq!((intent.name.as_str(), intent.mode, intent.cause.as_str()), ("Fixer", WakeMode::Start, cause));
        assert_eq!(intent.input, "Refund webhook times out\n\nGateway timeout");
    }
}

#[test]
fn on_assign_off_means_no_start() {
    let (mut i, rs) = setup();
    i.triggers.on_assign = false;
    assert_eq!(wake_intents(&i, &Happening::Created, &rs), Wakes::default());
    assert_eq!(wake_intents(&i, &Happening::Attached, &rs), Wakes::default());
}

#[test]
fn no_runner_wakes_nobody() {
    let (mut i, rs) = setup();
    i.runner = None;
    assert_eq!(wake_intents(&i, &Happening::Created, &rs), Wakes::default());
    assert_eq!(wake_intents(&i, &comment("hello", &[]), &rs), Wakes::default());
    assert_eq!(wake_intents(&i, &Happening::MovedTo(IssueStatus::Todo), &rs), Wakes::default());
}

#[test]
fn title_alone_is_the_input_without_a_body() {
    let (mut i, rs) = setup();
    i.body = "  ".into();
    assert_eq!(wake_intents(&i, &Happening::Created, &rs).intents[0].input, "Refund webhook times out");
}

#[test]
fn moving_to_todo_needs_on_todo() {
    let (mut i, rs) = setup();
    assert!(wake_intents(&i, &Happening::MovedTo(IssueStatus::Todo), &rs).intents.is_empty());
    i.triggers.on_todo = true;
    let w = wake_intents(&i, &Happening::MovedTo(IssueStatus::Todo), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].cause, "moved to Todo");
    for other in [IssueStatus::Backlog, IssueStatus::InProgress, IssueStatus::InReview, IssueStatus::Done] {
        assert!(wake_intents(&i, &Happening::MovedTo(other), &rs).intents.is_empty());
    }
}

#[test]
fn a_comment_wakes_the_attached_runner_only_with_on_comment() {
    let (mut i, rs) = setup();
    assert!(wake_intents(&i, &comment("looks fine", &[]), &rs).intents.is_empty());
    i.triggers.on_comment = true;
    let w = wake_intents(&i, &comment("looks fine", &[]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].mode, WakeMode::Continue);
    assert_eq!(w.intents[0].cause, "comment passed as input");
    assert_eq!(w.intents[0].input, "looks fine");
}

#[test]
fn mentioning_the_attached_runner_continues_on_this_issue() {
    let (i, rs) = setup();
    let fixer = &rs[0].reference;
    let w = wake_intents(&i, &comment("@Fixer batch by 50", &[fixer]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].mode, WakeMode::Continue);
    assert_eq!(w.intents[0].cause, "@mention · comment passed as input");
    assert_eq!(w.intents[0].input, "@Fixer batch by 50");
}

#[test]
fn mentioning_another_runnable_starts_a_new_run() {
    let (i, rs) = setup();
    let codex = &rs[1].reference;
    let w = wake_intents(&i, &comment("@Codex write a second variant", &[codex]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].target, *codex);
    assert_eq!(w.intents[0].mode, WakeMode::Start);
    assert_eq!(w.intents[0].input, "@Codex write a second variant");
}

#[test]
fn mentions_wake_even_when_no_runner_is_attached() {
    let (mut i, rs) = setup();
    i.runner = None;
    let codex = &rs[1].reference;
    let w = wake_intents(&i, &comment("@Codex go", &[codex]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].mode, WakeMode::Start);
}

#[test]
fn nobody_is_woken_twice() {
    let (mut i, rs) = setup();
    i.triggers.on_comment = true;
    let (fixer, codex) = (&rs[0].reference, &rs[1].reference);
    // The attached runner is both mentioned (twice) and woken by on_comment; Codex is mentioned twice.
    let w = wake_intents(&i, &comment("@Fixer @Codex @Codex @Fixer", &[fixer, codex, codex, fixer]), &rs);
    let targets: Vec<_> = w.intents.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(targets, ["Fixer", "Codex"]);
    assert_eq!(w.intents[0].cause, "@mention · comment passed as input");
}

#[test]
fn the_attached_runner_comes_first() {
    let (i, rs) = setup();
    let (fixer, codex) = (&rs[0].reference, &rs[1].reference);
    let w = wake_intents(&i, &comment("x", &[codex, fixer]), &rs);
    assert_eq!(w.intents[0].name, "Fixer");
    assert_eq!(w.intents[1].name, "Codex");
}

#[test]
fn on_mention_off_silences_a_mention_of_the_attached_runner_only() {
    let (mut i, rs) = setup();
    i.triggers.on_mention = false;
    let (fixer, codex) = (&rs[0].reference, &rs[1].reference);
    let w = wake_intents(&i, &comment("@Fixer @Codex", &[fixer, codex]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.intents[0].name, "Codex");
    // on_comment still wakes it, without the mention cause.
    i.triggers.on_comment = true;
    let w = wake_intents(&i, &comment("@Fixer", &[fixer]), &rs);
    assert_eq!(w.intents[0].cause, "comment passed as input");
}

#[test]
fn paused_and_archived_runnables_are_refused() {
    let (mut i, mut rs) = setup();
    rs[0].paused = true;
    rs[1].archived = true;
    let (fixer, codex) = (rs[0].reference.clone(), rs[1].reference.clone());
    i.triggers.on_comment = true;
    let w = wake_intents(&i, &comment("@Fixer @Codex", &[&fixer, &codex]), &rs);
    assert!(w.intents.is_empty());
    assert_eq!(w.refusals.len(), 2);
    assert_eq!((w.refusals[0].name.as_str(), w.refusals[0].reason.as_str()), ("Fixer", "paused · takes no new work"));
    assert_eq!((w.refusals[1].name.as_str(), w.refusals[1].reason.as_str()), ("Codex", "archived"));
    // Archived wins over paused, and a paused attached runner is refused on creation too.
    rs[0].archived = true;
    let w = wake_intents(&i, &Happening::Created, &rs);
    assert_eq!(w.refusals.len(), 1);
    assert_eq!(w.refusals[0].reason, "archived");
}

#[test]
fn a_refused_runnable_is_listed_once_and_others_still_wake() {
    let (mut i, mut rs) = setup();
    rs[1].paused = true;
    let (fixer, codex) = (rs[0].reference.clone(), rs[1].reference.clone());
    i.triggers.on_comment = true;
    let w = wake_intents(&i, &comment("x", &[&codex, &codex, &fixer]), &rs);
    assert_eq!(w.intents.len(), 1);
    assert_eq!(w.refusals.len(), 1);
}

#[test]
fn an_unknown_runnable_is_refused() {
    let (i, rs) = setup();
    let ghost = RunnableRef::agent("ghost");
    let w = wake_intents(&i, &comment("@Ghost", &[&ghost]), &rs);
    assert!(w.intents.is_empty());
    assert_eq!(w.refusals[0].name, "ghost");
    assert_eq!(w.refusals[0].reason, "no longer exists");
}
