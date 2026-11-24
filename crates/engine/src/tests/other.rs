//! Worktrees, issues, the catalog, and the methods that are not wired yet.

use catalog::Place;
use model::{
    Actor, AgentDef, Event, IssueEventKind, IssueStatus, Origin, PipelineId, Project, ProjectId, ReviewDecision,
    ReviewId, RunId, Runnable, RunnableKind, RunnableRef, Uses, WorkMode,
};

use super::{Fx, git, new_session};
use crate::api::{
    CatalogApi, ChatApi, InboxApi, IssueApi, IssuePatch, NewIssue, OpsApi, RunApi, RunInput, WorkdirApi, WorkdirChoice,
};

#[test]
fn a_worktree_chat_gets_its_own_branch_and_folder() {
    let fx = Fx::new();
    let repo = fx.repo("payments", &[("README.md", "hello\n")]);
    let choice = WorkdirChoice { path: repo.clone(), mode: WorkMode::Worktree, base: None, branch: None };
    let session = fx.engine.new_session(new_session(Some(choice.clone()))).expect("session");
    let workdir = session.workdir.expect("workdir");

    assert_eq!(workdir.mode, WorkMode::Worktree);
    assert_eq!(workdir.repo, "payments");
    assert_eq!(workdir.source.as_deref(), Some(repo.as_path()));
    let branch = workdir.branch.clone().expect("branch");
    assert!(branch.starts_with("chat/"), "{branch}");
    assert!(workdir.path.starts_with(fx.paths.worktrees_dir()), "{}", workdir.path.display());
    assert_eq!(std::fs::read_to_string(workdir.path.join("README.md")).expect("readme"), "hello\n");
    assert!(git(&repo, &["branch", "--list"]).contains(&branch));

    // A second chat gets another branch, and the suggestion skips taken names.
    let second = fx.engine.new_session(new_session(Some(choice))).expect("second").workdir.expect("workdir");
    assert_ne!(second.branch, workdir.branch);
    assert_ne!(fx.engine.suggest_branch(&repo, "chat"), branch);
    let trees = fx.engine.worktrees(&repo);
    assert_eq!(trees.len(), 3, "the checkout and two worktrees");
    assert_eq!(fx.engine.recent_workdirs().len(), 2);
}

#[test]
fn issues_are_created_patched_and_commented_with_their_feed() {
    let fx = Fx::new();
    let project = Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() };
    fx.engine.save_project(project).expect("project");
    let new = NewIssue { project: ProjectId::new("har"), title: "Fix the retry".into(), body: "It loops.".into(), ..Default::default() };
    let issue = fx.engine.create_issue(new).expect("create");
    assert_eq!(issue.key.as_str(), "HAR-1");

    let patch = IssuePatch { status: Some(IssueStatus::Todo), title: Some("Fix the retry loop".into()), ..Default::default() };
    let patched = fx.engine.update_issue(&issue.key, patch).expect("patch");
    assert_eq!((patched.status, patched.title.as_str()), (IssueStatus::Todo, "Fix the retry loop"));
    fx.engine.comment(&issue.key, "Looks like a timeout", Vec::new()).expect("comment");

    let events = fx.engine.issue_events(&issue.key).expect("events");
    let kinds: Vec<&IssueEventKind> = events.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], IssueEventKind::Created));
    assert!(matches!(kinds[1], IssueEventKind::StatusChanged { from: IssueStatus::Backlog, to: IssueStatus::Todo }));
    assert!(matches!(kinds[2], IssueEventKind::Comment { text, .. } if text == "Looks like a timeout"));
    assert!(events.iter().all(|e| e.actor == Actor::Me));
    assert_eq!(fx.engine.issue(&issue.key).expect("read").expect("issue").title, "Fix the retry loop");
    assert_eq!(fx.engine.issues(&ProjectId::new("har")).expect("list").len(), 1);

    fx.drain();
    let log = fx.log.lock().expect("log");
    assert_eq!(log.iter().filter(|e| matches!(e, Event::IssueActivity { .. })).count(), 3);
    assert!(log.iter().any(|e| matches!(e, Event::IssueChanged { issue } if issue.status == IssueStatus::Todo)));
    drop(log);

    assert!(fx.engine.start_issue(&issue.key).is_err());
    fx.engine.delete_issue(&issue.key).expect("delete");
    assert!(fx.engine.issue(&issue.key).expect("read").is_none());
    // Keys are not handed out twice.
    let again = NewIssue { project: ProjectId::new("har"), title: "Another".into(), ..Default::default() };
    assert_eq!(fx.engine.create_issue(again).expect("create").key.as_str(), "HAR-2");
}

#[test]
fn a_saved_agent_is_in_the_catalog_and_survives_a_reload() {
    let fx = Fx::new();
    let runnable = Runnable {
        reference: RunnableRef::agent("fixer"),
        kind: RunnableKind::Agent,
        name: "Fixer".into(),
        description: "Fixes bugs".into(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef { role: "You fix bugs.".into(), ..Default::default() }),
        path: None,
    };
    fx.engine.save_runnable(&Place::Library, runnable).expect("save");
    fx.drain();
    assert!(fx.log.lock().expect("log").iter().any(|e| matches!(e, Event::CatalogChanged)));
    let found = fx.engine.runnables().into_iter().find(|r| r.reference == RunnableRef::agent("fixer")).expect("fixer");
    assert_eq!(found.name, "Fixer");
    assert!(fx.paths.library_dir().join("agents/fixer.md").is_file());

    let (engine, _) = fx.reopen();
    assert!(engine.runnables().iter().any(|r| r.reference == RunnableRef::agent("fixer")));
    engine.set_runnable_paused(&RunnableRef::agent("fixer"), true).expect("pause");
    assert!(engine.runnables().iter().any(|r| r.reference == RunnableRef::agent("fixer") && r.paused));
    let copy = engine.duplicate_runnable(&RunnableRef::agent("fixer")).expect("duplicate");
    assert_ne!(copy, RunnableRef::agent("fixer"));
    engine.delete_runnable(&copy).expect("delete");
    assert!(!engine.runnables().iter().any(|r| r.reference == copy));
    assert!(engine.catalog_problems().is_empty());
}

#[test]
fn pipeline_methods_on_missing_things_say_what_is_missing() {
    let fx = Fx::new();
    let (pipeline, run) = (PipelineId::new("p"), RunId::new("r"));
    let says = |result: anyhow::Result<()>, what: &str| {
        let error = result.expect_err("refused").to_string();
        assert!(error.contains(what), "{error}");
    };
    says(fx.engine.start_run(&pipeline, RunInput::default()).map(|_| ()), "no deployed version");
    says(fx.engine.test_run(&pipeline, RunInput::default()).map(|_| ()), "There is no pipeline p");
    says(fx.engine.cancel_run(&run), "There is no run r");
    says(fx.engine.resume_run(&run), "There is no run r");
    says(fx.engine.decide_review(&ReviewId::new("v"), ReviewDecision::Skip), "There is no review v");
    says(fx.engine.deploy_pipeline(&pipeline, "first").map(|_| ()), "There is no pipeline p");
    // Reads of what the store holds work.
    assert!(fx.engine.latest_runs().expect("runs").is_empty());
    assert!(fx.engine.inbox(false).expect("inbox").is_empty());
    let runtimes = fx.engine.runtimes();
    assert_eq!(runtimes.iter().filter(|r| r.available).count(), 1);
    assert_eq!(fx.engine.home_stats(crate::api::Period::Today).expect("stats").runs, 0);
}
