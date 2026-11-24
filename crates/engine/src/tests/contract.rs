//! The contract additions the screens asked for: deleting a comment, running an autopilot now,
//! kept tool displays, queues across a restart, idle MCP servers, folder listings.

use llm::Reply;
use model::{
    Actor, Autopilot, AutopilotId, AutopilotTrigger, DiffLineKind, Event, IssueEvent, IssueEventKind, McpState, PermissionDecision,
    Project, ProjectId, RunnableRef, Schedule, ToolDisplay,
};
use serde_json::json;

use super::{Fx, new_session};
use crate::api::{ChatApi, IssueApi, NewIssue, OpsApi, UserMessage, WorkdirApi};

fn project(fx: &Fx) -> ProjectId {
    let project = Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() };
    fx.engine.save_project(project).expect("project");
    ProjectId::new("har")
}

#[test]
fn only_ones_own_comments_can_be_deleted() {
    let fx = Fx::new();
    let new = NewIssue { project: project(&fx), title: "Fix the retry".into(), ..Default::default() };
    let issue = fx.engine.create_issue(new).expect("create");
    fx.engine.comment(&issue.key, "first", Vec::new()).expect("comment");
    fx.engine.comment(&issue.key, "second", Vec::new()).expect("comment");
    let agent = Actor::Runnable { reference: RunnableRef::agent("fixer"), name: "Fixer".into(), spawned: false };
    let kind = IssueEventKind::Comment { text: "from an agent".into(), images: Vec::new(), mentions: Vec::new() };
    let theirs = fx.engine.inner.store.append_issue_event(&issue.key, IssueEvent { seq: 0, at: 1, actor: agent, kind }).expect("append");
    let seq_of = |text: &str| {
        fx.engine.issue_events(&issue.key).expect("events").into_iter().find_map(|e| match &e.kind {
            IssueEventKind::Comment { text: t, .. } if t == text => Some(e.seq),
            _ => None,
        })
    };
    let second = seq_of("second").expect("second");
    fx.drain();

    fx.engine.delete_comment(&issue.key, second).expect("delete");
    assert_eq!(seq_of("second"), None);
    assert!(seq_of("first").is_some());
    fx.wait_for("the issue as changed", |e| matches!(e, Event::IssueChanged { issue: changed } if changed.key == issue.key));

    let created = fx.engine.issue_events(&issue.key).expect("events")[0].seq;
    assert!(fx.engine.delete_comment(&issue.key, created).is_err(), "not a comment");
    assert!(fx.engine.delete_comment(&issue.key, theirs.seq).is_err(), "an agent's comment");
    assert!(fx.engine.delete_comment(&issue.key, second).is_err(), "already deleted");
    fx.engine.comment(&issue.key, "third", Vec::new()).expect("comment");
    assert!(seq_of("third").expect("third") > theirs.seq, "a deleted seq is not handed out again");
}

fn autopilot(project: &ProjectId, target: RunnableRef) -> Autopilot {
    Autopilot {
        id: AutopilotId::new("ap-1"),
        project: project.clone(),
        name: "Nightly triage".into(),
        trigger: AutopilotTrigger::Schedule { schedule: Schedule::Day { hour: 3, minute: 0 } },
        target,
        prompt: "Look at yesterday's errors and file issues.".into(),
        enabled: true,
        paused_reason: None,
        next_run: None,
        last_run: None,
        last_result: String::new(),
        budget_usd: None,
    }
}

#[test]
fn running_an_autopilot_starts_its_agent_and_records_the_tick() {
    let fx = Fx::new();
    let project = project(&fx);
    fx.engine.save_autopilot(autopilot(&project, catalog::Catalog::default_chat_agent())).expect("save");
    fx.llm.push(Reply::text("Filed nothing: no errors."));
    fx.engine.run_autopilot(&AutopilotId::new("ap-1")).expect("run");

    let ran = fx.engine.autopilots(&project).expect("list").remove(0);
    assert_eq!((ran.last_run, ran.last_result.as_str()), (Some(super::NOW), "started a chat"));
    assert!(ran.next_run.is_some_and(|at| at > super::NOW), "the schedule moves on");
    let chats = fx.engine.sessions(&Default::default()).expect("sessions");
    assert_eq!(chats.len(), 1);
    let chat = fx.settle(&chats[0].id);
    let first = fx.engine.messages(&chat.id, 1, 2).expect("messages").remove(0);
    assert_eq!(first.message.plain_text(), "Look at yesterday's errors and file issues.");

    fx.engine.save_autopilot(autopilot(&project, RunnableRef::agent("nobody"))).expect("save");
    assert!(fx.engine.run_autopilot(&AutopilotId::new("ap-1")).is_err());
    let failed = fx.engine.autopilots(&project).expect("list").remove(0);
    assert!(failed.last_result.starts_with("failed: "), "{}", failed.last_result);
    assert!(fx.engine.run_autopilot(&AutopilotId::new("nope")).is_err());
}

#[test]
fn a_finished_edit_keeps_its_display_with_line_numbers_for_history() {
    let fx = Fx::new();
    let body: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    let repo = fx.repo("pay", &[("src/a.txt", &body)]);
    let session = fx.chat_in(&repo);
    let edit = json!({"path": "src/a.txt", "old_string": "line 6\n", "new_string": "line six\n"});
    fx.llm.push(Reply::tool("t1", "edit_file", edit));
    fx.llm.push(Reply::text("Changed it."));
    fx.say(&session.id, "Spell six out");
    let Event::PermissionRequested { request } =
        fx.wait_for("a permission request", |e| matches!(e, Event::PermissionRequested { .. }))
    else {
        unreachable!()
    };
    fx.engine.answer_permission(&request.id, PermissionDecision::AllowOnce).expect("answer");
    fx.settle(&session.id);

    let kept = fx.engine.tool_displays(&session.id, &["t1".to_string(), "t9".to_string()]).expect("displays");
    assert_eq!(kept.len(), 1, "only calls that ran have one");
    let ToolDisplay::Diff(diff) = &kept["t1"] else { panic!("{:?}", kept["t1"]) };
    assert_eq!((diff.added, diff.removed), (1, 1));
    let lines: Vec<_> = diff.hunks.iter().flat_map(|h| &h.lines).collect();
    assert!(lines.iter().any(|l| l.kind == DiffLineKind::Context), "unchanged lines around the change");
    assert!(lines.iter().any(|l| l.kind == DiffLineKind::Del && l.old_no == Some(6)), "{lines:?}");

    let fork = fx.engine.fork_session(&session.id, None).expect("fork");
    assert_eq!(fx.engine.tool_displays(&fork.id, &["t1".to_string()]).expect("displays").len(), 1);
}

#[test]
fn queued_messages_survive_a_restart() {
    let fx = Fx::new();
    let session = fx.engine.new_session(new_session(None)).expect("session");
    let message = |text: &str| UserMessage { text: text.into(), images: Vec::new() };
    // A turn holds the session; the next two wait behind it.
    let live = &fx.engine.inner.live;
    assert!(matches!(live.send(&session.id, message("first")), crate::live::Sent::Start { .. }));
    live.send(&session.id, message("second"));
    live.send(&session.id, message("third"));
    assert_eq!(live.next(&session.id).map(|m| m.text).as_deref(), Some("second"));

    let (engine, _llm) = fx.reopen();
    let detail = engine.session_detail(&session.id).expect("detail").expect("session");
    assert_eq!(detail.queued, vec![message("third")]);
    let taken = engine.inner.live.take_queued(&session.id);
    assert_eq!(taken, vec![message("third")]);
    let (again, _llm) = fx.reopen();
    assert!(again.session_detail(&session.id).expect("detail").expect("session").queued.is_empty(), "taken is gone");
}

#[test]
fn mcp_servers_read_idle_until_a_turn_needs_them() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.paths.config_dir.clone()).expect("config dir");
    std::fs::write(&fx.paths.config_file, "[[mcp_servers]]\nname = \"lazy\"\ncommand = \"/nonexistent/mcp\"\n").expect("config");
    let session = fx.engine.new_session(new_session(None)).expect("session");
    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    let lazy = detail.mcp.iter().find(|s| s.name == "lazy").expect("listed");
    assert_eq!(lazy.state, McpState::Idle, "looking at the panel starts nothing");

    fx.llm.push(Reply::text("Hi."));
    fx.say(&session.id, "Hello");
    fx.settle(&session.id);
    let detail = fx.engine.session_detail(&session.id).expect("detail").expect("session");
    let lazy = detail.mcp.iter().find(|s| s.name == "lazy").expect("listed");
    assert!(matches!(lazy.state, McpState::Starting | McpState::Failed), "{:?}", lazy.state);
}

#[test]
fn folders_are_listed_for_completion_and_missing_ones_told_apart() {
    let fx = Fx::new();
    let root = fx.root().join("code");
    for dir in ["pay", "Platform", ".cache"] {
        std::fs::create_dir_all(root.join(dir)).expect("dir");
    }
    fx.repo("code/payroll", &[("a", "b")]);
    let found = fx.engine.list_dir(&root, "p", 10).expect("list");
    let names: Vec<(&str, bool)> = found.iter().map(|c| (c.name.as_str(), c.is_repo)).collect();
    assert_eq!(names, [("pay", false), ("payroll", true), ("Platform", false)]);
    assert_eq!(fx.engine.list_dir(&root, "", 10).expect("list").last().map(|c| c.name.as_str()), Some(".cache"));
    assert!(fx.engine.list_dir(&root.join("missing"), "", 10).is_err());
    assert!(fx.engine.folder_exists(&root.join("pay")));
    assert!(!fx.engine.folder_exists(&root.join("missing")));
    assert!(fx.engine.folder_info(&root.join("pay")).facts.is_none(), "empty and missing read alike there");
}
