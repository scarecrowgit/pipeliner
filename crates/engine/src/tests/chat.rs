//! Chat turns end to end: tools, events, queued messages, sub-agents, the ledger, restarts
//! and exports.

use std::time::Duration;

use catalog::Place;
use llm::Reply;
use model::{
    AgentDef, AgentEvent, ContentBlock, Event, ModelRef, Origin, PermissionDecision, Role, Runnable, RunnableKind,
    RunnableRef, SessionState, SpawnPolicy, ToolAccess, ToolGrants, Usage, Uses,
};
use serde_json::json;

use super::{Fx, new_session};
use crate::api::{CatalogApi, ChatApi, ExportFormat, ExportOptions, OpsApi, UserMessage};

fn kind(event: &AgentEvent) -> String {
    serde_json::to_value(event).expect("event json")["type"].as_str().unwrap_or_default().to_string()
}

/// Whether `wanted` appears in `seen` in this order, other items between allowed.
fn in_order(seen: &[String], wanted: &[&str]) -> bool {
    let mut at = 0;
    for item in seen {
        if at < wanted.len() && item == wanted[at] {
            at += 1;
        }
    }
    at == wanted.len()
}

fn usage() -> Usage {
    Usage { input: 1_000, output: 200, ..Default::default() }
}

#[test]
fn a_turn_edits_a_file_through_the_tools_and_reports_every_step() {
    let fx = Fx::new();
    let repo = fx.repo("pay", &[("src/a.txt", "hello\n")]);
    let session = fx.chat_in(&repo);
    let edit = json!({"path": "src/a.txt", "old_string": "hello", "new_string": "goodbye"});
    fx.llm.push(Reply::tool("t1", "edit_file", edit).with_usage(usage()));
    fx.llm.push(Reply::text("Changed it.").with_usage(usage()));
    fx.say(&session.id, "Say goodbye");

    let Event::PermissionRequested { request } =
        fx.wait_for("a permission request", |e| matches!(e, Event::PermissionRequested { .. }))
    else {
        unreachable!()
    };
    assert_eq!((request.tool.as_str(), &request.session), ("edit_file", &session.id));
    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    assert_eq!(detail.pending, vec![request.clone()]);
    assert_eq!(detail.session.state, SessionState::Waiting);
    fx.engine.answer_permission(&request.id, PermissionDecision::AllowOnce).expect("answer");

    let done = fx.settle(&session.id);
    assert_eq!(std::fs::read_to_string(repo.join("src/a.txt")).expect("read"), "goodbye\n");
    assert_eq!(done.state, SessionState::Idle);
    assert_eq!(done.title, "Say goodbye");
    assert_eq!(done.message_count, 4);
    assert!(done.context_tokens > 0 && done.context_window > 0);

    let kinds: Vec<String> = fx.agent_events(&session.id).iter().map(kind).collect();
    assert!(
        in_order(
            &kinds,
            &[
                "message_committed",
                "turn_started",
                "tool_call_started",
                "tool_call_ready",
                "message_committed",
                "files_changed",
                "tool_finished",
                "message_committed",
                "turn_started",
                "text_delta",
                "message_committed",
                "turn_finished"
            ]
        ),
        "{kinds:?}"
    );
    let log = fx.log.lock().expect("log");
    let position = |pred: &dyn Fn(&Event) -> bool| log.iter().position(pred).expect("event");
    let asked = position(&|e| matches!(e, Event::PermissionRequested { .. }));
    let resolved = position(&|e| matches!(e, Event::PermissionResolved { id } if *id == request.id));
    let finished =
        position(&|e| matches!(e, Event::Agent { event: AgentEvent::ToolFinished { is_error: false, .. }, .. }));
    assert!(asked < resolved && resolved < finished);
    assert!(log.iter().any(|e| matches!(e, Event::SpendChanged)));
    drop(log);
    assert_eq!(
        fx.states(&session.id),
        [SessionState::Idle, SessionState::Running, SessionState::Waiting, SessionState::Running, SessionState::Idle]
    );

    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    assert_eq!(detail.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["src/a.txt"]);
    assert!(detail.pending.is_empty() && detail.queued.is_empty());
    assert_eq!(detail.repo.map(|r| r.name), Some("pay".to_string()));
    let page = fx.engine.messages(&session.id, 1, u64::MAX).expect("messages");
    assert_eq!(page.len(), 4);
    assert_eq!(page[3].message.plain_text(), "Changed it.");
}

#[test]
fn a_message_sent_while_the_agent_works_is_delivered_in_the_same_turn() {
    let fx = Fx::new();
    let session = fx.chat_in(&fx.root());
    fx.llm.push(Reply::text("First answer.").with_delay(Duration::from_millis(400)));
    fx.llm.push(Reply::text("Second answer."));
    fx.say(&session.id, "one");
    fx.wait_for("the turn to start", |e| matches!(e, Event::Agent { event: AgentEvent::TurnStarted, .. }));
    fx.say(&session.id, "two");
    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    assert_eq!(detail.queued, vec![UserMessage { text: "two".into(), images: Vec::new() }]);

    let done = fx.settle(&session.id);
    assert_eq!(done.state, SessionState::Idle);
    let texts: Vec<String> =
        fx.engine.messages(&session.id, 1, u64::MAX).expect("messages").iter().map(|m| m.message.plain_text()).collect();
    assert_eq!(texts, ["one", "First answer.", "two", "Second answer."]);
    assert_eq!(fx.llm.call_count(), 2);
    assert!(fx.engine.session_detail(&session.id).expect("detail").expect("session").queued.is_empty());
}

fn lead() -> Runnable {
    Runnable {
        reference: RunnableRef::agent("lead"),
        kind: RunnableKind::Agent,
        name: "Lead".into(),
        description: "Hands work to sub-agents".into(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef {
            role: "You are Lead.".into(),
            tools: ToolGrants { access: ToolAccess::Full, ..Default::default() },
            spawn: SpawnPolicy { enabled: true, ..Default::default() },
            ..Default::default()
        }),
        path: None,
    }
}

#[test]
fn a_sub_agent_reports_under_its_own_session_and_its_spend_lands_in_the_ledger() {
    let fx = Fx::new();
    fx.engine.save_runnable(&Place::Library, lead()).expect("save lead");
    std::fs::create_dir_all(fx.root().join("src")).expect("mkdir");
    std::fs::write(fx.root().join("src/a.rs"), "").expect("write");
    let mut new = new_session(Some(crate::api::WorkdirChoice {
        path: fx.root(),
        mode: model::WorkMode::InPlace,
        base: None,
        branch: None,
    }));
    new.runnable = Some(RunnableRef::agent("lead"));
    new.model = Some(ModelRef::new("scripted", "claude-sonnet-5"));
    let session = fx.engine.new_session(new).expect("session");

    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Count the files in src", "label": "Counter"})).with_usage(usage()));
    fx.llm.push(Reply::tool("c1", "list_dir", json!({"path": "src"})).with_usage(usage()));
    fx.llm.push(Reply::text("There is 1 file.").with_usage(usage()));
    fx.llm.push(Reply::text("The sub-agent counted 1 file.").with_usage(usage()));
    fx.say(&session.id, "How many files are in src?");
    let done = fx.settle(&session.id);
    assert_eq!(done.state, SessionState::Idle);

    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    assert_eq!(detail.children.len(), 1);
    let child = &detail.children[0];
    assert_eq!((child.title.as_str(), child.depth, child.parent.as_ref()), ("Counter", 1, Some(&session.id)));
    assert_eq!(child.state, SessionState::Done);
    let child_kinds: Vec<String> = fx.agent_events(&child.id).iter().map(kind).collect();
    assert!(in_order(&child_kinds, &["turn_started", "tool_call_ready", "tool_finished", "turn_finished"]), "{child_kinds:?}");
    let parent_kinds: Vec<String> = fx.agent_events(&session.id).iter().map(kind).collect();
    assert!(in_order(&parent_kinds, &["subagent_started", "subagent_finished", "turn_finished"]), "{parent_kinds:?}");

    // Four calls at 1k in / 200 out on claude-sonnet-5.
    let child_spend = fx.engine.inner.store.spend_for_session(&child.id).expect("spend").cost_usd;
    assert!(child_spend > 0.0);
    let today = fx.engine.spend_today().expect("spend today");
    assert!((today.cost_usd - done.cost_usd).abs() < 1e-9, "{} vs {}", today.cost_usd, done.cost_usd);
    assert!(done.cost_usd > child_spend);
    assert_eq!(done.usage.input, 2_000, "the header's usage is the session's own calls");
}

#[test]
fn a_new_engine_lists_the_sessions_and_continues_an_interrupted_one() {
    let fx = Fx::new();
    let finished = fx.chat_in(&fx.root());
    fx.llm.push(Reply::text("Hello."));
    fx.say(&finished.id, "Hi");
    fx.settle(&finished.id);

    // A session that died mid-call: running, with a tool call that never got its result.
    let crashed = fx.chat_in(&fx.root());
    let store = &fx.engine.inner.store;
    let stored = |role, content| model::StoredMessage {
        seq: 0,
        at: super::NOW,
        message: model::Message { role, content },
        compacted: false,
        model: None,
        usage: Default::default(),
        cost_usd: 0.0,
    };
    store.append_message(&crashed.id, stored(Role::User, vec![ContentBlock::text("List the files")])).expect("append");
    let call = ContentBlock::ToolUse { id: "t9".into(), name: "list_dir".into(), input: json!({"path": "."}) };
    store.append_message(&crashed.id, stored(Role::Assistant, vec![call])).expect("append");
    let mut header = store.session(&crashed.id).expect("read").expect("session");
    header.state = SessionState::Running;
    store.upsert_session(&header).expect("upsert");

    let (engine, llm) = fx.reopen();
    let listed = engine.sessions(&Default::default()).expect("sessions");
    assert_eq!(listed.len(), 2);
    let crashed_now = listed.iter().find(|s| s.id == crashed.id).expect("crashed");
    assert_eq!(crashed_now.state, SessionState::Failed, "interrupted sessions can be retried");
    assert_eq!(listed.iter().find(|s| s.id == finished.id).expect("finished").state, SessionState::Idle);

    llm.push(Reply::text("Continuing."));
    engine.retry(&crashed.id).expect("retry");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while engine.inner.live.is_running(&crashed.id) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let messages = engine.messages(&crashed.id, 1, u64::MAX).expect("messages");
    let answered = messages.iter().any(|m| {
        m.message.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t9"))
    });
    assert!(answered, "the dangling call got a result: {messages:#?}");
    assert_eq!(messages.last().expect("last").message.plain_text(), "Continuing.");
    assert_eq!(engine.inner.session(&crashed.id).expect("session").state, SessionState::Idle);
    engine.shutdown();
}

#[test]
fn transcripts_export_as_markdown_and_json_lines_without_secrets() {
    let fx = Fx::new();
    let session = fx.chat_in(&fx.root());
    fx.llm.push(Reply::tool("t1", "list_dir", json!({"path": "."})));
    fx.llm.push(Reply::text("Stored it."));
    fx.say(&session.id, "Remember sk-ant-api03-abcdefghijklmnopqrstuvwxyz please");
    let done = fx.settle(&session.id);
    let out = fx.root().join("exports");
    let options = ExportOptions { subagents: true, tool_output: true, images: false, local_paths: false };

    let md = fx.engine.export_session(&session.id, ExportFormat::Markdown, options, &out).expect("markdown");
    assert_eq!(md.extension().and_then(|e| e.to_str()), Some("md"));
    let text = std::fs::read_to_string(&md).expect("read md");
    assert!(text.starts_with("# Remember [redacted] please"), "{text}");
    assert!(done.title.starts_with("Remember sk-ant"));
    assert!(text.contains("## Tool results · "));
    assert!(text.contains("## You · ") && text.contains("## Build · ") && text.contains("Stored it."));
    assert!(text.contains("**list_dir**") && text.contains("<details>"));
    assert!(text.contains("[redacted]") && !text.contains("abcdefghijklmnopqrstuvwxyz"), "{text}");

    let jsonl = fx.engine.export_session(&session.id, ExportFormat::Jsonl, options, &out.join("t.jsonl")).expect("jsonl");
    let text = std::fs::read_to_string(&jsonl).expect("read jsonl");
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).expect("json line")).collect();
    assert_eq!(lines.len(), done.message_count as usize);
    assert_eq!(lines[0]["role"], "user");
    assert!(lines[1]["content"].as_array().expect("content").iter().any(|b| b["type"] == "tool_use"));
    assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
}

#[test]
fn messages_the_loop_could_not_store_go_back_to_the_front_of_the_queue() {
    use agent::Host;
    let fx = Fx::new();
    let session = fx.chat_in(&fx.root());
    let live = &fx.engine.inner.live;
    let message = |text: &str| UserMessage { text: text.into(), images: Vec::new() };
    assert!(matches!(live.send(&session.id, message("a")), crate::live::Sent::Start { .. }));
    assert!(matches!(live.send(&session.id, message("b")), crate::live::Sent::Queued));
    let host = crate::host::StoreHost::new(&fx.engine.inner);
    let taken = host.take_queued(&session.id);
    assert_eq!(taken.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["b"]);
    live.send(&session.id, message("c"));
    host.requeue(&session.id, taken);
    assert_eq!(live.queued(&session.id), [message("b"), message("c")]);
    live.release(&session.id, false);
}
