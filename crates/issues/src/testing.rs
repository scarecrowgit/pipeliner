//! Builders shared by the tests of this crate.

use model::{
    Issue, IssueKey, IssueStatus, Priority, ProjectId, Run, RunId, RunState, Runnable, RunnableKind, RunnableRef, Origin,
    StartTriggers, Uses,
};

pub(crate) fn issue(status: IssueStatus) -> Issue {
    Issue {
        key: IssueKey::new("PAY-1"),
        project: ProjectId::new("pay"),
        title: "Title".into(),
        body: String::new(),
        status,
        priority: Priority::Medium,
        kind: Default::default(),
        owner: String::new(),
        repo: None,
        created: 0,
        updated: 0,
        filed_by: None,
        runner: None,
        triggers: StartTriggers::default(),
        run: None,
        run_count: 0,
        pr_url: None,
        images: Vec::new(),
    }
}

pub(crate) fn run(id: &str, state: RunState) -> Run {
    Run {
        id: RunId::new(id),
        pipeline: "p".into(),
        version: 1,
        issue: None,
        number: 1,
        title: String::new(),
        input: String::new(),
        state,
        test: false,
        started: 0,
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: Vec::new(),
    }
}

pub(crate) fn runnable(kind: RunnableKind, name: &str) -> Runnable {
    let id = name.to_lowercase().replace(' ', "-");
    let reference = match kind {
        RunnableKind::Agent => RunnableRef::agent(&id),
        RunnableKind::Orchestrator => RunnableRef::orchestrator(&id),
        RunnableKind::Pipeline => RunnableRef::pipeline(&id),
    };
    Runnable {
        reference,
        kind,
        name: name.to_string(),
        description: String::new(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: None,
        path: None,
    }
}

pub(crate) fn agent(name: &str) -> Runnable {
    runnable(RunnableKind::Agent, name)
}

pub(crate) fn node(name: &str, status: model::NodeStatus) -> model::NodeInstance {
    model::NodeInstance {
        node: model::NodeId::new(name.to_lowercase()),
        name: name.to_string(),
        variant: 0,
        attempt: 1,
        status,
        kind: model::SpanKind::Agent,
        port: None,
        session: None,
        worktree: None,
        branch: None,
        started: None,
        finished: None,
        cost_usd: 0.0,
        summary: String::new(),
        spawned_by: None,
    }
}
