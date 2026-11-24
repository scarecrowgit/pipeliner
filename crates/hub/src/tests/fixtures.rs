//! FixtureApi: every `Api` method answers sensibly for the Acme world, commands are recorded
//! and emit their events, and scripted turns follow the engine's event order.

use std::path::Path;

use crate::api::{
    AgentEvent, Api, ChatMode, Event, ExportFormat, ExportOptions, InboxId, IssueKey, IssuePatch, IssueStatus,
    NewIssue, NewSession, Period, PermissionDecision, PipelineId, ProjectId, RequestId, ReviewDecision, ReviewId,
    Role, RunId, RunInput, RunState, RunnableKind, RunnableRef, SessionFilter, SessionId, SessionQuery, SessionState,
    StartFrom, StopReason, UserMessage,
};
use crate::api::{CatalogApi, ChatApi, InboxApi, IssueApi, OpsApi, RunApi, WorkdirApi};
use crate::fixtures::{FixtureApi, SCENARIOS, World, start_route};

const NOW: i64 = 1_795_000_000_000;

fn acme() -> FixtureApi {
    FixtureApi::new(World::acme(NOW))
}

/// Every event emitted while `f` runs.
fn emitted(api: &FixtureApi, f: impl FnOnce(&FixtureApi)) -> Vec<Event> {
    let events = api.subscribe();
    f(api);
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

/// A short name per event, for comparing sequences.
fn shape(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            Event::Agent { event, .. } => match event {
                AgentEvent::TurnStarted => "turn".into(),
                AgentEvent::TextDelta { .. } => "text".into(),
                AgentEvent::ThinkingDelta { .. } => "thinking".into(),
                AgentEvent::MessageCommitted { message } => format!("commit:{:?}", message.message.role),
                AgentEvent::ToolCallStarted { .. } => "call".into(),
                AgentEvent::ToolCallReady { .. } => "ready".into(),
                AgentEvent::ToolFinished { is_error, .. } => format!("finished:{is_error}"),
                AgentEvent::Usage { .. } => "usage".into(),
                AgentEvent::TodoUpdated { .. } => "todos".into(),
                AgentEvent::TurnFinished { stop } => format!("end:{stop:?}"),
                other => format!("{other:?}"),
            },
            Event::SessionChanged { session } => format!("session:{:?}", session.state),
            Event::PermissionResolved { .. } => "resolved".into(),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Runs of the same name collapse into one, so streamed text counts once.
fn collapse(shape: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in shape {
        if out.last() != Some(&item) || item != "text" {
            out.push(item);
        }
    }
    out
}

#[test]
fn chat_queries_answer_for_the_acme_world() {
    let api = acme();
    let fix = SessionId::new("chat-1423");
    let top = api.sessions(&SessionFilter { archived: Some(false), ..SessionFilter::default() }).unwrap_or_default();
    assert_eq!(top.len(), 13);
    assert!(top.windows(2).all(|pair| pair[0].updated >= pair[1].updated), "newest first");
    let waiting = SessionFilter { state: Some(SessionState::Waiting), ..SessionFilter::default() };
    assert_eq!(api.sessions(&waiting).map(|list| list.len()).ok(), Some(1));

    let Ok(Some(detail)) = api.session_detail(&fix) else { panic!("no detail for chat-1423") };
    assert_eq!(detail.children.len(), 2);
    assert_eq!(detail.pending.len(), 1);
    assert_eq!(detail.todos.len(), 4);
    assert_eq!(detail.mcp.len(), 3);
    assert!(detail.repo.is_some());
    assert!(api.session_detail(&SessionId::new("nope")).ok().flatten().is_none());

    let messages = api.messages(&fix, 0, u64::MAX).unwrap_or_default();
    assert_eq!(messages.len() as u32, detail.session.message_count);
    assert_eq!(api.messages(&SessionId::new("chat-ledger"), 10, 20).map(|m| m.len()).ok(), Some(10));

    let result = api.search_sessions(&SessionQuery { text: "ledger".into(), ..SessionQuery::default() });
    assert_eq!(result.map(|r| r.hits.len()).ok(), Some(1));

    let models = api.models();
    assert!(models.iter().any(|provider| provider.models.contains(&"opus-5.5".to_string())));
    assert_eq!(api.mention_files(&fix, "charges", 3).len(), 3);
    assert!(api.slash_commands(&fix).iter().any(|command| command.from_project));
    assert!(api.blob_path(&crate::api::BlobId::new("b1")).ends_with("b1"));
}

#[test]
fn workdir_issue_and_run_queries_answer() {
    let api = acme();
    assert!(!api.recent_workdirs().is_empty());
    let repo = Path::new("/home/me/code/acme/payments-api");
    assert!(!api.worktrees(repo).is_empty());
    assert!(api.repo_facts(repo).is_some());
    assert!(api.repo_facts(Path::new("/tmp/elsewhere")).is_none());
    assert!(api.branches(repo).contains(&"main".to_string()));
    assert_eq!(api.folder_info(repo).agents_md_lines, Some(32));
    assert!(api.suggest_branch(repo, "Refund timeout").starts_with("chat/"));

    let project = ProjectId::new("payments");
    let issues = api.issues(&project).unwrap_or_default();
    assert!(issues.len() >= 10);
    assert!(issues.iter().all(|issue| issue.project == project));
    assert!(api.issue(&IssueKey::new("PAY-1427")).ok().flatten().is_some());
    assert_eq!(api.issue_events(&IssueKey::new("PAY-1427")).map(|e| e.len()).ok(), Some(9));
    assert!(!api.autopilots(&project).unwrap_or_default().is_empty());

    let run = RunId::new("run-1427");
    assert!(api.run(&run).ok().flatten().is_some_and(|run| run.issue == Some(IssueKey::new("PAY-1427"))));
    let latest = api.latest_runs().unwrap_or_default();
    assert_eq!(latest.len(), 3, "one per issue; the finished test run is left out");
    assert!(latest.iter().all(|run| run.issue.is_some()));
    assert!(!api.runs_for_issue(&IssueKey::new("PAY-1427")).unwrap_or_default().is_empty());
    assert_eq!(api.run_log_tail(&run, 5).map(|lines| lines.len()).ok(), Some(5));
    let after = api.run_log(&run, 10, 100).unwrap_or_default();
    assert!(after.iter().all(|line| line.seq > 10) && !after.is_empty());
    assert!(api.run_stats(&run).is_ok_and(|stats| stats.budget_usd.is_some()));
}

#[test]
fn inbox_catalog_and_ops_queries_answer() {
    let api = acme();
    let inbox = api.inbox(false).unwrap_or_default();
    assert!(inbox.len() >= 4 && inbox.windows(2).all(|pair| pair[0].created >= pair[1].created));
    assert!(api.inbox(true).unwrap_or_default().iter().all(|item| item.archived));
    let review = ReviewId::new("review-1423");
    assert_eq!(api.open_reviews().map(|r| r.len()).ok(), Some(3));
    assert!(api.review(&review).ok().flatten().is_some());
    assert!(api.variant_diff(&review, 1, "src/charges/retry.ts").is_ok());
    assert!(api.variant_diff(&review, 1, "no/such/file").is_err());
    let estimate = api.send_back_estimate(&review, StartFrom::All, 2).unwrap_or_default();
    assert!(estimate.secs > 0 && estimate.cost_usd > 0.0);
    assert_eq!(api.pull_request("https://github.com/acme/payments-api/pull/2291").map(|pr| pr.number).ok(), Some(2291));

    let runnables = api.runnables();
    for kind in [RunnableKind::Agent, RunnableKind::Orchestrator, RunnableKind::Pipeline] {
        assert!(runnables.iter().any(|runnable| runnable.kind == kind), "{kind:?} in the library");
    }
    assert_eq!(api.runnable_activity().map(|a| a.len()).ok(), Some(runnables.len()));
    assert!(!api.skills().is_empty());
    assert!(api.catalog_problems().is_empty());
    assert!(api.used_by(&RunnableRef::orchestrator("payments-lead")).is_ok());
    assert!(!api.templates().is_empty());
    let triage = PipelineId::new("pay-triage");
    let Some(graph) = api.pipeline(&triage) else { panic!("no pay-triage") };
    assert!(!graph.nodes.is_empty());
    let problems = api.pipeline_problems(&graph);
    assert!(!problems.is_empty() && problems.iter().all(|problem| problem.node.is_some()), "its Jira source");
    assert!(!api.pipeline_versions(&triage).unwrap_or_default().is_empty());
    assert_eq!(api.pipeline_changes(&triage).map(|c| c.len()).ok(), Some(2));

    for period in [Period::Today, Period::Days7, Period::Days30, Period::Days90] {
        assert!(api.home_stats(period).is_ok_and(|stats| stats.runs > 0), "{period:?}");
        assert!(!api.spend_buckets(period).unwrap_or_default().is_empty(), "{period:?}");
        assert_eq!(api.spend_by_runnable(period, 3).map(|rows| rows.len()).ok(), Some(4), "top 3 and others");
    }
    assert!(!api.needs_you().unwrap_or_default().is_empty());
    assert!(!api.working_now().unwrap_or_default().is_empty());
    assert!(api.spend_today().is_ok());
    assert!(!api.runtimes().is_empty());
    assert!(api.settings().is_ok_and(|settings| settings.current_project == Some(ProjectId::new("payments"))));
    assert!(!api.projects().unwrap_or_default().is_empty());
    assert_eq!(api.permission_rules().map(|rules| rules.len()).ok(), Some(3));
}

#[test]
fn every_scenario_loads_and_has_a_start() {
    for name in SCENARIOS {
        assert!(FixtureApi::scenario(name).is_some(), "{name}");
        assert!(start_route(name).is_some(), "{name}");
    }
    assert!(FixtureApi::scenario("nope").is_none());
    let Some(empty) = FixtureApi::scenario("empty") else { panic!("no empty scenario") };
    assert!(empty.sessions(&SessionFilter::default()).unwrap_or_default().is_empty());
    assert_eq!(empty.projects().map(|p| p.len()).ok(), Some(1));
}

#[test]
fn chat_commands_are_recorded_and_emit_their_events() {
    let api = acme();
    let ledger = SessionId::new("chat-ledger");
    let events = emitted(&api, |api| {
        assert!(api.pin_session(&ledger, true).is_ok());
        assert!(api.rename_session(&ledger, "Ledger").is_ok());
        assert!(api.set_session_model(&ledger, &crate::api::ModelRef::new("anthropic", "sonnet-5.5")).is_ok());
        assert!(api.set_session_runnable(&ledger, &RunnableRef::agent("codex"), ChatMode::Plan).is_ok());
        assert!(api.archive_session(&ledger, true).is_ok());
    });
    assert_eq!(events.len(), 5);
    let Some(Event::SessionChanged { session }) = events.last() else { panic!("no header") };
    assert!(session.pinned && session.archived && session.title == "Ledger" && session.mode == ChatMode::Plan);
    assert_eq!(
        api.recorded_names(),
        ["pin_session", "rename_session", "set_session_model", "set_session_runnable", "archive_session"]
    );

    let new = NewSession {
        runnable: None,
        mode: ChatMode::Build,
        model: None,
        workdir: None,
        forced_skills: Vec::new(),
        issue: None,
    };
    let Ok(created) = api.new_session(new) else { panic!("new_session failed") };
    assert_eq!(created.message_count, 0);
    let Ok(fork) = api.fork_session(&SessionId::new("chat-1423"), Some(3)) else { panic!("fork failed") };
    assert_eq!(fork.message_count, 3);
    let to = Path::new("/out");
    let options = ExportOptions { subagents: true, tool_output: true, images: false, local_paths: false };
    assert!(api.export_session(&fork.id, ExportFormat::Jsonl, options, to).is_ok_and(|p| p.ends_with(format!("{}.jsonl", fork.id))));
    assert!(api.put_image("shot.png", "image/png", &[0; 10]).is_ok_and(|a| a.bytes == 10));
    let removed = emitted(&api, |api| assert!(api.delete_session(&fork.id).is_ok()));
    assert_eq!(removed, [Event::SessionRemoved { session: fork.id.clone() }]);
    assert!(api.delete_session(&fork.id).is_err(), "gone");

    api.fail_next("compact");
    assert!(api.compact(&ledger).is_err());
    assert!(api.compact(&ledger).is_ok(), "only the next call fails");
}

#[test]
fn a_sent_message_plays_the_engines_event_order() {
    let api = acme();
    let id = SessionId::new("chat-split-checkout");
    let events = emitted(&api, |api| {
        assert!(api.send(&id, UserMessage { text: "Go on".into(), images: Vec::new() }).is_ok());
    });
    assert_eq!(
        collapse(shape(&events)),
        [
            "session:Running",
            "commit:User",
            "turn",
            "text",
            "call",
            "ready",
            "usage",
            "commit:Assistant",
            "finished:false",
            "commit:User",
            "turn",
            "text",
            "usage",
            "commit:Assistant",
            "end:EndTurn",
            "session:Idle",
        ]
    );
    // Stored seqs continue the history, and the header counts the new messages.
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            Event::Agent { event: AgentEvent::MessageCommitted { message }, .. } => Some(message.seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, [3, 4, 5, 6]);
    let messages = api.messages(&id, 0, u64::MAX).unwrap_or_default();
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[2].message.role, Role::User);
    assert_eq!(api.session_detail(&id).ok().flatten().map(|d| d.session.message_count), Some(6));
}

#[test]
fn a_busy_session_queues_the_message() {
    let api = acme();
    let busy = SessionId::new("chat-refund-timeout");
    let events = emitted(&api, |api| {
        assert!(api.send(&busy, UserMessage { text: "Also check the batcher".into(), images: Vec::new() }).is_ok());
    });
    assert!(events.is_empty());
    let queued = api.session_detail(&busy).ok().flatten().map(|detail| detail.queued.len());
    assert_eq!(queued, Some(1));
}

#[test]
fn an_answered_permission_finishes_the_waiting_turn() {
    let api = acme();
    let events = emitted(&api, |api| {
        assert!(api.answer_permission(&RequestId::new("perm-1423-bun-test"), PermissionDecision::AllowOnce).is_ok());
    });
    assert_eq!(
        collapse(shape(&events)),
        [
            "resolved",
            "session:Running",
            "finished:false",
            "commit:User",
            "turn",
            "call",
            "ready",
            "usage",
            "commit:Assistant",
            "todos",
            "finished:false",
            "commit:User",
            "turn",
            "text",
            "usage",
            "commit:Assistant",
            "end:EndTurn",
            "session:Done",
        ]
    );
    assert!(api.answer_permission(&RequestId::new("perm-1423-bun-test"), PermissionDecision::AllowOnce).is_err());
    assert!(api.session_detail(&SessionId::new("chat-1423")).ok().flatten().is_some_and(|d| d.pending.is_empty()));
}

#[test]
fn a_stop_answers_the_dangling_call_and_cancels() {
    let api = acme();
    let fix = SessionId::new("chat-1423");
    let events = emitted(&api, |api| api.interrupt(&fix));
    assert_eq!(shape(&events), ["resolved", "finished:true", "commit:User", "end:Cancelled", "session:Idle"]);
    let last = api.messages(&fix, 0, u64::MAX).unwrap_or_default().pop();
    assert!(last.is_some_and(|m| m.message.role == Role::User && m.seq == 11));
    // A second stop has nothing left to answer.
    let again = emitted(&api, |api| api.interrupt(&fix));
    assert_eq!(shape(&again), ["end:Cancelled", "session:Idle"]);
    let unused = emitted(&api, |api| api.interrupt(&SessionId::new("nope")));
    assert!(unused.is_empty());
    assert!(matches!(
        events.get(3),
        Some(Event::Agent { event: AgentEvent::TurnFinished { stop: StopReason::Cancelled }, .. })
    ));
}

#[test]
fn work_commands_change_the_world_and_emit() {
    let api = acme();
    let key = IssueKey::new("PAY-1440");
    let patch = IssuePatch { status: Some(IssueStatus::InProgress), ..IssuePatch::default() };
    let events = emitted(&api, |api| assert!(api.update_issue(&key, patch).is_ok()));
    assert!(matches!(&events[..], [Event::IssueChanged { .. }, Event::IssueActivity { .. }]));

    let new = NewIssue { project: ProjectId::new("payments"), title: "Flaky payout test".into(), ..NewIssue::default() };
    let Ok(issue) = api.create_issue(new) else { panic!("create failed") };
    assert!(issue.key.as_str().starts_with("PAY-"));
    assert!(api.issue(&issue.key).ok().flatten().is_some());
    assert!(api.attach_runner(&issue.key, Some(RunnableRef::agent("build"))).is_ok_and(|i| i.runner.is_some()));
    assert!(api.start_issue(&issue.key).is_ok());
    assert!(api.retry_wake(&issue.key, &RunnableRef::agent("build")).is_err(), "the scripted offline runtime");
    assert!(api.delete_issue(&issue.key).is_ok());

    let Ok(run) = api.start_run(&PipelineId::new("pay-triage"), RunInput { title: "Manual".into(), text: String::new() }) else {
        panic!("start_run failed")
    };
    assert!(api.cancel_run(&run.id).is_ok());
    assert_eq!(api.run(&run.id).ok().flatten().map(|run| run.state), Some(RunState::Cancelled));
    assert!(api.resume_run(&run.id).is_ok());
    assert!(api.test_run(&PipelineId::new("pay-triage"), RunInput::default()).is_ok_and(|run| run.test));

    let item = InboxId::new("inbox-1427");
    let events = emitted(&api, |api| assert!(api.set_inbox_read(&item, true).is_ok()));
    assert!(matches!(&events[..], [Event::InboxChanged { item }] if item.read));
    let decision = ReviewDecision::SendBack { note: "Smaller batches".into(), start: StartFrom::All, variants: 2 };
    let events = emitted(&api, |api| assert!(api.decide_review(&ReviewId::new("review-1427"), decision).is_ok()));
    assert!(matches!(events.first(), Some(Event::ReviewChanged { .. })));
    assert!(matches!(events.last(), Some(Event::SpendChanged)));

    let reference = RunnableRef::orchestrator("payments-lead");
    let events = emitted(&api, |api| assert!(api.set_runnable_paused(&reference, true).is_ok()));
    assert_eq!(events, [Event::CatalogChanged]);
    assert!(api.duplicate_runnable(&reference).is_ok());
    assert!(api.deploy_pipeline(&PipelineId::new("pay-triage"), "variants 3").is_ok_and(|v| v.note == "variants 3"));
    assert!(api.import_runnable(Path::new("/tmp/agent.md")).is_err(), "not in previews");

    let mut settings = api.settings().unwrap_or_default();
    settings.slots = 4;
    let events = emitted(&api, |api| assert!(api.set_settings(settings).is_ok()));
    assert_eq!(events, [Event::SettingsChanged]);
    api.shutdown();
    assert_eq!(api.recorded_names().last(), Some(&"shutdown"));
}
