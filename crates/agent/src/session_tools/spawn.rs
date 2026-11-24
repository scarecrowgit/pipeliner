//! `spawn_agent`: its spec and input, and what a sub-agent is made of. Running sub-agents is
//! the loop's part (`subagents.rs`).

use model::{RunnableRef, SessionId, ToolAccess, ToolGrants, ToolSpec};
use serde_json::{Value, json};

use super::names::SPAWN_AGENT;
use super::{object, optional_str, required_str};
use crate::spec::{AgentSpec, Finish, SubagentTemplate};

/// The display name of a sub-agent created without a template.
pub(crate) const GENERAL: &str = "Sub-agent";

pub(crate) fn spec(templates: &[SubagentTemplate]) -> ToolSpec {
    let mut description = String::from(
        "Create a sub-agent and give it one task. It works in the same folder with its own context and returns \
         a summary; you see nothing else of its work. Use it for a self-contained piece of work or research \
         whose details you do not need. Give it everything it must know: it has not seen this conversation. \
         Several calls in one message run at the same time.",
    );
    let mut agent = json!({ "type": "string", "description": "Leave out for a general-purpose sub-agent with your own tools." });
    if !templates.is_empty() {
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        description.push_str(" The kinds you can create are listed in the system prompt.");
        agent = json!({
            "type": "string",
            "enum": names,
            "description": "The kind of sub-agent; leave out for a general-purpose one with your own tools."
        });
    }
    ToolSpec {
        name: SPAWN_AGENT.into(),
        description,
        input_schema: json!({
            "type": "object",
            "properties": {
                "agent": agent,
                "task": { "type": "string", "description": "What to do, what to return, and how to check it" },
                "label": { "type": "string", "description": "A few words naming the task, shown to the person" }
            },
            "required": ["task"]
        }),
    }
}

/// A `spawn_agent` call's input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpawnInput {
    /// The template name; `None` for the general-purpose sub-agent.
    pub agent: Option<String>,
    pub task: String,
    pub label: Option<String>,
}

pub(crate) fn parse(input: &Value) -> Result<SpawnInput, String> {
    let map = object(SPAWN_AGENT, input)?;
    Ok(SpawnInput {
        agent: optional_str(&map, "agent").map(str::to_string),
        task: required_str(SPAWN_AGENT, &map, "task")?.to_string(),
        label: optional_str(&map, "label").map(str::to_string),
    })
}

/// The "Sub-agents" section of the system prompt.
pub(crate) fn prompt_section(templates: &[SubagentTemplate]) -> String {
    let mut out = String::from(
        "You can hand self-contained work to sub-agents with `spawn_agent`. A sub-agent starts with only the \
         task you write, works in the same folder and answers with a summary.",
    );
    if !templates.is_empty() {
        out.push_str(" Kinds you can create by name:\n");
        for template in templates {
            out.push_str(&format!("\n- {}: {}", template.name, base::fmt::first_line(&template.description)));
        }
    }
    out
}

/// Whether `pattern` (a name, or a prefix ending in `*`) covers the tool `name`.
pub(crate) fn pattern_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

fn rank(access: ToolAccess) -> u8 {
    match access {
        ToolAccess::ReadOnly => 0,
        ToolAccess::ReadAndRun => 1,
        ToolAccess::Full => 2,
    }
}

/// The grants of a sub-agent: what its template asks for, cut down to what the parent has.
pub(crate) fn narrow(parent: &ToolGrants, child: &ToolGrants) -> ToolGrants {
    let access = if rank(child.access) < rank(parent.access) { child.access } else { parent.access };
    // An extra tool is kept when both sides name it, by name or by pattern.
    let covers = |pattern: &str, entry: &str| pattern_matches(pattern, entry.trim_end_matches('*'));
    let mut allow: Vec<String> = Vec::new();
    let both = child
        .allow
        .iter()
        .filter(|c| parent.allow.iter().any(|p| covers(p, c)))
        .chain(parent.allow.iter().filter(|p| child.allow.iter().any(|c| covers(c, p))));
    for entry in both {
        if !allow.contains(entry) {
            allow.push(entry.clone());
        }
    }
    let mut deny = parent.deny.clone();
    for entry in &child.deny {
        if !deny.contains(entry) {
            deny.push(entry.clone());
        }
    }
    ToolGrants { access, allow, deny }
}

const HANDBACK: &str = "Work only on the task you are given. When you are done, reply with a summary of what you \
                        did, what you found and anything your creator must know: your last message is all they will see.";

/// The spec of a sub-agent of `parent`: the template's role, model and tools (the general
/// one takes the parent's), the parent's forced skills, limits and spawn policy, `budget_usd`
/// as what it may spend, and a reply as the way to finish.
pub(crate) fn child_spec(
    parent: &AgentSpec,
    template: Option<&SubagentTemplate>,
    name: &str,
    session: &SessionId,
    budget_usd: Option<f64>,
) -> AgentSpec {
    let mut skills = template.map(|t| t.skills.clone()).unwrap_or_else(|| parent.skills.clone());
    for forced in &parent.skills.forced {
        if !skills.forced.contains(forced) {
            skills.forced.push(forced.clone());
        }
    }
    let role = match template.map(|t| t.role.trim()).filter(|role| !role.is_empty()) {
        Some(role) => format!("{role}\n\nYou were created by {} as a sub-agent. {HANDBACK}", parent.name),
        None => format!("You are a sub-agent created by {} to do one task. {HANDBACK}", parent.name),
    };
    AgentSpec {
        name: name.to_string(),
        runnable: RunnableRef::spawned(session.as_str()),
        role,
        model: template.and_then(|t| t.model.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| parent.model.clone()),
        reasoning: parent.reasoning,
        tools: match template {
            Some(template) => narrow(&parent.tools, &template.tools),
            None => parent.tools.clone(),
        },
        skills,
        spawn: parent.spawn.clone(),
        templates: parent.templates.clone(),
        budget_usd,
        max_steps: template.map_or(0, |t| t.max_steps),
        finish: Finish::Reply,
        limits: parent.limits.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(access: ToolAccess, allow: &[&str], deny: &[&str]) -> ToolGrants {
        ToolGrants {
            access,
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_child_never_gets_more_than_its_parent() {
        let parent = grants(ToolAccess::ReadAndRun, &["mcp__github__*", "mcp__docs__search"], &["bash"]);
        let child = grants(ToolAccess::Full, &["mcp__github__create_issue", "mcp__docs__*", "mcp__slack__post"], &["grep"]);
        let got = narrow(&parent, &child);
        assert_eq!(got.access, ToolAccess::ReadAndRun);
        assert_eq!(got.allow, ["mcp__github__create_issue", "mcp__docs__search"]);
        assert_eq!(got.deny, ["bash", "grep"]);
        let read_only = narrow(&grants(ToolAccess::Full, &["mcp__*"], &[]), &grants(ToolAccess::ReadOnly, &["mcp__a__*"], &[]));
        assert_eq!(read_only.access, ToolAccess::ReadOnly);
        assert_eq!(read_only.allow, ["mcp__a__*"]);
    }

    #[test]
    fn patterns_match_by_name_or_prefix() {
        assert!(pattern_matches("bash", "bash"));
        assert!(!pattern_matches("bash", "bash2"));
        assert!(pattern_matches("mcp__gh__*", "mcp__gh__create"));
        assert!(!pattern_matches("mcp__gh__*", "mcp__gl__create"));
        assert!(pattern_matches("*", "anything"));
    }

    #[test]
    fn input_needs_a_task() {
        let got = parse(&json!({"agent": "Reviewer", "task": " Check it ", "label": ""})).expect("valid");
        assert_eq!(got, SpawnInput { agent: Some("Reviewer".into()), task: "Check it".into(), label: None });
        assert!(parse(&json!({"agent": "Reviewer"})).unwrap_err().contains("`task`"));
    }

    #[test]
    fn the_spec_lists_template_names() {
        let templates = vec![SubagentTemplate { name: "Reviewer".into(), description: "Reads a diff\nmore".into(), ..Default::default() }];
        assert_eq!(spec(&templates).input_schema["properties"]["agent"]["enum"], json!(["Reviewer"]));
        assert!(spec(&[]).input_schema["properties"]["agent"].get("enum").is_none());
        assert!(prompt_section(&templates).ends_with("- Reviewer: Reads a diff"));
    }
}
