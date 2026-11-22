//! Agent skills (Claude Code compatible): folders containing a `SKILL.md` with YAML
//! frontmatter `name` and `description`, followed by markdown instructions.
//!
//! Discovery roots (a later root overrides an earlier one with the same name):
//! `~/.claude/skills/*/SKILL.md`, `~/.config/harness/skills/*/SKILL.md`,
//! `<workspace>/.claude/skills/*/SKILL.md`.
//!
//! The model sees every skill's name + description in the system prompt
//! ([`prompt_section`]) and loads one on demand with the `skill` tool ([`tool_spec`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Harness,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Folder that contains `SKILL.md`.
    pub dir: PathBuf,
    pub source: SkillSource,
}

/// Name of the tool that loads a skill.
pub const SKILL_TOOL: &str = "skill";

const SKILL_FILE: &str = "SKILL.md";
/// `SKILL.md` larger than this is skipped (not a real skill).
const MAX_SKILL_BYTES: u64 = 1024 * 1024;
/// At most this many extra files are listed after a loaded skill.
const MAX_LISTED_FILES: usize = 200;
/// Descriptions longer than this are cut in the system prompt.
const MAX_PROMPT_DESCRIPTION: usize = 1024;

/// Find skills for `workspace_root`. Never fails: unreadable/malformed skills are skipped.
/// Sorted by name.
pub fn discover(workspace_root: &Path) -> Vec<Skill> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut roots = Vec::new();
    if let Some(home) = &home {
        roots.push((home.join(".claude").join("skills"), SkillSource::User));
    }
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    if let Some(config) = config_dir {
        roots.push((config.join("harness").join("skills"), SkillSource::Harness));
    }
    roots.push((
        workspace_root.join(".claude").join("skills"),
        SkillSource::Project,
    ));
    discover_in(&roots)
}

/// Discovery over explicit roots; a later root overrides an earlier one with the same name.
pub(crate) fn discover_in(roots: &[(PathBuf, SkillSource)]) -> Vec<Skill> {
    let mut by_name: BTreeMap<String, Skill> = BTreeMap::new();
    for (root, source) in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            if let Some(skill) = read_skill(&dir, source.clone()) {
                by_name.insert(skill.name.clone(), skill);
            }
        }
    }
    by_name.into_values().collect()
}

fn read_skill(dir: &Path, source: SkillSource) -> Option<Skill> {
    let file = dir.join(SKILL_FILE);
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > MAX_SKILL_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let (front, _) = split_frontmatter(&text)?;
    let fields = parse_frontmatter(front);
    let description = fields
        .get("description")
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())?;
    let name = fields
        .get("name")
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))?;
    Some(Skill {
        name,
        description,
        dir: dir.to_path_buf(),
        source,
    })
}

/// `(frontmatter, body)`: the frontmatter is the block between the first two `---` lines
/// (the first must be the file's first non-empty line). `None` without frontmatter.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']).trim_end();
        match start {
            None => {
                if trimmed == "---" {
                    start = Some(offset + line.len());
                } else if !trimmed.is_empty() {
                    return None;
                }
            }
            Some(s) => {
                if trimmed == "---" {
                    return Some((&text[s..offset], &text[offset + line.len()..]));
                }
            }
        }
        offset += line.len();
    }
    None
}

/// Top-level `key: value` pairs. Values may be plain, single/double quoted, YAML folded
/// (`>`) or literal (`|`) block scalars, or plain scalars continued on indented lines.
/// Nested mappings (indented lines under a key with an empty value) are ignored.
fn parse_frontmatter(front: &str) -> BTreeMap<String, String> {
    let lines: Vec<&str> = front.lines().collect();
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty()
            || line.trim_start().starts_with('#')
            || line.starts_with([' ', '\t'])
        {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let rest = strip_comment(rest.trim());
        // Indented continuation lines belonging to this key.
        let mut cont = Vec::new();
        while i < lines.len() && (lines[i].trim().is_empty() || lines[i].starts_with([' ', '\t'])) {
            cont.push(lines[i]);
            i += 1;
        }
        while cont.last().is_some_and(|l| l.trim().is_empty()) {
            cont.pop();
        }
        let value = if let Some(indicator) = block_indicator(rest) {
            block_scalar(indicator, &cont)
        } else if rest.is_empty() {
            // A nested mapping/sequence or an empty value: not a string we care about.
            if cont.iter().any(|l| {
                let t = l.trim();
                t.starts_with("- ") || t == "-" || t.contains(": ") || t.ends_with(':')
            }) {
                continue;
            }
            fold_plain(&cont)
        } else if rest.starts_with('"') || rest.starts_with('\'') {
            quoted(rest, &cont)
        } else {
            let mut all = vec![rest];
            all.extend(cont.iter().map(|l| l.trim()));
            fold_plain(&all)
        };
        out.insert(key, value);
    }
    out
}

fn strip_comment(s: &str) -> &str {
    if s.starts_with('"') || s.starts_with('\'') {
        return s;
    }
    match s.find(" #") {
        Some(pos) => s[..pos].trim_end(),
        None if s.starts_with('#') => "",
        None => s,
    }
}

/// `(folded, chomp)` for `>`, `>-`, `>+`, `|`, `|-`, `|+` (an indentation digit is accepted).
fn block_indicator(s: &str) -> Option<(bool, char)> {
    let mut chars = s.chars();
    let folded = match chars.next()? {
        '>' => true,
        '|' => false,
        _ => return None,
    };
    let mut chomp = ' ';
    for c in chars {
        match c {
            '-' | '+' => chomp = c,
            '1'..='9' => {}
            _ => return None,
        }
    }
    Some((folded, chomp))
}

fn block_scalar((folded, chomp): (bool, char), lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let body: Vec<&str> = lines
        .iter()
        .map(|l| if l.len() >= indent { &l[indent..] } else { "" })
        .collect();
    let mut text = if folded {
        let mut s = String::new();
        let mut prev_blank = true;
        let mut prev_more_indented = false;
        for line in &body {
            if line.trim().is_empty() {
                s.push('\n');
                prev_blank = true;
                continue;
            }
            let more_indented = line.starts_with([' ', '\t']);
            if !prev_blank {
                s.push(if more_indented || prev_more_indented {
                    '\n'
                } else {
                    ' '
                });
            }
            s.push_str(line.trim_end());
            prev_blank = false;
            prev_more_indented = more_indented;
        }
        s
    } else {
        body.iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    };
    match chomp {
        '-' => {}
        _ => text.push('\n'),
    }
    text
}

/// Plain multi-line scalar: lines joined with spaces, blank lines become newlines.
fn fold_plain(lines: &[&str]) -> String {
    let mut s = String::new();
    let mut prev_blank = true;
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            s.push('\n');
            prev_blank = true;
            continue;
        }
        if !prev_blank {
            s.push(' ');
        }
        s.push_str(t);
        prev_blank = false;
    }
    s.trim().to_string()
}

fn quoted(first: &str, cont: &[&str]) -> String {
    let mut all = vec![first];
    all.extend(cont.iter().map(|l| l.trim()));
    let joined = fold_plain(&all);
    let quote = joined.chars().next().unwrap_or('"');
    let inner = &joined[1..];
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if quote == '\'' && c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return out;
        }
        if quote == '"' && c == '"' {
            return out;
        }
        if quote == '"' && c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => {}
            }
            continue;
        }
        out.push(c);
    }
    // Unterminated quote: keep what we have.
    out
}

/// Full instructions for the model: `SKILL.md` body without frontmatter, prefixed with
/// `Base directory for this skill: <dir>` and followed by a list of the other files in the
/// skill folder (relative paths, so the model can `read_file`/`bash` them if allowed).
pub fn load(skill: &Skill) -> anyhow::Result<String> {
    let path = skill.dir.join(SKILL_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let body = split_frontmatter(&text)
        .map(|(_, body)| body)
        .unwrap_or(&text);
    let mut out = format!("Base directory for this skill: {}\n\n", skill.dir.display());
    out.push_str(body.trim());
    out.push('\n');
    let mut files = Vec::new();
    list_files(&skill.dir, &skill.dir, 0, &mut files);
    files.sort();
    if !files.is_empty() {
        out.push_str("\nOther files in this skill folder (relative to the base directory):\n");
        for f in files.iter().take(MAX_LISTED_FILES) {
            out.push_str("- ");
            out.push_str(f);
            out.push('\n');
        }
        if files.len() > MAX_LISTED_FILES {
            out.push_str(&format!("- … {} more\n", files.len() - MAX_LISTED_FILES));
        }
    }
    Ok(out)
}

fn list_files(base: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 6 || out.len() > MAX_LISTED_FILES * 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target"
            || name == "__pycache__"
        {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            list_files(base, &path, depth + 1, out);
        } else if ft.is_file() {
            let Ok(rel) = path.strip_prefix(base) else {
                continue;
            };
            if depth == 0 && name == SKILL_FILE {
                continue;
            }
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The `skill {name}` tool (name is an enum of available skill names).
pub fn tool_spec(skills: &[Skill]) -> llm::ToolSpec {
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    llm::ToolSpec {
        name: SKILL_TOOL.to_string(),
        description: "Load a skill: returns its full instructions (and the list of its supporting \
                      files). Call it as soon as the task matches a skill's description in the \
                      system prompt, before doing the work, and follow the loaded instructions. \
                      Do not load a skill that is already loaded in this conversation."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "enum": names,
                    "description": "Skill name, exactly as listed in the system prompt."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
    }
}

/// System-prompt section listing skills (`- name: description`) with instructions to load
/// a skill via the tool when a task matches its description. Empty string if none.
pub fn prompt_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "# Skills\n\nSkills are packaged instructions for specific kinds of tasks. When the user's \
         request matches a skill's description, call the `{SKILL_TOOL}` tool with its name \
         first and follow the instructions it returns. Only load skills that are relevant.\n\n\
         Available skills:\n"
    );
    for s in skills {
        let desc: String = s
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let desc = if desc.chars().count() > MAX_PROMPT_DESCRIPTION {
            let cut: String = desc.chars().take(MAX_PROMPT_DESCRIPTION).collect();
            format!("{cut}…")
        } else {
            desc
        };
        out.push_str(&format!("- {}: {}\n", s.name, desc));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "agent_tools_skills_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fm(text: &str) -> BTreeMap<String, String> {
        parse_frontmatter(split_frontmatter(text).unwrap().0)
    }

    #[test]
    fn plain_and_quoted_values() {
        let f = fm("---\nname: alpha\ndescription: Does things: well.\n---\nbody");
        assert_eq!(f["name"], "alpha");
        assert_eq!(f["description"], "Does things: well.");
        let f = fm("---\nname: \"beta\"\ndescription: 'It''s quoted # not a comment'\n---\n");
        assert_eq!(f["name"], "beta");
        assert_eq!(f["description"], "It's quoted # not a comment");
        let f = fm("---\ndescription: \"line\\none \\\"q\\\"\"\n---\n");
        assert_eq!(f["description"], "line\none \"q\"");
        let f = fm("---\nname: gamma # trailing comment\n---\n");
        assert_eq!(f["name"], "gamma");
    }

    #[test]
    fn folded_and_literal_descriptions() {
        let f = fm(
            "---\nname: x\ndescription: >\n  First line\n  continues here.\n\n  New paragraph.\nlicense: MIT\n---\n",
        );
        assert_eq!(
            f["description"],
            "First line continues here.\nNew paragraph.\n"
        );
        assert_eq!(f["license"], "MIT");
        let f = fm("---\ndescription: >-\n    folded\n    strip\n---\n");
        assert_eq!(f["description"], "folded strip");
        let f = fm("---\ndescription: |\n  line one\n  line two\nname: y\n---\n");
        assert_eq!(f["description"], "line one\nline two\n");
        assert_eq!(f["name"], "y");
        let f = fm("---\ndescription: plain start\n  continued plain\n---\n");
        assert_eq!(f["description"], "plain start continued plain");
    }

    #[test]
    fn nested_mapping_is_ignored() {
        let f = fm(
            "---\nname: z\nmetadata:\n  author: someone\n  version: \"1.0\"\ndescription: d\n---\n",
        );
        assert_eq!(f["name"], "z");
        assert_eq!(f["description"], "d");
        assert!(!f.contains_key("metadata"));
        assert!(!f.contains_key("author"));
    }

    #[test]
    fn frontmatter_must_start_the_file() {
        assert!(split_frontmatter("# Title\n---\nname: a\n---\n").is_none());
        assert!(split_frontmatter("---\nname: a\n").is_none());
        let (front, body) = split_frontmatter("\u{feff}\n---\r\nname: a\r\n---\r\nbody\n").unwrap();
        assert_eq!(front.trim(), "name: a");
        assert_eq!(body, "body\n");
    }

    #[test]
    fn discovery_precedence_and_malformed() {
        let user = TempDir::new();
        let project = TempDir::new();
        user.write(
            "shared/SKILL.md",
            "---\nname: shared\ndescription: user version\n---\nuser body\n",
        );
        user.write(
            "only-user/SKILL.md",
            "---\ndescription: no name given\n---\nbody\n",
        );
        user.write("no-desc/SKILL.md", "---\nname: no-desc\n---\nbody\n");
        user.write("no-front/SKILL.md", "# Just markdown\n");
        user.write("unterminated/SKILL.md", "---\nname: u\ndescription: d\n");
        user.write("empty/SKILL.md", "");
        user.write(
            "not-a-skill/README.md",
            "---\nname: r\ndescription: r\n---\n",
        );
        user.write("loose-file.md", "---\nname: loose\ndescription: d\n---\n");
        project.write(
            "shared/SKILL.md",
            "---\nname: shared\ndescription: >\n  project\n  version\n---\nproject body\n",
        );
        project.write(
            "renamed-dir/SKILL.md",
            "---\nname: zeta\ndescription: z\n---\n",
        );

        let skills = discover_in(&[
            (user.0.clone(), SkillSource::User),
            (user.0.join("missing"), SkillSource::Harness),
            (project.0.clone(), SkillSource::Project),
        ]);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["only-user", "shared", "zeta"]);
        let shared = &skills[1];
        assert_eq!(shared.source, SkillSource::Project);
        assert_eq!(shared.description, "project version");
        assert_eq!(shared.dir, project.0.join("shared"));
        assert_eq!(skills[0].source, SkillSource::User);
        assert_eq!(skills[2].dir, project.0.join("renamed-dir"));
    }

    #[test]
    fn load_strips_frontmatter_and_lists_files() {
        let root = TempDir::new();
        root.write(
            "s/SKILL.md",
            "---\nname: s\ndescription: d\n---\n\n# S\n\nDo it.\n",
        );
        root.write("s/references/guide.md", "g");
        root.write("s/scripts/run.sh", "echo");
        root.write("s/.hidden", "h");
        let skill = Skill {
            name: "s".into(),
            description: "d".into(),
            dir: root.0.join("s"),
            source: SkillSource::User,
        };
        let text = load(&skill).unwrap();
        assert!(text.starts_with(&format!(
            "Base directory for this skill: {}\n\n# S\n\nDo it.\n",
            skill.dir.display()
        )));
        assert!(!text.contains("name: s"));
        assert!(text.contains("- references/guide.md\n"));
        assert!(text.contains("- scripts/run.sh\n"));
        assert!(!text.contains(".hidden"));
        assert!(!text.contains("- SKILL.md"));

        let gone = Skill {
            dir: root.0.join("missing"),
            ..skill
        };
        assert!(load(&gone).is_err());
    }

    #[test]
    fn spec_and_prompt() {
        assert_eq!(prompt_section(&[]), "");
        let skills = vec![
            Skill {
                name: "a".into(),
                description: "Alpha\nskill".into(),
                dir: "/a".into(),
                source: SkillSource::User,
            },
            Skill {
                name: "b".into(),
                description: "Beta".into(),
                dir: "/b".into(),
                source: SkillSource::Project,
            },
        ];
        let p = prompt_section(&skills);
        assert!(p.contains("- a: Alpha skill\n"));
        assert!(p.contains("- b: Beta\n"));
        assert!(p.contains("`skill`"));
        let spec = tool_spec(&skills);
        assert_eq!(spec.name, SKILL_TOOL);
        assert_eq!(
            spec.input_schema["properties"]["name"]["enum"],
            json!(["a", "b"])
        );
        assert_eq!(spec.input_schema["required"], json!(["name"]));
    }
}
