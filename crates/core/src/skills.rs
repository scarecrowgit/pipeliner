//! Skills: folders with a `SKILL.md` (YAML-ish front matter `name:` / `description:`).

use std::path::PathBuf;

use crate::config::{expand_tilde, Config};

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// All skills found in the configured folders. Earlier folders win on name clashes.
pub fn list(config: &Config) -> Vec<Skill> {
    let mut out: Vec<Skill> = vec![];
    for dir in &config.skill_dirs {
        let dir = expand_tilde(dir);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let file = e.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let dir_name = e.file_name().to_string_lossy().to_string();
            let (name, description) = front_matter(&text);
            let name = name.unwrap_or(dir_name);
            if out.iter().any(|s| s.name == name) {
                continue;
            }
            out.push(Skill { name, description: description.unwrap_or_default(), path: file });
        }
    }
    out
}

/// The body of a skill (front matter stripped), with its base directory noted.
pub fn load(config: &Config, name: &str) -> Option<String> {
    let skill = list(config).into_iter().find(|s| s.name == name)?;
    let text = std::fs::read_to_string(&skill.path).ok()?;
    let body = strip_front_matter(&text);
    let base = skill.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
    Some(format!("# Skill: {name}\nBase directory: {base}\n\n{body}"))
}

fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut desc = None;
    if let Some(rest) = text.strip_prefix("---") {
        for line in rest.lines().skip(1) {
            if line.trim() == "---" {
                break;
            }
            if let Some(v) = line.strip_prefix("name:") {
                name = Some(v.trim().trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("description:") {
                desc = Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    (name, desc)
}

fn strip_front_matter(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_front_matter() {
        let t = "---\nname: x\ndescription: does y\n---\nbody";
        assert_eq!(front_matter(t), (Some("x".into()), Some("does y".into())));
        assert_eq!(strip_front_matter(t), "body");
    }
}
