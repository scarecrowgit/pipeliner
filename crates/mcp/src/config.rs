//! Loading [`ServerConfig`]s from the Harness TOML, `.mcp.json` and `~/.claude.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde_json::Value;

use crate::{ServerConfig, ServerSource};

pub(crate) fn load(
    harness_config: &Path,
    workspace_root: &Path,
    claude_json: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<(Vec<ServerConfig>, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut servers: Vec<ServerConfig> = Vec::new();

    // 1. Harness config (malformed TOML is a hard error).
    let mut import_claude_code = false;
    if let Some(text) = read_optional(harness_config, &mut warnings) {
        let (list, import) = parse_harness_toml(&text, lookup, &mut warnings)
            .with_context(|| format!("parsing {}", harness_config.display()))?;
        import_claude_code = import;
        merge(&mut servers, list, &mut warnings);
    }

    // 2. <workspace>/.mcp.json
    let mcp_json = workspace_root.join(".mcp.json");
    if let Some(text) = read_optional(&mcp_json, &mut warnings) {
        let list = parse_mcp_json(
            &text,
            ServerSource::ProjectMcpJson,
            Some(workspace_root),
            &mcp_json.display().to_string(),
            lookup,
            &mut warnings,
        );
        merge(&mut servers, list, &mut warnings);
    }

    // 3. ~/.claude.json
    if import_claude_code && let Some(text) = read_optional(claude_json, &mut warnings) {
        let list = parse_mcp_json(
            &text,
            ServerSource::ClaudeCode,
            None,
            &claude_json.display().to_string(),
            lookup,
            &mut warnings,
        );
        merge(&mut servers, list, &mut warnings);
    }

    Ok((servers, warnings))
}

fn read_optional(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warnings.push(format!("cannot read {}: {e}", path.display()));
            None
        }
    }
}

fn source_label(s: &ServerSource) -> &'static str {
    match s {
        ServerSource::HarnessConfig => "Harness config",
        ServerSource::ProjectMcpJson => ".mcp.json",
        ServerSource::ClaudeCode => "~/.claude.json",
    }
}

/// Append `new` to `servers`; names already present win (earlier sources / entries).
fn merge(servers: &mut Vec<ServerConfig>, new: Vec<ServerConfig>, warnings: &mut Vec<String>) {
    for s in new {
        if let Some(existing) = servers.iter().find(|e| e.name == s.name) {
            warnings.push(format!(
                "MCP server `{}` from {} ignored: already defined in {}",
                s.name,
                source_label(&s.source),
                source_label(&existing.source)
            ));
        } else {
            servers.push(s);
        }
    }
}

fn validate_name(name: &str, origin: &str, warnings: &mut Vec<String>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        warnings.push(format!("{origin}: MCP server with an empty name skipped"));
        None
    } else {
        Some(name.to_string())
    }
}

#[derive(serde::Deserialize)]
struct TomlServer {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    auto_approve: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Returns the servers and the `import_claude_code` flag. Only TOML syntax errors are `Err`;
/// a bad `[[mcp_servers]]` entry is skipped with a warning.
pub(crate) fn parse_harness_toml(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> anyhow::Result<(Vec<ServerConfig>, bool)> {
    let table: toml::Table = text.parse()?;
    let import = match table.get("import_claude_code") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(other) => {
            warnings.push(format!(
                "Harness config: `import_claude_code` must be a boolean, got {}",
                other.type_str()
            ));
            false
        }
    };
    let entries = match table.get("mcp_servers") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) => a.clone(),
        Some(other) => {
            warnings.push(format!(
                "Harness config: `mcp_servers` must be an array of tables ([[mcp_servers]]), got {}",
                other.type_str()
            ));
            Vec::new()
        }
    };
    let mut out: Vec<ServerConfig> = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        let origin = format!("Harness config [[mcp_servers]] #{}", i + 1);
        let parsed: TomlServer = match entry.try_into() {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("{origin} skipped: {}", e.message().trim()));
                continue;
            }
        };
        let Some(name) = validate_name(&parsed.name, &origin, warnings) else { continue };
        if parsed.command.trim().is_empty() {
            warnings.push(format!("MCP server `{name}`: empty `command`, skipped"));
            continue;
        }
        let mut missing = Vec::new();
        let cfg = ServerConfig {
            command: expand(&parsed.command, lookup, &mut missing),
            args: parsed.args.iter().map(|a| expand(a, lookup, &mut missing)).collect(),
            env: parsed
                .env
                .iter()
                .map(|(k, v)| (k.clone(), expand(v, lookup, &mut missing)))
                .collect(),
            cwd: parsed.cwd.map(|c| expand_path(&c, lookup, &mut missing)),
            enabled: parsed.enabled,
            auto_approve: parsed.auto_approve,
            source: ServerSource::HarnessConfig,
            name,
        };
        warn_missing(&cfg.name, missing, warnings);
        if out.iter().any(|s| s.name == cfg.name) {
            warnings.push(format!(
                "MCP server `{}` defined twice in Harness config; the first one wins",
                cfg.name
            ));
            continue;
        }
        out.push(cfg);
    }
    Ok((out, import))
}

/// Parse a Claude Code style `{"mcpServers": {...}}` document. Never fails: problems become
/// warnings. `default_cwd` is used when an entry has no `cwd`.
pub(crate) fn parse_mcp_json(
    text: &str,
    source: ServerSource,
    default_cwd: Option<&Path>,
    origin: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Vec<ServerConfig> {
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("{origin}: invalid JSON ({e}), ignored"));
            return Vec::new();
        }
    };
    let servers = match doc.get("mcpServers") {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Object(m)) => m,
        Some(_) => {
            warnings.push(format!("{origin}: `mcpServers` is not an object, ignored"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (raw_name, entry) in servers {
        let Some(name) = validate_name(raw_name, origin, warnings) else { continue };
        let Some(obj) = entry.as_object() else {
            warnings.push(format!("{origin}: MCP server `{name}` is not an object, skipped"));
            continue;
        };
        let ty = obj.get("type").and_then(Value::as_str);
        match ty {
            None | Some("stdio") => {}
            Some(other) => {
                warnings.push(format!(
                    "{origin}: MCP server `{name}` has type \"{other}\"; only stdio servers are supported, skipped"
                ));
                continue;
            }
        }
        let command = obj.get("command").and_then(Value::as_str).unwrap_or("");
        if command.trim().is_empty() {
            if ty.is_none() && obj.contains_key("url") {
                warnings.push(format!(
                    "{origin}: MCP server `{name}` is a remote (url) server; only stdio servers are supported, skipped"
                ));
            } else {
                warnings.push(format!("{origin}: MCP server `{name}` has no `command`, skipped"));
            }
            continue;
        }
        let mut missing = Vec::new();
        let args = string_list(obj.get("args"), &name, "args", origin, warnings)
            .iter()
            .map(|a| expand(a, lookup, &mut missing))
            .collect();
        let mut env = BTreeMap::new();
        match obj.get("env") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, v) in m {
                    let v = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(_) | Value::Bool(_) => v.to_string(),
                        _ => {
                            warnings.push(format!(
                                "{origin}: MCP server `{name}`: env `{k}` is not a string, ignored"
                            ));
                            continue;
                        }
                    };
                    env.insert(k.clone(), expand(&v, lookup, &mut missing));
                }
            }
            Some(_) => {
                warnings.push(format!("{origin}: MCP server `{name}`: `env` is not an object, ignored"))
            }
        }
        let cwd = match obj.get("cwd").and_then(Value::as_str) {
            Some(c) if !c.trim().is_empty() => {
                let p = expand_path(c, lookup, &mut missing);
                Some(match default_cwd {
                    Some(base) if p.is_relative() => base.join(p),
                    _ => p,
                })
            }
            _ => default_cwd.map(Path::to_path_buf),
        };
        let disabled = obj.get("disabled").and_then(Value::as_bool).unwrap_or(false);
        let enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true) && !disabled;
        let auto_approve = string_list(obj.get("autoApprove"), &name, "autoApprove", origin, warnings);
        let cfg = ServerConfig {
            name,
            command: expand(command, lookup, &mut missing),
            args,
            env,
            cwd,
            enabled,
            auto_approve,
            source: source.clone(),
        };
        warn_missing(&cfg.name, missing, warnings);
        out.push(cfg);
    }
    out
}

fn string_list(
    v: Option<&Value>,
    name: &str,
    field: &str,
    origin: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match v {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Number(_) | Value::Bool(_) => Some(x.to_string()),
                _ => {
                    warnings.push(format!(
                        "{origin}: MCP server `{name}`: non-string item in `{field}` ignored"
                    ));
                    None
                }
            })
            .collect(),
        Some(_) => {
            warnings.push(format!("{origin}: MCP server `{name}`: `{field}` is not an array, ignored"));
            Vec::new()
        }
    }
}

fn warn_missing(name: &str, mut missing: Vec<String>, warnings: &mut Vec<String>) {
    missing.sort();
    missing.dedup();
    for var in missing {
        warnings.push(format!(
            "MCP server `{name}`: environment variable `{var}` is not set (left as `${{{var}}}`)"
        ));
    }
}

fn expand_path(s: &str, lookup: &dyn Fn(&str) -> Option<String>, missing: &mut Vec<String>) -> PathBuf {
    let s = expand(s, lookup, missing);
    if s == "~" {
        if let Some(home) = lookup("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = lookup("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(s)
}

/// Claude Code style expansion: `${VAR}` and `${VAR:-default}`. An unset variable without a
/// default is left verbatim and recorded in `missing`. A bare `$VAR` is not expanded.
pub(crate) fn expand(
    s: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let content = &after[..end];
        let (var, default) = match content.split_once(":-") {
            Some((v, d)) => (v, Some(d)),
            None => (content, None),
        };
        match (lookup(var), default) {
            (Some(v), _) => out.push_str(&v),
            (None, Some(d)) => out.push_str(d),
            (None, None) => {
                missing.push(var.to_string());
                out.push_str(&rest[start..start + 2 + end + 1]);
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/test".into()),
            "TOKEN" => Some("secret".into()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "mcp-config-test-{}-{tag}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, text).unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn expansion() {
        let mut missing = Vec::new();
        assert_eq!(expand("a ${TOKEN} b", &env, &mut missing), "a secret b");
        assert_eq!(expand("${NOPE:-fallback}", &env, &mut missing), "fallback");
        assert_eq!(expand("${TOKEN:-fallback}", &env, &mut missing), "secret");
        assert_eq!(expand("${EMPTY:-fallback}", &env, &mut missing), "");
        assert_eq!(expand("${NOPE:-}", &env, &mut missing), "");
        assert_eq!(expand("$TOKEN ${unterminated", &env, &mut missing), "$TOKEN ${unterminated");
        assert_eq!(expand("x${HOME}/y${TOKEN}", &env, &mut missing), "x/home/test/ysecret");
        assert!(missing.is_empty());
        assert_eq!(expand("k=${NOPE}!", &env, &mut missing), "k=${NOPE}!");
        assert_eq!(missing, vec!["NOPE".to_string()]);
    }

    #[test]
    fn harness_toml() {
        let text = r#"
            default_model = "x"
            import_claude_code = true
            [[providers]]
            id = "p"
            [[mcp_servers]]
            name = "fs"
            command = "npx"
            args = ["-y", "server", "${HOME}"]
            env = { KEY = "${TOKEN}", OTHER = "${NOPE:-d}" }
            cwd = "~/proj"
            auto_approve = ["read_file"]
            [[mcp_servers]]
            name = "off"
            command = "x"
            enabled = false
            [[mcp_servers]]
            name = "bad"
            [[mcp_servers]]
            name = "  "
            command = "x"
            [[mcp_servers]]
            name = "fs"
            command = "dup"
        "#;
        let mut w = Vec::new();
        let (servers, import) = parse_harness_toml(text, &env, &mut w).unwrap();
        assert!(import);
        assert_eq!(servers.len(), 2, "{servers:?}");
        let fs = &servers[0];
        assert_eq!(fs.name, "fs");
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args, vec!["-y", "server", "/home/test"]);
        assert_eq!(fs.env["KEY"], "secret");
        assert_eq!(fs.env["OTHER"], "d");
        assert_eq!(fs.cwd, Some(PathBuf::from("/home/test/proj")));
        assert!(fs.enabled);
        assert!(fs.auto_approves("read_file") && !fs.auto_approves("write_file"));
        assert_eq!(fs.source, ServerSource::HarnessConfig);
        assert!(!servers[1].enabled);
        assert_eq!(w.len(), 3, "{w:?}"); // missing command, empty name, duplicate
        assert!(w.iter().any(|m| m.contains("#3")));
        assert!(w.iter().any(|m| m.contains("defined twice")));

        assert!(parse_harness_toml("not = [valid", &env, &mut w).is_err());
        let (s, import) = parse_harness_toml("", &env, &mut w).unwrap();
        assert!(s.is_empty() && !import);
    }

    #[test]
    fn mcp_json() {
        let text = r#"{"mcpServers": {
            "a": {"command": "node", "args": ["srv.js", "--token=${TOKEN}"], "env": {"N": 5, "K": "${MISSING}"}, "autoApprove": ["*"]},
            "b": {"type": "stdio", "command": "b", "disabled": true},
            "web": {"type": "http", "url": "https://example.invalid/mcp"},
            "sse": {"type": "sse", "url": "https://example.invalid/sse"},
            "remote": {"url": "https://example.invalid/mcp"},
            "nocmd": {"args": []},
            "": {"command": "x"}
        }}"#;
        let mut w = Vec::new();
        let ws = Path::new("/ws");
        let servers =
            parse_mcp_json(text, ServerSource::ProjectMcpJson, Some(ws), "test", &env, &mut w);
        let names: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        let a = &servers[0];
        assert_eq!(a.args, vec!["srv.js", "--token=secret"]);
        assert_eq!(a.env["N"], "5");
        assert_eq!(a.env["K"], "${MISSING}");
        assert_eq!(a.cwd.as_deref(), Some(ws));
        assert!(a.auto_approves("anything"));
        assert!(!servers[1].enabled);
        assert_eq!(w.iter().filter(|m| m.contains("only stdio")).count(), 3, "{w:?}");
        assert!(w.iter().any(|m| m.contains("MISSING")));
        assert!(w.iter().any(|m| m.contains("no `command`")));
        assert!(w.iter().any(|m| m.contains("empty name")));

        let mut w = Vec::new();
        assert!(parse_mcp_json("{oops", ServerSource::ClaudeCode, None, "t", &env, &mut w).is_empty());
        assert_eq!(w.len(), 1);
        assert!(parse_mcp_json("{}", ServerSource::ClaudeCode, None, "t", &env, &mut w).is_empty());
    }

    #[test]
    fn load_all_sources_with_clashes() {
        let dir = TempDir::new("all");
        let harness = dir.write(
            "config.toml",
            r#"
                import_claude_code = true
                [[mcp_servers]]
                name = "shared"
                command = "from-harness"
            "#,
        );
        let ws = dir.0.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(
            ws.join(".mcp.json"),
            r#"{"mcpServers": {"shared": {"command": "from-project"}, "proj": {"command": "p"}}}"#,
        )
        .unwrap();
        let claude = dir.write(
            "claude.json",
            r#"{"numStartups": 3, "projects": {}, "mcpServers": {
                "proj": {"command": "from-claude"}, "user": {"command": "u", "args": ["${NOPE:-dflt}"]},
                "remote": {"type": "http", "url": "https://example.invalid"}}}"#,
        );
        let (servers, warnings) = load(&harness, &ws, &claude, &env).unwrap();
        let summary: Vec<_> =
            servers.iter().map(|s| (s.name.as_str(), s.command.as_str(), s.source.clone())).collect();
        assert_eq!(
            summary,
            vec![
                ("shared", "from-harness", ServerSource::HarnessConfig),
                ("proj", "p", ServerSource::ProjectMcpJson),
                ("user", "u", ServerSource::ClaudeCode),
            ]
        );
        assert_eq!(servers[1].cwd.as_deref(), Some(ws.as_path()));
        assert_eq!(servers[2].cwd, None);
        assert_eq!(servers[2].args, vec!["dflt"]);
        assert_eq!(warnings.iter().filter(|w| w.contains("already defined")).count(), 2, "{warnings:?}");
        assert_eq!(warnings.iter().filter(|w| w.contains("only stdio")).count(), 1, "{warnings:?}");

        // Without import_claude_code, ~/.claude.json is ignored.
        let harness2 = dir.write("config2.toml", "default_model = \"m\"\n");
        let (servers, _) = load(&harness2, &ws, &claude, &env).unwrap();
        assert!(servers.iter().all(|s| s.source != ServerSource::ClaudeCode));
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn load_missing_and_malformed() {
        let dir = TempDir::new("missing");
        let none = dir.0.join("nope");
        let (servers, warnings) = load(&none.join("c.toml"), &none, &none.join("c.json"), &env).unwrap();
        assert!(servers.is_empty() && warnings.is_empty());

        let bad = dir.write("bad.toml", "[[mcp_servers]\nname=");
        let err = load(&bad, &none, &none, &env).unwrap_err();
        assert!(format!("{err:#}").contains("bad.toml"), "{err:#}");

        // Malformed optional JSON is a warning, not an error.
        let ws = dir.0.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join(".mcp.json"), "{not json").unwrap();
        let (servers, warnings) = load(&none, &ws, &none, &env).unwrap();
        assert!(servers.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }
}
