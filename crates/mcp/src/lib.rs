//! MCP stdio client, server config sources and the supervisor that owns server processes.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`config`] | Server definitions from `config.toml`, `.mcp.json` and `~/.claude.json` |
//! | [`client`] | A JSON-RPC client for one stdio server process |
//! | [`supervisor`] | Every server of a workspace on its own threads: status, tools, calls |
//!
//! The engine loads configs with [`load_server_configs`], hands them to
//! [`Supervisor::start`] and from then on asks the supervisor for status, tools and calls.

pub mod client;
pub mod config;
mod content;
mod rpc;
pub mod supervisor;

pub use client::{CallResult, Client, McpError, PROTOCOL_VERSION, ServerInfo, ToolInfo};
pub use config::{Loaded, ServerConfig, ServerSource, load_server_configs};
pub use content::MAX_RESULT_BYTES;
pub use supervisor::{McpTool, OnChange, Supervisor, Timeouts};

/// Longest tool name model providers accept.
const MAX_TOOL_ID: usize = 64;

/// The model-facing name of a server's tool: `mcp__<server>__<tool>`, every character
/// outside `[A-Za-z0-9_-]` replaced by `_`, cut to 64 characters.
pub fn tool_id(server: &str, tool: &str) -> String {
    let mut id = format!("mcp__{}__{}", sanitize(server), sanitize(tool));
    // Sanitised text is ASCII, so any byte index is a character boundary.
    id.truncate(MAX_TOOL_ID);
    id
}

/// `text` with every character outside `[A-Za-z0-9_-]` replaced by `_`.
pub(crate) fn sanitize(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_ids_are_sanitised_and_capped() {
        assert_eq!(tool_id("my server", "read.file"), "mcp__my_server__read_file");
        assert_eq!(tool_id("gh-1", "créer"), "mcp__gh-1__cr_er");
        assert_eq!(tool_id("s", &"x".repeat(100)).len(), 64);
    }

    #[test]
    fn handles_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Client>();
        check::<Supervisor>();
    }
}
