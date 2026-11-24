//! Contract additions: kept tool displays in history, the interrupted flag, deleting a
//! comment, running an autopilot, folder listings.

use std::path::{Path, PathBuf};

use gpui::TestAppContext;

use super::setup;
use crate::api::{
    Actor, AutopilotId, ContentBlock, Period, DiffLineKind, IssueEventKind, IssueKey, Message, Role, SessionId, StoredMessage,
    ToolDisplay,
};
use crate::transcript::{BlockKind, Transcript};

fn diff_of(blocks: &[std::sync::Arc<crate::Block>], id: &str) -> Option<crate::api::FileDiff> {
    blocks.iter().find_map(|block| match &block.kind {
        BlockKind::Tool(tool) if tool.id == id => match &tool.display {
            Some(ToolDisplay::Diff(diff)) => Some(diff.clone()),
            _ => None,
        },
        _ => None,
    })
}

#[gpui::test]
fn history_draws_the_kept_display_and_derives_one_without_it(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let kept = cx.update(|cx| diff_of(session.read(cx).blocks(), "t7")).expect("t7");
    assert_eq!((kept.added, kept.removed), (3, 2), "the real counts, not every line in and out");
    let lines: Vec<_> = kept.hunks.iter().flat_map(|h| &h.lines).collect();
    assert!(lines.iter().any(|l| l.kind == DiffLineKind::Context && l.old_no == Some(44)));

    // A session whose displays were never kept (an old or imported one) derives the diff.
    api.with_world(|world| world.tool_displays.clear());
    drop(session);
    cx.run_until_parked();
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let derived = cx.update(|cx| diff_of(session.read(cx).blocks(), "t7")).expect("t7");
    assert_eq!((derived.added, derived.removed), (6, 5));
    assert!(derived.hunks.iter().flat_map(|h| &h.lines).all(|l| l.old_no.is_none() && l.new_no.is_none()));
}

#[gpui::test]
fn a_stopped_call_is_flagged_interrupted(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    assert!(!cx.update(|cx| session.read(cx).is_interrupted("t8")));
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.interrupt(&id, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let model = session.read(cx);
        assert!(model.is_interrupted("t8"), "the call waiting at the stop");
        assert!(!model.is_interrupted("t7"), "a call that finished");
    });
}

#[test]
fn a_call_answered_as_interrupted_in_history_is_flagged() {
    let stored = |seq, role, content| StoredMessage {
        seq,
        at: 0,
        message: Message { role, content },
        compacted: false,
        model: None,
        usage: Default::default(),
        cost_usd: 0.0,
    };
    let call = ContentBlock::ToolUse { id: "c1".into(), name: "edit_file".into(), input: r#"{"path": "a", "new_string": "b"}"#.parse().expect("json") };
    let result = ContentBlock::ToolResult {
        tool_use_id: "c1".into(),
        content: crate::fixtures::script::INTERRUPTED.into(),
        is_error: true,
    };
    let mut transcript = Transcript::default();
    transcript.append_history(&[stored(1, Role::Assistant, vec![call]), stored(2, Role::User, vec![result])]);
    assert!(transcript.is_interrupted("c1"), "a diff shows no result text, the flag still tells");
    assert!(!transcript.is_interrupted("c2"));
}

#[gpui::test]
fn deleting_a_comment_drops_it_from_the_loaded_feed(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let key = IssueKey::new("PAY-1427");
    let issues = cx.update(crate::issues);
    cx.update(|cx| issues.update(cx, |model, cx| model.ensure_loaded(cx)));
    cx.run_until_parked();
    cx.update(|cx| issues.update(cx, |model, cx| model.ensure_activity(&key, cx)));
    cx.run_until_parked();
    let own = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let model = issues.read(cx);
            let feed = model.activity(&key).and_then(|feed| feed.value().cloned()).unwrap_or_default();
            feed.iter()
                .filter(|e| e.actor == Actor::Me && matches!(e.kind, IssueEventKind::Comment { .. }))
                .map(|e| e.seq)
                .collect::<Vec<u64>>()
        })
    };
    let before = own(cx);
    let seq = *before.first().expect("an own comment in the fixture");
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.delete_comment(&key, seq, cx));
    cx.run_until_parked();
    assert_eq!(own(cx).len(), before.len() - 1);
    assert!(!own(cx).contains(&seq));
    assert_eq!(api.recorded_names().last(), Some(&"delete_comment"));
}

#[gpui::test]
fn autopilots_run_by_hand_and_folders_list_for_completion(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let id = api.with_world(|world| world.autopilots.first().map(|a| a.id.clone())).expect("an autopilot");
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.run_autopilot(&id, cx));
    cx.run_until_parked();
    assert!(api.with_world(|world| world.autopilots.iter().any(|a| a.id == id && a.last_result == "started a run")));
    cx.update(|cx| commands.run_autopilot(&AutopilotId::new("nope"), cx));
    cx.run_until_parked();

    let queries = cx.update(|cx| crate::queries(cx));
    let acme = PathBuf::from("/home/me/code/acme");
    let listed = cx.update(|cx| queries.list_dir(&acme, "", 10, cx));
    let listed = cx.executor().block_test(listed).expect("list");
    let names: Vec<&str> = listed.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["checkout-web", "ledger", "payments-api", ".archive"]);
    assert!(listed.iter().find(|c| c.name == "ledger").is_some_and(|c| c.is_repo));
    let exists = |path: &Path, cx: &mut TestAppContext| {
        let task = cx.update(|cx| queries.folder_exists(path, cx));
        cx.executor().block_test(task)
    };
    assert!(exists(&acme, cx));
    assert!(!exists(Path::new("/home/me/code/gone"), cx));
    let missing = cx.update(|cx| queries.list_dir(Path::new("/home/me/code/gone"), "", 10, cx));
    assert!(cx.executor().block_test(missing).is_err());
}

#[gpui::test]
fn home_reloads_after_failed_queries(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    for query in ["home_stats", "needs_you", "working_now", "spend_today", "spend_buckets", "spend_by_runnable"] {
        api.fail_next(query);
    }
    let ops = cx.update(crate::ops);
    cx.run_until_parked();
    let failed = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let ops = ops.read(cx);
            let failed = |state: &crate::LoadState| matches!(state, crate::LoadState::Failed(_));
            [
                ops.stats(Period::Today).is_some_and(|s| failed(s.state())),
                failed(ops.needs().state()),
                failed(ops.working().state()),
                failed(ops.spend_today().state()),
                ops.spend_buckets(Period::Today).is_some_and(|s| failed(s.state())),
                ops.spend_by_runnable(Period::Today).is_some_and(|s| failed(s.state())),
            ]
        })
    };
    assert_eq!(failed(cx), [true; 6], "every injected failure shows");

    ops.update(cx, |ops, cx| ops.ensure_period(Period::Today, cx));
    cx.run_until_parked();
    assert_eq!(failed(cx), [true; 6], "ensure does not retry a failed load");

    ops.update(cx, |ops, cx| ops.reload(Period::Today, cx));
    cx.run_until_parked();
    assert_eq!(failed(cx), [false; 6], "reload asks again");
    cx.update(|cx| assert!(ops.read(cx).stats(Period::Today).is_some_and(|s| s.is_ready())));
}
