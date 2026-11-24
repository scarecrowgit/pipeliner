//! The "Agents & pipelines" list: two orchestrators, three pipelines, six agents and three
//! agents created by other agents, with their activity, plus the installed skills.

use crate::api::{
    AgentDef, IssueKey, ModelRef, Origin, RunId, Runnable, RunnableActivity, RunnableKind, RunnableRef, RunnableState,
    Skill, SkillPolicy, SkillSource, SpawnPolicy, ToolAccess, ToolGrants, Uses,
};

use super::data_sessions::{model, path};
use super::World;

fn uses(codes: &str) -> Uses {
    Uses { chat: codes.contains('C'), issues: codes.contains('I'), pipelines: codes.contains('P'), autopilots: codes.contains('A') }
}

fn def(role: &str, model: ModelRef, runtime: &str, access: ToolAccess) -> AgentDef {
    AgentDef {
        role: role.to_string(),
        model,
        runtime: runtime.to_string(),
        tools: ToolGrants { access, ..ToolGrants::default() },
        ..AgentDef::default()
    }
}

struct Row {
    reference: RunnableRef,
    kind: RunnableKind,
    name: &'static str,
    description: &'static str,
    origin: Origin,
    uses: &'static str,
    agent: Option<AgentDef>,
    state: RunnableState,
    runs_today: u32,
    cost_today: f64,
    queued: u32,
}

pub fn fill(world: &mut World) {
    use RunnableKind::*;
    let issue = |key: &str| Some(IssueKey::new(key));
    let lead = RunnableRef::orchestrator("payments-lead");
    let fixer = RunnableRef::agent("fixer");
    let library = || Origin::Library;
    let spawned = |by: &RunnableRef| Origin::Spawned { by: by.clone(), run: Some(RunId::new("run-1423")), session: None };

    let payments_lead = AgentDef {
        skills: SkillPolicy { forced: vec!["payments-retries".into()], auto: vec!["jira-hygiene".into()] },
        spawn: SpawnPolicy { enabled: true, alive_at_once: 3, max_depth: 2, per_run: 3, budget_usd: Some(0.5) },
        team: vec![
            RunnableRef::agent("fixer"),
            RunnableRef::agent("codex"),
            RunnableRef::agent("reproducer"),
            RunnableRef::pipeline("pay-triage"),
        ],
        routing: "Splits the issue, hands each part to the cheapest teammate with the right skill, and creates a helper only when nobody fits. Asks you when two plans disagree.".into(),
        ..def("Lead payments work across the team.", model("opus-5.5"), "cloud", ToolAccess::ReadOnly)
    };
    let release_captain = AgentDef {
        team: vec![RunnableRef::pipeline("nightly-audit"), RunnableRef::pipeline("release-notes"), RunnableRef::agent("analyst")],
        routing: "Runs the checklist in order and stops at the first red item.".into(),
        ..def("Run the release checklist.", model("sonnet-5.5"), "cloud", ToolAccess::ReadOnly)
    };
    let fixer_def = AgentDef {
        skills: SkillPolicy { forced: vec!["write-failing-test".into()], auto: Vec::new() },
        spawn: SpawnPolicy { enabled: true, max_depth: 2, per_run: 2, ..SpawnPolicy::default() },
        budget_usd: Some(0.6),
        ..def("Fix bugs. Write the failing test first.", model("opus-5.5"), "mac-mini", ToolAccess::Full)
    };

    let rows = vec![
        Row { reference: lead.clone(), kind: Orchestrator, name: "Payments lead", description: "Splits payments work across the team and creates helpers when nobody fits.",
            origin: library(), uses: "CIP", agent: Some(payments_lead), state: RunnableState::Running { issue: issue("PAY-1436") }, runs_today: 14, cost_today: 1.12, queued: 0 },
        Row { reference: RunnableRef::orchestrator("release-captain"), kind: Orchestrator, name: "Release captain", description: "Runs the release checklist: audit, notes, tag. Never writes code.",
            origin: library(), uses: "CIA", agent: Some(release_captain), state: RunnableState::Idle, runs_today: 1, cost_today: 0.09, queued: 0 },
        Row { reference: RunnableRef::pipeline("pay-triage"), kind: Pipeline, name: "PAY triage", description: "Router › Fixer · Reproducer · Analyst › Review. Returns variants for a person to pick.",
            origin: Origin::Project { repo: path("code/acme/payments-api") }, uses: "CIPA", agent: None, state: RunnableState::Live { version: 14 }, runs_today: 22, cost_today: 0.94, queued: 37 },
        Row { reference: RunnableRef::pipeline("nightly-audit"), kind: Pipeline, name: "Nightly audit", description: "Auditor ×2 › Report. Checks log redaction and dependency licences.",
            origin: library(), uses: "CIPA", agent: None, state: RunnableState::Live { version: 3 }, runs_today: 1, cost_today: 0.12, queued: 0 },
        Row { reference: RunnableRef::pipeline("release-notes"), kind: Pipeline, name: "Release notes", description: "Collects merged PRs and drafts notes for review.",
            origin: library(), uses: "IP", agent: None, state: RunnableState::Draft { version: 1 }, runs_today: 0, cost_today: 0.0, queued: 0 },
        Row { reference: RunnableRef::agent("build"), kind: Agent, name: "Build", description: "General coding agent with full tools. The default in chat.",
            origin: Origin::Builtin, uses: "CIP", agent: Some(AgentDef { spawn: SpawnPolicy { enabled: true, ..SpawnPolicy::default() }, ..def("General coding agent.", model("opus-5.5"), "mac-mini", ToolAccess::Full) }), state: RunnableState::Ready, runs_today: 9, cost_today: 0.62, queued: 0 },
        Row { reference: fixer.clone(), kind: Agent, name: "Fixer", description: "Fixes bugs and writes the failing test first. May spawn Test writer.",
            origin: library(), uses: "CIP", agent: Some(fixer_def), state: RunnableState::Running { issue: issue("PAY-1423") }, runs_today: 6, cost_today: 0.41, queued: 0 },
        Row { reference: RunnableRef::agent("reproducer"), kind: Agent, name: "Reproducer", description: "Finds a minimal reproduction and attaches logs. Never edits code.",
            origin: library(), uses: "CIP", agent: Some(def("Reproduce, never fix.", model("sonnet-5.5"), "cloud", ToolAccess::ReadAndRun)), state: RunnableState::Idle, runs_today: 4, cost_today: 0.10, queued: 0 },
        Row { reference: RunnableRef::agent("analyst"), kind: Agent, name: "Analyst", description: "Turns stories into specs with acceptance criteria.",
            origin: library(), uses: "CIP", agent: Some(def("Write specs.", model("sonnet-5.5"), "cloud", ToolAccess::ReadOnly)), state: RunnableState::Idle, runs_today: 2, cost_today: 0.08, queued: 0 },
        Row { reference: RunnableRef::agent("codex"), kind: Agent, name: "Codex", description: "Dependency bumps and mechanical refactors.",
            origin: library(), uses: "CIP", agent: Some(def("Bump and refactor.", ModelRef::new("openai", "gpt-5.2"), "laptop", ToolAccess::Full)), state: RunnableState::Queued { count: 1 }, runs_today: 2, cost_today: 0.18, queued: 1 },
        Row { reference: RunnableRef::agent("auditor"), kind: Agent, name: "Auditor", description: "Read-only security checks. Reports, never fixes.",
            origin: library(), uses: "CIPA", agent: Some(def("Audit, report, never fix.", model("opus-5.5"), "cloud", ToolAccess::ReadOnly)), state: RunnableState::Waiting { issue: issue("PAY-1435") }, runs_today: 1, cost_today: 0.15, queued: 0 },
        Row { reference: RunnableRef::spawned("migration-checker"), kind: Agent, name: "Migration checker", description: "Reads the gateway SDK v9 diff and lists breaking changes for Codex.",
            origin: spawned(&lead), uses: "C", agent: Some(def("List breaking changes.", model("sonnet-5.5"), "cloud", ToolAccess::ReadOnly)), state: RunnableState::Alive { issue: issue("PAY-1438") }, runs_today: 1, cost_today: 0.08, queued: 0 },
        Row { reference: RunnableRef::spawned("sdk-diff-reader"), kind: Agent, name: "SDK diff reader", description: "Compared webhook payloads between SDK versions.",
            origin: spawned(&lead), uses: "", agent: Some(def("Compare payloads.", model("haiku-5.5"), "cloud", ToolAccess::ReadOnly)), state: RunnableState::Ended { times: 1 }, runs_today: 1, cost_today: 0.03, queued: 0 },
        Row { reference: RunnableRef::spawned("test-writer"), kind: Agent, name: "Test writer", description: "Writes one failing test that pins a bug.",
            origin: spawned(&fixer), uses: "P", agent: Some(def("Write one failing test.", model("sonnet-5.5"), "cloud", ToolAccess::ReadAndRun)), state: RunnableState::Ended { times: 2 }, runs_today: 2, cost_today: 0.04, queued: 0 },
    ];
    for row in rows {
        world.activity.push((
            row.reference.clone(),
            RunnableActivity { state: row.state, runs_today: row.runs_today, cost_today_usd: row.cost_today, queued: row.queued },
        ));
        world.runnables.push(Runnable {
            reference: row.reference,
            kind: row.kind,
            name: row.name.to_string(),
            description: row.description.to_string(),
            origin: row.origin,
            uses: uses(row.uses),
            paused: false,
            archived: false,
            agent: row.agent,
            path: None,
        });
    }
    let skill = |name: &str, description: &str, source: SkillSource, tokens: u64| Skill {
        name: name.to_string(),
        description: description.to_string(),
        path: path(&format!(".config/harness/skills/{name}")),
        source,
        tokens,
    };
    world.skills = vec![
        skill("payments-retries", "Idempotency keys, retry budgets, gateway timeouts", SkillSource::Project, 3_200),
        skill("write-failing-test", "Pin a bug with one failing test before fixing it", SkillSource::Project, 1_800),
        skill("jira-hygiene", "Titles, labels and links for Jira issues", SkillSource::User, 900),
        skill("release-checklist", "Audit, notes, tag: the release in order", SkillSource::User, 1_400),
    ];
}
