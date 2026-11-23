//! Claude Code agent files: how their `tools:` and `model:` lines map onto this app.

use model::{ModelRef, ToolAccess, ToolGrants};

/// Tool names of this app that the access levels are made of.
const WRITE_FILE: &str = "write_file";
const EDIT_FILE: &str = "edit_file";
const BASH: &str = "bash";

/// Grants for a Claude Code `tools:` list. `None` (no `tools:` line) means every tool.
///
/// `Bash(git:*)`-style patterns count as the bare tool, `mcp__*` names are extra grants, other
/// names (`Task`, `WebFetch`, `TodoWrite`…) have no counterpart here and are ignored. The level
/// is the widest the list asks for: any editing tool gives `Full`, `Bash` alone `ReadAndRun`,
/// otherwise `ReadOnly`. Under `Full`, the editing tools and `Bash` the list left out are
/// denied, so `[Read, Edit]` does not get a shell.
pub fn map_tools(tools: Option<&[String]>) -> ToolGrants {
    let Some(tools) = tools else {
        return ToolGrants::default();
    };
    let (mut bash, mut edit, mut write) = (false, false, false);
    let mut allow: Vec<String> = Vec::new();
    for raw in tools {
        let name = raw.split('(').next().unwrap_or("").trim();
        match name {
            "Bash" => bash = true,
            "Edit" | "MultiEdit" | "NotebookEdit" => edit = true,
            "Write" => write = true,
            n if n.starts_with("mcp__") && !allow.iter().any(|a| a == n) => allow.push(n.to_string()),
            _ => {}
        }
    }
    let (access, deny) = if edit || write {
        let mut deny = Vec::new();
        if !bash {
            deny.push(BASH.to_string());
        }
        if !edit {
            deny.push(EDIT_FILE.to_string());
        }
        if !write {
            deny.push(WRITE_FILE.to_string());
        }
        (ToolAccess::Full, deny)
    } else if bash {
        (ToolAccess::ReadAndRun, Vec::new())
    } else {
        (ToolAccess::ReadOnly, Vec::new())
    };
    ToolGrants { access, allow, deny }
}

/// The `tools:` list written next to the `harness:` block so Claude Code can read the file too.
pub fn claude_tools(grants: &ToolGrants) -> Vec<String> {
    let mut out: Vec<&str> = vec!["Read", "Grep", "Glob"];
    let denied = |name: &str| grants.deny.iter().any(|d| d == name);
    if matches!(grants.access, ToolAccess::ReadAndRun | ToolAccess::Full) && !denied(BASH) {
        out.push("Bash");
    }
    if grants.access == ToolAccess::Full {
        if !denied(EDIT_FILE) {
            out.push("Edit");
        }
        if !denied(WRITE_FILE) {
            out.push("Write");
        }
    }
    let mut out: Vec<String> = out.into_iter().map(str::to_string).collect();
    out.extend(grants.allow.iter().cloned());
    out
}

/// A Claude Code `model:` value kept only when it names a model id (`provider/model`, or
/// something like `claude-sonnet-4-5`). Aliases (`sonnet`, `opus`, `inherit`) mean nothing here
/// and become the empty model, which falls back to whatever the caller picks.
pub fn model_from_claude(value: &str) -> ModelRef {
    let value = value.trim();
    let looks_like_id = value.contains('/') || (value.contains('-') && value.chars().any(|c| c.is_ascii_digit()));
    if looks_like_id && !value.contains(char::is_whitespace) { ModelRef::parse(value) } else { ModelRef::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_tools_line_means_everything() {
        assert_eq!(map_tools(None), ToolGrants::default());
    }

    #[test]
    fn levels_follow_the_widest_tool() {
        assert_eq!(map_tools(Some(&list(&["Read", "Grep", "Glob", "Task"]))).access, ToolAccess::ReadOnly);
        assert_eq!(map_tools(Some(&list(&["Read", "Bash(git:*)"]))).access, ToolAccess::ReadAndRun);
        let full = map_tools(Some(&list(&["Read", "Edit", "Write", "Bash"])));
        assert_eq!(full.access, ToolAccess::Full);
        assert!(full.deny.is_empty());
        assert_eq!(map_tools(Some(&[])).access, ToolAccess::ReadOnly);
    }

    #[test]
    fn missing_editing_tools_are_denied_under_full() {
        let grants = map_tools(Some(&list(&["Read", "Edit"])));
        assert_eq!(grants.access, ToolAccess::Full);
        assert_eq!(grants.deny, ["bash", "write_file"]);
    }

    #[test]
    fn mcp_tools_become_allow_entries_once() {
        let grants = map_tools(Some(&list(&["Read", "mcp__github__create_issue", "mcp__github__create_issue", "mcp__x"])));
        assert_eq!(grants.allow, ["mcp__github__create_issue", "mcp__x"]);
    }

    #[test]
    fn tools_line_for_each_level() {
        let read = ToolGrants { access: ToolAccess::ReadOnly, ..Default::default() };
        assert_eq!(claude_tools(&read), ["Read", "Grep", "Glob"]);
        let full = ToolGrants { access: ToolAccess::Full, deny: vec!["bash".into()], allow: vec!["mcp__a__b".into()] };
        assert_eq!(claude_tools(&full), ["Read", "Grep", "Glob", "Edit", "Write", "mcp__a__b"]);
        assert_eq!(map_tools(Some(&claude_tools(&full))), ToolGrants { deny: vec!["bash".into()], ..full });
    }

    #[test]
    fn only_model_ids_survive() {
        for alias in ["sonnet", "opus", "haiku", "inherit", "", "default"] {
            assert!(model_from_claude(alias).is_empty(), "{alias}");
        }
        assert_eq!(model_from_claude("claude-sonnet-4-5").to_string(), "claude-sonnet-4-5");
        assert_eq!(model_from_claude("nr/cc/claude-opus-5-5").provider, "nr");
    }
}
