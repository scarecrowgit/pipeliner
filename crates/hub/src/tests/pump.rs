//! The pump batches events and hands each to the one model it concerns.

use gpui::TestAppContext;

use super::{notifications, setup};
use crate::api::{AgentEvent, Event, IssueKey, IssueStatus, RunId, RunLogLine, SessionId, SessionState};
use crate::transcript::{BlockKind, PermissionState};

#[gpui::test]
fn a_batch_notifies_each_touched_model_once(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let issues = cx.update(crate::issues);
    let index = cx.update(crate::chat_index);
    cx.run_until_parked();
    let issue_notes = notifications(cx, &issues);
    let index_notes = notifications(cx, &index);

    let mut batch = Vec::new();
    api.with_world(|world| {
        for issue in world.issues.iter_mut().take(5) {
            issue.title.push_str(" (edited)");
            batch.push(Event::IssueChanged { issue: issue.clone() });
        }
        for session in world.sessions.iter_mut().filter(|s| s.parent.is_none()).take(3) {
            session.title.push('!');
            batch.push(Event::SessionChanged { session: session.clone() });
        }
    });
    assert_eq!(batch.len(), 8);
    api.emit_all(batch);
    cx.run_until_parked();

    assert_eq!(issue_notes.get(), 1, "five issue events, one notify");
    assert_eq!(index_notes.get(), 1, "three session events, one notify");
    cx.update(|cx| {
        let issues = issues.read(cx);
        assert!(issues.iter().filter(|issue| issue.title.ends_with("(edited)")).count() == 5);
    });
}

#[gpui::test]
fn an_event_reaches_only_its_model(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let issues = cx.update(crate::issues);
    let index = cx.update(crate::chat_index);
    let runs = cx.update(crate::runs);
    let inbox = cx.update(crate::inbox);
    let catalog = cx.update(crate::catalog);
    let ops = cx.update(crate::ops);
    let permissions = cx.update(crate::permissions);
    let router = cx.update(|cx| crate::router(cx));
    cx.run_until_parked();
    let counts = [
        notifications(cx, &index),
        notifications(cx, &runs),
        notifications(cx, &inbox),
        notifications(cx, &catalog),
        notifications(cx, &ops),
        notifications(cx, &permissions),
        notifications(cx, &router),
    ];
    let issue_notes = notifications(cx, &issues);

    let mut issue = api.with_world(|world| world.issues[0].clone());
    issue.status = IssueStatus::Todo;
    api.emit(Event::IssueChanged { issue });
    cx.run_until_parked();

    assert_eq!(issue_notes.get(), 1);
    for count in counts {
        assert_eq!(count.get(), 0);
    }
}

#[gpui::test]
fn issues_follow_changes_removals_and_activity(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let issues = cx.update(crate::issues);
    cx.run_until_parked();
    let key = IssueKey::new("PAY-1427");
    issues.update(cx, |model, cx| model.ensure_activity(&key, cx));
    cx.run_until_parked();
    let before = cx.update(|cx| issues.read(cx).activity(&key).and_then(|feed| feed.value().map(|v| v.len())));
    assert_eq!(before, Some(12));

    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.comment(&key, "@Reproducer try 500 items", Vec::new(), cx));
    cx.update(|cx| {
        commands.delete_issue(&IssueKey::new("PAY-1417"), cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let model = issues.read(cx);
        assert_eq!(model.activity(&key).and_then(|feed| feed.value().map(|v| v.len())), Some(13));
        assert!(model.issue(&IssueKey::new("PAY-1417")).is_none());
        assert_eq!(model.column(IssueStatus::InProgress).len(), 3);
        assert_eq!(model.column(IssueStatus::InProgress)[0].key.as_str(), "PAY-1435", "high priority first");
    });
}

#[gpui::test]
fn a_run_model_keeps_a_bounded_log_tail(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let id = RunId::new("run-1427");
    let run = cx.update(|cx| crate::run(&id, cx));
    cx.run_until_parked();
    let appended = super::collect::<_, crate::RunLogEvent>(cx, &run);
    assert_eq!(cx.update(|cx| run.read(cx).log().len()), 16);

    let lines: Vec<Event> = (17..=17 + crate::models::LOG_CAPACITY as u64)
        .map(|seq| Event::RunLog {
            run: id.clone(),
            line: RunLogLine { seq, at: 0, node: None, variant: 0, level: Default::default(), text: format!("line {seq}") },
        })
        .collect();
    api.emit_all(lines);
    cx.run_until_parked();

    cx.update(|cx| {
        let model = run.read(cx);
        assert_eq!(model.log().len(), crate::models::LOG_CAPACITY);
        assert_eq!(model.log().back().map(|line| line.seq), Some(17 + crate::models::LOG_CAPACITY as u64));
    });
    let events = appended.borrow();
    assert_eq!(events.len(), 1, "one batch, one event");
    assert!(matches!(events[0], crate::RunLogEvent::Appended { added, dropped } if added == crate::models::LOG_CAPACITY + 1 && dropped == 17));
}

#[gpui::test]
fn events_for_closed_sessions_are_dropped(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    api.emit(super::agent("chat-1423", crate::api::AgentEvent::TextDelta { text: "nobody listens".into() }));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(hub.read(cx).open_sessions(), 0));
}

#[gpui::test]
fn the_pump_stops_when_the_hub_is_dropped(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    let index = cx.update(crate::chat_index);
    cx.run_until_parked();
    assert_eq!(api.subscriber_count(), 1);
    let weak = hub.downgrade();
    drop(hub);
    cx.update(|cx| drop(cx.remove_global::<crate::hub::GlobalHub>()));
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "nothing else holds the hub");
    assert_eq!(api.subscriber_count(), 0, "the pump's receiver is gone");

    // The index outlives the hub here, but nothing feeds it any more.
    let mut ledger = api.with_world(|world| world.session(&SessionId::new("chat-ledger")).cloned()).expect("the session is in the world");
    ledger.pinned = true;
    api.emit(Event::SessionChanged { session: ledger });
    cx.run_until_parked();
    cx.update(|cx| assert!(index.read(cx).pinned().iter().all(|s| s.id.as_str() != "chat-ledger")));
}

#[gpui::test]
fn a_second_hub_replaces_the_first(cx: &mut TestAppContext) {
    let (api, first) = setup(cx);
    let weak = first.downgrade();
    drop(first);
    let second = cx.update(|cx| crate::Hub::new(api.clone(), cx));
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    assert_eq!(api.subscriber_count(), 1);
    cx.update(|cx| assert_eq!(crate::hub(cx).entity_id(), second.entity_id()));
}

#[gpui::test]
fn sessions_nobody_opened_still_reach_the_index(cx: &mut TestAppContext) {
    let (api, hub) = setup(cx);
    let index = cx.update(crate::chat_index);
    cx.run_until_parked();
    let index_notes = notifications(cx, &index);

    let mut ledger = api.with_world(|world| world.session(&SessionId::new("chat-ledger")).cloned()).expect("the session is in the world");
    ledger.pinned = true;
    ledger.state = SessionState::Running;
    api.emit_all([
        Event::SessionChanged { session: ledger },
        super::agent("chat-ledger", AgentEvent::TurnStarted),
        super::agent("chat-ledger", AgentEvent::TextDelta { text: "Reading the ledger".into() }),
    ]);
    cx.run_until_parked();
    assert_eq!(index_notes.get(), 1);
    cx.update(|cx| {
        let index = index.read(cx);
        assert!(index.pinned().iter().any(|s| s.id.as_str() == "chat-ledger" && s.state == SessionState::Running));
        assert_eq!(index.pinned().len() + index.recent().len(), 8, "top-level chats only");
        let hub = hub.read(cx);
        assert_eq!(hub.open_sessions(), 0, "agent events open no model");
        assert!(hub.sessions.is_empty());
    });

    // A chat created elsewhere and then removed.
    let mut fresh = api.with_world(|world| world.session(&SessionId::new("chat-ledger")).cloned()).expect("the session is in the world");
    fresh.id = SessionId::new("chat-new");
    fresh.pinned = false;
    fresh.updated = super::NOW + 1;
    api.emit(Event::SessionChanged { session: fresh });
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(index.read(cx).recent().first().map(|s| s.id.as_str()), Some("chat-new")));
    api.emit(Event::SessionRemoved { session: SessionId::new("chat-new") });
    cx.run_until_parked();
    cx.update(|cx| assert!(index.read(cx).session(&SessionId::new("chat-new")).is_none()));
}

#[gpui::test]
fn one_batch_for_several_models_notifies_each_once(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-split-checkout";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    let issues = cx.update(crate::issues);
    let index = cx.update(crate::chat_index);
    let permissions = cx.update(crate::permissions);
    cx.run_until_parked();
    let counts = [notifications(cx, &session), notifications(cx, &issues), notifications(cx, &index)];
    let permission_notes = notifications(cx, &permissions);

    let mut header = api.with_world(|world| world.session(&SessionId::new(s)).cloned()).expect("the session is in the world");
    header.state = SessionState::Running;
    let mut batch = vec![Event::SessionChanged { session: header }, super::agent(s, AgentEvent::TurnStarted)];
    batch.extend(["Two ", "seams ", "look ", "clean."].map(|text| super::agent(s, AgentEvent::TextDelta { text: text.into() })));
    let mut issue = api.with_world(|world| world.issues[1].clone());
    issue.priority = crate::api::Priority::High;
    batch.push(Event::IssueChanged { issue });
    api.emit_all(batch);
    cx.run_until_parked();

    for (name, count) in ["session", "issues", "index"].iter().zip(&counts) {
        assert_eq!(count.get(), 1, "{name}");
    }
    assert_eq!(permission_notes.get(), 0);
    cx.update(|cx| assert!(session.read(cx).is_busy()));
}

#[gpui::test]
fn a_request_resolved_in_the_same_batch_is_not_left_pending(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-split-checkout";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    let permissions = cx.update(crate::permissions);
    cx.run_until_parked();
    let request = crate::api::PermissionRequest {
        id: crate::api::RequestId::new("perm-fast"),
        session: SessionId::new(s),
        agent: "Build".into(),
        tool: "bash".into(),
        call_id: None,
        class: crate::api::ToolClass::Execute,
        display: crate::api::ToolDisplay::Command { command: "ls".into(), output: String::new(), exit_code: None },
        policy: "bash = ask".into(),
        always_rule: None,
        at: 0,
    };
    api.emit_all([
        Event::PermissionRequested { request },
        Event::PermissionResolved { id: crate::api::RequestId::new("perm-fast") },
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert!(model.pending().is_empty());
        let block = model.blocks().last().map(|block| block.kind.clone());
        assert!(matches!(block, Some(BlockKind::Permission(p)) if p.state == PermissionState::Resolved));
        assert_eq!(permissions.read(cx).count(), 1, "only the fixture's own request is left");
    });
}
