//! AI-drafted agents: the workspace model turns a short description into an agent
//! definition (name, role prompt, model, tools, skills). `draft_agent` saves nothing; the
//! caller shows the draft and saves it when the user presses Save.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use harness_core::skills::Skill;
use harness_core::*;
use serde_json::{json, Value};

use crate::llm::{self, Msg, Reply, ToolSpec};
use crate::Engine;

/// An agent written by the workspace model. Not saved yet.
#[derive(Clone, Debug)]
pub struct AgentDraft {
    pub agent: AgentDef,
    /// Workspace model that wrote the draft (`config.default_model`).
    pub drafted_by: String,
    pub cost_usd: f64,
}

const TOOL_NAME: &str = "draft_agent";

impl Engine {
    /// Has the workspace model draft an agent from a description. Blocking: call from a
    /// background task. Does not save anything (no `put_agent`); it only logs the event.
    pub fn draft_agent(&self, prompt: &str, cancel: Arc<AtomicBool>) -> Result<AgentDraft> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("describe what the agent should do");
        }
        let config = self.config();
        let drafted_by = config.default_model.clone();
        if drafted_by.trim().is_empty() {
            bail!("no workspace model configured (Settings › Models)");
        }
        let (provider, model_id) = config
            .resolve(&drafted_by)
            .map(|(p, m)| (p.clone(), m))
            .context("no model provider configured (Settings › Models)")?;

        let mut models = self.models();
        if !models.contains(&drafted_by) {
            models.push(drafted_by.clone());
        }
        let skills = self.skills();
        let existing_names: Vec<String> =
            self.store().agents().into_iter().map(|a| a.name).collect();
        let rules = config.rules.clone();

        let msgs = vec![
            Msg::System(system_prompt(
                &models,
                &drafted_by,
                &skills,
                &existing_names,
                &rules,
            )),
            Msg::User(prompt.to_string()),
        ];
        let reply = llm::complete(
            &provider,
            &model_id,
            &msgs,
            &[tool_spec()],
            Some(1600),
            &|| cancel.load(Ordering::SeqCst),
            &mut |_| {},
        )?;
        let agent = parse_reply(&reply, &models, &drafted_by, &skills, &existing_names)?;
        let cost = config.cost(&drafted_by, reply.tokens_in, reply.tokens_out);
        self.store().log(
            "agent.drafted",
            None,
            None,
            format!("AI drafted agent {} · ${cost:.3}", agent.name),
        );
        Ok(AgentDraft {
            agent,
            drafted_by,
            cost_usd: cost,
        })
    }

    /// `draft_agent`, then saves the result as a new agent (version 1). For the CLI and
    /// scripts; the UI uses `draft_agent` so the user can review before saving.
    pub fn create_agent_with_ai(
        &self,
        prompt: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<AgentDraft> {
        let draft = self.draft_agent(prompt, cancel)?;
        self.store().put_agent(&draft.agent)?;
        self.store().log(
            "agent.created",
            None,
            None,
            format!("Created agent {}", draft.agent.name),
        );
        Ok(draft)
    }
}

fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME,
        description: "Return the drafted agent definition.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Concise agent name, 1-3 words (e.g. \"Log reader\", \"Fixer\", \"Release writer\"). Avoid duplicating existing agent names."
                },
                "description": {
                    "type": "string",
                    "description": "One short line describing what the agent is for (shown in lists)."
                },
                "role": {
                    "type": "string",
                    "description": "Prompt layer 2: instructions sent with every task of this agent. Direct second person (\"You ...\"), clear bullet rules and constraints, how to work, and what to return."
                },
                "tools": {
                    "type": "string",
                    "enum": ["none", "read_only", "read_and_run", "full"],
                    "description": "none: no tools, answers from the prompt only. read_only: read_file, list_dir, search, load_skill, read_workdir (inspects files, code and logs; never runs commands or edits files). read_and_run: read_only plus run_command (runs tests, CLI tools or queries; never edits files). full: all tools including write_file and edit_file."
                },
                "model": {
                    "type": "string",
                    "description": "One of the available model ids. A lighter, faster model (haiku/mini/flash) for simple triage or formatting, the workspace default for general coding and analysis, a stronger model (opus/pro) for complex planning or architecture."
                },
                "max_steps": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "description": "Step limit, e.g. 10 for none, 25 for read_only, 35 for read_and_run, 40-60 for full."
                },
                "skills": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Installed skill names always loaded into the prompt. Only skills needed on every task."
                },
                "skills_on_demand": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Installed skill names the agent may load with load_skill when relevant."
                }
            },
            "required": ["name", "description", "role", "tools", "model"]
        }),
    }
}

fn system_prompt(
    models: &[String],
    default_model: &str,
    skills: &[Skill],
    existing_names: &[String],
    rules: &str,
) -> String {
    let mut s = String::from(
        "You design agents for Pipeliner, a harness for AI agent pipelines. The user describes \
what an agent should do; you write its definition. Call the `draft_agent` tool once (or, if you \
cannot call tools, reply with a single JSON object with the same fields and nothing else).\n\n\
Fields:\n\
- name: concise, 1-3 words (e.g. \"Log reader\", \"Fixer\", \"Release writer\").\n\
- description: one short line saying what the agent is for; it is shown in lists.\n\
- role: the agent's instructions (prompt layer 2), sent with every task of this agent. Write in direct \
second person (\"You ...\"), with clear bullet rules and constraints, how to work, and what to return.\n\
- tools: one of\n\
  - none: no tools; answers from the prompt only.\n\
  - read_only: read_file, list_dir, search, load_skill, read_workdir (inspects files, code and logs; \
never runs commands or edits files).\n\
  - read_and_run: read_only plus run_command (runs tests, CLI tools or queries; never edits files).\n\
  - full: all tools including write_file and edit_file.\n\
  Give the agent the least access that lets it do its job.\n\
- model: one of the available models below. Pick a lighter, faster model (haiku, mini, flash) for simple \
triage or formatting when one is available, the workspace default for general coding and analysis, a \
stronger model (opus, pro) for complex planning or architecture.\n\
- max_steps (optional): integer from 1 to 500, e.g. 10 for none, 25 for read_only, 35 for read_and_run, \
40-60 for full.\n\
- skills (optional): installed skill names always loaded into the prompt; only for skills needed on every task.\n\
- skills_on_demand (optional): installed skill names the agent may load with load_skill when relevant.\n\n\
Available models:\n",
    );
    for m in models {
        s.push_str("- ");
        s.push_str(m);
        if m == default_model {
            s.push_str(" (workspace default)");
        }
        s.push('\n');
    }
    s.push_str("\nInstalled skills:\n");
    if skills.is_empty() {
        s.push_str("(none installed; leave skills and skills_on_demand empty)\n");
    }
    for k in skills {
        if k.description.trim().is_empty() {
            s.push_str(&format!("- {}\n", k.name));
        } else {
            s.push_str(&format!("- {}: {}\n", k.name, k.description.trim()));
        }
    }
    if !existing_names.is_empty() {
        s.push_str("\nExisting agent names (pick a distinct name):\n");
        for n in existing_names {
            s.push_str(&format!("- {n}\n"));
        }
    }
    if !rules.trim().is_empty() {
        s.push_str(
            "\nWorkspace rules (the agent's prompts should match this house style and language):\n",
        );
        s.push_str(rules.trim());
        s.push('\n');
    }
    s
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Reads the model's reply (tool call, else JSON in the text) into an agent definition.
fn parse_reply(
    reply: &Reply,
    models: &[String],
    drafted_by: &str,
    skills: &[Skill],
    existing_names: &[String],
) -> Result<AgentDef> {
    let call = reply
        .calls
        .iter()
        .find(|c| c.name == TOOL_NAME)
        .or_else(|| reply.calls.first());
    let from_call = call.and_then(|c| serde_json::from_str::<Value>(&c.arguments).ok());
    let value = match from_call {
        Some(v) if v.is_object() => v,
        _ => {
            extract_json(&reply.content).context("the model did not return an agent definition")?
        }
    };
    build_agent(&value, models, drafted_by, skills, existing_names)
}

/// Finds a JSON object in free text: raw JSON, a ```json fence, or `{ ... }` inside prose.
fn extract_json(text: &str) -> Option<Value> {
    let text = text.trim();
    let mut candidates: Vec<&str> = vec![text];
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // Skip the language tag line.
        let body = after.split_once('\n').map(|(_, b)| b).unwrap_or(after);
        let body = body.split("```").next().unwrap_or(body);
        candidates.push(body.trim());
    }
    if let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) {
        if a < b {
            candidates.push(&text[a..=b]);
        }
    }
    candidates
        .into_iter()
        .filter_map(|c| serde_json::from_str::<Value>(c).ok())
        .find(|v| v.is_object())
}

fn build_agent(
    v: &Value,
    models: &[String],
    drafted_by: &str,
    skills: &[Skill],
    existing_names: &[String],
) -> Result<AgentDef> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim();

    let role = text("role").to_string();
    if role.is_empty() {
        bail!("the model returned an empty role prompt");
    }
    let description = text("description")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut name = clean_name(text("name"));
    if name.is_empty() {
        name = fallback_name(&description);
    }
    let name = unique_name(&name, existing_names);
    let tools = parse_tools(text("tools"));
    let model = resolve_model(text("model"), models, drafted_by);
    let installed = |n: &str| skills.iter().any(|s| s.name == n);
    let always = name_list(v.get("skills"), &installed, &[]);
    let on_demand = name_list(v.get("skills_on_demand"), &installed, &always);
    let max_steps = v
        .get("max_steps")
        .and_then(Value::as_u64)
        .map(|n| (n.min(u32::MAX as u64) as u32).clamp(1, 500))
        .unwrap_or_else(default_max_steps);

    Ok(AgentDef {
        id: new_id(),
        name,
        description,
        role,
        model,
        tools,
        skills: always,
        skills_on_demand: on_demand,
        max_steps,
        version: 1,
        created_at: now(),
        updated_at: now(),
    })
}

/// `name` unless it equals (ignoring case) an existing name; then "name 2", "name 3", ...
fn unique_name(name: &str, existing: &[String]) -> String {
    let taken = |n: &str| {
        existing
            .iter()
            .any(|e| e.trim().to_lowercase() == n.to_lowercase())
    };
    if !taken(name) {
        return name.to_string();
    }
    (2u32..)
        .map(|i| format!("{name} {i}"))
        .find(|c| !taken(c))
        .unwrap_or_else(|| name.to_string())
}

fn clean_name(s: &str) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .trim()
        .to_string()
}

/// A short name from the description's first words, else "New agent".
fn fallback_name(description: &str) -> String {
    let words: Vec<&str> = description.split_whitespace().take(3).collect();
    let name = clean_name(
        words
            .join(" ")
            .trim_end_matches(|c: char| !c.is_alphanumeric()),
    );
    if name.is_empty() {
        "New agent".into()
    } else {
        name
    }
}

fn parse_tools(s: &str) -> ToolAccess {
    match s.trim().to_lowercase().as_str() {
        "none" | "no tools" => ToolAccess::None,
        "read_only" | "readonly" | "read only" | "read-only" => ToolAccess::ReadOnly,
        "read_and_run" | "readandrun" | "read and run" | "read-and-run" => ToolAccess::ReadAndRun,
        _ => ToolAccess::Full,
    }
}

fn resolve_model(picked: &str, models: &[String], drafted_by: &str) -> String {
    let picked = picked.trim();
    if picked.is_empty() {
        return drafted_by.to_string();
    }
    if let Some(m) = models.iter().find(|m| m.as_str() == picked) {
        return m.clone();
    }
    let lower = picked.to_lowercase();
    if let Some(m) = models.iter().find(|m| m.to_lowercase() == lower) {
        return m.clone();
    }
    let suffix = format!("/{lower}");
    if let Some(m) = models.iter().find(|m| m.to_lowercase().ends_with(&suffix)) {
        return m.clone();
    }
    drafted_by.to_string()
}

/// Installed names only, deduplicated in order, without those in `exclude`.
fn name_list(
    v: Option<&Value>,
    installed: &dyn Fn(&str) -> bool,
    exclude: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for n in v.and_then(Value::as_array).into_iter().flatten() {
        let Some(n) = n.as_str().map(str::trim) else {
            continue;
        };
        if installed(n) && !out.iter().any(|o| o == n) && !exclude.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Call;
    use harness_core::config::Provider;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn skill(name: &str) -> Skill {
        Skill {
            name: name.into(),
            description: format!("about {name}"),
            path: format!("/skills/{name}/SKILL.md").into(),
            dir_label: "/skills".into(),
        }
    }

    fn models() -> Vec<String> {
        vec![
            "p/claude-haiku".into(),
            "p/claude-sonnet".into(),
            "p/claude-opus".into(),
        ]
    }

    fn call_reply(args: Value) -> Reply {
        Reply {
            calls: vec![Call {
                id: "c1".into(),
                name: TOOL_NAME.into(),
                arguments: args.to_string(),
            }],
            ..Reply::default()
        }
    }

    fn text_reply(text: &str) -> Reply {
        Reply {
            content: text.into(),
            ..Reply::default()
        }
    }

    fn parse(reply: &Reply) -> Result<AgentDef> {
        parse_reply(
            reply,
            &models(),
            "p/claude-sonnet",
            &[skill("rust"), skill("git"), skill("docs")],
            &[],
        )
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unique_name_keeps_free_names() {
        assert_eq!(unique_name("Fixer", &[]), "Fixer");
        assert_eq!(unique_name("Fixer", &names(&["Reviewer"])), "Fixer");
    }

    #[test]
    fn unique_name_exact_and_case_insensitive() {
        assert_eq!(unique_name("Fixer", &names(&["Fixer"])), "Fixer 2");
        assert_eq!(unique_name("Fixer", &names(&["fixer"])), "Fixer 2");
        assert_eq!(unique_name("fixer", &names(&["FIXER"])), "fixer 2");
    }

    #[test]
    fn unique_name_counts_up() {
        assert_eq!(
            unique_name("Fixer", &names(&["Fixer", "Fixer 2"])),
            "Fixer 3"
        );
        assert_eq!(
            unique_name("Fixer", &names(&["fixer", "FIXER 2", "Fixer 3"])),
            "Fixer 4"
        );
    }

    #[test]
    fn parsed_name_is_deduplicated_against_existing() {
        let r = call_reply(json!({"name": "fixer", "role": "You fix."}));
        let a = parse_reply(&r, &models(), "p/claude-sonnet", &[], &names(&["Fixer"])).unwrap();
        assert_eq!(a.name, "fixer 2");
    }

    fn full_args() -> Value {
        json!({
            "name": "Log reader",
            "description": "Reads logs\nand explains failures",
            "role": "You read logs.\n- Be brief.\n- Quote lines.",
            "tools": "read_only",
            "model": "p/claude-haiku",
            "max_steps": 25,
            "skills": ["rust"],
            "skills_on_demand": ["git"]
        })
    }

    #[test]
    fn parses_tool_call() {
        let a = parse(&call_reply(full_args())).unwrap();
        assert_eq!(a.name, "Log reader");
        assert_eq!(a.description, "Reads logs and explains failures");
        assert_eq!(a.role, "You read logs.\n- Be brief.\n- Quote lines.");
        assert_eq!(a.tools, ToolAccess::ReadOnly);
        assert_eq!(a.model, "p/claude-haiku");
        assert_eq!(a.max_steps, 25);
        assert_eq!(a.skills, vec!["rust"]);
        assert_eq!(a.skills_on_demand, vec!["git"]);
        assert_eq!(a.version, 1);
        assert!(!a.id.is_empty());
    }

    #[test]
    fn prefers_draft_agent_call_over_other_calls() {
        let mut r = call_reply(full_args());
        r.calls.insert(
            0,
            Call {
                id: "c0".into(),
                name: "other".into(),
                arguments: json!({"role": "wrong", "name": "Wrong"}).to_string(),
            },
        );
        assert_eq!(parse(&r).unwrap().name, "Log reader");
    }

    #[test]
    fn uses_first_call_when_name_differs() {
        let mut r = call_reply(full_args());
        r.calls[0].name = "something_else".into();
        assert_eq!(parse(&r).unwrap().name, "Log reader");
    }

    #[test]
    fn parses_plain_json_content() {
        let a = parse(&text_reply(&full_args().to_string())).unwrap();
        assert_eq!(a.name, "Log reader");
        assert_eq!(a.tools, ToolAccess::ReadOnly);
    }

    #[test]
    fn parses_fenced_json_content() {
        let text = format!("Here you go:\n```json\n{}\n```\nEnjoy!", full_args());
        let a = parse(&text_reply(&text)).unwrap();
        assert_eq!(a.name, "Log reader");
        assert_eq!(a.max_steps, 25);
    }

    #[test]
    fn parses_json_inside_prose() {
        let text = format!("Sure. {} Hope that helps.", full_args());
        assert_eq!(parse(&text_reply(&text)).unwrap().name, "Log reader");
    }

    #[test]
    fn falls_back_to_content_when_call_is_not_json() {
        let mut r = text_reply(&full_args().to_string());
        r.calls = vec![Call {
            id: "c".into(),
            name: TOOL_NAME.into(),
            arguments: "{not json".into(),
        }];
        assert_eq!(parse(&r).unwrap().name, "Log reader");
    }

    #[test]
    fn no_definition_is_an_error() {
        assert!(parse(&text_reply("I cannot do that.")).is_err());
        assert!(parse(&Reply::default()).is_err());
    }

    #[test]
    fn empty_role_is_an_error() {
        let e = parse(&call_reply(json!({"name": "X", "role": "  \n "}))).unwrap_err();
        assert!(e.to_string().contains("empty role prompt"));
        assert!(parse(&call_reply(json!({"name": "X"}))).is_err());
    }

    #[test]
    fn name_is_cleaned_and_falls_back() {
        let a = parse(&call_reply(
            json!({"name": " \"Fixer\"\n", "role": "You fix."}),
        ))
        .unwrap();
        assert_eq!(a.name, "Fixer");
        let a = parse(&call_reply(json!({
            "role": "You fix.",
            "description": "Fixes failing tests quickly, then reports."
        })))
        .unwrap();
        assert_eq!(a.name, "Fixes failing tests");
        let a = parse(&call_reply(json!({"name": "", "role": "You fix."}))).unwrap();
        assert_eq!(a.name, "New agent");
    }

    #[test]
    fn model_resolution() {
        let m = models();
        // exact
        assert_eq!(
            resolve_model("p/claude-opus", &m, "p/claude-sonnet"),
            "p/claude-opus"
        );
        // case-insensitive
        assert_eq!(
            resolve_model("P/Claude-Opus", &m, "p/claude-sonnet"),
            "p/claude-opus"
        );
        // provider prefix stripped
        assert_eq!(
            resolve_model("claude-haiku", &m, "p/claude-sonnet"),
            "p/claude-haiku"
        );
        assert_eq!(
            resolve_model("Claude-Haiku", &m, "p/claude-sonnet"),
            "p/claude-haiku"
        );
        // unknown and empty
        assert_eq!(
            resolve_model("gpt-9", &m, "p/claude-sonnet"),
            "p/claude-sonnet"
        );
        assert_eq!(resolve_model("", &m, "p/claude-sonnet"), "p/claude-sonnet");
        // a suffix must start at a path boundary
        assert_eq!(
            resolve_model("haiku", &m, "p/claude-sonnet"),
            "p/claude-sonnet"
        );
    }

    #[test]
    fn model_resolution_in_parsed_agent() {
        let mut args = full_args();
        args["model"] = json!("claude-opus");
        assert_eq!(
            parse(&call_reply(args.clone())).unwrap().model,
            "p/claude-opus"
        );
        args["model"] = json!("nonexistent");
        assert_eq!(
            parse(&call_reply(args.clone())).unwrap().model,
            "p/claude-sonnet"
        );
        args.as_object_mut().unwrap().remove("model");
        assert_eq!(parse(&call_reply(args)).unwrap().model, "p/claude-sonnet");
    }

    #[test]
    fn skills_are_filtered_deduplicated_and_disjoint() {
        let mut args = full_args();
        args["skills"] = json!(["rust", "ghost", "rust", "git", 5]);
        args["skills_on_demand"] = json!(["git", "docs", "docs", "rust", "phantom"]);
        let a = parse(&call_reply(args)).unwrap();
        assert_eq!(a.skills, vec!["rust", "git"]);
        assert_eq!(a.skills_on_demand, vec!["docs"]);
    }

    #[test]
    fn skills_missing_or_wrong_type_are_empty() {
        let mut args = full_args();
        args["skills"] = json!("rust");
        args.as_object_mut().unwrap().remove("skills_on_demand");
        let a = parse(&call_reply(args)).unwrap();
        assert!(a.skills.is_empty());
        assert!(a.skills_on_demand.is_empty());
    }

    #[test]
    fn tool_access_parsing() {
        for (s, want) in [
            ("none", ToolAccess::None),
            ("No tools", ToolAccess::None),
            ("read_only", ToolAccess::ReadOnly),
            ("ReadOnly", ToolAccess::ReadOnly),
            ("read only", ToolAccess::ReadOnly),
            ("Read-Only", ToolAccess::ReadOnly),
            ("read_and_run", ToolAccess::ReadAndRun),
            ("readandrun", ToolAccess::ReadAndRun),
            ("Read And Run", ToolAccess::ReadAndRun),
            ("read-and-run", ToolAccess::ReadAndRun),
            ("full", ToolAccess::Full),
            ("FULL", ToolAccess::Full),
            ("everything", ToolAccess::Full),
            ("", ToolAccess::Full),
        ] {
            assert_eq!(parse_tools(s), want, "{s:?}");
        }
    }

    #[test]
    fn max_steps_is_clamped() {
        let steps = |v: Value| {
            let mut args = full_args();
            args["max_steps"] = v;
            parse(&call_reply(args)).unwrap().max_steps
        };
        assert_eq!(steps(json!(0)), 1);
        assert_eq!(steps(json!(1)), 1);
        assert_eq!(steps(json!(60)), 60);
        assert_eq!(steps(json!(500)), 500);
        assert_eq!(steps(json!(100000)), 500);
        assert_eq!(steps(json!(u64::MAX)), 500);
        assert_eq!(steps(json!(-3)), default_max_steps());
        assert_eq!(steps(json!("many")), default_max_steps());
        let mut args = full_args();
        args.as_object_mut().unwrap().remove("max_steps");
        assert_eq!(
            parse(&call_reply(args)).unwrap().max_steps,
            default_max_steps()
        );
    }

    #[test]
    fn system_prompt_lists_context() {
        let p = system_prompt(
            &models(),
            "p/claude-sonnet",
            &[skill("rust")],
            &["Fixer".into()],
            "Answer in German.",
        );
        assert!(p.contains("- p/claude-sonnet (workspace default)"));
        assert!(p.contains("- p/claude-haiku\n"));
        assert!(p.contains("- rust: about rust"));
        assert!(p.contains("- Fixer"));
        assert!(p.contains("Answer in German."));
        assert!(p.contains("draft_agent"));
        let p = system_prompt(&models(), "p/claude-sonnet", &[], &[], "  ");
        assert!(p.contains("none installed"));
        assert!(!p.contains("Existing agent names"));
        assert!(!p.contains("Workspace rules"));
    }

    #[test]
    fn tool_spec_schema() {
        let t = tool_spec();
        assert_eq!(t.name, "draft_agent");
        let required: Vec<&str> = t.parameters["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(required, ["name", "description", "role", "tools", "model"]);
        assert_eq!(
            t.parameters["properties"]["tools"]["enum"],
            json!(["none", "read_only", "read_and_run", "full"])
        );
    }

    // ---- end to end against a local SSE server ---------------------------------

    /// Serves one scripted `draft_agent` call per request; records request bodies.
    fn serve(args: Value, bodies: Arc<Mutex<Vec<String>>>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in l.incoming().flatten() {
                let args = args.clone();
                let bodies = bodies.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut len = 0usize;
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let mut body = vec![0; len];
                    r.read_exact(&mut body).unwrap();
                    bodies
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&body).to_string());
                    let call = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"draft_agent","arguments":args.to_string()}}]},"finish_reason":null}]});
                    let end = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":100}});
                    let sse = format!("data: {call}\n\ndata: {end}\n\ndata: [DONE]\n\n");
                    let mut s = stream;
                    let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", sse.len(), sse);
                });
            }
        });
        format!("http://{addr}/v1")
    }

    fn engine(url: &str, default_model: &str) -> Engine {
        let config = Config {
            default_model: default_model.into(),
            providers: vec![Provider {
                id: "t".into(),
                kind: "openai".into(),
                base_url: url.into(),
                api_key: "x".into(),
                api_key_env: String::new(),
            }],
            models: vec!["t/test-haiku".into(), "t/test-sonnet".into()],
            rules: "Write in French.".into(),
            skill_dirs: vec![],
            ..Config::default()
        };
        Engine::new(Store::open_in_memory().unwrap(), config)
    }

    #[test]
    fn draft_agent_end_to_end() {
        let bodies = Arc::new(Mutex::new(vec![]));
        let url = serve(
            json!({
                "name": "Triager",
                "description": "Sorts incoming issues",
                "role": "You triage issues.",
                "tools": "none",
                "model": "test-haiku",
                "max_steps": 10,
                "skills": ["not-installed"]
            }),
            bodies.clone(),
        );
        let e = engine(&url, "t/test-sonnet");
        let draft = e
            .draft_agent(
                "  an agent that triages issues  ",
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert_eq!(draft.drafted_by, "t/test-sonnet");
        assert_eq!(draft.agent.name, "Triager");
        assert_eq!(draft.agent.tools, ToolAccess::None);
        assert_eq!(draft.agent.model, "t/test-haiku");
        assert_eq!(draft.agent.max_steps, 10);
        assert!(draft.agent.skills.is_empty());
        // No price entry matches "test-sonnet": cost is zero but finite.
        assert!(draft.cost_usd >= 0.0);

        // Nothing is saved, the event is logged.
        assert!(e.store().agents().is_empty());
        let events = e.store().events(10);
        let ev = events.iter().find(|ev| ev.kind == "agent.drafted").unwrap();
        assert!(
            ev.text.starts_with("AI drafted agent Triager · $"),
            "{}",
            ev.text
        );

        // The request used the workspace model, the tool and the context.
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let req: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(req["model"], "test-sonnet");
        assert_eq!(req["tools"][0]["function"]["name"], "draft_agent");
        let sys = req["messages"][0]["content"].as_str().unwrap();
        assert!(sys.contains("t/test-haiku"));
        assert!(sys.contains("Write in French."));
        assert_eq!(
            req["messages"][1]["content"],
            "an agent that triages issues"
        );
    }

    #[test]
    fn create_agent_with_ai_saves_and_dedups() {
        let url = serve(
            json!({"name": "FIXER", "role": "You fix."}),
            Arc::new(Mutex::new(vec![])),
        );
        let e = engine(&url, "t/test-sonnet");
        let first = e
            .create_agent_with_ai("fix things", Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(first.agent.name, "FIXER");
        let second = e
            .create_agent_with_ai("fix things", Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(second.agent.name, "FIXER 2");
        let saved = e.store().agents();
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().any(|a| a.id == second.agent.id));
        assert!(e
            .store()
            .events(10)
            .iter()
            .any(|ev| ev.kind == "agent.created" && ev.text == "Created agent FIXER 2"));
    }

    #[test]
    fn draft_agent_cost_uses_workspace_model_price() {
        let url = serve(
            json!({"name": "A", "role": "You help."}),
            Arc::new(Mutex::new(vec![])),
        );
        // "haiku" price: $1/M in, $5/M out; 1000 in + 100 out = $0.0015.
        let e = engine(&url, "t/claude-haiku");
        let d = e
            .draft_agent("help", Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!((d.cost_usd - 0.0015).abs() < 1e-9, "{}", d.cost_usd);
        assert_eq!(d.agent.model, "t/claude-haiku");
        assert_eq!(d.agent.tools, ToolAccess::Full);
    }

    #[test]
    fn draft_agent_rejects_empty_prompt() {
        let e = engine("http://127.0.0.1:1/v1", "t/test-sonnet");
        let err = e
            .draft_agent(" \n\t ", Arc::new(AtomicBool::new(false)))
            .unwrap_err();
        assert_eq!(err.to_string(), "describe what the agent should do");
    }

    #[test]
    fn draft_agent_needs_a_workspace_model() {
        let e = engine("http://127.0.0.1:1/v1", "  ");
        let err = e
            .draft_agent("do things", Arc::new(AtomicBool::new(false)))
            .unwrap_err();
        assert!(err.to_string().contains("no workspace model configured"));
    }

    #[test]
    fn draft_agent_needs_a_provider() {
        let e = Engine::new(
            Store::open_in_memory().unwrap(),
            Config {
                default_model: "t/m".into(),
                skill_dirs: vec![],
                models: vec!["t/m".into()],
                ..Config::default()
            },
        );
        let err = e
            .draft_agent("do things", Arc::new(AtomicBool::new(false)))
            .unwrap_err();
        assert!(err.to_string().contains("no model provider configured"));
        assert!(e
            .store()
            .events(10)
            .iter()
            .all(|ev| ev.kind != "agent.drafted"));
    }
}
