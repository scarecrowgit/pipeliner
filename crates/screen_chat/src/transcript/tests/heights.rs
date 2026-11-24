//! Rows with long content are as tall as their wrapped text, and the text lies inside its row.

use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext};
use hub::fixtures::World;
use hub::model::{AgentEvent, BlobId, ContentBlock, Role, SessionId, StopReason, ToolClass, ToolDisplay};

use super::{counts, emit, open_with, selector, settle};

const CHAT: &str = "chat-split-checkout";
/// Window width of these tests; the column is at most 808 px of it.
const WIDTH: f32 = 1200.;

const LONG: &str = "Create a file named hello.txt in this folder containing exactly one line: hello from harness. \
                    Then run git status --short, tell me what it printed, and explain every line of that output to me.";

fn paragraphs() -> String {
    (0..4).map(|_| LONG).collect::<Vec<_>>().join("\n\n")
}

fn stored(seq: u64, role: Role, content: Vec<ContentBlock>) -> hub::model::StoredMessage {
    hub::model::StoredMessage {
        seq,
        at: super::NOW - 60_000,
        message: hub::model::Message { role, content },
        compacted: false,
        model: None,
        usage: Default::default(),
        cost_usd: 0.0,
    }
}

/// The chat with these messages and nothing else.
fn with_messages(messages: Vec<hub::model::StoredMessage>) -> impl FnOnce(&mut World) {
    move |world| {
        let count = messages.len() as u32;
        world.messages.insert(SessionId::new(CHAT), messages);
        if let Ok(chat) = world.session_mut(&SessionId::new(CHAT)) {
            chat.message_count = count;
        }
    }
}

fn bounds(cx: &mut VisualTestContext, name: String) -> Bounds<Pixels> {
    cx.debug_bounds(selector(name.clone())).unwrap_or_else(|| panic!("`{name}` was not drawn"))
}

fn height(cx: &mut VisualTestContext, index: usize) -> f32 {
    f32::from(bounds(cx, format!("block-{index}")).size.height)
}

/// Lines `text` needs in the column at the test font (0.6 em per glyph), with slack.
fn most_lines(text: &str, size: f32) -> f32 {
    let per_line = (760. / (size * 0.6)).floor();
    text.split('\n').map(|para| (para.chars().count() as f32 / per_line).ceil().max(1.)).sum::<f32>() + 2.
}

fn assert_inside(cx: &mut VisualTestContext, inner: String, outer: String) {
    let (i, o) = (bounds(cx, inner.clone()), bounds(cx, outer.clone()));
    assert!(
        i.top() >= o.top() && i.bottom() <= o.bottom() + px1() && i.left() >= o.left() && i.right() <= o.right() + px1(),
        "`{inner}` {i:?} is not inside `{outer}` {o:?}"
    );
}

fn px1() -> Pixels {
    gpui::px(1.)
}

#[gpui::test]
fn long_user_messages_are_as_tall_as_their_text(cx: &mut TestAppContext) {
    let image = ContentBlock::Image {
        blob: BlobId::new("blob-shot"),
        media_type: "image/png".into(),
        name: "shot.png".into(),
        bytes: 1000,
    };
    let messages = vec![
        stored(1, Role::User, vec![ContentBlock::text(LONG)]),
        stored(2, Role::Assistant, vec![ContentBlock::text("Done.")]),
        stored(3, Role::User, vec![ContentBlock::text(paragraphs())]),
        stored(4, Role::Assistant, vec![ContentBlock::text("Done.")]),
        stored(5, Role::User, vec![image, ContentBlock::text(format!("Look at @src/charges/retry.ts first. {LONG}"))]),
        stored(6, Role::Assistant, vec![ContentBlock::text("Done.")]),
    ];
    let (open, cx) = open_with(cx, CHAT, 6000., with_messages(messages));
    let _ = open;
    let texts = [LONG.to_string(), paragraphs(), format!("Look at @src/charges/retry.ts first. {LONG}")];
    for (row, text) in [0usize, 2, 4].into_iter().zip(texts) {
        let h = height(cx, row);
        // Header (24) + gap + card padding + lines of 20 px (+ an 84 px image row on the last).
        let limit = 24. + 12. + 28. + 28. + most_lines(&text, 13.) * 20. + if row == 4 { 94. } else { 0. } + 30.;
        assert!(h > 60. && h < limit, "user row {row} is {h} px tall, expected under {limit}");
        assert_inside(cx, format!("user-text-{row}"), format!("block-{row}"));
    }
}

#[gpui::test]
fn every_long_block_is_as_tall_as_its_content(cx: &mut TestAppContext) {
    let messages = vec![
        stored(1, Role::User, vec![ContentBlock::text("Go.")]),
        stored(2, Role::Assistant, vec![ContentBlock::text(paragraphs())]),
    ];
    let (open, cx) = open_with(cx, CHAT, 6000., with_messages(messages));
    let output: String = (0..8).map(|_| format!("{LONG}\n")).collect();
    let command = |output: String| ToolDisplay::Command { command: LONG.into(), output, exit_code: Some(0) };
    emit(
        &open,
        cx,
        CHAT,
        vec![
            AgentEvent::TurnStarted,
            AgentEvent::ThinkingDelta { text: paragraphs() },
            AgentEvent::ToolCallStarted { id: "c".into(), name: "bash".into() },
            AgentEvent::ToolCallReady { id: "c".into(), name: "bash".into(), class: ToolClass::Execute, display: command(String::new()) },
        ],
    );
    emit(
        &open,
        cx,
        CHAT,
        vec![
            AgentEvent::ToolFinished { id: "c".into(), display: command(output.clone()), is_error: false },
            AgentEvent::Notice { text: LONG.into() },
            AgentEvent::Failed { error: paragraphs() },
            AgentEvent::TurnFinished { stop: StopReason::EndTurn },
        ],
    );
    let blocks = counts(&open, cx).1;
    assert_eq!(blocks, 6, "user, assistant, thinking, tool, notice, failure");
    // assistant: long markdown
    let h = height(cx, 1);
    assert!(h < 36. + most_lines(&paragraphs(), 13.) * 20. + 60., "assistant row is {h} px");
    // thinking: folded after the turn ended; open it and measure
    super::click(cx, "thinking-2");
    let h = height(cx, 2);
    assert!(h > 40. && h < 30. + most_lines(&paragraphs(), 13.) * 20. + 40., "thinking row is {h} px");
    // command: clamped output, 12 lines of wrapped text at most
    let h = height(cx, 3);
    assert!(h > 60. && h < 36. + 12. * 2. * 20. + 32. + 40., "command row is {h} px");
    // notice: one line
    let h = height(cx, 4);
    assert!(h < 60., "notice row is {h} px");
    // failure: the wrapped error
    let h = height(cx, 5);
    assert!(h < 20. + 24. + most_lines(&paragraphs(), 12.) * 18. + 60., "failure row is {h} px");
    for row in 0..blocks {
        let b = bounds(cx, format!("block-{row}"));
        assert!(f32::from(b.size.width) <= WIDTH, "row {row} is wider than the window: {b:?}");
    }
}

#[gpui::test]
fn a_long_history_opens_at_its_newest_message(cx: &mut TestAppContext) {
    let messages: Vec<_> = (0..120u64)
        .map(|i| {
            let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
            stored(i + 1, role, vec![ContentBlock::text(format!("{i}: {LONG}"))])
        })
        .collect();
    let (open, cx) = open_with(cx, CHAT, 800., with_messages(messages));
    let (_, blocks) = counts(&open, cx);
    assert!(open.view.read_with(cx, |view, _| view.is_following()));
    let last = bounds(cx, format!("block-{}", blocks - 1));
    let window_bottom = 800.;
    assert!(
        f32::from(last.bottom()) <= window_bottom + 1. && f32::from(last.top()) >= 0.,
        "the newest message is in view: {last:?}"
    );
}

#[gpui::test]
fn a_narrow_list_wraps_at_its_own_width(cx: &mut TestAppContext) {
    let messages = vec![stored(1, Role::User, vec![ContentBlock::text(paragraphs())])];
    let (_open, cx) = open_with(cx, CHAT, 3000., with_messages(messages));
    cx.simulate_resize(gpui::size(gpui::px(520.), gpui::px(3000.)));
    settle(cx);
    settle(cx);
    let row = bounds(cx, "block-0".into());
    assert!(f32::from(row.size.width) <= 520., "{row:?}");
    assert_inside(cx, "user-text-0".into(), "block-0".into());
    let text = bounds(cx, "user-text-0".into());
    assert!(f32::from(text.size.width) <= 520. - 48. - 32. - 28., "the text wraps inside the card: {text:?}");
}
