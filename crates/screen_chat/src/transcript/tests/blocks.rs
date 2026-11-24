//! Every kind of block from fixture data and live events; folding; "Show more"; sub-agents.

use gpui::TestAppContext;
use hub::Route;
use hub::model::{AgentEvent, SessionId, StopReason, ToolClass, ToolDisplay};
use hub::transcript::BlockKind;

use super::{FIX, click, counts, drawn, emit, kinds, open, open_with};
use crate::transcript::message::mentions;
use crate::transcript::tools::clamp_lines;

#[gpui::test]
fn the_mockup_chat_draws_every_block_it_has(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    assert_eq!(
        kinds(&open, cx),
        [
            "user", "assistant", "tool", "tool", "tool", "subagent", "subagent", "todos", "tool", "assistant", "tool",
            "permission"
        ]
    );
    let (rows, blocks) = counts(&open, cx);
    assert_eq!(rows, blocks + 1, "one row per block plus the footer");
    for index in 0..blocks {
        assert!(drawn(cx, format!("block-{index}")), "block {index} is not drawn");
    }
    for call in ["t1", "t2", "t3", "t7", "t8"] {
        assert!(drawn(cx, format!("tool-{call}")), "tool {call} is not drawn");
    }
    // The question sits on the `bun test` row; its own row draws nothing.
    open.view.read_with(cx, |view, _| {
        assert!(view.links.pending_on(10).is_some());
        assert!(view.links.is_hidden(11));
    });
    assert!(drawn(cx, "perm-allow".into()) && drawn(cx, "perm-always".into()) && drawn(cx, "perm-deny".into()));
}

#[gpui::test]
fn live_events_add_thinking_notices_compaction_and_failure(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, "chat-split-checkout", 3000.);
    let before = counts(&open, cx).1;
    emit(
        &open,
        cx,
        "chat-split-checkout",
        vec![
            AgentEvent::TurnStarted,
            AgentEvent::ThinkingDelta { text: "Weighing the seams".into() },
            AgentEvent::CompactionStarted,
            AgentEvent::Compacted { summarised: 12, context_tokens: 9_000 },
            AgentEvent::Retrying { attempt: 2, reason: "overloaded".into() },
            AgentEvent::Failed { error: "rate limited by the gateway".into() },
        ],
    );
    assert_eq!(&kinds(&open, cx)[before..], ["thinking", "compaction", "notice", "failure"]);
    let (rows, blocks) = counts(&open, cx);
    assert_eq!(rows, blocks + 1);
    for index in before..blocks {
        assert!(drawn(cx, format!("block-{index}")), "block {index} is not drawn");
    }
    assert!(drawn(cx, "retry-turn".into()), "the last failure offers Retry");
    click(cx, "retry-turn");
    assert!(open.api.recorded_names().contains(&"retry"));
}

#[gpui::test]
fn thinking_folds_when_finished_and_opens_on_click(cx: &mut TestAppContext) {
    let s = "chat-split-checkout";
    let (open, cx) = open(cx, s, 3000.);
    emit(&open, cx, s, vec![AgentEvent::TurnStarted, AgentEvent::ThinkingDelta { text: "Weighing".into() }]);
    let index = counts(&open, cx).1 - 1;
    let id = open.view.read_with(cx, |view, cx| view.model.read(cx).blocks()[index].id);
    let height = |cx: &mut gpui::VisualTestContext| {
        cx.debug_bounds(super::selector(format!("block-{index}"))).map(|b| f32::from(b.size.height)).unwrap_or(0.)
    };
    let streaming = height(cx);
    assert!(open.view.read_with(cx, |view, _| view.markdown.contains_key(&id)), "streaming thinking is open");

    emit(&open, cx, s, vec![AgentEvent::TurnFinished { stop: StopReason::EndTurn }]);
    let folded = height(cx);
    assert!(folded < streaming, "finished thinking folds: {streaming} -> {folded}");

    click(cx, &format!("thinking-{index}"));
    assert_eq!(open.view.read_with(cx, |view, _| view.row_ui(id).open), Some(true));
    let reopened = height(cx);
    assert!(reopened > folded, "a click opens it again: {folded} -> {reopened}");
    click(cx, &format!("thinking-{index}"));
    assert_eq!(open.view.read_with(cx, |view, _| view.row_ui(id).open), Some(false));
}

#[gpui::test]
fn long_command_output_is_clamped_behind_show_more(cx: &mut TestAppContext) {
    let s = "chat-split-checkout";
    let (open, cx) = open(cx, s, 3000.);
    let command = |output: String| ToolDisplay::Command { command: "bun test".into(), output, exit_code: Some(0) };
    let output: String = (1..=40).map(|n| format!("✓ test {n}\n")).collect();
    emit(
        &open,
        cx,
        s,
        vec![
            AgentEvent::TurnStarted,
            AgentEvent::ToolCallStarted { id: "long".into(), name: "bash".into() },
            AgentEvent::ToolCallReady {
                id: "long".into(),
                name: "bash".into(),
                class: ToolClass::Execute,
                display: command(String::new()),
            },
        ],
    );
    emit(&open, cx, s, vec![AgentEvent::ToolFinished { id: "long".into(), display: command(output), is_error: false }]);
    let index = counts(&open, cx).1 - 1;
    assert!(drawn(cx, "tool-long".into()));
    let more = format!("show-more-{index}");
    assert!(drawn(cx, more.clone()), "40 lines are clamped");
    let id = open.view.read_with(cx, |view, cx| view.model.read(cx).blocks()[index].id);
    let height = |cx: &mut gpui::VisualTestContext| {
        cx.debug_bounds(super::selector(format!("block-{index}"))).map(|b| f32::from(b.size.height)).unwrap_or(0.)
    };
    let clamped = height(cx);
    click(cx, &more);
    assert!(open.view.read_with(cx, |view, _| view.row_ui(id).expanded));
    assert!(height(cx) > clamped, "Show more shows the rest");
    // The view timed the call it saw start.
    assert!(open.view.read_with(cx, |view, _| view.timings.get("long").is_some_and(|t| t.end.is_some())));
}

#[gpui::test]
fn a_sub_agent_opens_its_session(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    let (index, id, session) = open.view.read_with(cx, |view, cx| {
        let blocks = view.model.read(cx).blocks();
        let index = blocks.iter().position(|block| matches!(block.kind, BlockKind::Subagent(_))).unwrap_or(0);
        let session = match &blocks[index].kind {
            BlockKind::Subagent(sub) => sub.session.clone(),
            _ => None,
        };
        (index, blocks[index].id, session)
    });
    assert_eq!(index, 5);
    assert_eq!(session, Some(SessionId::new("chat-1423-explore")));
    click(cx, &format!("open-subagent-{id}"));
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(route, Route::Chat { session: SessionId::new("chat-1423-explore") });
}

#[gpui::test]
fn a_chat_without_messages_shows_the_empty_state(cx: &mut TestAppContext) {
    let s = "chat-split-checkout";
    let (_open, cx) = open_with(cx, s, 1000., |world| {
        world.messages.remove(&SessionId::new(s));
        if let Ok(chat) = world.session_mut(&SessionId::new(s)) {
            chat.message_count = 0;
        }
    });
    assert!(drawn(cx, "transcript-empty".into()));
    assert!(!drawn(cx, "block-0".into()));
}

#[gpui::test]
fn a_missing_chat_shows_the_load_error(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, "no-such-chat", 1000.);
    assert!(open.view.read_with(cx, |view, cx| matches!(view.model.read(cx).state(), hub::LoadState::Failed(_))));
    assert!(drawn(cx, "transcript-error".into()));
}

#[test]
fn mentions_drop_trailing_punctuation() {
    assert_eq!(
        mentions("Fix @src/charges/retry.ts. Ask @Fixer, then @Fixer again"),
        ["src/charges/retry.ts", "Fixer"]
    );
    assert!(mentions("no at here, mail@example.com").is_empty());
}

#[test]
fn clamping_keeps_the_first_lines() {
    let text = (1..=20).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
    let (shown, hidden) = clamp_lines(&text, 12, false);
    assert_eq!(shown.lines().count(), 12);
    assert_eq!(hidden, 8);
    assert_eq!(clamp_lines(&text, 12, true).1, 0);
    assert_eq!(clamp_lines("a\nb\n", 12, false), ("a\nb".to_string(), 0));
}

#[test]
fn a_summary_row_shows_its_first_line_without_heading_marks() {
    use crate::transcript::agents::summary_line;
    assert_eq!(summary_line("\n## Findings\nThe retry path double-charges."), "Findings");
    assert_eq!(summary_line("Found **two** call sites.\nMore."), "Found **two** call sites.");
    assert_eq!(summary_line("#hashtag first"), "#hashtag first");
    assert_eq!(summary_line("###"), "");
}
