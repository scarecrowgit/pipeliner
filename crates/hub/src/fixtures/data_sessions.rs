//! Chats of the Acme Payments world: the sidebar's pinned and recent sessions, their sub-agents,
//! context panels and the pending permission request.

use std::path::PathBuf;

use crate::api::{
    ChatMode, FileChange, IssueKey, McpServerInfo, McpState, ModelRef, PermissionRequest, PermissionRule, RepoFacts,
    RepoHost, RequestId, RuleDecision, RuleScope, RunnableRef, Session, SessionId, SessionOrigin, SessionState,
    SkillInContext, TodoItem, TodoStatus, ToolClass, ToolDisplay, Usage, WorkMode, Workdir,
};

use super::{DAY, Extras, HOUR, MINUTE, World};

pub const FIX_1423: &str = "chat-1423";
pub const LEDGER: &str = "chat-ledger";
pub const HOME: &str = "/home/me";

pub fn path(rest: &str) -> PathBuf {
    PathBuf::from(format!("{HOME}/{rest}"))
}

pub fn model(name: &str) -> ModelRef {
    ModelRef::new("anthropic", name)
}

/// A top-level chat with the defaults of this world.
pub fn chat(id: &str, title: &str, runnable: RunnableRef, state: SessionState, created: i64, updated: i64) -> Session {
    Session {
        id: SessionId::new(id),
        title: title.to_string(),
        parent: None,
        depth: 0,
        runnable,
        mode: ChatMode::Build,
        model: model("opus-5.5"),
        workdir: Some(checkout("payments-api", "main")),
        state,
        origin: SessionOrigin::Me,
        issue: None,
        pinned: false,
        archived: false,
        created,
        updated,
        message_count: 2,
        cost_usd: 0.0,
        usage: Usage::default(),
        context_tokens: 0,
        context_window: 200_000,
        forced_skills: Vec::new(),
        pr_url: None,
    }
}

pub fn checkout(repo: &str, branch: &str) -> Workdir {
    Workdir {
        path: path(&format!("code/acme/{repo}")),
        repo: repo.to_string(),
        branch: Some(branch.to_string()),
        host: if repo == "checkout-web" { RepoHost::GitLab } else { RepoHost::GitHub },
        mode: WorkMode::InPlace,
        source: None,
        runtime: "mac-mini".to_string(),
    }
}

pub fn worktree(repo: &str, branch: &str, folder: &str) -> Workdir {
    Workdir {
        path: path(&format!(".harness/worktrees/{repo}/{folder}")),
        mode: WorkMode::Worktree,
        source: Some(path(&format!("code/acme/{repo}"))),
        ..checkout(repo, branch)
    }
}

fn child(parent: &Session, id: &str, title: &str, agent: &str, model_name: &str, cost: f64, state: SessionState) -> Session {
    Session {
        parent: Some(parent.id.clone()),
        depth: 1,
        runnable: RunnableRef::agent(agent),
        model: model(model_name),
        origin: SessionOrigin::Agent { parent: parent.id.clone() },
        cost_usd: cost,
        message_count: 6,
        ..chat(id, title, RunnableRef::agent(agent), state, parent.created + 20 * MINUTE, parent.created + 40 * MINUTE)
    }
}

pub fn fill(world: &mut World) {
    let now = world.now;
    let build = RunnableRef::agent("build");

    let fix = Session {
        pinned: true,
        issue: Some(IssueKey::new("PAY-1423")),
        workdir: Some(worktree("payments-api", "fix/PAY-1423", "fix-PAY-1423")),
        message_count: 10,
        cost_usd: 0.21,
        usage: Usage { input: 31_200, output: 4_100, cache_read: 160_000, cache_write: 12_000 },
        context_tokens: 48_200,
        forced_skills: vec!["payments-retries".to_string()],
        ..chat(FIX_1423, "Fix double charge on retry", build.clone(), SessionState::Waiting, now - 2 * HOUR, now - 12 * MINUTE)
    };
    let split = Session {
        pinned: true,
        mode: ChatMode::Plan,
        workdir: Some(Workdir { mode: WorkMode::ReadOnly, ..checkout("checkout-web", "main") }),
        message_count: 6,
        cost_usd: 0.34,
        context_tokens: 22_400,
        ..chat("chat-split-checkout", "Plan: split checkout service", build.clone(), SessionState::Idle, now - 4 * DAY - HOUR, now - 4 * DAY)
    };
    let refund = Session {
        workdir: Some(worktree("payments-api", "chat/refund-timeout", "chat-refund-timeout")),
        message_count: 4,
        cost_usd: 0.12,
        context_tokens: 31_000,
        issue: Some(IssueKey::new("PAY-1427")),
        ..chat("chat-refund-timeout", "Refund webhook timeout", build.clone(), SessionState::Running, now - 50 * MINUTE, now - 20 * MINUTE)
    };
    let budget = Session {
        workdir: Some(worktree("payments-api", "pay-1436-retry-budget", "pay-1436")),
        model: model("opus-5.5"),
        issue: Some(IssueKey::new("PAY-1436")),
        message_count: 8,
        cost_usd: 0.44,
        context_tokens: 64_000,
        ..chat(
            "chat-retry-budget",
            "Retry budget · split plan",
            RunnableRef::orchestrator("payments-lead"),
            SessionState::Running,
            now - 90 * MINUTE,
            now - 31 * MINUTE,
        )
    };
    let flake = Session {
        workdir: Some(checkout("checkout-web", "feat/saved-cards")),
        cost_usd: 0.09,
        ..chat("chat-checkout-flake", "Why does checkout.spec flake?", build.clone(), SessionState::Done, now - 2 * HOUR, now - HOUR)
    };
    let sdk = Session {
        runnable: RunnableRef::agent("codex"),
        model: ModelRef::new("openai", "gpt-5.2"),
        cost_usd: 0.18,
        issue: Some(IssueKey::new("PAY-1438")),
        pr_url: Some("https://github.com/acme/payments-api/pull/2291".to_string()),
        ..chat("chat-sdk-v9", "Bump gateway SDK to v9", RunnableRef::agent("codex"), SessionState::Done, now - 3 * HOUR, now - 2 * HOUR)
    };
    let pci = Session {
        runnable: RunnableRef::agent("auditor"),
        workdir: Some(Workdir { mode: WorkMode::ReadOnly, ..checkout("payments-api", "main") }),
        cost_usd: 0.07,
        ..chat("chat-pci-audit", "PCI log redaction audit", RunnableRef::agent("auditor"), SessionState::Failed, now - 2 * DAY - HOUR, now - 2 * DAY)
    };
    let ledger = Session {
        workdir: Some(Workdir { mode: WorkMode::ReadOnly, ..checkout("ledger", "main") }),
        message_count: 96,
        cost_usd: 0.27,
        context_tokens: 88_000,
        ..chat(LEDGER, "Explain ledger reconciliation", build.clone(), SessionState::Done, now - 2 * DAY - 4 * HOUR, now - 2 * DAY - 3 * HOUR)
    };

    let explore = child(&fix, "chat-1423-explore", "Find every caller of retryCharge", "explore", "haiku-5.5", 0.01, SessionState::Done);
    let general = child(&fix, "chat-1423-general", "Read the gateway idempotency docs", "general", "sonnet-5.5", 0.02, SessionState::Done);
    let spec = child(&budget, "chat-1436-spec", "Analyst spec for retry budgets", "analyst", "sonnet-5.5", 0.11, SessionState::Done);
    let code = child(&budget, "chat-1436-code", "Fixer code for retry budgets", "fixer", "opus-5.5", 0.19, SessionState::Running);
    let migration =
        child(&budget, "chat-1436-migration", "Read SDK v9 diff for Codex", "migration-checker", "sonnet-5.5", 0.08, SessionState::Running);

    world.extras.insert(fix.id.clone(), fix_extras());
    world.permissions.push(PermissionRequest {
        id: RequestId::new("perm-1423-bun-test"),
        session: fix.id.clone(),
        agent: "Build".to_string(),
        tool: "bash".to_string(),
        call_id: Some("t8".to_string()),
        class: ToolClass::Execute,
        display: ToolDisplay::Command { command: "bun test src/charges".to_string(), output: String::new(), exit_code: None },
        policy: "bash = ask".to_string(),
        always_rule: Some(PermissionRule {
            tool: "bash".to_string(),
            pattern: "bun test".to_string(),
            decision: RuleDecision::Allow,
            scope: RuleScope::Repo { path: path("code/acme/payments-api") },
        }),
        at: now - 12 * MINUTE,
    });
    world.rules = vec![
        PermissionRule { tool: "bash".into(), pattern: "git status".into(), decision: RuleDecision::Allow, scope: RuleScope::Global },
        PermissionRule { tool: "bash".into(), pattern: "rm ".into(), decision: RuleDecision::Deny, scope: RuleScope::Global },
        PermissionRule {
            tool: "bash".into(),
            pattern: "bun run lint".into(),
            decision: RuleDecision::Allow,
            scope: RuleScope::Repo { path: path("code/acme/payments-api") },
        },
    ];
    world.sessions = vec![fix, split, refund, budget, flake, sdk, pci, ledger, explore, general, spec, code, migration];
}

fn fix_extras() -> Extras {
    let todo = |text: &str, done: bool| TodoItem {
        text: text.to_string(),
        status: if done { TodoStatus::Done } else { TodoStatus::Pending },
    };
    Extras {
        todos: vec![
            todo("Find every caller of retryCharge", true),
            todo("Add an idempotency key to retries", true),
            todo("Write a test that fails on main", true),
            todo("Run the charges test suite", false),
        ],
        files: vec![
            FileChange { path: "src/charges/retry.ts".to_string(), added: 3, removed: 2 },
            FileChange { path: "src/charges/retry.test.ts".to_string(), added: 12, removed: 0 },
        ],
        skills: vec![
            SkillInContext { name: "payments-retries".to_string(), forced: true, loaded: true, tokens: 3_200 },
            SkillInContext { name: "write-failing-test".to_string(), forced: false, loaded: true, tokens: 1_800 },
        ],
        mcp: vec![
            McpServerInfo { name: "jira".into(), state: McpState::Ready, tools: 12, error: None, source: "config.toml".into() },
            McpServerInfo { name: "github".into(), state: McpState::Ready, tools: 31, error: None, source: "config.toml".into() },
            McpServerInfo {
                name: "sentry".into(),
                state: McpState::Failed,
                tools: 0,
                error: Some("token expired".into()),
                source: ".mcp.json".into(),
            },
        ],
        repo: Some(repo_facts(
            ".harness/worktrees/payments-api/fix-PAY-1423",
            "fix/PAY-1423",
            2,
            1,
            true,
        )),
        queued: Vec::new(),
    }
}

/// Git facts of a payments-api checkout or worktree.
pub fn repo_facts(folder: &str, branch: &str, dirty: u32, ahead: u32, is_worktree: bool) -> RepoFacts {
    RepoFacts {
        root: path(folder),
        name: "payments-api".to_string(),
        branch: Some(branch.to_string()),
        head: Some("9f2c4e1b7a3d5c8e0f1a2b3c4d5e6f708192a3b4".to_string()),
        host: RepoHost::GitHub,
        remote_url: Some("https://github.com/acme/payments-api".to_string()),
        dirty,
        ahead,
        behind: 0,
        is_worktree,
        main_checkout: path("code/acme/payments-api"),
        default_branch: Some("main".to_string()),
        compared_to: Some("main".to_string()),
    }
}
