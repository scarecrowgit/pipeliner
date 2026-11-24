//! What the screens asked of hub: display paths, refreshed details, search failures, busy
//! states, tool timings, sub-agent depth limits, and splices that cover only changed rows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::TestAppContext;

use super::{NOW, agent, collect, setup};
use crate::api::{
    AgentEvent, ChatApi, Event, McpState, PermissionRequest, RequestId, SessionId, SessionQuery, ToolClass,
    ToolDisplay,
};
use crate::fixtures::{FixtureApi, HOUR, MINUTE, World};
use crate::transcript::{BlockKind, Splice, ToolBlock};
use crate::{Hub, LoadState, SessionEvent};

fn tool<'a>(blocks: &'a [Arc<crate::Block>], id: &str) -> Option<&'a ToolBlock> {
    blocks.iter().find_map(|block| match &block.kind {
        BlockKind::Tool(tool) if tool.id == id => Some(tool),
        _ => None,
    })
}

fn line(target: &str, result: &str) -> ToolDisplay {
    ToolDisplay::Line { verb: "read".into(), target: target.into(), result: result.into() }
}

#[gpui::test]
fn paths_display_against_the_hubs_home(cx: &mut TestAppContext) {
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    cx.update(|cx| {
        assert_eq!(crate::home(cx), PathBuf::from("/home/me"));
        assert_eq!(hub.read(cx).home(), Path::new("/home/me"));
        assert_eq!(crate::display_path(Path::new("/home/me/code/acme/payments-api"), cx), "~/code/acme/payments-api");
        assert_eq!(crate::display_path(Path::new("/home/me"), cx), "~");
        assert_eq!(crate::display_path(Path::new("/srv/repo"), cx), "/srv/repo");
        assert_eq!(hub.read(cx).paths().worktrees_dir(), PathBuf::from("/home/me/.harness/worktrees"));
    });

    // The binary hands over the engine's paths.
    let notes = super::notifications(cx, &hub);
    hub.update(cx, |hub, cx| hub.set_paths(base::paths::Paths::under(Path::new("/tmp/t")), cx));
    assert_eq!(notes.get(), 1);
    cx.update(|cx| assert_eq!(crate::display_path(Path::new("/tmp/t/home/x"), cx), "~/x"));
    let other = cx.update(|cx| Hub::new_with_paths(api.clone(), base::paths::Paths::under(Path::new("/r")), cx));
    cx.update(|cx| assert_eq!(other.read(cx).home(), Path::new("/r/home")));
}

#[gpui::test]
fn mcp_and_skill_commands_refresh_the_open_detail(cx: &mut TestAppContext) {
    let (_api, _hub) = setup(cx);
    let id = SessionId::new("chat-1423");
    let session = cx.update(|cx| crate::session(&id, cx));
    cx.run_until_parked();
    let sentry = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let detail = session.read(cx).detail().cloned();
            detail.and_then(|d| d.mcp.iter().find(|m| m.name == "sentry").map(|m| m.state))
        })
    };
    assert_eq!(sentry(cx), Some(McpState::Failed));
    let commands = cx.update(|cx| crate::commands(cx));
    cx.update(|cx| commands.reconnect_mcp(&id, "sentry", cx));
    cx.run_until_parked();
    assert_eq!(sentry(cx), Some(McpState::Ready));

    cx.update(|cx| commands.set_forced_skills(&id, vec!["write-failing-test".into()], cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let detail = session.read(cx).detail().cloned().expect("loaded");
        let forced: Vec<&str> = detail.skills.iter().filter(|s| s.forced).map(|s| s.name.as_str()).collect();
        assert_eq!(forced, ["write-failing-test"]);
        assert_eq!(detail.session.forced_skills, ["write-failing-test"]);
    });
}

#[gpui::test]
fn search_honours_failure_injection(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    api.fail_next("search_sessions");
    let search = cx.update(crate::search);
    cx.executor().advance_clock(crate::models::DEBOUNCE * 2);
    cx.run_until_parked();
    cx.update(|cx| assert!(matches!(search.read(cx).state(), LoadState::Failed(_))));
    search.update(cx, |model, cx| model.set_text("ledger", cx));
    cx.executor().advance_clock(crate::models::DEBOUNCE * 2);
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(search.read(cx).state(), &LoadState::Ready));
    assert!(api.recorded().is_empty(), "queries are not recorded");
    api.fail_next("search_sessions");
    assert!(api.search_sessions(&SessionQuery::default()).is_err());
    assert!(api.search_sessions(&SessionQuery::default()).is_ok());
}

#[gpui::test]
fn waiting_for_a_person_is_busy_but_not_running(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let waiting = cx.update(|cx| crate::session(&SessionId::new("chat-1423"), cx));
    let s = "chat-split-checkout";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    cx.run_until_parked();
    let state = |cx: &mut TestAppContext, model: &gpui::Entity<crate::SessionModel>| {
        cx.update(|cx| {
            let model = model.read(cx);
            (model.is_busy(), model.is_running(), model.is_waiting())
        })
    };
    assert_eq!(state(cx, &waiting), (true, false, true), "the fixture's chat waits on bun test");
    assert_eq!(state(cx, &session), (false, false, false));

    api.emit(agent(s, AgentEvent::TurnStarted));
    cx.run_until_parked();
    assert_eq!(state(cx, &session), (true, true, false));
    let request = PermissionRequest {
        id: RequestId::new("perm-ls"),
        session: SessionId::new(s),
        agent: "Build".into(),
        tool: "bash".into(),
        call_id: None,
        class: ToolClass::Execute,
        display: ToolDisplay::Command { command: "ls".into(), output: String::new(), exit_code: None },
        policy: "bash = ask".into(),
        always_rule: None,
        at: 0,
    };
    api.emit(Event::PermissionRequested { request });
    cx.run_until_parked();
    assert_eq!(state(cx, &session), (true, false, true));
    api.emit(Event::PermissionResolved { id: RequestId::new("perm-ls") });
    cx.run_until_parked();
    assert_eq!(state(cx, &session), (true, true, false));
}

#[gpui::test]
fn tool_rows_carry_their_times(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let history = cx.update(|cx| crate::session(&SessionId::new("chat-1423"), cx));
    let s = "chat-split-checkout";
    let live = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let blocks = history.read(cx).blocks();
        let read = tool(blocks, "t1").expect("t1");
        assert_eq!(read.started, Some(NOW - HOUR), "the call's stored message");
        assert_eq!(read.finished, Some(NOW - HOUR + MINUTE), "the result's stored message");
        assert_eq!(read.duration_ms(), Some(MINUTE));
        let pending = tool(blocks, "t8").expect("t8");
        assert!(pending.started.is_some() && pending.finished.is_none() && pending.duration_ms().is_none());
    });

    api.emit_all([
        agent(s, AgentEvent::TurnStarted),
        agent(s, AgentEvent::ToolCallStarted { id: "x1".into(), name: "read_file".into() }),
    ]);
    cx.run_until_parked();
    let started = cx.update(|cx| tool(live.read(cx).blocks(), "x1").and_then(|t| t.started));
    assert!(started.is_some());
    api.emit(agent(s, AgentEvent::ToolFinished { id: "x1".into(), display: line("a.ts", "3 lines"), is_error: false }));
    cx.run_until_parked();
    cx.update(|cx| {
        let finished = tool(live.read(cx).blocks(), "x1").expect("x1");
        assert_eq!(finished.started, started, "the start is kept");
        assert!(finished.duration_ms().is_some_and(|ms| ms >= 0));
    });
}

#[gpui::test]
fn sub_agent_rows_know_the_spawn_depth_limit(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let fix = cx.update(|cx| crate::session(&SessionId::new("chat-1423"), cx));
    let lead = cx.update(|cx| crate::session(&SessionId::new("chat-retry-budget"), cx));
    let codex = cx.update(|cx| crate::session(&SessionId::new("chat-sdk-v9"), cx));
    cx.run_until_parked();
    let depths = |cx: &mut TestAppContext, model: &gpui::Entity<crate::SessionModel>| -> Vec<(u32, Option<u32>)> {
        cx.update(|cx| {
            model
                .read(cx)
                .blocks()
                .iter()
                .filter_map(|block| match &block.kind {
                    BlockKind::Subagent(sub) => Some((sub.depth, sub.max_depth)),
                    _ => None,
                })
                .collect()
        })
    };
    assert_eq!(depths(cx, &fix), [(1, Some(2)), (1, Some(2))], "history rows of Build, which may spawn 2 deep");

    for session in ["chat-retry-budget", "chat-sdk-v9"] {
        api.emit(agent(
            session,
            AgentEvent::SubagentStarted { session: SessionId::new("kid"), name: "@x".into(), task: "t".into(), depth: 1 },
        ));
    }
    cx.run_until_parked();
    assert_eq!(depths(cx, &lead), [(1, Some(2))], "Payments lead's policy");
    assert_eq!(depths(cx, &codex), [(1, None)], "Codex may not spawn: no limit to show");
}

#[gpui::test]
fn splices_cover_only_rows_that_changed(cx: &mut TestAppContext) {
    let (api, _hub) = setup(cx);
    let s = "chat-split-checkout";
    let session = cx.update(|cx| crate::session(&SessionId::new(s), cx));
    cx.run_until_parked();
    api.emit_all([
        agent(s, AgentEvent::TurnStarted),
        agent(s, AgentEvent::ToolCallReady { id: "a".into(), name: "read_file".into(), class: ToolClass::Read, display: line("a.ts", "") }),
        agent(s, AgentEvent::ToolCallReady { id: "b".into(), name: "read_file".into(), class: ToolClass::Read, display: line("b.ts", "") }),
        agent(s, AgentEvent::TextDelta { text: "Reading".into() }),
    ]);
    cx.run_until_parked();
    let events = collect::<_, SessionEvent>(cx, &session);

    // Row 2 finishes and row 4 streams on; row 3 between them is not touched.
    api.emit_all([
        agent(s, AgentEvent::ToolFinished { id: "a".into(), display: line("a.ts", "3 lines"), is_error: false }),
        agent(s, AgentEvent::TextDelta { text: " both files.".into() }),
    ]);
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [SessionEvent::Spliced(Splice { old: 2..3, count: 1 }), SessionEvent::Spliced(Splice { old: 4..5, count: 1 })],
        "no wide splice, and no scroll to the bottom for edits"
    );
    events.borrow_mut().clear();

    // An event that changes nothing reports nothing.
    api.emit(agent(s, AgentEvent::ToolFinished { id: "a".into(), display: line("a.ts", "3 lines"), is_error: false }));
    cx.run_until_parked();
    assert!(events.borrow().is_empty());
}
