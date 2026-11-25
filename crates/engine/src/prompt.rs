//! Prompt stack: six layers assembled fresh for every model call.
//!
//! 1 Workspace rules · 2 Agent role · 3 Step instructions · 4 Run context ·
//! 5 Memory (not enabled in this release) · 6 Your message.
//! system = 1 + 2 + 3 (+ environment, skills); context = 4 (+5); turn = 6.

use std::path::Path;

use harness_core::{skills, Config};

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
                let d = all.iter().find(|k| &k.name == name).map(|k| k.description.as_str()).unwrap_or("");
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

/// Describes a working folder and includes its project instructions (AGENTS.md / CLAUDE.md).
pub fn environment(cwd: Option<&Path>, shell: &str, other_workdirs: &[(String, String)]) -> String {
    let mut s = String::new();
    if let Some(cwd) = cwd {
        s.push_str(&format!("Working directory: {}\n", cwd.display()));
        if !shell.is_empty() {
            s.push_str(&format!(
                "Commands you run with run_command are wrapped as: {shell} '<command>' (so build tools are available).\n"
            ));
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if let Ok(t) = std::fs::read_to_string(cwd.join(name)) {
                let t: String = t.chars().take(12_000).collect();
                s.push_str(&format!("\n### Project instructions ({name})\n{t}\n"));
                break;
            }
        }
    }
    if !other_workdirs.is_empty() {
        s.push_str("\nOther folders you can read with read_workdir:\n");
        for (id, path) in other_workdirs {
            s.push_str(&format!("- `{id}`: {path}\n"));
        }
    }
    s
}
