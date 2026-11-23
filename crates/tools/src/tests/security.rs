//! Regression tests for the path side of the security review (items 4 to 9).

use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::{Duration, Instant};

use model::RuleDecision::{Allow, Ask, Deny};
use model::{PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};
use serde_json::json;
use tempfile::TempDir;

use super::Fixture;
use crate::api::{Tool, ToolOutput};
use crate::permission::decide;
use crate::sandbox::{OpenError, normalize_subject, open_regular, resolve, write_regular};
use crate::workspace::{context_summary, list_files, project_notes};
use crate::{EditFile, Grep, ListDir, ReadFile, WriteFile};

fn rule(tool: &str, pattern: &str, decision: RuleDecision) -> PermissionRule {
    PermissionRule { tool: tool.into(), pattern: pattern.into(), decision, scope: RuleScope::Global }
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo").arg(path).status().expect("mkfifo");
    assert!(status.success());
}

/// Runs a tool call on another thread and fails the test if it hangs.
fn run_bounded(f: &Fixture, tool: &dyn Tool, input: serde_json::Value) -> ToolOutput {
    let started = Instant::now();
    let out = f.run(tool, input);
    assert!(started.elapsed() < Duration::from_secs(5));
    out
}

#[test]
fn file_subjects_are_the_resolved_path() {
    let f = Fixture::new();
    f.write(".env", "SECRET=1\n");
    f.write("src/a.rs", "");
    std::fs::create_dir_all(f.path(".git/hooks")).unwrap();
    std::fs::create_dir(f.path("docs")).unwrap();
    symlink("../.git/hooks", f.path("docs/l")).unwrap();
    let ctx = f.ctx();
    let abs_env = f.path(".env");
    assert_eq!(ReadFile.subject(&ctx, &json!({"path": abs_env.to_str().unwrap()})), ".env");
    assert_eq!(ReadFile.subject(&ctx, &json!({"path": "./src//../.env"})), ".env");
    assert_eq!(WriteFile.subject(&ctx, &json!({"path": "docs/l/pre-commit"})), ".git/hooks/pre-commit");
    assert_eq!(ListDir.subject(&ctx, &json!({})), ".");
    assert_eq!(Grep.subject(&ctx, &json!({"pattern": "x", "path": "docs/l"})), ".git/hooks");
    // Refused or missing paths have nothing to match.
    assert_eq!(ReadFile.subject(&ctx, &json!({"path": "../x"})), "");
    assert_eq!(ReadFile.subject(&ctx, &json!({})), "");

    let s = SessionId::new("s");
    let deny_env = [rule("read_file", "", Allow), rule("read_file", ".env", Deny)];
    let subject = ReadFile.subject(&ctx, &json!({"path": abs_env.to_str().unwrap()}));
    assert_eq!(decide(&deny_env, "read_file", ToolClass::Read, &subject, &s, None), Deny);
    let allow_docs = [rule("write_file", "docs", Allow)];
    let subject = WriteFile.subject(&ctx, &json!({"path": "docs/l/pre-commit"}));
    assert_eq!(decide(&allow_docs, "write_file", ToolClass::Write, &subject, &s, None), Ask);
    // An unresolvable path is not approved by a path rule.
    let allow_all = [rule("write_file", ".", Allow)];
    assert_eq!(decide(&allow_all, "write_file", ToolClass::Write, "", &s, None), Ask);
}

#[test]
fn patterns_that_climb_out_normalise_before_matching() {
    assert_eq!(normalize_subject("src/../../x"), "../x");
    assert_eq!(normalize_subject("a/b/../../../c/./d"), "../c/d");
    let s = SessionId::new("s");
    let rules = [rule("edit_file", "src", Allow)];
    assert_eq!(decide(&rules, "edit_file", ToolClass::Write, "src/../../x", &s, None), Ask);
    let rules = [rule("edit_file", "src/../../x", Allow)];
    assert_eq!(decide(&rules, "edit_file", ToolClass::Write, "x", &s, None), Ask);
}

#[test]
fn project_notes_never_follow_a_symlink_out_of_the_root() {
    let f = Fixture::new();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("id_rsa"), "PRIVATE KEY").unwrap();
    symlink(outside.path().join("id_rsa"), f.path("AGENTS.md")).unwrap();
    assert_eq!(project_notes(f.root()), None);
    f.write("CLAUDE.md", "real notes");
    assert_eq!(project_notes(f.root()).as_deref(), Some("real notes"));
    // A link inside the root is fine.
    std::fs::remove_file(f.path("AGENTS.md")).unwrap();
    f.write("docs/agents.md", "linked notes");
    symlink("docs/agents.md", f.path("AGENTS.md")).unwrap();
    assert_eq!(project_notes(f.root()).as_deref(), Some("linked notes"));
}

#[test]
fn workspace_helpers_do_not_follow_links_or_block_on_fifos() {
    let f = Fixture::new();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
    symlink(outside.path(), f.path("out")).unwrap();
    symlink(outside.path().join("secret.txt"), f.path("link.txt")).unwrap();
    mkfifo(&f.path("AGENTS.md"));
    f.write("a.txt", "");
    assert_eq!(list_files(f.root(), 100), ["a.txt"]);
    assert_eq!(project_notes(f.root()), None);
    assert!(!context_summary(f.root()).contains("secret"));
}

#[test]
fn deny_in_any_scope_beats_allow_in_a_narrower_one() {
    let s = SessionId::new("s");
    let rules = [
        rule("bash", "git push", Deny),
        PermissionRule {
            tool: "bash".into(),
            pattern: String::new(),
            decision: Allow,
            scope: RuleScope::Session { session: s.clone() },
        },
    ];
    assert_eq!(decide(&rules, "bash", ToolClass::Execute, "git push", &s, None), Deny);
    assert_eq!(decide(&rules, "bash", ToolClass::Execute, "git status", &s, None), Allow);
}

#[test]
fn repo_scope_compares_canonical_paths() {
    let real = TempDir::new().unwrap();
    let holder = TempDir::new().unwrap();
    let link = holder.path().join("repo");
    symlink(real.path(), &link).unwrap();
    let s = SessionId::new("s");
    let rules = [PermissionRule {
        tool: "bash".into(),
        pattern: String::new(),
        decision: Allow,
        scope: RuleScope::Repo { path: link.clone() },
    }];
    assert_eq!(decide(&rules, "bash", ToolClass::Execute, "ls", &s, Some(real.path())), Allow);
    let dotted = real.path().join("sub/..");
    std::fs::create_dir(real.path().join("sub")).unwrap();
    assert_eq!(decide(&rules, "bash", ToolClass::Execute, "ls", &s, Some(&dotted)), Allow);
    assert_eq!(decide(&rules, "bash", ToolClass::Execute, "ls", &s, Some(holder.path())), Ask);
    // Paths that do not exist compare literally.
    let gone = [PermissionRule { scope: RuleScope::Repo { path: "/no/such/repo".into() }, ..rules[0].clone() }];
    assert_eq!(decide(&gone, "bash", ToolClass::Execute, "ls", &s, Some(Path::new("/no/such/repo"))), Allow);
}

#[test]
fn file_tools_refuse_fifos_instead_of_blocking() {
    let f = Fixture::new();
    mkfifo(&f.path("pipe"));
    for (tool, input) in [
        (&ReadFile as &dyn Tool, json!({"path": "pipe"})),
        (&EditFile, json!({"path": "pipe", "old_string": "a", "new_string": "b"})),
        (&WriteFile, json!({"path": "pipe", "content": "x"})),
    ] {
        let out = run_bounded(&f, tool, input);
        assert!(out.is_error && out.content.contains("not a regular file"), "{}", out.content);
    }
    let preview = WriteFile.preview(&f.ctx(), &json!({"path": "pipe", "content": "x"}));
    assert!(matches!(preview, model::ToolDisplay::Text { .. }));
    let dev = run_bounded(&f, &ReadFile, json!({"path": "/dev/zero"}));
    assert!(dev.is_error);
}

#[test]
fn a_symlink_swapped_in_after_resolve_is_refused() {
    let f = Fixture::new();
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("secret");
    std::fs::write(&secret, "keep").unwrap();
    f.write("a.txt", "inside");
    let target = resolve(f.root(), "a.txt").unwrap();
    // The layout changes between the check and the open.
    std::fs::remove_file(f.path("a.txt")).unwrap();
    symlink(&secret, f.path("a.txt")).unwrap();
    assert!(matches!(open_regular(&target), Err(OpenError::Changed)));
    assert!(matches!(write_regular(&target, b"pwned"), Err(OpenError::Changed)));
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "keep");

    // A new file whose name became a dangling link is not created through it.
    let new = resolve(f.root(), "new.txt").unwrap();
    let planted = outside.path().join("planted");
    symlink(&planted, f.path("new.txt")).unwrap();
    assert!(matches!(write_regular(&new, b"x"), Err(OpenError::Changed)));
    assert!(!planted.exists());
}
