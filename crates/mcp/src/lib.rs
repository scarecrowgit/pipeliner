//! Minimal MCP client over stdio (JSON-RPC 2.0, newline-delimited JSON).
//!
//! Blocking API — run on background threads. `Client` is `Send + Sync`; calls from several
//! threads are multiplexed by request id (a reader thread routes responses).
//!
//! Server sources (see [`load_server_configs`]), later sources do NOT override earlier names:
//! 1. `[[mcp_servers]]` in the Harness config TOML (`llm::Config::path()`):
//!    ```toml
//!    import_claude_code = false        # top-level: also import stdio servers from ~/.claude.json
//!    [[mcp_servers]]
//!    name = "fs"
//!    command = "npx"
//!    args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
//!    env = { KEY = "value" }
//!    cwd = "/optional/dir"
//!    enabled = true                    # default true
//!    auto_approve = ["read_file"]      # tools run without asking; ["*"] = all
//!    ```
//! 2. `<workspace>/.mcp.json` — Claude Code project format
//!    `{"mcpServers": {"name": {"command": "...", "args": [], "env": {}, "autoApprove": []}}}`.
//! 3. `~/.claude.json` top-level `mcpServers` — only when `import_claude_code = true`.
//!
//! Only stdio servers are supported; `type: "http" | "sse"` entries are skipped with a warning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

mod client;
mod config;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerSource {
    HarnessConfig,
    ProjectMcpJson,
    ClaudeCode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub enabled: bool,
    /// Tool names (without the `mcp__server__` prefix) that run without a permission
    /// prompt; `"*"` approves every tool of this server.
    pub auto_approve: Vec<String>,
    pub source: ServerSource,
}

impl ServerConfig {
    pub fn auto_approves(&self, tool: &str) -> bool {
        self.auto_approve.iter().any(|t| t == "*" || t == tool)
    }
}

/// Load server configs from the three sources. Returns the servers plus human-readable
/// warnings (skipped http servers, parse problems in optional files). A missing file is
/// not an error; a malformed Harness config TOML is.
pub fn load_server_configs(
    harness_config: &Path,
    workspace_root: &Path,
    claude_json: &Path,
) -> anyhow::Result<(Vec<ServerConfig>, Vec<String>)> {
    // Note: servers from `.mcp.json` get `cwd = workspace_root` unless they set one (relative
    // `cwd`s resolve against it), matching Claude Code which runs them in the project root.
    config::load(harness_config, workspace_root, claude_json, &|k| std::env::var(k).ok())
}

/// Default location of Claude Code's config: `$HOME/.claude.json`.
pub fn claude_json_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude.json")
}

/// A tool advertised by a server (`tools/list`).
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema (`inputSchema`), `{"type":"object"}` if the server omitted it.
    pub input_schema: Value,
}

/// Result of `tools/call`: text content blocks joined with blank lines; non-text blocks
/// become placeholders like `[image: image/png, 12 KB]`. `is_error` from the result or
/// from a JSON-RPC error.
#[derive(Clone, Debug, PartialEq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

/// Model-facing tool name: `mcp__{server}__{tool}`, characters outside `[A-Za-z0-9_-]`
/// replaced by `_`, truncated to 64 chars (OpenAI/Anthropic limit).
pub fn tool_id(server: &str, tool: &str) -> String {
    let raw = format!("mcp__{server}__{tool}");
    let mut s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    s.truncate(64);
    s
}

/// A running MCP server process with an initialized session.
pub struct Client {
    shared: Arc<client::Shared>,
    server_info: (String, String),
}

impl Client {
    /// Spawn the server (stdin/stdout piped, stderr captured into a ring buffer), run the
    /// `initialize` handshake (protocolVersion "2025-06-18", falls back to what the server
    /// answers) and send `notifications/initialized`. Fails if it doesn't answer in `timeout`.
    pub fn start(config: &ServerConfig, timeout: Duration) -> anyhow::Result<Client> {
        let (shared, server_info) = client::Shared::start(config, timeout)?;
        Ok(Client { shared, server_info })
    }

    /// `(serverInfo.name, serverInfo.version)` from `initialize`.
    pub fn server_info(&self) -> (String, String) {
        self.server_info.clone()
    }

    /// `tools/list`, following `nextCursor` pagination.
    pub fn list_tools(&self, timeout: Duration) -> anyhow::Result<Vec<McpTool>> {
        self.shared.list_tools(timeout)
    }

    /// `tools/call`. On `cancel` or timeout sends `notifications/cancelled` for the request
    /// and returns an `is_error` result (not `Err`). `Err` only for a dead/broken transport.
    pub fn call_tool(
        &self,
        tool: &str,
        arguments: Value,
        cancel: Arc<AtomicBool>,
        timeout: Duration,
    ) -> anyhow::Result<CallResult> {
        self.shared.call_tool(tool, arguments, &cancel, timeout)
    }

    /// The process is still running and the transport is open.
    pub fn is_alive(&self) -> bool {
        self.shared.is_alive()
    }

    /// Last ~4 KB of the server's stderr (for error messages in the UI).
    pub fn stderr_tail(&self) -> String {
        self.shared.stderr_tail()
    }

    /// Close stdin, wait briefly, then kill. Also done on drop.
    pub fn shutdown(&self) {
        self.shared.shutdown()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shared.shutdown();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::Client>();
    }

    #[test]
    fn tool_ids() {
        assert_eq!(super::tool_id("my server", "read.file"), "mcp__my_server__read_file");
        assert_eq!(super::tool_id("s", &"x".repeat(100)).len(), 64);
    }
}
