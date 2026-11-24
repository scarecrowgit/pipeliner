//! The session tools: the task list, skills, deferred MCP tools, reports, questions and the
//! finish tool of a pipeline node.

use llm::Reply;
use model::{Link, Message, Outcome, TodoItem, TodoStatus};
use serde_json::json;

use super::*;
use crate::notes::note;
use crate::session_tools::ask::ASKED;
use crate::session_tools::finish::RECORDED;
use crate::{FinishTool, Host, Report, StopCause};

#[test]
fn todo_write_replaces_the_list_and_reports_it() {
    let fx = Fixture::new();
    let first = json!({"items": [{"text": "Read", "status": "in_progress"}, {"text": "Fix", "status": "pending"}]});
    let second = json!({"items": [{"text": "Read", "status": "done"}, {"text": "Fix", "status": "in_progress"}]});
    fx.llm.push(Reply::tool("t1", "todo_write", first.clone()));
    fx.llm.push(Reply::tool("t2", "todo_write", second).and_tool("t3", "todo_write", json!({"items": "all of them"})));
    fx.llm.push(Reply::text("Working."));
    let (_, log) = fx.run(say("Fix it"));

    let list = vec![
        TodoItem { text: "Read".into(), status: TodoStatus::Done },
        TodoItem { text: "Fix".into(), status: TodoStatus::InProgress },
    ];
    assert_eq!(fx.host.todos(fx.session()).expect("todos"), list, "the broken third call changed nothing");
    let updates: Vec<_> = log
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::TodoUpdated { items } => Some(items.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[1], list);
    let transcript = fx.transcript();
    assert_eq!(transcript[2], user(vec![result("t1", "Todo list updated (0 of 2 done).\n[~] Read\n[ ] Fix")]));
    assert_eq!(result_of(&transcript, "t2").0, "Todo list updated (1 of 2 done).\n[x] Read\n[~] Fix");
    assert_eq!(result_of(&transcript, "t3"), ("todo_write: `items` must be an array.".to_string(), true));
    // The list is not in the prompt: it lives in the conversation.
    assert!(!fx.requests()[2].system.contains("[~]"));
}

#[test]
fn forced_skills_are_in_the_prompt_and_the_others_load_on_demand() {
    let mut fx = Fixture::with_skills(&[
        ("deploy", "How to ship a release", "Run `make release`."),
        ("style", "The house style", "Use tabs, never spaces."),
        ("secret", "Not for this agent", "Hidden."),
    ]);
    fx.spec.skills.forced = vec!["style".into(), "missing".into()];
    fx.spec.skills.auto = vec!["deploy".into(), "style".into()];
    fx.llm.push(Reply::tool("t1", "skill", json!({"name": "deploy"})));
    fx.llm.push(
        Reply::tool("t2", "skill", json!({"name": "deploy"}))
            .and_tool("t3", "skill", json!({"name": "style"}))
            .and_tool("t4", "skill", json!({"name": "secret"})),
    );
    fx.llm.push(Reply::text("Shipped."));
    let (_, log) = fx.run(say("Ship it"));

    let requests = fx.requests();
    let system = &requests[0].system;
    assert!(system.contains("Available skills:\n- deploy: How to ship a release\n"), "{system}");
    assert!(!system.contains("- style:") && !system.contains("secret"), "forced and ungranted skills are not listed");
    assert!(system.contains("<skill name=\"style\">\nBase directory for this skill: "));
    assert!(system.contains("Use tabs, never spaces.\n</skill>"));
    assert!(system.contains("(Skill `missing` is not installed on this machine.)"));
    assert!(!system.contains("make release"), "an unloaded skill's text is not in the prompt");
    let skill_tool = requests[0].tools.iter().find(|t| t.name == "skill").expect("the skill tool");
    assert_eq!(skill_tool.input_schema["properties"]["name"]["enum"], json!(["deploy"]));

    let transcript = fx.transcript();
    let (loaded, is_error) = result_of(&transcript, "t1");
    assert!(!is_error && loaded.contains("Run `make release`.") && loaded.starts_with("Base directory for this skill: "));
    assert_eq!(result_of(&transcript, "t2").0, "Skill `deploy` is already loaded earlier in this conversation. Follow it.");
    assert_eq!(result_of(&transcript, "t3").0, "Skill `style` is always loaded: its instructions are in your system prompt.");
    assert_eq!(result_of(&transcript, "t4"), ("There is no skill `secret`. Available skills: deploy.".to_string(), true));
    let loads: Vec<_> = log.events.iter().filter(|(_, e)| matches!(e, AgentEvent::SkillLoaded { name } if name == "deploy")).collect();
    assert_eq!(loads.len(), 1);
    assert!(requests.iter().all(|r| &r.system == system));

    // A later run of the session knows the skill is still in the conversation.
    fx.llm.push(Reply::tool("t5", "skill", json!({"name": "deploy"})));
    fx.llm.push(Reply::text("Yes."));
    fx.run(say("Is deploy loaded?"));
    assert!(result_of(&fx.transcript(), "t5").0.contains("already loaded"));
}

#[test]
fn an_agent_without_skills_gets_no_skill_tool() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    let request = fx.requests().remove(0);
    assert!(!tool_names(&request).contains(&"skill"));
    assert!(!request.system.contains("# Skills"));
}

#[test]
fn many_mcp_tools_are_listed_by_name_and_loaded_with_tool_search() {
    let mut fx = Fixture::new();
    fx.mcp = Some(FakeMcp::with_tools(20));
    fx.spec.tools.allow = vec!["mcp__*".into()];
    fx.llm.push(Reply::tool("t1", "mcp__grafana__tool_3", json!({})));
    fx.llm.push(Reply::tool("t2", "tool_search", json!({"query": "select:mcp__grafana__tool_3,tool_7"})));
    fx.llm.push(Reply::tool("t3", "mcp__grafana__tool_3", json!({"q": 1})));
    fx.llm.push(Reply::text("Done."));
    fx.run(say("Use tool 3"));

    let requests = fx.requests();
    assert!(tool_names(&requests[0]).contains(&"tool_search"));
    assert!(!tool_names(&requests[0]).iter().any(|name| name.starts_with("mcp__")), "no schema is sent up front");
    let system = &requests[0].system;
    assert!(system.contains("# More MCP tools\n"));
    assert!(system.contains("- grafana: tool_0, tool_1, tool_2, tool_3,"), "{system}");

    let transcript = fx.transcript();
    assert_eq!(
        result_of(&transcript, "t1"),
        ("`mcp__grafana__tool_3` is not loaded yet. Call `tool_search` with `select:mcp__grafana__tool_3` first, then call it.".to_string(), true)
    );
    let (found, _) = result_of(&transcript, "t2");
    assert!(found.starts_with("Loaded 2 tools; you can call them from your next step:\n\n## mcp__grafana__tool_3\nDoes thing 3\n"), "{found}");
    // Loaded tools are appended, so everything before them stays cached; the prompt is unchanged.
    let before = tool_names(&requests[1]).len();
    assert_eq!(tool_names(&requests[2])[before..], ["mcp__grafana__tool_3", "mcp__grafana__tool_7"]);
    assert_eq!(tool_names(&requests[2])[..before], tool_names(&requests[1])[..]);
    assert!(requests.iter().all(|r| &r.system == system));
    assert_eq!(result_of(&transcript, "t3"), ("mcp__grafana__tool_3 ok".to_string(), false));
    assert_eq!(lock(&fx.mcp.as_ref().expect("mcp").calls).clone(), [("mcp__grafana__tool_3".to_string(), json!({"q": 1}))]);

    // The next run of the session still has what was loaded.
    fx.llm.push(Reply::text("Still there."));
    fx.run(say("And now?"));
    let last = fx.requests().pop().expect("request");
    assert_eq!(tool_names(&last)[before..], ["mcp__grafana__tool_3", "mcp__grafana__tool_7"]);
    assert_eq!(&last.system, system);
}

#[test]
fn a_few_mcp_tools_are_offered_whole() {
    let mut fx = Fixture::new();
    fx.mcp = Some(FakeMcp::with_tools(15));
    fx.spec.tools.allow = vec!["mcp__*".into()];
    fx.spec.tools.deny = vec!["mcp__grafana__tool_14".into()];
    fx.llm.push(Reply::text("ok"));
    fx.run(say("Hi"));
    let request = fx.requests().remove(0);
    let names = tool_names(&request);
    assert!(!names.contains(&"tool_search"));
    assert_eq!(names.iter().filter(|name| name.starts_with("mcp__")).count(), 14);
    assert!(!request.system.contains("# More MCP tools"));
}

#[test]
fn report_to_inbox_posts_through_the_host() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool(
        "t1",
        "report_to_inbox",
        json!({
            "title": "Retry count raised",
            "summary": "Changed `src/retry.rs`.",
            "outcome": "needs_review",
            "links": [{"url": "https://github.com/acme/pay/pull/7", "label": "PR #7"}]
        }),
    ));
    fx.llm.push(Reply::text("Reported."));
    fx.run(say("Do it"));

    let report = Report {
        title: "Retry count raised".into(),
        summary: "Changed `src/retry.rs`.".into(),
        outcome: Outcome::NeedsReview,
        links: vec![Link { label: "PR #7".into(), url: "https://github.com/acme/pay/pull/7".into() }],
    };
    assert_eq!(fx.host.reports(), vec![(fx.session().clone(), report)]);
    assert_eq!(result_of(&fx.transcript(), "t1"), ("Posted to the Inbox with 1 link: Retry count raised".to_string(), false));
}

#[test]
fn ask_user_stops_the_run_and_the_answer_is_the_next_message() {
    let fx = Fixture::new();
    let question = json!({"question": "Which branch should I base this on?"});
    fx.llm.push(Reply::text("I need to know one thing.").and_tool("t1", "ask_user", question.clone()));
    let (outcome, log) = fx.run(say("Start the release"));

    assert_eq!(outcome.stop, StopCause::Waiting { question: "Which branch should I base this on?".into() });
    assert_eq!(fx.llm.call_count(), 1, "the loop does not ask the model again while a person is asked");
    assert_eq!(log.events.last().map(|(_, e)| e), Some(&AgentEvent::TurnFinished { stop: model::StopReason::EndTurn }));
    let waiting = vec![
        Message::user("Start the release"),
        assistant(vec![text("I need to know one thing."), tool_use("t1", "ask_user", question)]),
        user(vec![result("t1", ASKED)]),
    ];
    assert_eq!(fx.transcript(), waiting);

    fx.llm.push(Reply::text("Basing it on main."));
    let (outcome, _) = fx.run(say("main"));
    assert_eq!(outcome.stop, StopCause::Finished);
    let mut expected = waiting;
    expected[2].content.push(text("main"));
    assert_eq!(fx.requests()[1].messages, expected);
}

fn node() -> Fixture {
    let mut fx = Fixture::new();
    fx.spec.finish = Finish::Tool(FinishTool::step_done(json!({
        "type": "object",
        "properties": { "summary": { "type": "string" }, "verified": { "type": "boolean" } },
        "required": ["summary", "verified"]
    })));
    fx
}

#[test]
fn a_node_finishes_with_its_tool_and_is_reminded_when_it_forgets() {
    let fx = node();
    let short = json!({"summary": "Fixed."});
    let full = json!({"summary": "Fixed.", "verified": true});
    fx.llm.push(Reply::text("All done."));
    fx.llm.push(Reply::tool("t1", "step_done", short.clone()));
    fx.llm.push(Reply::tool("t2", "step_done", full.clone()).and_text("Never sent."));
    let (outcome, _) = fx.run(Input::Item("Fix PAY-1: retries stop too early".into()));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.payload, Some(full.clone()));
    assert_eq!(fx.llm.call_count(), 3);
    let reminder = note(
        "You stopped without calling `step_done`. If the work is complete, call `step_done` now with the result. \
         If it is not, continue working and call `step_done` when it is.",
    );
    assert_eq!(
        fx.transcript(),
        vec![
            Message::user("Fix PAY-1: retries stop too early"),
            Message::assistant("All done."),
            Message::user(reminder),
            assistant(vec![tool_use("t1", "step_done", short)]),
            user(vec![error("t1", "step_done: missing required field(s): verified. Call it again with all of them.")]),
            assistant(vec![tool_use("t2", "step_done", full), text("Never sent.")]),
            user(vec![result("t2", RECORDED)]),
        ]
    );
    let request = fx.requests().remove(0);
    assert_eq!(tool_names(&request), ["ask_user", "bash", "edit_file", "grep", "list_dir", "read_file", "step_done", "todo_write", "write_file"]);
    assert!(request.system.contains("call `step_done` with the result. The step is not finished until you do."));
    assert!(!request.system.contains("report_to_inbox"));
}

#[test]
fn a_node_that_never_calls_its_finish_tool_ends_without_a_payload() {
    let mut fx = node();
    // Grants cannot take the finish tool away.
    fx.spec.tools.deny = vec!["step_done".into()];
    for _ in 0..3 {
        fx.llm.push(Reply::text("I am done, really."));
    }
    let (outcome, _) = fx.run(Input::Item("Do the thing".into()));
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.payload, None);
    assert_eq!(outcome.text, "I am done, really.");
    assert_eq!(fx.llm.call_count(), 3, "two reminders, then it gives up");
    assert!(tool_names(&fx.requests()[0]).contains(&"step_done"));
}
