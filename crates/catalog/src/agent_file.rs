//! The Markdown file format of agents and orchestrators.
//!
//! Top-level keys are what Claude Code agent files have (`name`, `description`, `tools`,
//! `model`), so such a file opens in both tools. Everything else sits in a `harness:` block:
//!
//! ```text
//! ---
//! name: Payments lead
//! description: Splits payments work across the team
//! tools: [Read, Grep, Glob, Bash]
//! model: nr/cc/claude-opus-5-5
//! harness:
//!   kind: orchestrator
//!   uses: [chat, issues, pipelines]
//!   access: read_and_run
//!   skills_forced: [payments-retries]
//!   spawn:
//!     enabled: true
//!     max_depth: 2
//!   team: [agent:fixer, pipeline:pay-triage]
//! ---
//! The role (system prompt) in Markdown.
//! ```

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use base::frontmatter::{self, Value};
use model::{
    AgentDef, ModelRef, Origin, Runnable, RunnableKind, RunnableRef, SkillPolicy, SpawnPolicy, ToolAccess,
    ToolGrants, Uses,
};

use crate::claude;

type Fields = BTreeMap<String, Value>;

/// What a parsed agent file holds, before it is placed anywhere.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentFile {
    pub kind: RunnableKind,
    pub name: String,
    pub description: String,
    pub uses: Uses,
    pub paused: bool,
    pub archived: bool,
    pub def: AgentDef,
}

impl AgentFile {
    /// The list row for this file under `id`, as found at `origin`.
    pub fn into_runnable(self, id: &str, origin: Origin, path: Option<std::path::PathBuf>) -> Runnable {
        let reference = match self.kind {
            RunnableKind::Orchestrator => RunnableRef::orchestrator(id),
            _ => RunnableRef::agent(id),
        };
        Runnable {
            reference,
            kind: self.kind,
            name: self.name,
            description: self.description,
            origin,
            uses: self.uses,
            paused: self.paused,
            archived: self.archived,
            agent: Some(self.def),
            path,
        }
    }
}

/// Parses an agent or orchestrator file. `id` (the file stem) names the agent when the header has
/// no `name`. With `claude_code` the file is a Claude Code agent: its `tools:` line decides the
/// access level and a `model:` alias is dropped.
pub fn parse_agent(text: &str, id: &str, claude_code: bool) -> Result<AgentFile> {
    let doc = frontmatter::parse(text);
    if doc.fields.is_empty() {
        bail!("no header: an agent file starts with a `---` block holding at least a name");
    }
    let empty = Fields::new();
    let h = doc.map("harness").unwrap_or(&empty);

    let kind = match str_of(h, "kind") {
        None | Some("agent") => RunnableKind::Agent,
        Some("orchestrator") => RunnableKind::Orchestrator,
        Some(other) => bail!("harness.kind: expected agent or orchestrator, found `{other}`"),
    };
    let tools = if h.contains_key("access") || !doc.fields.contains_key("tools") {
        let access = match str_of(h, "access") {
            None => ToolAccess::Full,
            Some("read_only") => ToolAccess::ReadOnly,
            Some("read_and_run") => ToolAccess::ReadAndRun,
            Some("full") => ToolAccess::Full,
            Some(other) => bail!("harness.access: expected read_only, read_and_run or full, found `{other}`"),
        };
        ToolGrants { access, allow: list_of(h, "allow"), deny: list_of(h, "deny") }
    } else {
        // A Claude Code file: no `access`, so its `tools:` line decides.
        claude::map_tools(Some(&doc.list("tools")))
    };
    let model = match doc.str("model") {
        Some(m) if claude_code => claude::model_from_claude(m),
        Some(m) => ModelRef::parse(m.trim()),
        None => ModelRef::default(),
    };
    let uses = match h.get("uses") {
        None => Uses::default(),
        Some(_) => {
            let mut uses = Uses { chat: false, issues: false, pipelines: false, autopilots: false };
            for item in list_of(h, "uses") {
                match item.as_str() {
                    "chat" => uses.chat = true,
                    "issues" => uses.issues = true,
                    "pipelines" => uses.pipelines = true,
                    "autopilots" => uses.autopilots = true,
                    other => bail!("harness.uses: unknown place `{other}`"),
                }
            }
            uses
        }
    };
    let spawn = match h.get("spawn") {
        Some(Value::Map(s)) => {
            let d = SpawnPolicy::default();
            SpawnPolicy {
                enabled: bool_of(s, "enabled", "harness.spawn.enabled")?.unwrap_or(d.enabled),
                alive_at_once: num_of(s, "alive_at_once", "harness.spawn.alive_at_once")?.unwrap_or(d.alive_at_once),
                max_depth: num_of(s, "max_depth", "harness.spawn.max_depth")?.unwrap_or(d.max_depth),
                per_run: num_of(s, "per_run", "harness.spawn.per_run")?.unwrap_or(d.per_run),
                budget_usd: num_of(s, "budget_usd", "harness.spawn.budget_usd")?,
            }
        }
        Some(_) => bail!("harness.spawn: expected a block with enabled, alive_at_once…"),
        None => SpawnPolicy::default(),
    };
    let name = doc.str("name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(id).to_string();
    let def = AgentDef {
        role: doc.body.trim().to_string(),
        model,
        runtime: str_of(h, "runtime").unwrap_or_default().to_string(),
        tools,
        skills: SkillPolicy { forced: list_of(h, "skills_forced"), auto: list_of(h, "skills_auto") },
        spawn,
        budget_usd: num_of(h, "budget_usd", "harness.budget_usd")?,
        max_steps: num_of(h, "max_steps", "harness.max_steps")?.unwrap_or(0),
        team: list_of(h, "team").into_iter().map(RunnableRef).collect(),
        routing: str_of(h, "routing").unwrap_or_default().to_string(),
    };
    Ok(AgentFile {
        kind,
        name,
        description: doc.str("description").unwrap_or_default().trim().to_string(),
        uses,
        paused: bool_of(h, "paused", "harness.paused")?.unwrap_or(false),
        archived: bool_of(h, "archived", "harness.archived")?.unwrap_or(false),
        def,
    })
}

/// The file text of an agent or orchestrator; `parse_agent` reads it back to the same values
/// (the role is stored trimmed).
pub fn render_agent(runnable: &Runnable) -> Result<String> {
    let def = runnable.agent.as_ref().ok_or_else(|| anyhow!("`{}` is not an agent", runnable.reference))?;
    let mut top = Fields::new();
    top.insert("name".into(), Value::Str(runnable.name.clone()));
    if !runnable.description.is_empty() {
        top.insert("description".into(), Value::Str(runnable.description.clone()));
    }
    top.insert("tools".into(), Value::List(claude::claude_tools(&def.tools)));
    if !def.model.is_empty() {
        top.insert("model".into(), Value::Str(def.model.to_string()));
    }

    let mut h = Fields::new();
    let kind = if runnable.kind == RunnableKind::Orchestrator { "orchestrator" } else { "agent" };
    h.insert("kind".into(), s(kind));
    if runnable.uses != Uses::default() {
        let u = runnable.uses;
        let on = [("chat", u.chat), ("issues", u.issues), ("pipelines", u.pipelines), ("autopilots", u.autopilots)];
        h.insert("uses".into(), Value::List(on.iter().filter(|(_, v)| *v).map(|(k, _)| k.to_string()).collect()));
    }
    if !def.runtime.is_empty() {
        h.insert("runtime".into(), s(&def.runtime));
    }
    let access = match def.tools.access {
        ToolAccess::ReadOnly => "read_only",
        ToolAccess::ReadAndRun => "read_and_run",
        ToolAccess::Full => "full",
    };
    h.insert("access".into(), s(access));
    put_list(&mut h, "allow", &def.tools.allow);
    put_list(&mut h, "deny", &def.tools.deny);
    put_list(&mut h, "skills_forced", &def.skills.forced);
    put_list(&mut h, "skills_auto", &def.skills.auto);
    if def.spawn != SpawnPolicy::default() {
        let p = &def.spawn;
        let mut m = Fields::new();
        m.insert("enabled".into(), s(&p.enabled.to_string()));
        m.insert("alive_at_once".into(), s(&p.alive_at_once.to_string()));
        m.insert("max_depth".into(), s(&p.max_depth.to_string()));
        m.insert("per_run".into(), s(&p.per_run.to_string()));
        if let Some(b) = p.budget_usd {
            m.insert("budget_usd".into(), s(&b.to_string()));
        }
        h.insert("spawn".into(), Value::Map(m));
    }
    if let Some(b) = def.budget_usd {
        h.insert("budget_usd".into(), s(&b.to_string()));
    }
    if def.max_steps > 0 {
        h.insert("max_steps".into(), s(&def.max_steps.to_string()));
    }
    let team: Vec<String> = def.team.iter().map(|r| r.0.clone()).collect();
    put_list(&mut h, "team", &team);
    if !def.routing.is_empty() {
        h.insert("routing".into(), s(&def.routing));
    }
    if runnable.paused {
        h.insert("paused".into(), s("true"));
    }
    if runnable.archived {
        h.insert("archived".into(), s("true"));
    }
    top.insert("harness".into(), Value::Map(h));

    let mut body = def.role.trim().to_string();
    body.push('\n');
    Ok(frontmatter::render(&top, &body))
}

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn put_list(map: &mut Fields, key: &str, items: &[String]) {
    if !items.is_empty() {
        map.insert(key.into(), Value::List(items.to_vec()));
    }
}

fn str_of<'a>(map: &'a Fields, key: &str) -> Option<&'a str> {
    match map.get(key) {
        Some(Value::Str(v)) if !v.trim().is_empty() => Some(v.trim()),
        _ => None,
    }
}

fn list_of(map: &Fields, key: &str) -> Vec<String> {
    match map.get(key) {
        Some(Value::List(items)) => items.clone(),
        Some(Value::Str(v)) => v.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn bool_of(map: &Fields, key: &str, what: &str) -> Result<Option<bool>> {
    match str_of(map, key) {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(other) => bail!("{what}: expected true or false, found `{other}`"),
    }
}

fn num_of<T: std::str::FromStr>(map: &Fields, key: &str, what: &str) -> Result<Option<T>> {
    match str_of(map, key) {
        None => Ok(None),
        Some(v) => v.parse().map(Some).map_err(|_| anyhow!("{what}: `{v}` is not a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Runnable {
        Runnable {
            reference: RunnableRef::orchestrator("lead"),
            kind: RunnableKind::Orchestrator,
            name: "Payments lead".into(),
            description: "Splits work: across the team".into(),
            origin: Origin::Library,
            uses: Uses { chat: true, issues: false, pipelines: true, autopilots: true },
            paused: true,
            archived: true,
            agent: Some(AgentDef {
                role: "You lead.\n\n- Split the issue\n- Hand parts out\n\n```\ncode: here\n```".into(),
                model: ModelRef::parse("nr/cc/claude-opus-5-5"),
                runtime: "local".into(),
                tools: ToolGrants {
                    access: ToolAccess::ReadAndRun,
                    allow: vec!["mcp__github__create_pull_request".into()],
                    deny: vec!["bash".into()],
                },
                skills: SkillPolicy { forced: vec!["payments-retries".into()], auto: vec!["jira-hygiene".into(), "x".into()] },
                spawn: SpawnPolicy { enabled: true, alive_at_once: 4, max_depth: 3, per_run: 5, budget_usd: Some(0.5) },
                budget_usd: Some(2.25),
                max_steps: 40,
                team: vec![RunnableRef::agent("fixer"), RunnableRef::pipeline("pay-triage")],
                routing: "Cheapest teammate with the right skill: asks when plans disagree.".into(),
            }),
            path: None,
        }
    }

    #[test]
    fn every_field_round_trips() {
        let runnable = full();
        let text = render_agent(&runnable).unwrap();
        let back = parse_agent(&text, "lead", false).unwrap().into_runnable("lead", Origin::Library, None);
        assert_eq!(back, runnable, "{text}");
    }

    #[test]
    fn a_default_agent_round_trips_and_stays_small() {
        let mut runnable = full();
        runnable.kind = RunnableKind::Agent;
        runnable.reference = RunnableRef::agent("lead");
        runnable.uses = Uses::default();
        runnable.paused = false;
        runnable.archived = false;
        runnable.agent = Some(AgentDef { role: "Do it.".into(), ..Default::default() });
        let text = render_agent(&runnable).unwrap();
        assert!(!text.contains("spawn") && !text.contains("budget") && !text.contains("uses"), "{text}");
        assert!(text.contains("tools: [Read, Grep, Glob, Edit, Write, Bash]") || text.contains("tools:"), "{text}");
        let back = parse_agent(&text, "lead", false).unwrap().into_runnable("lead", Origin::Library, None);
        assert_eq!(back, runnable);
    }

    #[test]
    fn all_uses_off_is_not_the_default() {
        let mut runnable = full();
        runnable.uses = Uses { chat: false, issues: false, pipelines: false, autopilots: false };
        let back = parse_agent(&render_agent(&runnable).unwrap(), "lead", false).unwrap();
        assert_eq!(back.uses, runnable.uses);
    }

    #[test]
    fn a_claude_code_file_maps_tools_and_model() {
        let text = "---\nname: reviewer\ndescription: Reviews\ntools: Read, Grep, Bash\nmodel: sonnet\n---\nBe careful.\n";
        let file = parse_agent(text, "reviewer", true).unwrap();
        assert_eq!(file.def.tools.access, ToolAccess::ReadAndRun);
        assert!(file.def.model.is_empty());
        assert_eq!(file.def.role, "Be careful.");
        assert_eq!(file.kind, RunnableKind::Agent);
        let kept = parse_agent("---\nmodel: nr/cc/claude-haiku-5-5\n---\nx", "m", true).unwrap();
        assert_eq!(kept.def.model.short(), "claude-haiku-5-5");
        assert_eq!(kept.name, "m");
    }

    #[test]
    fn bad_values_are_errors_with_the_key_named() {
        let err = |h: &str| parse_agent(&format!("---\nname: a\nharness:\n{h}\n---\nx"), "a", false).unwrap_err().to_string();
        assert!(err("  kind: robot").contains("harness.kind"));
        assert!(err("  access: root").contains("harness.access"));
        assert!(err("  paused: maybe").contains("harness.paused"));
        assert!(err("  max_steps: lots").contains("harness.max_steps"));
        assert!(err("  uses: [chat, mail]").contains("mail"));
        assert!(parse_agent("just text", "a", false).unwrap_err().to_string().contains("no header"));
    }
}
