//! Custom agents (Claude Code subagent definitions): markdown files with YAML frontmatter
//! `name`, `description`, `tools` (comma-separated, or a YAML list) and `model`; the body
//! is the agent's system prompt.
//!
//! Discovery roots (a later root overrides an earlier one with the same name):
//! `~/.claude/agents/*.md`, `<workspace>/.claude/agents/*.md`.
//!
//! [`map_tools`] translates Claude Code tool names into this crate's tools.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSource {
    User,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// `None` when the frontmatter has no `tools` (the agent gets the default toolset).
    pub tools: Option<Vec<String>>,
    /// Raw `model` value (Claude Code aliases such as `sonnet` or `inherit` included).
    pub model: Option<String>,
    /// The markdown body: the agent's system prompt.
    pub prompt: String,
    pub path: PathBuf,
    pub source: AgentSource,
}

/// Agent files larger than this are skipped.
const MAX_AGENT_BYTES: u64 = 512 * 1024;

/// Find agents for `workspace_root`. Never fails: unreadable/malformed files are skipped.
/// Sorted by name.
pub fn discover(workspace_root: &Path) -> Vec<AgentDef> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push((home.join(".claude").join("agents"), AgentSource::User));
    }
    roots.push((
        workspace_root.join(".claude").join("agents"),
        AgentSource::Project,
    ));
    discover_in(&roots)
}

/// Discovery over explicit roots; a later root overrides an earlier one with the same name.
pub(crate) fn discover_in(roots: &[(PathBuf, AgentSource)]) -> Vec<AgentDef> {
    let mut by_name: BTreeMap<String, AgentDef> = BTreeMap::new();
    for (root, source) in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
            .collect();
        files.sort();
        for file in files {
            if let Some(agent) = read_agent(&file, source.clone()) {
                by_name.insert(agent.name.clone(), agent);
            }
        }
    }
    by_name.into_values().collect()
}

fn read_agent(path: &Path, source: AgentSource) -> Option<AgentDef> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_AGENT_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    parse(&text, &stem, path, source)
}

/// Parse an agent file; `fallback_name` (the file stem) is used without a `name` field.
/// `None` without frontmatter.
pub fn parse(
    text: &str,
    fallback_name: &str,
    path: &Path,
    source: AgentSource,
) -> Option<AgentDef> {
    let (front, body) = split_frontmatter(text)?;
    let fields = parse_frontmatter(front);
    let get = |k: &str| {
        fields
            .get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let name = get("name").unwrap_or_else(|| fallback_name.trim().to_string());
    if name.is_empty() {
        return None;
    }
    let tools = get("tools").map(|t| split_list(&t));
    Some(AgentDef {
        name,
        description: get("description").unwrap_or_default(),
        tools,
        model: get("model"),
        prompt: body.trim().to_string(),
        path: path.to_path_buf(),
        source,
    })
}

/// `Read, Grep`, `[Read, "Grep"]` or newline-separated items → trimmed, unquoted names.
fn split_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(s);
    s.split([',', '\n'])
        .map(|t| t.trim().trim_matches(['"', '\'']).trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Tools granted by an agent's `tools` list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolGrants {
    /// Names of this crate's tools (`read_file`, `bash`, …) plus `todo_write` / `skill`.
    pub builtin: Vec<&'static str>,
    /// `mcp__server__tool` ids, or `mcp__server` for every tool of a server.
    pub mcp: Vec<String>,
}

impl ToolGrants {
    pub fn has(&self, tool: &str) -> bool {
        self.builtin.contains(&tool)
    }

    /// The model-facing MCP tool id is granted (exactly, or by its whole server).
    pub fn grants_mcp(&self, id: &str) -> bool {
        self.mcp.iter().any(|g| {
            if g == id {
                return true;
            }
            // `mcp__server` (no tool part) grants every tool of that server.
            let server_only = g.strip_prefix("mcp__").is_some_and(|s| !s.contains("__"));
            server_only
                && id
                    .strip_prefix(g.as_str())
                    .is_some_and(|r| r.starts_with("__"))
        })
    }
}

/// Map Claude Code tool names to this harness's tools. `Bash(git:*)`-style patterns count as
/// the bare tool; unknown names are ignored.
pub fn map_tools(tools: &[String]) -> ToolGrants {
    let mut out = ToolGrants::default();
    for raw in tools {
        let name = raw.split('(').next().unwrap_or("").trim();
        if name.starts_with("mcp__") {
            if !out.mcp.iter().any(|m| m == name) {
                out.mcp.push(name.to_string());
            }
            continue;
        }
        let mapped: &[&'static str] = match name {
            "Read" | "NotebookRead" => &["read_file"],
            "Grep" => &["grep"],
            "Glob" | "LS" => &["list_dir"],
            "Edit" | "MultiEdit" | "NotebookEdit" => &["edit_file"],
            "Write" => &["write_file"],
            "Bash" => &["bash"],
            "TodoWrite" => &["todo_write"],
            "Skill" => &["skill"],
            _ => &[],
        };
        for m in mapped {
            if !out.builtin.contains(m) {
                out.builtin.push(m);
            }
        }
    }
    out
}

/// `(frontmatter, body)`: the frontmatter is the block between the first two `---` lines
/// (the first must be the file's first non-empty line).
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        match start {
            None if trimmed == "---" => start = Some(offset + line.len()),
            None if !trimmed.is_empty() => return None,
            None => {}
            Some(s) if trimmed == "---" => {
                return Some((&text[s..offset], &text[offset + line.len()..]));
            }
            Some(_) => {}
        }
        offset += line.len();
    }
    None
}

/// Top-level `key: value` pairs: plain, quoted, block (`|` / `>`) and plain multi-line
/// scalars; a block sequence (`- item` lines) becomes its items joined with newlines.
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
        let rest = rest.trim();
        let mut cont = Vec::new();
        while i < lines.len()
            && (lines[i].trim().is_empty()
                || lines[i].starts_with([' ', '\t'])
                || (rest.is_empty() && lines[i].starts_with("- ")))
        {
            cont.push(lines[i].trim());
            i += 1;
        }
        let value = if rest.starts_with('|') || rest.starts_with('>') {
            let sep = if rest.starts_with('|') { "\n" } else { " " };
            join_lines(&cont, sep)
        } else if rest.is_empty() && cont.iter().any(|l| l.starts_with('-')) {
            cont.iter()
                .filter_map(|l| l.strip_prefix('-'))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n")
        } else if rest.starts_with('"') || rest.starts_with('\'') {
            let mut all = vec![rest];
            all.extend(cont.iter().copied());
            unquote(&join_lines(&all, " "))
        } else {
            let mut all = vec![strip_comment(rest)];
            all.extend(cont.iter().copied());
            join_lines(&all, " ")
        };
        out.insert(key.trim().to_string(), value);
    }
    out
}

fn strip_comment(s: &str) -> &str {
    match s.find(" #") {
        Some(pos) => s[..pos].trim_end(),
        None => s,
    }
}

/// Non-empty lines joined with `sep`; blank lines become paragraph breaks.
fn join_lines(lines: &[&str], sep: &str) -> String {
    let mut s = String::new();
    let mut prev_blank = true;
    for l in lines {
        if l.is_empty() {
            s.push('\n');
            prev_blank = true;
            continue;
        }
        if !prev_blank {
            s.push_str(sep);
        }
        s.push_str(l);
        prev_blank = false;
    }
    s.trim().to_string()
}

fn unquote(s: &str) -> String {
    let quote = s.chars().next().unwrap_or('"');
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            ('\'', '\'') if chars.peek() == Some(&'\'') => {
                chars.next();
                out.push('\'');
            }
            ('\'', '\'') | ('"', '"') => return out,
            ('"', '\\') => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            },
            _ => out.push(c),
        }
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
                "agent_tools_agents_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn p(text: &str) -> Option<AgentDef> {
        parse(text, "stem", Path::new("/x/stem.md"), AgentSource::User)
    }

    #[test]
    fn parses_claude_code_agent() {
        let a = p(
            "---\nname: ext-reviewer\ndescription: Independent review: GPT/Gemini.\n\
                   tools: Read, Grep, Glob, Bash, mcp__9router__ask, mcp__9router__panel\n\
                   model: sonnet\n---\n\nYou review code.\n\n## Steps\n1. Do it.\n",
        )
        .unwrap();
        assert_eq!(a.name, "ext-reviewer");
        assert_eq!(a.description, "Independent review: GPT/Gemini.");
        assert_eq!(
            a.tools.as_deref().unwrap(),
            [
                "Read",
                "Grep",
                "Glob",
                "Bash",
                "mcp__9router__ask",
                "mcp__9router__panel"
            ]
        );
        assert_eq!(a.model.as_deref(), Some("sonnet"));
        assert_eq!(a.prompt, "You review code.\n\n## Steps\n1. Do it.");
    }

    #[test]
    fn optional_fields_and_formats() {
        let a = p("---\ndescription: \"Quoted \\\"d\\\"\"\n---\nbody").unwrap();
        assert_eq!(a.name, "stem");
        assert_eq!(a.description, "Quoted \"d\"");
        assert_eq!(a.tools, None);
        assert_eq!(a.model, None);
        let a = p("---\nname: x\ntools: [Read, 'Write']\n---\n").unwrap();
        assert_eq!(a.tools.as_deref().unwrap(), ["Read", "Write"]);
        assert_eq!(a.prompt, "");
        let a =
            p("---\nname: y\ntools:\n  - Read\n  - Bash\nmodel: inherit # alias\n---\n").unwrap();
        assert_eq!(a.tools.as_deref().unwrap(), ["Read", "Bash"]);
        assert_eq!(a.model.as_deref(), Some("inherit"));
        let a = p("---\nname: z\ndescription: >\n  folded\n  text\n---\n").unwrap();
        assert_eq!(a.description, "folded text");
        let a = p("---\nname: w\ntools:\n---\n").unwrap();
        assert_eq!(a.tools, None);
        assert!(p("no frontmatter\n").is_none());
        assert!(p("---\nname: u\n").is_none());
    }

    #[test]
    fn maps_tools() {
        let g = map_tools(
            &[
                "Read",
                "Grep",
                "Glob",
                "LS",
                "Edit",
                "MultiEdit",
                "Bash(git diff:*)",
                "TodoWrite",
                "WebFetch",
                "mcp__srv__ask",
                "mcp__srv__ask",
                "mcp__all",
            ]
            .map(String::from),
        );
        assert_eq!(
            g.builtin,
            [
                "read_file",
                "grep",
                "list_dir",
                "edit_file",
                "bash",
                "todo_write"
            ]
        );
        assert_eq!(g.mcp, ["mcp__srv__ask", "mcp__all"]);
        assert!(g.has("bash"));
        assert!(!g.has("write_file"));
        assert!(g.grants_mcp("mcp__srv__ask"));
        assert!(!g.grants_mcp("mcp__srv__panel"));
        assert!(g.grants_mcp("mcp__all__anything"));
        assert!(!g.grants_mcp("mcp__allx__t"));
    }

    #[test]
    fn discovery_project_overrides_user() {
        let user = TempDir::new();
        let project = TempDir::new();
        user.write("a.md", "---\nname: shared\ndescription: user\n---\nu\n");
        user.write("b.md", "---\ndescription: from stem\n---\nb\n");
        user.write("notes.txt", "---\nname: txt\n---\n");
        user.write("broken.md", "# no frontmatter\n");
        project.write("c.md", "---\nname: shared\ndescription: project\n---\np\n");
        let agents = discover_in(&[
            (user.0.clone(), AgentSource::User),
            (user.0.join("missing"), AgentSource::User),
            (project.0.clone(), AgentSource::Project),
        ]);
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["b", "shared"]);
        assert_eq!(agents[1].description, "project");
        assert_eq!(agents[1].source, AgentSource::Project);
        assert_eq!(agents[1].prompt, "p");
        assert_eq!(agents[0].path, user.0.join("b.md"));
    }
}
