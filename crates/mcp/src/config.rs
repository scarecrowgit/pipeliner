//! Where MCP server definitions come from.
//!
//! Three sources, read in this order; a name defined by an earlier source wins:
//!
//! 1. `[[mcp_servers]]` in the app's `config.toml`:
//!    ```toml
//!    import_claude_code = false        # also read ~/.claude.json
//!    [[mcp_servers]]
//!    name = "fs"
//!    command = "npx"
//!    args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
//!    env = { KEY = "${API_KEY}" }
//!    cwd = "~/src"                     # optional
//!    enabled = true                    # default true
//!    auto_approve = ["read_file"]      # run without asking; ["*"] approves every tool
//!    ```
//! 2. `<workspace>/.mcp.json`, Claude Code's project format:
//!    `{"mcpServers": {"fs": {"command": "…", "args": [], "env": {}, "autoApprove": []}}}`.
//!    These servers run in the workspace unless they name a `cwd`.
//! 3. The top-level `mcpServers` of `~/.claude.json`, only with `import_claude_code = true`.
//!
//! `${VAR}` and `${VAR:-default}` expand in `command`, `args`, `env` and `cwd`. Only stdio
//! servers are supported; `http` and `sse` entries are skipped with a warning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base::paths::Paths;
use serde_json::Value;

#[cfg(test)]
mod tests;

/// Variables whose values are never treated as secrets when they are expanded into `args`
/// or `env`; hiding them would only make paths in messages unreadable.
const PLAIN_VARS: &[&str] =
    &["HOME", "USER", "LOGNAME", "PWD", "PATH", "SHELL", "TMPDIR", "LANG", "TERM"];
/// Shorter values are not hidden: `1` or `yes` would garble every message.
const MIN_SECRET_LEN: usize = 4;

/// Which file a server definition came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServerSource {
    /// `[[mcp_servers]]` in the app's `config.toml`.
    ConfigToml,
    /// `<workspace>/.mcp.json`.
    McpJson,
    /// `~/.claude.json`, imported with `import_claude_code = true`.
    ClaudeJson,
}

impl ServerSource {
    /// The name the UI shows for this source (`McpServerInfo::source`).
    pub fn label(self) -> &'static str {
        match self {
            Self::ConfigToml => "config.toml",
            Self::McpJson => ".mcp.json",
            Self::ClaudeJson => "~/.claude.json",
        }
    }
}

/// One stdio MCP server as configured, with variables already expanded.
///
/// `Debug` shows env keys but not their values, and hides expanded secrets in `args`.
#[derive(Clone, PartialEq)]
pub struct ServerConfig {
    /// The server's name, unique across sources.
    pub name: String,
    /// The program to run, looked up in `PATH`.
    pub command: String,
    pub args: Vec<String>,
    /// Variables set on top of the app's environment.
    pub env: BTreeMap<String, String>,
    /// Working directory; the app's own when `None`.
    pub cwd: Option<PathBuf>,
    /// `false` keeps the server listed but never started.
    pub enabled: bool,
    /// Tool names (without the `mcp__server__` prefix) that run without asking; `"*"`
    /// approves every tool of the server.
    pub auto_approve: Vec<String>,
    pub source: ServerSource,
    /// Values taken from the environment into `args`, hidden in `Debug` and error text.
    secrets: Vec<String>,
}

impl ServerConfig {
    /// A minimal enabled server from `config.toml`; set the other fields directly.
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            enabled: true,
            auto_approve: Vec::new(),
            source: ServerSource::ConfigToml,
            secrets: Vec::new(),
        }
    }

    /// Whether `tool` (the server's own tool name) may run without asking.
    pub fn auto_approves(&self, tool: &str) -> bool {
        self.auto_approve.iter().any(|t| t == "*" || t == tool)
    }

    /// `text` with every env value and every secret expanded into `args` replaced by `***`,
    /// for error messages and logs.
    pub fn redact(&self, text: &str) -> String {
        let mut hidden: Vec<&str> = self
            .env
            .values()
            .chain(&self.secrets)
            .map(String::as_str)
            .filter(|v| v.len() >= MIN_SECRET_LEN)
            .collect();
        // Longest first, so a secret that contains another is hidden whole.
        hidden.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let mut out = text.to_string();
        for value in hidden {
            if out.contains(value) {
                out = out.replace(value, "***");
            }
        }
        out
    }
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let args: Vec<String> = self.args.iter().map(|a| self.redact(a)).collect();
        f.debug_struct("ServerConfig")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &args)
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .field("enabled", &self.enabled)
            .field("auto_approve", &self.auto_approve)
            .field("source", &self.source)
            .finish()
    }
}

/// What [`load_server_configs`] found: the servers in precedence order, and warnings the
/// caller can show (skipped entries, unreadable files, unset variables).
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub servers: Vec<ServerConfig>,
    pub warnings: Vec<String>,
}

/// Reads the three sources (see the module docs). `workspace` is the chat's working
/// directory, whose `.mcp.json` is read when given. `env` looks up variables for `${VAR}`.
///
/// Never fails: a missing file is skipped silently, a malformed one becomes a warning.
pub fn load_server_configs(
    paths: &Paths,
    workspace: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Loaded {
    let mut out = Loaded::default();
    let mut ctx = Ctx { paths, env, warnings: &mut out.warnings };

    let mut import_claude_code = false;
    let config_file = paths.display(&paths.config_file);
    if let Some(text) = read_optional(&paths.config_file, ctx.warnings) {
        let (list, import) = parse_config_toml(&text, &config_file, &mut ctx);
        import_claude_code = import;
        merge(&mut out.servers, list, ctx.warnings);
    }

    if let Some(ws) = workspace {
        let file = ws.join(".mcp.json");
        if let Some(text) = read_optional(&file, ctx.warnings) {
            let origin = paths.display(&file);
            let list = parse_mcp_json(&text, ServerSource::McpJson, Some(ws), &origin, &mut ctx);
            merge(&mut out.servers, list, ctx.warnings);
        }
    }

    if import_claude_code {
        let file = paths.home.join(".claude.json");
        if let Some(text) = read_optional(&file, ctx.warnings) {
            let list = parse_mcp_json(&text, ServerSource::ClaudeJson, None, "~/.claude.json", &mut ctx);
            merge(&mut out.servers, list, ctx.warnings);
        }
    }
    out
}

/// What every parser needs: the home directory for `~`, the variable lookup and the warnings.
struct Ctx<'a> {
    paths: &'a Paths,
    env: &'a dyn Fn(&str) -> Option<String>,
    warnings: &'a mut Vec<String>,
}

impl Ctx<'_> {
    fn warn(&mut self, text: String) {
        self.warnings.push(text);
    }
}

/// Expansion state for one server: which variables were missing, which values are secret.
#[derive(Default)]
struct Expansion {
    missing: Vec<String>,
    secrets: Vec<String>,
}

impl Expansion {
    /// Expands a value that is not secret (command, cwd).
    fn plain(&mut self, text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
        expand(text, env, &mut self.missing, &mut Vec::new())
    }

    /// Expands an arg or env value, remembering what came from the environment.
    fn secret(&mut self, text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
        expand(text, env, &mut self.missing, &mut self.secrets)
    }

    fn finish(mut self, cfg: &mut ServerConfig, warnings: &mut Vec<String>) {
        self.missing.sort();
        self.missing.dedup();
        for var in self.missing {
            warnings.push(format!(
                "MCP server `{}`: environment variable `{var}` is not set (left as `${{{var}}}`)",
                cfg.name
            ));
        }
        self.secrets.retain(|s| s.len() >= MIN_SECRET_LEN);
        self.secrets.sort();
        self.secrets.dedup();
        cfg.secrets = self.secrets;
    }
}

/// `${VAR}` and `${VAR:-default}` expansion (Claude Code's rules). An unset variable without
/// a default stays as written and is recorded in `missing`; a bare `$VAR` is left alone.
/// Values taken from the environment for variables that are not [`PLAIN_VARS`] go to
/// `secrets`.
fn expand(
    text: &str,
    env: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
    secrets: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
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
        match (env(var), default) {
            (Some(value), _) => {
                if !PLAIN_VARS.contains(&var) && !var.starts_with("XDG_") {
                    secrets.push(value.clone());
                }
                out.push_str(&value);
            }
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

fn read_optional(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warnings.push(format!("Cannot read {}: {e}", path.display()));
            None
        }
    }
}

/// Appends `new` to `servers`; a name that is already there keeps its first definition.
fn merge(servers: &mut Vec<ServerConfig>, new: Vec<ServerConfig>, warnings: &mut Vec<String>) {
    for server in new {
        match servers.iter().find(|s| s.name == server.name) {
            Some(first) => warnings.push(format!(
                "MCP server `{}` from {} ignored: already defined in {}",
                server.name,
                server.source.label(),
                first.source.label()
            )),
            None => servers.push(server),
        }
    }
}

fn valid_name(name: &str, origin: &str, warnings: &mut Vec<String>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        warnings.push(format!("{origin}: MCP server with an empty name skipped"));
        return None;
    }
    Some(name.to_string())
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
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    auto_approve: Vec<String>,
}

fn enabled_by_default() -> bool {
    true
}

/// The servers of `config.toml` and its `import_claude_code` flag. A bad entry is skipped
/// with a warning; a file that is not TOML yields nothing and one warning.
fn parse_config_toml(text: &str, origin: &str, ctx: &mut Ctx) -> (Vec<ServerConfig>, bool) {
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            let msg = e.message().trim().to_string();
            ctx.warn(format!("{origin}: invalid TOML ({msg}); its MCP servers are ignored"));
            return (Vec::new(), false);
        }
    };
    let import = match table.get("import_claude_code") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(other) => {
            ctx.warn(format!(
                "{origin}: `import_claude_code` must be a boolean, got {}",
                other.type_str()
            ));
            false
        }
    };
    let entries = match table.get("mcp_servers") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) => a.clone(),
        Some(other) => {
            ctx.warn(format!(
                "{origin}: `mcp_servers` must be an array of tables ([[mcp_servers]]), got {}",
                other.type_str()
            ));
            Vec::new()
        }
    };
    let mut out: Vec<ServerConfig> = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        let entry_origin = format!("{origin} [[mcp_servers]] #{}", i + 1);
        let parsed: TomlServer = match entry.try_into() {
            Ok(p) => p,
            Err(e) => {
                ctx.warn(format!("{entry_origin} skipped: {}", e.message().trim()));
                continue;
            }
        };
        let Some(name) = valid_name(&parsed.name, &entry_origin, ctx.warnings) else { continue };
        if parsed.command.trim().is_empty() {
            ctx.warn(format!("MCP server `{name}`: empty `command`, skipped"));
            continue;
        }
        if out.iter().any(|s| s.name == name) {
            ctx.warn(format!("MCP server `{name}` is defined twice in {origin}; the first one wins"));
            continue;
        }
        let mut x = Expansion::default();
        let env = ctx.env;
        let mut cfg = ServerConfig {
            command: x.plain(&parsed.command, env),
            args: parsed.args.iter().map(|a| x.secret(a, env)).collect(),
            env: parsed.env.iter().map(|(k, v)| (k.clone(), x.secret(v, env))).collect(),
            cwd: parsed.cwd.map(|c| ctx.paths.expand(&x.plain(&c, env))),
            enabled: parsed.enabled,
            auto_approve: parsed.auto_approve,
            source: ServerSource::ConfigToml,
            secrets: Vec::new(),
            name,
        };
        x.finish(&mut cfg, ctx.warnings);
        out.push(cfg);
    }
    (out, import)
}

/// The servers of a Claude Code style `{"mcpServers": {…}}` document. Never fails; problems
/// become warnings. `default_cwd` is used when an entry has no `cwd`, and relative `cwd`s
/// resolve against it.
fn parse_mcp_json(
    text: &str,
    source: ServerSource,
    default_cwd: Option<&Path>,
    origin: &str,
    ctx: &mut Ctx,
) -> Vec<ServerConfig> {
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            ctx.warn(format!("{origin}: invalid JSON ({e}), ignored"));
            return Vec::new();
        }
    };
    let servers = match doc.get("mcpServers") {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Object(m)) => m,
        Some(_) => {
            ctx.warn(format!("{origin}: `mcpServers` is not an object, ignored"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (raw_name, entry) in servers {
        let Some(name) = valid_name(raw_name, origin, ctx.warnings) else { continue };
        if let Some(cfg) = parse_json_entry(name, entry, source, default_cwd, origin, ctx) {
            out.push(cfg);
        }
    }
    out
}

fn parse_json_entry(
    name: String,
    entry: &Value,
    source: ServerSource,
    default_cwd: Option<&Path>,
    origin: &str,
    ctx: &mut Ctx,
) -> Option<ServerConfig> {
    let Some(obj) = entry.as_object() else {
        ctx.warn(format!("{origin}: MCP server `{name}` is not an object, skipped"));
        return None;
    };
    let kind = obj.get("type").and_then(Value::as_str);
    let command = obj.get("command").and_then(Value::as_str).unwrap_or("");
    let remote = match kind {
        None | Some("stdio") => kind.is_none() && command.trim().is_empty() && obj.contains_key("url"),
        Some(_) => true,
    };
    if remote {
        let kind = kind.unwrap_or("url");
        ctx.warn(format!(
            "{origin}: MCP server `{name}` is a remote ({kind}) server; only stdio servers are supported, skipped"
        ));
        return None;
    }
    if command.trim().is_empty() {
        ctx.warn(format!("{origin}: MCP server `{name}` has no `command`, skipped"));
        return None;
    }
    let mut x = Expansion::default();
    let lookup = ctx.env;
    let args = string_list(obj.get("args"), &name, "args", origin, ctx.warnings)
        .iter()
        .map(|a| x.secret(a, lookup))
        .collect();
    let mut env = BTreeMap::new();
    match obj.get("env") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (key, value) in m {
                let value = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(_) | Value::Bool(_) => value.to_string(),
                    _ => {
                        ctx.warn(format!(
                            "{origin}: MCP server `{name}`: env `{key}` is not a string, ignored"
                        ));
                        continue;
                    }
                };
                env.insert(key.clone(), x.secret(&value, lookup));
            }
        }
        Some(_) => ctx.warn(format!("{origin}: MCP server `{name}`: `env` is not an object, ignored")),
    }
    let cwd = match obj.get("cwd").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => {
            let path = ctx.paths.expand(&x.plain(c, lookup));
            Some(match default_cwd {
                Some(base) if path.is_relative() => base.join(path),
                _ => path,
            })
        }
        _ => default_cwd.map(Path::to_path_buf),
    };
    let disabled = obj.get("disabled").and_then(Value::as_bool).unwrap_or(false);
    let enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true) && !disabled;
    let auto_approve = string_list(obj.get("autoApprove"), &name, "autoApprove", origin, ctx.warnings);
    let mut cfg = ServerConfig {
        command: x.plain(command, lookup),
        args,
        env,
        cwd,
        enabled,
        auto_approve,
        source,
        secrets: Vec::new(),
        name,
    };
    x.finish(&mut cfg, ctx.warnings);
    Some(cfg)
}

fn string_list(
    value: Option<&Value>,
    name: &str,
    field: &str,
    origin: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Number(_) | Value::Bool(_) => Some(item.to_string()),
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
