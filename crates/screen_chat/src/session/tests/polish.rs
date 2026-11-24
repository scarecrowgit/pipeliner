//! What the real window showed: an unknown context window, the workdir section's path and
//! Change link, MCP servers no turn has started, and the keys that answer a permission question.

use gpui::TestAppContext;
use hub::ContextUsage;
use hub::api::McpState;
use hub::model::SessionId;

use super::{FIX, calls, click, focus_composer, open, open_with, shown, type_text};
use crate::session::panel::{context_amount, context_line, mcp_starting_word, workdir_place};

#[test]
fn an_unknown_context_window_claims_no_share() {
    let unknown = ContextUsage { tokens: 131_300, window: 0, cost_usd: 0.0 };
    assert_eq!(context_amount(unknown), ("131.3k".to_string(), None));
    assert!(!context_line(unknown, Some(0.9)).contains('%'));
    let known = ContextUsage { tokens: 48_200, window: 200_000, cost_usd: 0.19 };
    assert_eq!(context_amount(known), ("48.2k".to_string(), Some("of 200k".to_string())));
    assert_eq!(context_line(known, Some(0.9)), "24% used · compacts at 90%");
}

#[test]
fn a_long_workdir_path_is_cut_in_the_middle() {
    let place = workdir_place("", "/tmp/claude-1000/-mnt-storage-projects-pipeliner/b931af7e/scratchpad/cli-smoke");
    assert!(place.starts_with("local · /tmp/…"), "{place}");
    assert!(place.ends_with("/cli-smoke"), "{place}");
    assert!(place.chars().count() <= 42, "{place}");
    assert_eq!(workdir_place("mac-mini", "~/code/acme"), "mac-mini · ~/code/acme");
}

#[gpui::test]
fn the_workdir_section_shows_the_home_relative_path_and_change_opens_the_popover(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx, FIX);
    let place = cx.update(|_, cx| workdir_place("mac-mini", &hub::display_path(std::path::Path::new("/home/me/code/acme"), cx)));
    assert_eq!(place, "mac-mini · ~/code/acme", "the fixture hub's home is /home/me");
    click(cx, "chat-panel-workdir-change");
    let open = cx.update(|window, cx| ui::overlays(window, cx).open_count(cx));
    assert_eq!(open, 1, "the workdir popover is up");
}

#[gpui::test]
fn mcp_servers_no_turn_started_read_not_started(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, FIX, |w| {
        if let Some(extras) = w.extras.get_mut(&SessionId::new(FIX)) {
            extras.mcp[0].state = McpState::Starting;
        }
    });
    assert_eq!(mcp_starting_word(), "not started");
    let state = t.view.read_with(cx, |v, cx| v.model.read(cx).detail().map(|d| d.mcp[0].state));
    assert_eq!(state, Some(McpState::Starting));
    assert!(shown(cx, "chat-panel-mcp-0"));
}

#[gpui::test]
fn enter_and_escape_in_an_empty_composer_answer_the_question(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    focus_composer(&t, cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let rejecting = t.view.read_with(cx, |v, cx| v.transcript.read(cx).is_rejecting());
    assert!(rejecting, "Esc opens the Reject note");
    t.view.update(cx, |v, cx| v.transcript.update(cx, |tr, cx| tr.cancel_reject(cx)));

    focus_composer(&t, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "answer_permission"), ["perm-1423-bun-test AllowOnce"]);
}

#[gpui::test]
fn enter_with_a_message_sends_it_and_leaves_the_question(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    type_text(&t, "Use the CI runner", cx);
    focus_composer(&t, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "send"), [format!("{FIX} Use the CI runner")]);
    assert!(calls(&t.api, "answer_permission").is_empty());
}
