//! Permission rules: whether a tool call runs, asks a person, or is refused, and what "Always
//! allow" adds. The agent loop and the UI both use these, so a rule means the same everywhere.
//!
//! Commands are matched word by word. A prefix rule that allows (`bun test`) never covers a
//! command that chains, pipes, redirects or substitutes (`bun test && curl …`), because the
//! person who allowed `bun test` did not see the rest; a rule that denies or asks applies when
//! any part of such a command starts with it.

use std::path::Path;

use model::{PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};

/// What happens to a call of `tool` (of `class`) on `subject` (see [`crate::Tool::subject`]).
///
/// The rules that match and apply here are ranked by scope (Session, then Repo, then Global)
/// and within a scope by decision (Deny, then Ask, then Allow); the first wins. With no
/// matching rule, `Read` and `Internal` calls are allowed and the rest ask.
pub fn decide(
    rules: &[PermissionRule],
    tool: &str,
    class: ToolClass,
    subject: &str,
    session: &SessionId,
    repo: Option<&Path>,
) -> RuleDecision {
    rules
        .iter()
        .filter(|rule| rule.tool == tool && scope_applies(&rule.scope, session, repo))
        .filter(|rule| matches(rule, class, subject))
        .max_by_key(|rule| (scope_rank(&rule.scope), severity(rule.decision)))
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
/// For commands: the leading words up to the first argument that looks like a path or a value
/// (`bun test src/charges` gives `bun test`). A flag ends the prefix too, unless a quoted value
/// follows it (`nix-shell shell.nix --run '…'` gives `nix-shell shell.nix --run`). Commands
/// that chain, pipe, redirect or substitute get `None`. Other tools get a rule for the whole
/// tool (an empty pattern).
pub fn always_rule(tool: &str, class: ToolClass, subject: &str, scope: RuleScope) -> Option<PermissionRule> {
    let pattern = if class == ToolClass::Execute { command_prefix(subject)? } else { String::new() };
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
        RuleScope::Repo { path } => repo.is_some_and(|r| r == path.as_path()),
        RuleScope::Session { session: s } => s == session,
    }
}

fn scope_rank(scope: &RuleScope) -> u8 {
    match scope {
        RuleScope::Global => 0,
        RuleScope::Repo { .. } => 1,
        RuleScope::Session { .. } => 2,
    }
}

fn severity(decision: RuleDecision) -> u8 {
    match decision {
        RuleDecision::Allow => 0,
        RuleDecision::Ask => 1,
        RuleDecision::Deny => 2,
    }
}

fn matches(rule: &PermissionRule, class: ToolClass, subject: &str) -> bool {
    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    match class {
        ToolClass::Read | ToolClass::Write => path_prefix(pattern, subject),
        ToolClass::Execute => command_matches(pattern, subject, rule.decision),
        ToolClass::External | ToolClass::Internal => {
            starts_with_words(&words(subject), &pattern.split_whitespace().collect::<Vec<_>>())
        }
    }
}

/// `src` covers `src` and `src/x.rs`, not `src2/x.rs`; `.` covers every path inside.
fn path_prefix(pattern: &str, subject: &str) -> bool {
    let pattern = crate::sandbox::normalize_subject(pattern);
    let subject = crate::sandbox::normalize_subject(subject);
    let inside = !Path::new(&subject).is_absolute() && subject != ".." && !subject.starts_with("../");
    if pattern == "." {
        return inside;
    }
    subject == pattern || subject.strip_prefix(&pattern).is_some_and(|rest| rest.starts_with('/'))
}

fn command_matches(pattern: &str, subject: &str, decision: RuleDecision) -> bool {
    let want: Vec<&str> = pattern.split_whitespace().collect();
    let Some(parsed) = Parsed::new(subject) else {
        // Unbalanced quotes: never allowed by a prefix, but a deny or ask still catches it.
        return decision != RuleDecision::Allow && starts_with_words(&words(subject), &want);
    };
    if decision == RuleDecision::Allow {
        return !parsed.compound && parsed.segments.len() == 1 && starts_with_words(&parsed.segments[0], &want);
    }
    parsed.segments.iter().any(|seg| {
        let without_env: Vec<&str> = seg.iter().map(String::as_str).skip_while(|w| is_assignment(w)).collect();
        starts_with_words(seg, &want) || without_env.starts_with(&want)
    })
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn starts_with_words(have: &[String], want: &[&str]) -> bool {
    have.len() >= want.len() && have.iter().zip(want).all(|(h, w)| h == w)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The "Always allow" prefix of a command; see [`always_rule`].
fn command_prefix(command: &str) -> Option<String> {
    let parsed = Parsed::new(command)?;
    if parsed.compound || parsed.segments.len() != 1 {
        return None;
    }
    let words = &parsed.segments[0];
    let first = words.first()?;
    if is_assignment(first) || is_quoted(first) {
        return None;
    }
    let mut keep = vec![first.as_str()];
    for (i, word) in words.iter().enumerate().skip(1) {
        if word.starts_with('-') {
            if words.get(i + 1).is_some_and(|next| is_quoted(next)) {
                keep.push(word);
            }
            break;
        }
        if looks_like_value(word) {
            break;
        }
        keep.push(word);
    }
    Some(keep.join(" "))
}

fn is_quoted(word: &str) -> bool {
    word.starts_with('\'') || word.starts_with('"')
}

/// Paths, globs, variables, assignments, numbers and quoted text: the part of a command that
/// changes from call to call.
fn looks_like_value(word: &str) -> bool {
    is_quoted(word)
        || word.starts_with('.')
        || word.contains(['/', '=', '$', '*', '?', '[', '~', ':', '\\', '@', '%', '#'])
        || word.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// A command split into simple commands. Quotes are kept in the words as written.
struct Parsed {
    segments: Vec<Vec<String>>,
    /// The command chains, pipes, redirects, runs in the background or substitutes.
    compound: bool,
}

impl Parsed {
    /// `None` when a quote is left open.
    fn new(command: &str) -> Option<Self> {
        let mut segments = vec![Vec::new()];
        let mut word = String::new();
        let mut compound = false;
        let mut quote: Option<char> = None;
        let mut chars = command.chars().peekable();
        let end_word = |word: &mut String, segments: &mut Vec<Vec<String>>| {
            if !word.is_empty()
                && let Some(seg) = segments.last_mut()
            {
                seg.push(std::mem::take(word));
            }
        };
        while let Some(c) = chars.next() {
            match quote {
                Some('\'') => {
                    word.push(c);
                    if c == '\'' {
                        quote = None;
                    }
                }
                Some(_) => {
                    word.push(c);
                    match c {
                        '"' => quote = None,
                        '\\' => word.extend(chars.next()),
                        '`' => compound = true,
                        '$' if chars.peek() == Some(&'(') => compound = true,
                        _ => {}
                    }
                }
                None => match c {
                    '\'' | '"' => {
                        quote = Some(c);
                        word.push(c);
                    }
                    '\\' => {
                        word.push(c);
                        word.extend(chars.next());
                    }
                    '$' if chars.peek() == Some(&'(') => {
                        compound = true;
                        word.push(c);
                    }
                    '`' | '>' | '<' | '(' | ')' => {
                        compound = true;
                        word.push(c);
                    }
                    ';' | '&' | '|' | '\n' => {
                        compound = true;
                        end_word(&mut word, &mut segments);
                        while chars.peek().is_some_and(|n| matches!(n, ';' | '&' | '|')) {
                            chars.next();
                        }
                        segments.push(Vec::new());
                    }
                    c if c.is_whitespace() => end_word(&mut word, &mut segments),
                    c => word.push(c),
                },
            }
        }
        if quote.is_some() {
            return None;
        }
        end_word(&mut word, &mut segments);
        segments.retain(|s| !s.is_empty());
        Some(Self { segments, compound })
    }
}
