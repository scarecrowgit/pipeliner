//! The permission gate: every way a call can be allowed, asked about or refused, and which
//! tools a mode offers at all.

use llm::Reply;
use model::{ChatMode, PermissionDecision, PermissionRule, RuleDecision, RuleScope, ToolDisplay};
use serde_json::json;

use super::*;
use crate::{RuleStore, StopCause};

fn touch(id: &str, file: &str) -> Reply {
    Reply::tool(id, "bash", json!({"command": format!("touch {file}")}))
}

#[test]
fn allow_once_runs_the_call_and_asks_again_next_time() {
    let fx = Fixture::new();
    fx.llm.push(touch("t1", "a.txt"));
    fx.llm.push(touch("t2", "b.txt"));
    fx.llm.push(Reply::text("Done."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let (outcome, _) = fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Touch two files"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert!(fx.work().join("a.txt").exists() && fx.work().join("b.txt").exists());
    assert_eq!(asker.asked(), 2);
    assert!(rules.rules().is_empty());

    let requests = lock(&asker.requests).clone();
    let first = &requests[0];
    assert_eq!((first.tool.as_str(), first.class, first.agent.as_str()), ("bash", ToolClass::Execute, "Build"));
    assert_eq!(&first.session, fx.session());
    assert_eq!(first.policy, "bash = ask");
    assert_eq!(first.at, NOW);
    assert!(matches!(&first.display, ToolDisplay::Command { command, .. } if command == "touch a.txt"), "{:?}", first.display);
    let rule = first.always_rule.clone().expect("a rule to offer");
    assert_eq!((rule.tool.as_str(), rule.decision), ("bash", RuleDecision::Allow));
    assert_eq!(rule.scope, RuleScope::Session { session: fx.session().clone() });
    assert_ne!(requests[0].id, requests[1].id);
}

#[test]
fn always_allow_adds_the_rule_and_stops_asking() {
    let mut fx = Fixture::new();
    fx.env.repo = Some(fx.work());
    // A path-like argument is not part of the rule, so the rule covers both commands.
    fx.llm.push(touch("t1", "./a.txt"));
    fx.llm.push(touch("t2", "./b.txt"));
    fx.llm.push(Reply::text("Done."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let gate = asker.gate(|request| request.always_rule.clone().map(|rule| PermissionDecision::AllowAlways { rule }));
    let (outcome, _) = fx.run_gated(&gate, say("Touch two files"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert!(fx.work().join("a.txt").exists() && fx.work().join("b.txt").exists());
    assert_eq!(asker.asked(), 1, "the second call matched the new rule");
    let stored = rules.rules();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].scope, RuleScope::Repo { path: fx.work() }, "with a repo the rule is the repo's");
    assert_eq!(stored[0].pattern, "touch");
}

#[test]
fn a_rejection_reaches_the_model_with_the_persons_note() {
    let fx = Fixture::new();
    fx.llm.push(touch("t1", "a.txt"));
    fx.llm.push(Reply::text("Understood."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let gate = asker.gate(|_| Some(PermissionDecision::Reject { note: " use mkdir instead ".into() }));
    let (outcome, log) = fx.run_gated(&gate, say("Touch a file"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert!(!fx.work().join("a.txt").exists(), "a rejected command ran");
    let transcript = fx.transcript();
    assert_eq!(
        transcript[2],
        user(vec![error(
            "t1",
            "The person rejected this action. Their note: \"use mkdir instead\". Do not repeat the same call; adapt."
        )])
    );
    assert_paired(&transcript);
    assert_eq!(fx.requests()[1].messages, transcript[..3]);
    assert!(log.events.iter().any(|(_, e)| matches!(e, AgentEvent::ToolFinished { id, is_error: true, .. } if id == "t1")));

    // Without a note the sentence stands alone.
    fx.llm.push(touch("t2", "a.txt"));
    fx.llm.push(Reply::text("Fine."));
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::Reject { note: String::new() })), say("Try again"));
    let (content, _) = result_of(&fx.transcript(), "t2");
    assert_eq!(content, "The person rejected this action. Do not repeat the same call; adapt.");
}

#[test]
fn a_deny_rule_refuses_without_asking_anyone() {
    let fx = Fixture::new();
    fx.llm.push(touch("t1", "a.txt"));
    fx.llm.push(Reply::text("I cannot."));
    let rules = MemoryRules::new(vec![PermissionRule {
        tool: "bash".into(),
        pattern: String::new(),
        decision: RuleDecision::Deny,
        scope: RuleScope::Global,
    }]);
    let asker = Asker::new(&rules);
    let (outcome, _) = fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Touch a file"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(asker.asked(), 0);
    assert!(!fx.work().join("a.txt").exists());
    let (content, is_error) = result_of(&fx.transcript(), "t1");
    assert!(is_error);
    assert_eq!(
        content,
        "`bash` was not run: a permission rule forbids it (bash = deny). Do not retry it; find another way or tell the person."
    );
}

#[test]
fn reading_and_session_tools_never_ask_but_writing_does() {
    let fx = Fixture::new();
    fx.write("a.txt", "one\n");
    fx.llm.push(
        Reply::tool("t1", "read_file", json!({"path": "a.txt"}))
            .and_tool("t2", "grep", json!({"pattern": "one"}))
            .and_tool("t3", "todo_write", json!({"items": []}))
            .and_tool("t4", "write_file", json!({"path": "b.txt", "content": "two\n"})),
    );
    fx.llm.push(Reply::text("Done."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Go"));

    let requests = lock(&asker.requests).clone();
    assert_eq!(requests.len(), 1);
    assert_eq!((requests[0].tool.as_str(), requests[0].policy.as_str()), ("write_file", "write_file = ask"));
    assert!(matches!(&requests[0].display, ToolDisplay::Diff(diff) if diff.path == "b.txt" && diff.created));
    assert_eq!(fx.read("b.txt"), "two\n");
    assert_paired(&fx.transcript());
}

#[test]
fn a_deny_rule_on_a_path_stops_a_read() {
    let fx = Fixture::new();
    fx.write("secrets/key.pem", "private\n");
    fx.llm.push(Reply::tool("t1", "read_file", json!({"path": "secrets/key.pem"})));
    fx.llm.push(Reply::text("I may not read that."));
    let rules = MemoryRules::new(vec![PermissionRule {
        tool: "read_file".into(),
        pattern: "secrets".into(),
        decision: RuleDecision::Deny,
        scope: RuleScope::Global,
    }]);
    let asker = Asker::new(&rules);
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Read the key"));
    let (content, is_error) = result_of(&fx.transcript(), "t1");
    assert!(is_error && content.contains("read_file = deny") && !content.contains("private"), "{content}");
}

#[test]
fn plan_mode_offers_no_tool_that_writes_or_runs() {
    let mut fx = Fixture::new();
    fx.env.mode = ChatMode::Plan;
    fx.write("a.txt", "one\n");
    fx.llm.push(Reply::tool("t1", "edit_file", json!({"path": "a.txt", "old_string": "one", "new_string": "two"})));
    fx.llm.push(Reply::text("1. Change a.txt."));
    let (outcome, _) = fx.run(say("Plan the change"));

    assert_eq!(outcome.stop, StopCause::Finished);
    let requests = fx.requests();
    assert_eq!(tool_names(&requests[0]), ["ask_user", "grep", "list_dir", "read_file", "report_to_inbox", "todo_write"]);
    assert!(requests[0].system.contains("# Plan mode\n\nYou are in Plan mode"));
    // A model that calls one anyway changes nothing, even with every permission granted.
    assert_eq!(fx.read("a.txt"), "one\n");
    let (content, is_error) = result_of(&fx.transcript(), "t1");
    assert!(is_error);
    assert_eq!(content, "`edit_file` is not available: this session changes nothing in its folder. Describe the change instead.");
}

#[test]
fn read_only_sessions_and_grants_trim_the_tools() {
    let mut fx = Fixture::new();
    fx.env.read_only = true;
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    let request = fx.requests().remove(0);
    assert_eq!(tool_names(&request), ["ask_user", "grep", "list_dir", "read_file", "report_to_inbox", "todo_write"]);
    assert!(request.system.contains("# Read-only\n"));

    let mut fx = Fixture::new();
    fx.spec.tools = ToolGrants { access: ToolAccess::ReadAndRun, allow: Vec::new(), deny: vec!["grep".into(), "report_to_inbox".into()] };
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    assert_eq!(tool_names(&fx.requests()[0]), ["ask_user", "bash", "list_dir", "read_file", "todo_write"]);

    // Without a folder there are no file tools at all.
    let mut fx = Fixture::new();
    fx.env.workdir = None;
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    let request = fx.requests().remove(0);
    assert_eq!(tool_names(&request), ["ask_user", "report_to_inbox", "todo_write"]);
    assert!(request.system.contains("- No working folder"));
}

#[test]
fn mcp_tools_need_a_grant_and_ask_unless_their_server_approves_them() {
    // No grant: the tools are not offered.
    let mut fx = Fixture::new();
    fx.mcp = Some(FakeMcp::with_tools(2));
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    assert!(!tool_names(&fx.requests()[0]).iter().any(|name| name.starts_with("mcp__")));

    let mut fx = Fixture::new();
    let mut servers = FakeMcp::with_tools(2);
    servers.auto = vec!["mcp__grafana__tool_1".into()];
    fx.mcp = Some(servers);
    fx.spec.tools.allow = vec!["mcp__grafana__*".into()];
    fx.llm.push(
        Reply::tool("t1", "mcp__grafana__tool_0", json!({"q": "up"})).and_tool("t2", "mcp__grafana__tool_1", json!({})),
    );
    fx.llm.push(Reply::text("Done."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Query"));

    let request = fx.requests().remove(0);
    assert_eq!(tool_names(&request)[9..], ["mcp__grafana__tool_0", "mcp__grafana__tool_1"]);
    assert!(request.system.contains("Tools named `mcp__<server>__<tool>`"));
    let asked = lock(&asker.requests).clone();
    assert_eq!(asked.len(), 1, "only the tool the config does not approve asks");
    assert_eq!((asked[0].tool.as_str(), asked[0].class), ("mcp__grafana__tool_0", ToolClass::External));
    assert_eq!(asked[0].display, ToolDisplay::Text { title: "grafana · tool_0".into(), body: "{\n  \"q\": \"up\"\n}".into() });
    assert_eq!(asked[0].always_rule.as_ref().map(|r| r.pattern.as_str()), Some(""));
    let calls = lock(&fx.mcp.as_ref().expect("mcp").calls).clone();
    assert_eq!(calls, [("mcp__grafana__tool_0".to_string(), json!({"q": "up"})), ("mcp__grafana__tool_1".to_string(), json!({}))]);
    assert_eq!(result_of(&fx.transcript(), "t1"), ("mcp__grafana__tool_0 ok".to_string(), false));

    // A person's rule beats the server's approval.
    let rules = MemoryRules::new(vec![PermissionRule {
        tool: "mcp__grafana__tool_1".into(),
        pattern: String::new(),
        decision: RuleDecision::Deny,
        scope: RuleScope::Global,
    }]);
    let asker = Asker::new(&rules);
    fx.llm.push(Reply::tool("t3", "mcp__grafana__tool_1", json!({})));
    fx.llm.push(Reply::text("Refused."));
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Again"));
    assert!(result_of(&fx.transcript(), "t3").1);
    assert_eq!(lock(&fx.mcp.as_ref().expect("mcp").calls).len(), 2);
}
