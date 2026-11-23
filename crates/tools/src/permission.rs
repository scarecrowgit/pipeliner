//! Permission rules: whether a tool call runs, asks a person, or is refused, and what "Always
//! allow" adds. The agent loop and the UI both use these, so a rule means the same everywhere.
//!
//! File tools are matched on the path the call really touches (see [`crate::Tool::subject`]),
//! as a path prefix.
//!
//! Commands are read by a small shell lexer (`shell.rs`). An Allow prefix (`bun test`)
//! covers only a single simple command whose leading words are exactly the prefix: never a
//! command that chains, pipes, redirects, substitutes or uses syntax the lexer does not model
//! exactly, and never a command that starts with a shell, an interpreter running inline code,
//! a wrapper such as `env`, `sudo`, `xargs` or `find`, or a destructive command such as `rm`.
//! Deny and Ask prefixes look into every part of a command: quotes are removed, a path in front
//! of the command word is dropped, wrappers are unwrapped, and `$(…)`, backticks, subshells,
//! groups and `sh -c` strings are read in turn; a command that cannot be decoded matches them.
//! They may over-match. They are still best-effort against a writer who means to get around
//! them: `bash` itself is not sandboxed, and a script or program it runs can do anything.

use std::path::Path;

use model::{PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};

use crate::sandbox::normalize_subject;
use crate::shell;

/// What happens to a call of `tool` (of `class`) on `subject` (see [`crate::Tool::subject`]).
///
/// A matching Deny rule in any scope refuses the call. Otherwise the matching rules of the
/// most specific scope decide (Session, then Repo, then Global), Ask over Allow. With no
/// matching rule, `Read` and `Internal` calls are allowed and the rest ask.
pub fn decide(
    rules: &[PermissionRule],
    tool: &str,
    class: ToolClass,
    subject: &str,
    session: &SessionId,
    repo: Option<&Path>,
) -> RuleDecision {
    let matching: Vec<&PermissionRule> = rules
        .iter()
        .filter(|rule| rule.tool == tool && scope_applies(&rule.scope, session, repo))
        .filter(|rule| matches(rule, class, subject))
        .collect();
    if matching.iter().any(|rule| rule.decision == RuleDecision::Deny) {
        return RuleDecision::Deny;
    }
    matching
        .iter()
        .max_by_key(|rule| (scope_rank(&rule.scope), rule.decision == RuleDecision::Ask))
        .map(|rule| rule.decision)
        .unwrap_or_else(|| default_decision(class))
}

/// What a call of `class` gets when no rule matches.
pub fn default_decision(class: ToolClass) -> RuleDecision {
    match class {
        ToolClass::Read | ToolClass::Internal => RuleDecision::Allow,
        ToolClass::Write | ToolClass::Execute | ToolClass::External => RuleDecision::Ask,
    }
}

/// The rule "Always allow" should add for this call, or `None` when no rule would be safe.
///
/// For commands: the leading words, flags included, up to the first quoted, expanded,
/// path-like or value argument (`bun test src/charges` gives `bun test`, `git push --force`
/// stays `git push --force`), so the rule reads as what the person saw. `None` for commands
/// that chain, pipe, redirect or substitute, for shells, wrappers and interpreters running
/// inline code (`bash -c`, `env`, `sudo`, `find`, `xargs`, `nix-shell`, `python -c`…), and for
/// destructive commands (`rm`, `dd`, `chmod`, `kill`…). Other tools get a rule for the whole
/// tool (an empty pattern).
pub fn always_rule(tool: &str, class: ToolClass, subject: &str, scope: RuleScope) -> Option<PermissionRule> {
    let pattern = if class == ToolClass::Execute { shell::command_prefix(subject)? } else { String::new() };
    Some(PermissionRule { tool: tool.to_string(), pattern, decision: RuleDecision::Allow, scope })
}

/// Why a call asks or is refused, in policy words: `bash = ask`.
pub fn policy_label(tool: &str, decision: RuleDecision) -> String {
    let word = match decision {
        RuleDecision::Allow => "allow",
        RuleDecision::Ask => "ask",
        RuleDecision::Deny => "deny",
    };
    format!("{tool} = {word}")
}

fn scope_applies(scope: &RuleScope, session: &SessionId, repo: Option<&Path>) -> bool {
    match scope {
        RuleScope::Global => true,
        RuleScope::Repo { path } => repo.is_some_and(|r| same_dir(r, path)),
        RuleScope::Session { session: s } => s == session,
    }
}

/// Paths compared in canonical form, or literally when one cannot be canonicalised.
fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b)
}

fn scope_rank(scope: &RuleScope) -> u8 {
    match scope {
        RuleScope::Global => 0,
        RuleScope::Repo { .. } => 1,
        RuleScope::Session { .. } => 2,
    }
}

fn matches(rule: &PermissionRule, class: ToolClass, subject: &str) -> bool {
    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    match class {
        ToolClass::Read | ToolClass::Write => path_prefix(pattern, subject),
        ToolClass::Execute if rule.decision == RuleDecision::Allow => shell::allow_matches(pattern, subject),
        ToolClass::Execute => shell::restrictive_matches(pattern, subject),
        ToolClass::External | ToolClass::Internal => {
            let have: Vec<&str> = subject.split_whitespace().collect();
            let want: Vec<&str> = pattern.split_whitespace().collect();
            have.starts_with(&want)
        }
    }
}

/// `src` covers `src` and `src/x.rs`, not `src2/x.rs`; `.` covers every path inside. An empty
/// subject (no path, or one the sandbox refused) matches no path pattern.
fn path_prefix(pattern: &str, subject: &str) -> bool {
    if subject.trim().is_empty() {
        return false;
    }
    let pattern = normalize_subject(pattern);
    let subject = normalize_subject(subject);
    let inside = !Path::new(&subject).is_absolute() && subject != ".." && !subject.starts_with("../");
    if !inside {
        return false;
    }
    pattern == "." || subject == pattern || subject.strip_prefix(&pattern).is_some_and(|rest| rest.starts_with('/'))
}
