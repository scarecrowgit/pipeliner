//! Session models: blocks from history and from the live stream, changed ranges, paging.

use std::sync::Arc;

use gpui::TestAppContext;

use super::{agent, collect, setup};
use crate::SessionEvent;
use crate::api::{
    AgentEvent, Event, PermissionDecision, PermissionRequest, RequestId, SessionId, SessionState, StopReason, TodoItem,
    TodoStatus, ToolClass, ToolDisplay,
};
use crate::transcript::{BlockKind, PermissionState, Splice, SubagentState, ToolState};

fn kinds(blocks: &[Arc<crate::Block>]) -> Vec<&'static str> {
    blocks
        .iter()
        .map(|block| match &block.kind {
            BlockKind::User(_) => "user",
            BlockKind::Assistant(_) => "assistant",
            BlockKind::Thinking(_) => "thinking",
            BlockKind::Tool(_) => "tool",
            BlockKind::Subagent(_) => "subagent",
            BlockKind::Todos(_) => "todos",
            BlockKind::Permission(_) => "permission",
            BlockKind::Compaction(_) => "compaction",
            BlockKind::Notice(_) => "notice",
            BlockKind::Failure(_) => "failure",
        })
        .collect()
}

fn tool_state(block: &crate::Block) -> Option<ToolState> {
    match &block.kind {
        BlockKind::Tool(tool) => Some(tool.state),
        _ => None,
    }
}

fn command(output: &str) -> ToolDisplay {
    ToolDisplay::Command { command: "bun test".into(), output: output.into(), exit_code: None }
}

#[gpui::test]
fn a_scripted_stream_builds_blocks_and_reports_changed_ranges(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-split-checkout";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    cx.run_until_parked();
    let events = collect::<_, SessionEvent>(cx, &session);
    assert_eq!(cx.update(|cx| kinds(session.read(cx).blocks())), ["user", "assistant"]);

    // Text streams into a new block.
    api.emit_all([
        agent(s, AgentEvent::TurnStarted),
        agent(s, AgentEvent::TextDelta { text: "Let me ".into() }),
        agent(s, AgentEvent::TextDelta { text: "check.".into() }),
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert!(model.is_busy());
        assert!(matches!(&model.blocks()[2].kind, BlockKind::Assistant(t) if t.streaming && t.text == "Let me check."));
    });
    assert_eq!(*events.borrow(), [SessionEvent::Spliced(Splice { old: 2..2, count: 1 }), SessionEvent::ScrollToBottom]);
    events.borrow_mut().clear();

    // More text, and a tool call whose input still streams.
    api.emit_all([
        agent(s, AgentEvent::TextDelta { text: " Running tests.".into() }),
        agent(s, AgentEvent::ToolCallStarted { id: "t1".into(), name: "bash".into() }),
    ]);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| tool_state(&session.read(cx).blocks()[3])), Some(ToolState::Started));
    assert_eq!(events.borrow()[0], SessionEvent::Spliced(Splice { old: 2..3, count: 2 }));
    events.borrow_mut().clear();

    // The call is ready and needs a person.
    let request = PermissionRequest {
        id: RequestId::new("perm-x"),
        session: SessionId::new(s),
        agent: "Build".into(),
        tool: "bash".into(),
        call_id: None,
        class: ToolClass::Execute,
        display: command(""),
        policy: "bash = ask".into(),
        always_rule: None,
        at: 0,
    };
    api.emit_all([
        agent(s, AgentEvent::ToolCallReady { id: "t1".into(), name: "bash".into(), class: ToolClass::Execute, display: command("") }),
        Event::PermissionRequested { request },
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert_eq!(tool_state(&model.blocks()[3]), Some(ToolState::Ready));
        assert!(matches!(&model.blocks()[4].kind, BlockKind::Permission(p) if p.state == PermissionState::Pending));
        assert_eq!(model.pending().len(), 1);
    });
    assert!(events.borrow().contains(&SessionEvent::PermissionArrived(RequestId::new("perm-x"))));
    assert!(events.borrow().contains(&SessionEvent::Spliced(Splice { old: 3..4, count: 2 })));
    events.borrow_mut().clear();

    // Answered; the tool finishes, a sub-agent runs, the todo list changes twice, the turn fails.
    let child = SessionId::new("child-1");
    let todo = |status| vec![TodoItem { text: "Run the suite".into(), status }];
    api.emit_all([
        Event::PermissionResolved { id: RequestId::new("perm-x") },
        agent(s, AgentEvent::ToolFinished { id: "t1".into(), display: command("212 pass"), is_error: false }),
        agent(s, AgentEvent::SubagentStarted { session: child.clone(), name: "@explore".into(), task: "Find callers".into(), depth: 1 }),
        agent(s, AgentEvent::TodoUpdated { items: todo(TodoStatus::InProgress) }),
        agent(
            s,
            AgentEvent::SubagentFinished { session: child, summary: "4 callers".into(), ok: true, tool_calls: 4, secs: 14, cost_usd: 0.01 },
        ),
        agent(s, AgentEvent::TodoUpdated { items: todo(TodoStatus::Done) }),
        agent(s, AgentEvent::Failed { error: "rate limited".into() }),
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(kinds(blocks), ["user", "assistant", "assistant", "tool", "permission", "subagent", "todos", "failure"]);
        assert!(matches!(&blocks[2].kind, BlockKind::Assistant(t) if !t.streaming && t.text == "Let me check. Running tests."));
        assert_eq!(tool_state(&blocks[3]), Some(ToolState::Finished { is_error: false }));
        assert!(matches!(&blocks[4].kind, BlockKind::Permission(p) if p.state == PermissionState::Resolved));
        assert!(matches!(&blocks[5].kind, BlockKind::Subagent(sub) if matches!(sub.state, SubagentState::Finished { ok: true, .. })));
        assert!(matches!(&blocks[6].kind, BlockKind::Todos(items) if items[0].status == TodoStatus::Done));
        assert!(!model.is_busy());
        assert!(model.pending().is_empty());
    });
    let events = events.borrow();
    assert_eq!(events[0], SessionEvent::Failed("rate limited".into()));
    assert!(events.contains(&SessionEvent::Spliced(Splice { old: 2..5, count: 6 })));
}

#[gpui::test]
fn history_becomes_blocks_and_an_answer_continues_the_turn(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let events = collect::<_, SessionEvent>(cx, &session);
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(
            kinds(blocks),
            [
                "user", "assistant", "tool", "tool", "tool", "subagent", "subagent", "todos", "tool", "assistant", "tool",
                "permission"
            ]
        );
        assert!(matches!(&blocks[0].kind, BlockKind::User(user) if user.images.len() == 1));
        assert!(matches!(&blocks[2].kind, BlockKind::Tool(t) if t.display == Some(ToolDisplay::Line {
            verb: "read".into(), target: "src/charges/retry.ts".into(), result: "58 lines".into()
        })));
        assert!(matches!(&blocks[3].kind, BlockKind::Tool(t) if matches!(&t.display, Some(ToolDisplay::Line { result, .. }) if result == "6 matches")));
        assert!(matches!(&blocks[5].kind, BlockKind::Subagent(sub) if sub.session == Some(SessionId::new("chat-1423-explore"))));
        assert!(matches!(&blocks[8].kind, BlockKind::Tool(t) if matches!(t.display, Some(ToolDisplay::Diff(_)))));
        assert_eq!(tool_state(&blocks[10]), Some(ToolState::Ready));
        assert_eq!(model.context().tokens, 48_200);
    });

    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.answer_permission(&RequestId::new("perm-1423-bun-test"), PermissionDecision::AllowOnce, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(blocks.len(), 13);
        assert_eq!(tool_state(&blocks[10]), Some(ToolState::Finished { is_error: false }));
        assert!(matches!(&blocks[11].kind, BlockKind::Permission(p) if p.state == PermissionState::Resolved));
        assert!(matches!(&blocks[12].kind, BlockKind::Assistant(t) if !t.streaming && t.seq == Some(14)));
        assert!(matches!(&blocks[7].kind, BlockKind::Todos(items) if items.iter().all(|i| i.status == TodoStatus::Done)));
        assert_eq!(model.session().map(|s| s.state), Some(SessionState::Done));
        assert_eq!(model.detail().map(|d| d.files.len()), Some(2));
    });
    assert!(events.borrow().contains(&SessionEvent::TurnFinished(StopReason::EndTurn)));
}

#[gpui::test]
fn the_transcript_loads_the_last_page_then_older_ones(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let session = cx.update(|cx| crate::session(&SessionId::new("chat-ledger"), cx));
    cx.run_until_parked();
    let events = collect::<_, SessionEvent>(cx, &session);
    cx.update(|cx| {
        let model = session.read(cx);
        assert_eq!(model.blocks().len(), 41);
        assert!(model.has_older());
    });

    session.update(cx, |model, cx| model.load_older(cx));
    cx.run_until_parked();
    assert_eq!(*events.borrow(), [SessionEvent::Spliced(Splice { old: 0..0, count: 41 })]);
    cx.update(|cx| {
        let model = session.read(cx);
        assert_eq!(model.blocks().len(), 82);
        assert!(model.has_older());
        assert!(matches!(&model.blocks()[15].kind, BlockKind::Compaction(c) if c.summarised == 15));
    });

    session.update(cx, |model, cx| model.load_older(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert_eq!(model.blocks().len(), 97);
        assert!(!model.has_older());
        assert_eq!(kinds(model.blocks()).iter().filter(|kind| **kind == "compaction").count(), 1);
        let ids: std::collections::HashSet<_> = model.blocks().iter().map(|block| block.id).collect();
        assert_eq!(ids.len(), 97, "block ids stay unique across pages");
    });
}

#[gpui::test]
fn events_during_the_first_load_are_applied_after_it(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-checkout-flake";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    api.emit_all([agent(s, AgentEvent::TurnStarted), agent(s, AgentEvent::TextDelta { text: "early".into() })]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert_eq!(kinds(model.blocks()), ["user", "assistant", "assistant"]);
        assert!(model.is_busy());
    });
}

#[gpui::test]
fn thinking_and_text_stream_into_separate_blocks_until_committed(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-checkout-flake";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    cx.run_until_parked();
    api.emit_all([
        agent(s, AgentEvent::TurnStarted),
        agent(s, AgentEvent::ThinkingDelta { text: "The iframe ".into() }),
        agent(s, AgentEvent::ThinkingDelta { text: "loads late.".into() }),
        agent(s, AgentEvent::TextDelta { text: "Wait for ".into() }),
        agent(s, AgentEvent::TextDelta { text: "its load event.".into() }),
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let blocks = session.read(cx).blocks();
        assert_eq!(kinds(blocks), ["user", "assistant", "thinking", "assistant"]);
        assert!(matches!(&blocks[2].kind, BlockKind::Thinking(t) if t.streaming && t.text == "The iframe loads late."));
        assert!(matches!(&blocks[3].kind, BlockKind::Assistant(t) if t.streaming && t.text == "Wait for its load event."));
    });

    let message = crate::api::StoredMessage {
        seq: 3,
        at: 5,
        message: crate::api::Message {
            role: crate::api::Role::Assistant,
            content: vec![
                crate::api::ContentBlock::Thinking { text: "The iframe loads late.".into(), signature: None },
                crate::api::ContentBlock::text("Wait for its load event."),
            ],
        },
        compacted: false,
        model: None,
        usage: Default::default(),
        cost_usd: 0.0,
    };
    api.emit_all([
        agent(s, AgentEvent::MessageCommitted { message }),
        agent(s, AgentEvent::TurnFinished { stop: StopReason::EndTurn }),
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(blocks.len(), 4, "the commit closes the streamed blocks and adds nothing");
        assert!(matches!(&blocks[2].kind, BlockKind::Thinking(t) if !t.streaming && t.seq == Some(3)));
        assert!(matches!(&blocks[3].kind, BlockKind::Assistant(t) if !t.streaming && t.seq == Some(3) && t.at == Some(5)));
        assert!(!model.is_busy());
        assert_eq!(model.transcript().newest_seq(), Some(3));
    });
}

#[gpui::test]
fn a_stopped_turn_ends_its_calls_and_says_so(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let events = collect::<_, SessionEvent>(cx, &session);
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.interrupt(&id, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(tool_state(&blocks[10]), Some(ToolState::Finished { is_error: true }));
        assert!(matches!(&blocks[11].kind, BlockKind::Permission(p) if p.state == PermissionState::Resolved));
        assert!(matches!(&blocks[12].kind, BlockKind::Notice(n) if n.kind == crate::transcript::NoticeKind::Interrupted));
        assert!(model.pending().is_empty());
        assert!(!model.is_busy());
        assert_eq!(model.session().map(|s| s.state), Some(SessionState::Idle));
    });
    assert!(events.borrow().contains(&SessionEvent::TurnFinished(StopReason::Cancelled)));
}

#[gpui::test]
fn a_sent_message_streams_a_whole_turn(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let id = SessionId::new("chat-split-checkout");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.send(&id, crate::api::UserMessage { text: "Go on".into(), images: Vec::new() }, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        let blocks = model.blocks();
        assert_eq!(kinds(blocks), ["user", "assistant", "user", "assistant", "tool", "assistant"]);
        assert!(matches!(&blocks[2].kind, BlockKind::User(u) if u.text == "Go on" && u.seq == 3));
        assert!(matches!(&blocks[3].kind, BlockKind::Assistant(t) if !t.streaming && t.seq == Some(4)));
        assert_eq!(tool_state(&blocks[4]), Some(ToolState::Finished { is_error: false }));
        assert!(matches!(&blocks[5].kind, BlockKind::Assistant(t) if !t.streaming && t.seq == Some(6)));
        assert_eq!(model.context().tokens, 48_900);
        assert!(!model.is_busy());
    });
}

#[gpui::test]
fn a_message_to_a_busy_session_shows_as_queued(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let id = SessionId::new("chat-refund-timeout");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| session.read(cx).is_busy()));
    let commands = cx.update(|cx| crate::commands(cx));
    let text = "Also check the batcher";
    cx.update(|cx| commands.send(&id, crate::api::UserMessage { text: text.into(), images: Vec::new() }, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(session.read(cx).queued().len(), 1));

    // Delivered at the next turn: the stored user message takes it off the queue.
    let message = crate::api::StoredMessage {
        seq: 5,
        at: 0,
        message: crate::api::Message::user(text),
        compacted: false,
        model: None,
        usage: Default::default(),
        cost_usd: 0.0,
    };
    api.emit(agent("chat-refund-timeout", AgentEvent::MessageCommitted { message }));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert!(model.queued().is_empty());
        assert!(matches!(model.blocks().last().map(|b| &b.kind), Some(BlockKind::User(u)) if u.text == text));
    });
}

#[gpui::test]
fn a_sub_agent_streams_under_its_own_session(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let parent_id = SessionId::new("chat-split-checkout");
    let parent = cx.update(|cx| crate::session(&parent_id, cx));
    cx.run_until_parked();
    let child_id = SessionId::new("chat-split-explore");
    let mut child_session = api.with_world(|world| world.session(&parent_id).cloned()).expect("the session is in the world");
    child_session.id = child_id.clone();
    child_session.parent = Some(parent_id.clone());
    child_session.depth = 1;
    child_session.message_count = 0;
    child_session.pinned = false;
    api.with_world(|world| world.sessions.push(child_session.clone()));

    let s = parent_id.as_str();
    api.emit_all([
        Event::SessionChanged { session: child_session.clone() },
        agent(s, AgentEvent::SubagentStarted { session: child_id.clone(), name: "@explore".into(), task: "Find seams".into(), depth: 1 }),
    ]);
    cx.run_until_parked();
    let child = cx.update(|cx| crate::session(&child_id, cx));
    cx.run_until_parked();
    let parent_blocks = cx.update(|cx| parent.read(cx).blocks().len());

    api.emit_all([
        agent(child_id.as_str(), AgentEvent::TurnStarted),
        agent(child_id.as_str(), AgentEvent::TextDelta { text: "Cart API first.".into() }),
    ]);
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(parent.read(cx).blocks().len(), parent_blocks, "the child's stream stays in the child");
        assert_eq!(kinds(child.read(cx).blocks()), ["assistant"]);
        assert!(child.read(cx).is_busy());
        assert_eq!(parent.read(cx).detail().map(|d| d.children.len()), Some(1));
    });

    api.emit(agent(
        s,
        AgentEvent::SubagentFinished { session: child_id.clone(), summary: "Cart API first".into(), ok: true, tool_calls: 3, secs: 9, cost_usd: 0.01 },
    ));
    cx.run_until_parked();
    cx.update(|cx| {
        let parent = parent.read(cx);
        let last = parent.blocks().last().map(|b| b.kind.clone());
        assert!(matches!(last, Some(BlockKind::Subagent(sub))
            if sub.session == Some(child_id.clone()) && matches!(sub.state, SubagentState::Finished { tool_calls: 3, .. })));
    });
}
