use model::{ToolAccess, ToolClass, ToolGrants};
use serde_json::json;

use super::{Fixture, junk_inputs};
use crate::workspace::{context_summary, list_files, project_notes};
use crate::{apply_grants, builtin};

#[test]
fn builtin_registries_follow_the_access_level() {
    let names = |a| builtin(a).names().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(names(ToolAccess::ReadOnly), ["grep", "list_dir", "read_file"]);
    assert_eq!(names(ToolAccess::ReadAndRun), ["bash", "grep", "list_dir", "read_file"]);
    assert_eq!(names(ToolAccess::Full), ["bash", "edit_file", "grep", "list_dir", "read_file", "write_file"]);
    let full = builtin(ToolAccess::Full);
    for spec in full.specs() {
        let tool = full.get(&spec.name).unwrap();
        assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
        assert!(!spec.description.is_empty());
        let class = tool.class();
        let expected = match spec.name.as_str() {
            "bash" => ToolClass::Execute,
            "edit_file" | "write_file" => ToolClass::Write,
            _ => ToolClass::Read,
        };
        assert_eq!(class, expected, "{}", spec.name);
    }
}

#[test]
fn grants_remove_denied_tools_only() {
    let mut registry = builtin(ToolAccess::Full);
    let grants = ToolGrants {
        access: ToolAccess::Full,
        allow: vec!["mcp__github__create_pull_request".into()],
        deny: vec!["bash".into(), "write_file".into(), "not_a_tool".into()],
    };
    apply_grants(&mut registry, &grants);
    assert_eq!(registry.names().collect::<Vec<_>>(), ["edit_file", "grep", "list_dir", "read_file"]);
}

#[test]
fn no_tool_panics_on_junk_input() {
    let f = Fixture::new();
    f.write("a", "content\n");
    let registry = builtin(ToolAccess::Full);
    for name in registry.names() {
        let tool = registry.get(name).unwrap();
        for input in junk_inputs() {
            let _ = tool.subject(&f.ctx(), &input);
            let _ = tool.preview(&f.ctx(), &input);
            let out = tool.run(&f.ctx(), &input);
            if matches!(input, serde_json::Value::Number(_) | serde_json::Value::Array(_)) {
                assert!(out.is_error && out.content.contains("JSON object"), "{name}: {}", out.content);
            }
        }
    }
    assert_eq!(f.read("a"), "content\n");
}

#[test]
fn list_files_respects_gitignore_and_sorts() {
    let f = Fixture::new();
    f.write(".gitignore", "build/\n");
    f.write("src/b.rs", "");
    f.write("src/a.rs", "");
    f.write("README.md", "");
    f.write("build/out.o", "");
    f.write(".git/config", "");
    f.write(".env", "");
    assert_eq!(list_files(f.root(), 100), ["README.md", "src/a.rs", "src/b.rs"]);
    assert_eq!(list_files(f.root(), 2), ["README.md", "src/a.rs"]);
    assert!(list_files(f.root(), 0).is_empty());
    assert!(list_files(&f.path("missing"), 10).is_empty());
}

#[test]
fn project_notes_prefer_agents_md_and_are_capped() {
    let f = Fixture::new();
    assert_eq!(project_notes(f.root()), None);
    f.write("CLAUDE.md", "claude notes\n");
    assert_eq!(project_notes(f.root()).as_deref(), Some("claude notes"));
    f.write("AGENTS.md", "  agents notes \n");
    assert_eq!(project_notes(f.root()).as_deref(), Some("agents notes"));
    f.write("AGENTS.md", &"é".repeat(20_000));
    let notes = project_notes(f.root()).unwrap();
    assert!(notes.len() < crate::workspace::NOTES_MAX_BYTES + 200);
    assert!(notes.contains("AGENTS.md is longer than"));
    f.write("AGENTS.md", "   \n");
    assert_eq!(project_notes(f.root()).as_deref(), Some("claude notes"));
}

#[test]
fn context_summary_names_top_level_entries() {
    let f = Fixture::new();
    f.write(".gitignore", "target/\n");
    f.write("src/main.rs", "");
    f.write("src/lib.rs", "");
    f.write("Cargo.toml", "");
    f.write("target/x", "");
    let summary = context_summary(f.root());
    assert!(summary.contains("Top level: 1 folder, 2 files"), "{summary}");
    assert!(summary.contains("  src/\n  .gitignore\n  Cargo.toml\n"), "{summary}");
    assert!(!summary.contains("target"), "{summary}");
    assert!(summary.contains("Files in all (not counting ignored and hidden ones): 3"), "{summary}");
}

#[test]
fn tool_errors_read_the_same_for_model_and_person() {
    let f = Fixture::new();
    let out = f.run(&crate::ReadFile, json!({"path": 3}));
    assert!(out.is_error);
    assert!(matches!(&out.display, model::ToolDisplay::Text { body, .. } if *body == out.content));
}
