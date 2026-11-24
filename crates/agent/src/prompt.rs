//! The system prompt, assembled in one place.
//!
//! It is built once per run and is the same text for every model call of that run, and for
//! every run of a session as long as the spec, the workdir's notes, the installed skills and
//! the day stay the same, so the provider's prompt cache holds. Nothing that changes from
//! turn to turn belongs here: no clock time, no todo list, no file listing.

use model::{ChatMode, Skill};

use crate::session_tools::names::{ASK_USER, REPORT_TO_INBOX, SPAWN_AGENT, TODO_WRITE};
use crate::session_tools::{search, skill, spawn};
use crate::spec::{AgentDeps, AgentEnv, AgentSpec, Finish};
use crate::toolbox::Toolbox;

/// The system prompt of a run: the role, the environment, the mode's rules, how to work with
/// the offered tools, the project's notes, skills and sub-agent kinds.
pub(crate) fn system_prompt(
    spec: &AgentSpec,
    env: &AgentEnv,
    deps: &AgentDeps<'_>,
    toolbox: &Toolbox,
    offered_skills: &[Skill],
) -> String {
    let mut sections: Vec<String> = Vec::new();

    let role = spec.role.trim();
    sections.push(if role.is_empty() {
        format!("You are {}, an agent working for a person through the tools you are offered.", spec.name)
    } else {
        role.to_string()
    });

    sections.push(environment(env, deps));

    if env.mode == ChatMode::Plan {
        sections.push(
            "# Plan mode\n\nYou are in Plan mode: tools that edit files or run commands are not available. \
             Investigate, then reply with a concise, numbered plan: the files to change and what changes in \
             each, the risks, and how to verify the result. Do not try to change anything."
                .to_string(),
        );
    } else if env.read_only {
        sections.push(
            "# Read-only\n\nThis session works in its folder read-only: tools that edit files or run commands \
             are not available. Answer questions and propose changes; do not try to make them."
                .to_string(),
        );
    }

    let rules = tool_rules(spec, toolbox);
    if !rules.is_empty() {
        sections.push(format!("# Working with tools\n\n{rules}"));
    }

    let deferred = toolbox.deferred_tools();
    if !deferred.is_empty() {
        sections.push(format!("# More MCP tools\n\n{}", search::prompt_section(deferred)));
    }

    if let Some(notes) = env.workdir.as_deref().and_then(tools::workspace::project_notes) {
        sections.push(format!(
            "# Project notes\n\nThe project keeps these instructions for agents. Follow them.\n\n<project-notes>\n{notes}\n</project-notes>"
        ));
    }

    let skills = skill::prompt_section(&spec.skills, offered_skills, deps.skills);
    if !skills.is_empty() {
        sections.push(format!("# Skills\n\n{skills}"));
    }

    if toolbox.has(SPAWN_AGENT) {
        sections.push(format!("# Sub-agents\n\n{}", spawn::prompt_section(&spec.templates)));
    }

    sections.join("\n\n")
}

fn environment(env: &AgentEnv, deps: &AgentDeps<'_>) -> String {
    let now = deps.clock.now_ms();
    let offset = base::time::utc_offset(now);
    let date = format!("{} {}", base::time::long_date(now, offset), base::time::civil(now, offset).year);
    let folder = match &env.workdir {
        Some(path) => format!("- Working folder: {}. Paths in tool calls are relative to it.", path.display()),
        None => "- No working folder: this session has no file or shell tools.".to_string(),
    };
    format!("# Environment\n\n{folder}\n- Platform: Linux\n- Date: {date}")
}

/// One line per rule that the offered tools make relevant.
fn tool_rules(spec: &AgentSpec, toolbox: &Toolbox) -> String {
    let mut rules: Vec<String> = Vec::new();
    if !toolbox.names().is_empty() {
        rules.push(
            "A person may have to approve an edit, a command or an external tool before it runs. When a call \
             is rejected, read the note that comes back and adapt; do not repeat the same call."
                .to_string(),
        );
    }
    if toolbox.has(TODO_WRITE) {
        rules.push(format!(
            "For work with several steps keep a task list with `{TODO_WRITE}`: one item in progress at a time, \
             each marked done as soon as it is."
        ));
    }
    if toolbox.has_mcp() {
        rules.push(
            "Tools named `mcp__<server>__<tool>` come from external servers the person configured.".to_string(),
        );
    }
    if toolbox.has(ASK_USER) {
        rules.push(format!(
            "When you cannot go on without a decision only the person can make, call `{ASK_USER}` with one \
             precise question and stop."
        ));
    }
    if toolbox.has(REPORT_TO_INBOX) {
        rules.push(format!(
            "When you finish work that produced something (changes, a branch, a pull request, a plan, an \
             investigation), call `{REPORT_TO_INBOX}` once at the end with links to everything you created. \
             Skip it for quick questions."
        ));
    }
    if let Finish::Tool(finish) = &spec.finish {
        rules.push(format!(
            "When your work is complete, call `{}` with the result. The step is not finished until you do.",
            finish.name
        ));
    }
    rules.iter().map(|rule| format!("- {rule}")).collect::<Vec<_>>().join("\n")
}
