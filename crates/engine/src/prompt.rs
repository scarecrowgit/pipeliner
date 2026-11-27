//! Prompt stack: six layers assembled fresh for every model call.
//!
//! 1 Workspace rules · 2 Agent role · 3 Step instructions · 4 Run context ·
//! 5 Memory (not enabled in this release) · 6 Your message.
//! system = 1 + 2 + 3 (+ environment, skills); context = 4 (+5); turn = 6.

use std::path::Path;

use harness_core::{skills, Config, ToolAccess};

use crate::llm::ImageData;

#[derive(Default, Clone, Debug)]
pub struct Stack {
    pub rules: String,
    pub role: String,
    pub instructions: String,
    pub context: String,
    pub message: String,
    /// Working directory description and project instructions.
    pub environment: String,
    pub skills: Vec<String>,
    pub skills_on_demand: Vec<String>,
    /// Images attached to the first user turn.
    pub images: Vec<ImageData>,
}

impl Stack {
    pub fn system(&self, config: &Config) -> String {
        let mut s = String::new();
        let mut section = |title: &str, body: &str| {
            if !body.trim().is_empty() {
                s.push_str(&format!("## {title}\n{}\n\n", body.trim()));
            }
        };
        section("Workspace rules", &self.rules);
        section("Your role", &self.role);
        section("This step", &self.instructions);
        section("Environment", &self.environment);
        for name in &self.skills {
            if let Some(body) = skills::load(config, name) {
                s.push_str(&body);
                s.push_str("\n\n");
            }
        }
        if !self.skills_on_demand.is_empty() {
            let all = skills::list(config);
            let mut lines = vec![];
            for name in &self.skills_on_demand {
                let d = all
                    .iter()
                    .find(|k| &k.name == name)
                    .map(|k| k.description.as_str())
                    .unwrap_or("");
                lines.push(format!("- {name}: {d}"));
            }
            s.push_str(&format!(
                "## Skills you can load with `load_skill`\n{}\nLoad a skill before working in its area.\n\n",
                lines.join("\n")
            ));
        }
        s.trim_end().to_string()
    }

    /// The first user turn: run context followed by the message.
    pub fn first_turn(&self) -> String {
        let mut s = String::new();
        if !self.context.trim().is_empty() {
            s.push_str(self.context.trim());
            s.push_str("\n\n");
        }
        s.push_str(self.message.trim());
        if s.trim().is_empty() {
            s.push_str("Begin.");
        }
        s
    }
}

/// How to work with files; part of every prompt that has a working directory.
pub const FILE_TOOLS_RULE: &str = "Look at files with read_file (several at once with `files`), \
find_files and search; change them with edit_file (several changes with `edits`) and write_file. \
Keep run_command for builds, tests, git and other programs, not for cat, sed, grep, find or \
heredocs.\n";

/// Describes a working folder and includes its project instructions (AGENTS.md / CLAUDE.md).
/// The tool instructions match what `access` actually allows.
pub fn environment(
    cwd: Option<&Path>,
    shell: &str,
    access: ToolAccess,
    other_workdirs: &[(String, String)],
) -> String {
    let mut s = String::new();
    let wrapper = |s: &mut String| {
        if !shell.is_empty() {
            s.push_str(&format!(
                "Commands you run with run_command are wrapped as: {shell} '<command>' (so build tools are available).\n"
            ));
        }
    };
    if let Some(cwd) = cwd {
        s.push_str(&format!("Working directory: {}\n", cwd.display()));
        match access {
            ToolAccess::Full => {
                s.push_str(FILE_TOOLS_RULE);
                wrapper(&mut s);
            }
            ToolAccess::ReadAndRun => {
                s.push_str("Look at files with read_file (several at once with `files`), list_dir, find_files and search (read-only; you cannot write or edit files). Keep run_command for builds, tests, git and other programs, not for cat, sed, grep or find.\n");
                wrapper(&mut s);
            }
            ToolAccess::ReadOnly => {
                s.push_str("Look at files with read_file (several at once with `files`), list_dir, find_files and search (read-only; you cannot edit files or run commands).\n");
            }
            ToolAccess::None => {}
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if let Ok(t) = std::fs::read_to_string(cwd.join(name)) {
                let t: String = t.chars().take(12_000).collect();
                s.push_str(&format!("\n### Project instructions ({name})\n{t}\n"));
                break;
            }
        }
    } else if matches!(access, ToolAccess::ReadAndRun | ToolAccess::Full) {
        s.push_str("No working folder is attached: file tools are unavailable, and run_command starts in the home directory.\n");
    }
    if !other_workdirs.is_empty() && access != ToolAccess::None {
        s.push_str("\nOther folders you can read with read_workdir:\n");
        for (id, path) in other_workdirs {
            s.push_str(&format!("- `{id}`: {path}\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        std::path::PathBuf::from("/nonexistent-pipeliner-test-dir")
    }

    #[test]
    fn full_with_cwd_and_shell() {
        let d = dir();
        let s = environment(Some(&d), "nix-shell --run", ToolAccess::Full, &[]);
        assert!(s.contains("Working directory: /nonexistent-pipeliner-test-dir"));
        assert!(s.contains("edit_file"));
        assert!(s.contains("wrapped as: nix-shell --run '<command>'"));
    }

    #[test]
    fn full_without_cwd() {
        let s = environment(None, "sh -c", ToolAccess::Full, &[]);
        assert!(s.contains("No working folder is attached"));
        assert!(s.contains("run_command starts in the home directory"));
        assert!(!s.contains("Working directory"));
        assert!(!s.contains("wrapped as"));
    }

    #[test]
    fn read_and_run_with_and_without_cwd() {
        let d = dir();
        let s = environment(Some(&d), "sh -c", ToolAccess::ReadAndRun, &[]);
        assert!(s.contains("you cannot write or edit files"));
        assert!(!s.contains("edit_file"));
        assert!(s.contains("wrapped as: sh -c '<command>'"));
        let s = environment(None, "", ToolAccess::ReadAndRun, &[]);
        assert!(s.contains("No working folder is attached"));
    }

    #[test]
    fn read_only_claims_no_writes_or_commands() {
        let d = dir();
        let s = environment(Some(&d), "sh -c", ToolAccess::ReadOnly, &[]);
        assert!(s.contains("cannot edit files or run commands"));
        assert!(!s.contains("edit_file"));
        assert!(!s.contains("write_file"));
        assert!(!s.contains("wrapped as"));
        assert!(!environment(None, "", ToolAccess::ReadOnly, &[]).contains("No working folder"));
    }

    #[test]
    fn none_has_no_tool_instructions() {
        let d = dir();
        let others = vec![("w1".to_string(), "/tmp/w1".to_string())];
        let s = environment(Some(&d), "sh -c", ToolAccess::None, &others);
        assert_eq!(s, "Working directory: /nonexistent-pipeliner-test-dir\n");
        assert_eq!(environment(None, "sh -c", ToolAccess::None, &[]), "");
        let s = environment(None, "", ToolAccess::ReadOnly, &others);
        assert!(s.contains("read_workdir") && s.contains("`w1`"));
    }
}
