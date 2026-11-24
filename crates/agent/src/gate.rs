//! The permission gate: whether a tool call runs, is refused, or waits for a person.

use std::path::Path;

use base::cancel::CancelToken;
use model::{PermissionDecision, PermissionRequest, PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};
use tools::permission::{decide, policy_label};

use crate::sink::lock;

/// A tool call as the gate sees it.
#[derive(Clone, Copy, Debug)]
pub struct GateCall<'a> {
    pub session: &'a SessionId,
    pub tool: &'a str,
    pub class: ToolClass,
    /// What rules match against: the command, or the path the call touches.
    pub subject: &'a str,
    /// The checkout repo-scoped rules apply to.
    pub repo: Option<&'a Path>,
    /// An MCP tool whose server config lets it run without asking.
    pub auto_approved: bool,
}

/// What [`PermissionGate::check`] decides without a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Refused; the model reads the reason.
    Deny { reason: String },
    /// A person decides.
    Ask,
}

/// Decides about tool calls. The loop calls `check` before every built-in, extra and MCP tool
/// call, and `ask` when the verdict is [`Verdict::Ask`] or a sub-agent would be created deeper
/// than the spawn policy allows.
///
/// Sub-agents use the same gate from their own threads, several at once, each with its own
/// session id in the call and in the request.
pub trait PermissionGate: Send + Sync {
    /// Decides without a person, and without blocking.
    fn check(&self, call: &GateCall<'_>) -> Verdict;

    /// Blocks until a person answers. `None` means nobody will: the token was cancelled. It
    /// must return within a fraction of a second of the cancel, because the run cannot stop
    /// before it does. An approval that arrives after the cancel is not acted on.
    fn ask(&self, request: PermissionRequest, cancel: &CancelToken) -> Option<PermissionDecision>;
}

/// A gate that lets everything run, for tests and trusted headless runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAll;

impl PermissionGate for AllowAll {
    fn check(&self, _call: &GateCall<'_>) -> Verdict {
        Verdict::Allow
    }

    fn ask(&self, _request: PermissionRequest, _cancel: &CancelToken) -> Option<PermissionDecision> {
        Some(PermissionDecision::AllowOnce)
    }
}

/// Where permission rules live. The engine implements it over the store.
pub trait RuleStore: Send + Sync {
    /// Every rule, of every scope.
    fn rules(&self) -> Vec<PermissionRule>;

    /// Keeps a rule a person chose with "Always allow".
    fn add(&self, rule: PermissionRule);
}

/// Rules kept in memory.
#[derive(Debug, Default)]
pub struct MemoryRules(std::sync::Mutex<Vec<PermissionRule>>);

impl MemoryRules {
    /// A store that starts with `rules`.
    pub fn new(rules: Vec<PermissionRule>) -> Self {
        Self(std::sync::Mutex::new(rules))
    }
}

impl RuleStore for MemoryRules {
    fn rules(&self) -> Vec<PermissionRule> {
        lock(&self.0).clone()
    }

    fn add(&self, rule: PermissionRule) {
        let mut rules = lock(&self.0);
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
}

/// How [`RuleGate`] reaches a person: called with the request, returns the answer, or `None`
/// once the token is cancelled. It must return soon after a cancel.
pub type AskFn<'a> = dyn Fn(PermissionRequest, &CancelToken) -> Option<PermissionDecision> + Send + Sync + 'a;

/// The gate the app runs with: rules decide (see [`tools::permission::decide`]), and what they
/// leave open goes to a person through the callback. An "Always allow" answer is added to the
/// rule store, so the same call does not ask again.
pub struct RuleGate<'a> {
    rules: &'a dyn RuleStore,
    ask: Box<AskFn<'a>>,
}

impl<'a> RuleGate<'a> {
    /// A gate over `rules` that reaches a person through `ask` (see [`AskFn`]).
    pub fn new(
        rules: &'a dyn RuleStore,
        ask: impl Fn(PermissionRequest, &CancelToken) -> Option<PermissionDecision> + Send + Sync + 'a,
    ) -> Self {
        Self { rules, ask: Box::new(ask) }
    }
}

impl PermissionGate for RuleGate<'_> {
    fn check(&self, call: &GateCall<'_>) -> Verdict {
        let mut rules = self.rules.rules();
        if call.auto_approved {
            // The server config's approval is the weakest rule: any rule a person made wins.
            rules.push(PermissionRule {
                tool: call.tool.to_string(),
                pattern: String::new(),
                decision: RuleDecision::Allow,
                scope: RuleScope::Global,
            });
        }
        match decide(&rules, call.tool, call.class, call.subject, call.session, call.repo) {
            RuleDecision::Allow => Verdict::Allow,
            RuleDecision::Ask => Verdict::Ask,
            RuleDecision::Deny => Verdict::Deny {
                reason: format!("a permission rule forbids it ({})", policy_label(call.tool, RuleDecision::Deny)),
            },
        }
    }

    fn ask(&self, request: PermissionRequest, cancel: &CancelToken) -> Option<PermissionDecision> {
        if cancel.is_cancelled() {
            return None;
        }
        let decision = (self.ask)(request, cancel)?;
        // An answer that crossed the cancel is nobody's answer: it allows nothing and adds no rule.
        if cancel.is_cancelled() {
            return None;
        }
        if let PermissionDecision::AllowAlways { rule } = &decision {
            self.rules.add(rule.clone());
        }
        Some(decision)
    }
}
