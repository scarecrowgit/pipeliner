//! `skill`: loading a skill's instructions into the conversation, and which skills an agent
//! sees.

use catalog::Catalog;
use model::{Skill, SkillPolicy, ToolSpec};
use serde_json::{Value, json};

use super::names::SKILL;
use super::{object, required_str};

/// Characters of a skill's description shown in the system prompt.
const MAX_DESCRIPTION_CHARS: usize = 300;

/// The skills the model may load: those the policy names in `auto` (every installed one when
/// it names none), without the forced ones, which are already in the prompt.
pub(crate) fn offered(policy: &SkillPolicy, catalog: &Catalog) -> Vec<Skill> {
    catalog
        .skills()
        .iter()
        .filter(|skill| policy.auto.is_empty() || policy.auto.contains(&skill.name))
        .filter(|skill| !policy.forced.contains(&skill.name))
        .cloned()
        .collect()
}

pub(crate) fn spec(offered: &[Skill]) -> ToolSpec {
    let names: Vec<&str> = offered.iter().map(|s| s.name.as_str()).collect();
    ToolSpec {
        name: SKILL.into(),
        description: "Load a skill: returns its full instructions and the list of its supporting files. Call it \
                      as soon as the task matches a skill's description in the system prompt, before doing the \
                      work, and follow the loaded instructions. Do not load a skill twice."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "enum": names, "description": "Skill name, exactly as listed in the system prompt." }
            },
            "required": ["name"]
        }),
    }
}

/// The skill name in the input.
pub(crate) fn parse(input: &Value) -> Result<String, String> {
    let map = object(SKILL, input)?;
    required_str(SKILL, &map, "name").map(str::to_string)
}

/// The "Skills" section of the system prompt: the offered skills by name and description,
/// then the forced ones in full. Empty when there are neither.
pub(crate) fn prompt_section(policy: &SkillPolicy, offered: &[Skill], catalog: &Catalog) -> String {
    let mut out = String::new();
    if !offered.is_empty() {
        out.push_str(
            "Skills are packaged instructions for particular kinds of work. When the task matches a skill's \
             description, call `skill` with its name first and follow what it returns. Load only skills that \
             are relevant.\n\nAvailable skills:\n",
        );
        for skill in offered {
            let description = skill.description.split_whitespace().collect::<Vec<_>>().join(" ");
            let description = base::fmt::truncate_chars(&description, MAX_DESCRIPTION_CHARS);
            out.push_str(&format!("- {}: {description}\n", skill.name));
        }
    }
    if !policy.forced.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("These skills are always loaded. Follow them; do not load them again.\n");
        for name in &policy.forced {
            match catalog.skill_text(name) {
                Some(text) => out.push_str(&format!("\n<skill name=\"{name}\">\n{}\n</skill>\n", text.trim())),
                None => out.push_str(&format!("\n(Skill `{name}` is not installed on this machine.)\n")),
            }
        }
    }
    out.trim_end().to_string()
}
