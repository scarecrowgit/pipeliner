//! Sub-agents: one, several at once, the spawn policy's limits, and a parent's cancel.

use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use llm::{BlobSource, LlmError, ProviderInfo, Reply, StreamEvent};
use model::{Message, PermissionDecision, ToolAccess, ToolGrants, Usage};
use serde_json::json;

use super::*;
use crate::{ChildSession, StopCause, SubagentTemplate};

fn spawner() -> Fixture {
    let mut fx = Fixture::new();
    fx.spec.spawn.enabled = true;
    fx
}

fn child(n: u32) -> SessionId {
    SessionId::new(format!("s1.{n}"))
}

#[test]
fn a_sub_agent_runs_the_same_loop_in_its_own_session_and_answers_with_its_last_message() {
    let mut fx = spawner();
    fx.spec.model = ModelRef::new("scripted", "claude-sonnet-5");
    let usage = Usage { input: 1_000, output: 200, ..Default::default() };
    let call = json!({"task": "Count the files in src", "label": "Counter"});
    fx.write("src/a.rs", "");
    fx.llm.push(Reply::tool("p1", "spawn_agent", call.clone()).with_usage(usage));
    fx.llm.push(Reply::tool("c1", "list_dir", json!({"path": "src"})).with_usage(usage));
    fx.llm.push(Reply::text("There is 1 file.").with_usage(usage));
    fx.llm.push(Reply::text("The sub-agent counted 1 file.").with_usage(usage));
    let (outcome, log) = fx.run(say("How many files are in src?"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.totals.tool_calls, 1, "the child's calls are its own");
    assert!((outcome.totals.cost_usd - 0.016).abs() < 1e-12, "the child's cost is the parent's too");

    // The child: its own session, its own conversation, the task as its first message.
    let children = fx.host.children(fx.session());
    assert_eq!(
        children,
        vec![(
            child(1),
            ChildSession {
                name: "Counter".into(),
                task: "Count the files in src".into(),
                model: fx.spec.model.clone(),
                depth: 1,
                created_by: fx.spec.runnable.clone(),
            }
        )]
    );
    let conversation = fx.host.transcript(&child(1));
    assert_eq!(conversation.len(), 4);
    assert_eq!(conversation[0], Message::user("Count the files in src"));
    assert_eq!(conversation[3], Message::assistant("There is 1 file."));
    assert_paired(&conversation);

    // The parent sees only the summary.
    assert_eq!(
        fx.transcript(),
        vec![
            Message::user("How many files are in src?"),
            assistant(vec![tool_use("p1", "spawn_agent", call)]),
            user(vec![result("p1", "There is 1 file.")]),
            Message::assistant("The sub-agent counted 1 file."),
        ]
    );

    let requests = fx.requests();
    let child_request = &requests[1];
    assert!(child_request.system.starts_with("You are a sub-agent created by Build to do one task."), "{}", child_request.system);
    assert_eq!(child_request.messages, vec![Message::user("Count the files in src")]);
    let names = tool_names(child_request);
    assert!(names.contains(&"bash") && names.contains(&"spawn_agent") && names.contains(&"todo_write"));
    assert!(!names.contains(&"ask_user") && !names.contains(&"report_to_inbox"), "a sub-agent talks only to its parent");
    assert!(requests[0].system.contains("# Sub-agents\n"));

    // Events: the parent announces the child; the child's own events carry its session id.
    let parent_kinds = kinds(&log, fx.session());
    assert_eq!(parent_kinds.iter().filter(|k| k.starts_with("subagent")).collect::<Vec<_>>(), [&"subagent_started", &"subagent_finished"]);
    assert_eq!(kinds(&log, &child(1))[..2], ["message", "turn_started"]);
    assert_eq!(kinds(&log, &child(1)).last(), Some(&"turn_finished"));
    assert!(log.of(fx.session()).iter().any(|e| {
        matches!(e, AgentEvent::SubagentStarted { session, name, task, depth: 1 }
            if *session == child(1) && name == "Counter" && task == "Count the files in src")
    }));
    assert!(log.of(fx.session()).iter().any(|e| {
        matches!(e, AgentEvent::SubagentFinished { session, summary, ok: true, tool_calls: 1, .. }
            if *session == child(1) && summary == "There is 1 file.")
    }));
    let ledger = fx.host.ledger();
    assert_eq!(ledger.len(), 4);
    assert_eq!(ledger[1].session, Some(child(1)));
    assert_eq!(ledger[1].runnable.as_str(), "spawned:s1.1");
    assert_eq!(ledger[1].created_by, Some(fx.spec.runnable.clone()));
}

/// A scripted model whose sub-agent calls wait for each other: every one of `need` calls
/// must have started before any is answered. Sub-agents run one after another never meet.
struct Rendezvous {
    inner: ScriptedLlm,
    need: usize,
    waiting: Mutex<usize>,
    arrived: Condvar,
    met: AtomicBool,
}

impl LlmClient for Rendezvous {
    fn stream(
        &self,
        model: &ModelRef,
        request: &ChatRequest,
        blobs: &dyn BlobSource,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), LlmError> {
        if request.system.starts_with("You are a sub-agent") {
            let mut waiting = lock(&self.waiting);
            *waiting += 1;
            self.arrived.notify_all();
            let (waiting, timeout) = self
                .arrived
                .wait_timeout_while(waiting, Duration::from_secs(10), |count| *count < self.need)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !timeout.timed_out() {
                self.met.store(true, Ordering::SeqCst);
            }
            drop(waiting);
        }
        self.inner.stream(model, request, blobs, cancel, on_event)
    }

    fn list_models(&self, provider: &str) -> Result<Vec<String>, LlmError> {
        self.inner.list_models(provider)
    }

    fn providers(&self) -> Vec<ProviderInfo> {
        self.inner.providers()
    }

    fn default_model(&self) -> Option<ModelRef> {
        self.inner.default_model()
    }
}

#[test]
fn three_sub_agents_in_one_message_run_at_the_same_time() {
    let fx = spawner();
    let tasks = ["Check the tests", "Check the docs", "Check the build"];
    let mut first = Reply::new();
    for (n, task) in tasks.iter().enumerate() {
        first = first.and_tool(format!("p{n}"), "spawn_agent", json!({"task": task}));
    }
    let llm = Rendezvous {
        inner: ScriptedLlm::new()
            .reply(first)
            .replies((0..3).map(|_| Reply::text("Looks fine.")))
            .reply(Reply::text("All three are fine.")),
        need: 3,
        waiting: Mutex::new(0),
        arrived: Condvar::new(),
        met: AtomicBool::new(false),
    };
    let mut log = EventLog::new();
    let outcome = run(&fx.spec, &fx.env, &fx.deps(&llm, &AllowAll), say("Check everything"), &mut log, &fx.cancel);

    assert_eq!(outcome.stop, StopCause::Finished);
    assert!(llm.met.load(Ordering::SeqCst), "the sub-agents did not run at the same time");
    assert_eq!(outcome.text, "All three are fine.");

    // One result per call, in the calls' order, in a single message.
    let transcript = fx.transcript();
    assert_eq!(
        transcript[2],
        user(vec![result("p0", "Looks fine."), result("p1", "Looks fine."), result("p2", "Looks fine.")])
    );
    assert_paired(&transcript);
    // Each child got its own task and its own session.
    let children = fx.host.children(fx.session());
    assert_eq!(children.iter().map(|(_, c)| c.task.as_str()).collect::<Vec<_>>(), tasks);
    for (n, task) in tasks.iter().enumerate() {
        let session = child(n as u32 + 1);
        assert_eq!(fx.host.transcript(&session), vec![Message::user(*task), Message::assistant("Looks fine.")]);
        assert_eq!(kinds(&log, &session), ["message", "turn_started", "text", "text", "message", "turn_finished"]);
    }
    let finished = log.of(fx.session()).into_iter().filter(|e| matches!(e, AgentEvent::SubagentFinished { ok: true, .. })).count();
    assert_eq!(finished, 3);
}

#[test]
fn results_are_stored_in_call_order_whatever_order_the_sub_agents_finish_in() {
    let fx = spawner();
    let tasks = ["first", "second", "third"];
    // Each sub-agent answers only after the one called after it: they finish in reverse.
    let answered: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let turn = Condvar::new();
    let llm = FnLlm(|request: &ChatRequest, _: &CancelToken, on_event: &mut dyn FnMut(StreamEvent)| {
        if request.system.starts_with("You are a sub-agent") {
            let task = request.messages[0].plain_text();
            let later = tasks.iter().position(|t| *t == task).map_or(0, |at| tasks.len() - 1 - at);
            let done = lock(&answered);
            let (mut done, _) = turn
                .wait_timeout_while(done, Duration::from_secs(10), |done| done.len() < later)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            done.push(task.clone());
            turn.notify_all();
            drop(done);
            on_event(StreamEvent::TextDelta(format!("{task} done")));
            on_event(StreamEvent::Stop(model::StopReason::EndTurn));
        } else if request.messages.len() == 1 {
            for (n, task) in tasks.iter().enumerate() {
                let (id, name) = (format!("p{n}"), "spawn_agent".to_string());
                on_event(StreamEvent::ToolUse { id, name, input: json!({"task": task}), error: None });
            }
            on_event(StreamEvent::Stop(model::StopReason::ToolUse));
        } else {
            on_event(StreamEvent::TextDelta("All done.".into()));
            on_event(StreamEvent::Stop(model::StopReason::EndTurn));
        }
        Ok(())
    });
    let mut log = EventLog::new();
    let outcome = run(&fx.spec, &fx.env, &fx.deps(&llm, &AllowAll), say("Go"), &mut log, &fx.cancel);

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(*lock(&answered), ["third", "second", "first"], "the sub-agents did not finish in reverse");
    let transcript = fx.transcript();
    assert_eq!(
        transcript[2],
        user(vec![result("p0", "first done"), result("p1", "second done"), result("p2", "third done")])
    );
    assert_paired(&transcript);
    // Each child session holds the task of its own call.
    for (n, task) in tasks.iter().enumerate() {
        assert_eq!(fx.host.transcript(&child(n as u32 + 1))[0], Message::user(*task));
    }
}

#[test]
fn a_template_gives_the_child_its_role_and_no_more_tools_than_the_parent_has() {
    let mut fx = spawner();
    fx.spec.skills.forced = vec!["missing-skill".into()];
    fx.spec.templates = vec![SubagentTemplate {
        name: "Reviewer".into(),
        description: "Reads a diff and lists problems".into(),
        role: "You review diffs.".into(),
        model: Some(ModelRef::new("scripted", "small-model")),
        tools: ToolGrants { access: ToolAccess::ReadOnly, ..Default::default() },
        max_steps: 1,
        per_run: 1,
        ..Default::default()
    }];
    fx.llm.push(
        Reply::tool("p1", "spawn_agent", json!({"agent": "reviewer", "task": "Review the diff"}))
            .and_tool("p2", "spawn_agent", json!({"agent": "Reviewer", "task": "Review it again"}))
            .and_tool("p3", "spawn_agent", json!({"agent": "Painter", "task": "Paint"})),
    );
    fx.llm.push(Reply::tool("c1", "grep", json!({"pattern": "todo"})));
    fx.llm.push(Reply::text("Reviewed."));
    fx.run(say("Review"));

    let calls = fx.llm.calls();
    let (model, request) = &calls[1];
    assert_eq!(model, &ModelRef::new("scripted", "small-model"));
    assert!(request.system.starts_with("You review diffs.\n\nYou were created by Build as a sub-agent."));
    assert!(request.system.contains("(Skill `missing-skill` is not installed"), "the parent's forced skills go along");
    assert_eq!(tool_names(request), ["grep", "list_dir", "read_file", "spawn_agent", "todo_write"]);
    assert!(calls[0].1.system.contains("Kinds you can create by name:\n\n- Reviewer: Reads a diff and lists problems"));

    let transcript = fx.transcript();
    // The child had one step: it stopped at its limit and the parent is told so.
    let (first, is_error) = result_of(&transcript, "p1");
    assert!(is_error);
    assert_eq!(first, "[The sub-agent reached its step limit before it finished.]");
    assert_eq!(
        result_of(&transcript, "p2"),
        ("A run may create 1 of kind `Reviewer` and that many were created; this one was not.".to_string(), true)
    );
    let (unknown, _) = result_of(&transcript, "p3");
    assert!(unknown.starts_with("There is no sub-agent kind `Painter`. Kinds you can create: Reviewer."), "{unknown}");
    assert_eq!(fx.host.children(fx.session()).len(), 1);
    assert_eq!(fx.host.children(fx.session())[0].1.name, "Reviewer");
}

#[test]
fn the_per_run_and_at_once_limits_refuse_with_an_error_result() {
    let mut fx = spawner();
    fx.spec.spawn.per_run = 1;
    let two = || {
        Reply::tool("p1", "spawn_agent", json!({"task": "One"})).and_tool("p2", "spawn_agent", json!({"task": "Two"}))
    };
    fx.llm.push(two());
    fx.llm.push(Reply::text("One done."));
    fx.llm.push(Reply::text("Only one ran."));
    fx.run(say("Go"));
    let transcript = fx.transcript();
    assert_eq!(result_of(&transcript, "p1"), ("One done.".to_string(), false));
    assert_eq!(
        result_of(&transcript, "p2"),
        ("The limit of 1 sub-agent per run is reached; this one was not created. Do the rest yourself.".to_string(), true)
    );
    assert_eq!(fx.host.children(fx.session()).len(), 1);
    assert_paired(&transcript);

    let mut fx = spawner();
    fx.spec.spawn.alive_at_once = 1;
    fx.llm.push(two());
    fx.llm.push(Reply::text("One done."));
    fx.llm.push(Reply::tool("p3", "spawn_agent", json!({"task": "Two"})));
    fx.llm.push(Reply::text("Two done."));
    fx.llm.push(Reply::text("Both ran, one after the other."));
    fx.run(say("Go"));
    let transcript = fx.transcript();
    let (refused, is_error) = result_of(&transcript, "p2");
    assert!(is_error && refused.starts_with("Only 1 sub-agent may run at the same time"), "{refused}");
    assert_eq!(result_of(&transcript, "p3"), ("Two done.".to_string(), false));
}

#[test]
fn spawning_deeper_than_the_policy_allows_asks_a_person() {
    let mut fx = spawner();
    fx.spec.spawn.max_depth = 0;
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Dig deeper", "label": "Digger"})));
    fx.llm.push(Reply::text("Not allowed."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let gate = asker.gate(|_| Some(PermissionDecision::Reject { note: "too deep".into() }));
    fx.run_gated(&gate, say("Go"));

    let asked = lock(&asker.requests).clone();
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].tool.as_str(), asked[0].class), ("spawn_agent", ToolClass::Internal));
    assert_eq!(asked[0].policy, "spawn_agent deeper than 0 = ask");
    assert_eq!(asked[0].display, ToolDisplay::Text { title: "Create sub-agent “Digger” at depth 1".into(), body: "Dig deeper".into() });
    assert_eq!(asked[0].always_rule, None);
    assert!(fx.host.children(fx.session()).is_empty());
    assert_eq!(
        result_of(&fx.transcript(), "p1"),
        ("The person rejected this action. Their note: \"too deep\". Do not repeat the same call; adapt.".to_string(), true)
    );

    // Allowed: the child runs.
    fx.llm.push(Reply::tool("p2", "spawn_agent", json!({"task": "Dig deeper"})));
    fx.llm.push(Reply::text("Dug."));
    fx.llm.push(Reply::text("Done."));
    fx.run_gated(&asker.gate(|_| Some(PermissionDecision::AllowOnce)), say("Go on"));
    assert_eq!(result_of(&fx.transcript(), "p2"), ("Dug.".to_string(), false));
    assert_eq!(asker.asked(), 2);
}

#[test]
fn a_grandchild_within_the_depth_limit_needs_no_question() {
    let fx = spawner();
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Level one"})));
    fx.llm.push(Reply::tool("c1", "spawn_agent", json!({"task": "Level two"})));
    fx.llm.push(Reply::text("Bottom."));
    fx.llm.push(Reply::text("Middle."));
    fx.llm.push(Reply::text("Top."));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let (outcome, _) = fx.run_gated(&asker.gate(|_| None), say("Go"));
    assert_eq!(outcome.text, "Top.");
    assert_eq!(asker.asked(), 0);
    let grandchild = SessionId::new("s1.1.1");
    assert_eq!(fx.host.children(&child(1)), vec![(
        grandchild.clone(),
        ChildSession {
            name: "Sub-agent".into(),
            task: "Level two".into(),
            model: fx.spec.model.clone(),
            depth: 2,
            created_by: model::RunnableRef::spawned("s1.1"),
        }
    )]);
    assert_eq!(fx.host.transcript(&grandchild), vec![Message::user("Level two"), Message::assistant("Bottom.")]);
}

#[test]
fn cancelling_the_parent_cancels_its_sub_agents() {
    let fx = spawner();
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Take a long time"})));
    fx.llm.push(Reply::text("Never said.").with_delay(Duration::from_secs(30)));
    let cancel = fx.cancel.clone();
    let mut sink = |_: &SessionId, event: AgentEvent| {
        if matches!(event, AgentEvent::SubagentStarted { .. }) {
            cancel.cancel();
        }
    };
    let started = std::time::Instant::now();
    let outcome = fx.run_into(&AllowAll, say("Go"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(
        result_of(&fx.transcript(), "p1"),
        ("[The sub-agent was cancelled before it finished.]".to_string(), true)
    );
    assert_paired(&fx.transcript());
    assert_eq!(fx.host.transcript(&child(1)), vec![Message::user("Take a long time")]);
}

#[test]
fn a_sub_agent_has_its_own_budget_and_spawning_is_off_by_default() {
    let mut fx = spawner();
    fx.spec.model = ModelRef::new("scripted", "claude-sonnet-5");
    fx.spec.spawn.budget_usd = Some(0.003);
    let usage = Usage { input: 1_000, output: 200, ..Default::default() };
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Spend"})));
    fx.llm.push(Reply::tool("c1", "todo_write", json!({"items": []})).with_usage(usage));
    fx.llm.push(Reply::text("The child ran out of money."));
    let (outcome, _) = fx.run(say("Go"));
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(result_of(&fx.transcript(), "p1"), ("[The sub-agent spent its budget before it finished.]".to_string(), true));

    let fx = Fixture::new();
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Anything"})));
    fx.llm.push(Reply::text("I cannot."));
    fx.run(say("Go"));
    assert!(!tool_names(&fx.requests()[0]).contains(&"spawn_agent"));
    assert!(result_of(&fx.transcript(), "p1").0.starts_with("There is no tool `spawn_agent`."));
    assert!(fx.host.children(fx.session()).is_empty());
}

#[test]
fn no_approval_reaches_past_the_depth_ceiling() {
    // A gate that allows everything, as a trusted headless run has.
    let mut fx = spawner();
    fx.env.depth = crate::subagents::DEPTH_CEILING;
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Go deeper"})));
    fx.llm.push(Reply::text("I will do it myself."));
    let (outcome, _) = fx.run(Input::Item("Dig".into()));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(
        result_of(&fx.transcript(), "p1"),
        ("Sub-agents cannot be nested deeper than 8 levels; this one was not created. Do the task yourself.".to_string(), true)
    );
    assert!(fx.host.children(fx.session()).is_empty());

    // One level up, the same gate lets it through.
    let mut fx = spawner();
    fx.env.depth = crate::subagents::DEPTH_CEILING - 1;
    fx.llm.push(Reply::tool("p1", "spawn_agent", json!({"task": "Go deeper"})));
    fx.llm.push(Reply::text("Bottom."));
    fx.llm.push(Reply::text("Done."));
    fx.run(Input::Item("Dig".into()));
    assert_eq!(result_of(&fx.transcript(), "p1"), ("Bottom.".to_string(), false));
}
