//! Tool rows after the fact: durations from the block's own times, interrupted calls keeping
//! the look of their kind, sub-agent depth against the policy's limit, diff gutter numbers.

use gpui::TestAppContext;
use hub::api::ChatApi as _;
use hub::fixtures::script::INTERRUPTED;
use hub::model::{ContentBlock, DiffLine, DiffLineKind, FileDiff, Message, Role, SessionId, StoredMessage, ToolDisplay};
use hub::transcript::{BlockKind, SubagentBlock, SubagentState, ToolBlock, ToolState};
use ui_rich::diff_view::{DiffRow, diff_rows};

use super::{FIX, NOW, Open, drawn, open, open_with, settle};
use crate::transcript::agents::depth_words;
use crate::transcript::tools::{Phase, interrupted_display, to_diff_file};

fn tool(open: &Open, cx: &mut gpui::VisualTestContext, id: &str) -> ToolBlock {
    open.view.read_with(cx, |view, cx| {
        view.model
            .read(cx)
            .blocks()
            .iter()
            .find_map(|b| match &b.kind {
                BlockKind::Tool(t) if t.id == id => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no tool {id}"))
    })
}

#[gpui::test]
fn durations_come_from_the_blocks_own_times(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    let call = ToolBlock {
        id: "x1".into(),
        name: "bash".into(),
        class: None,
        display: Some(ToolDisplay::Command { command: "ls".into(), output: String::new(), exit_code: Some(0) }),
        state: ToolState::Finished { is_error: false },
        started: Some(NOW - 14_000),
        finished: Some(NOW),
    };
    let status = open.view.read_with(cx, |view, cx| view.tool_status(&call, false, cx));
    assert_eq!((status.phase, status.secs), (Phase::Done, Some(14)), "a call the view never saw run");
    let untimed = ToolBlock { started: None, ..call };
    assert_eq!(open.view.read_with(cx, |view, cx| view.tool_status(&untimed, false, cx)).secs, None);
}

#[gpui::test]
fn a_call_interrupted_live_keeps_its_command_row(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    open.api.interrupt(&SessionId::new(FIX));
    settle(cx);
    let call = tool(&open, cx, "t8");
    assert!(matches!(&call.display, Some(ToolDisplay::Text { title, .. }) if title == "bash"), "the result replaced the display");
    let (phase, shown) = open.view.read_with(cx, |view, cx| {
        (view.tool_status(&call, false, cx).phase, interrupted_display(&call, view.call_displays.get("t8")))
    });
    assert_eq!(phase, Phase::Interrupted);
    assert_eq!(
        shown,
        Some(ToolDisplay::Command { command: "bun test src/charges".into(), output: String::new(), exit_code: None })
    );
    assert!(drawn(cx, "tool-t8".into()));
}

#[gpui::test]
fn a_call_interrupted_before_a_restart_reads_interrupted(cx: &mut TestAppContext) {
    let (open, cx) = open_with(cx, FIX, 4000., |world| {
        let id = SessionId::new(FIX);
        world.permissions.retain(|request| request.session != id);
        if let Some(messages) = world.messages.get_mut(&id) {
            let seq = messages.last().map_or(0, |m| m.seq) + 1;
            messages.push(StoredMessage {
                seq,
                at: NOW,
                message: Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "t8".into(),
                        content: INTERRUPTED.into(),
                        is_error: true,
                    }],
                },
                compacted: false,
                model: None,
                usage: Default::default(),
                cost_usd: 0.0,
            });
        }
    });
    let call = tool(&open, cx, "t8");
    let (phase, shown) = open.view.read_with(cx, |view, cx| {
        (view.tool_status(&call, false, cx).phase, interrupted_display(&call, view.call_displays.get("t8")))
    });
    assert_eq!(phase, Phase::Interrupted);
    assert!(matches!(shown, Some(ToolDisplay::Command { ref command, ref output, .. }) if command == "bun test src/charges" && output.is_empty()));
}

#[gpui::test]
fn sub_agent_groups_read_their_depth_against_the_limit(cx: &mut TestAppContext) {
    let sub = SubagentBlock {
        session: None,
        call: None,
        name: "@explore".into(),
        task: "Find callers".into(),
        depth: 1,
        state: SubagentState::Running,
        max_depth: Some(2),
    };
    assert_eq!(depth_words(&sub), "depth 1 of 2");
    assert_eq!(depth_words(&SubagentBlock { max_depth: None, ..sub }), "depth 1");

    let (open, cx) = open(cx, FIX, 4000.);
    let limits: Vec<Option<u32>> = open.view.read_with(cx, |view, cx| {
        view.model
            .read(cx)
            .blocks()
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::Subagent(s) => Some(s.max_depth),
                _ => None,
            })
            .collect()
    });
    assert_eq!(limits, [Some(2), Some(2)], "the fixture's Build may go two deep");
}

#[gpui::test]
fn diff_lines_keep_their_numbers_for_the_gutter() {
    let line = |kind, old_no, new_no, text: &str| DiffLine { kind, old_no, new_no, text: text.into(), no_newline: false };
    let file = FileDiff {
        path: "src/charges/retry.ts".into(),
        added: 1,
        removed: 1,
        hunks: vec![hub::model::DiffHunk {
            lines: vec![
                line(DiffLineKind::Context, Some(44), Some(44), "export async function retryCharge(charge: Charge) {"),
                line(DiffLineKind::Del, Some(45), None, "  const res = await gateway.charge(charge.amount, charge.card);"),
                line(DiffLineKind::Add, None, Some(45), "  const key = charge.idempotencyKey ?? deriveKey(charge);"),
            ],
        }],
        ..FileDiff::default()
    };
    let numbers: Vec<Option<u32>> = diff_rows(&to_diff_file(&file))
        .into_iter()
        .filter_map(|row| match row {
            DiffRow::Line { number, .. } => Some(number),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, [Some(44), Some(45), Some(45)]);
}
