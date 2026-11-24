//! The MCP servers as the loop uses them.

use base::cancel::CancelToken;
use mcp::{CallResult, McpTool, Supervisor};
use serde_json::Value;

/// The tools of the workspace's MCP servers. [`mcp::Supervisor`] is the real one.
pub trait Mcp: Send + Sync {
    /// The tools of every ready server.
    fn tools(&self) -> Vec<McpTool>;

    /// Runs a tool by its id; failures come back as an error result.
    fn call(&self, tool_id: &str, arguments: Value, cancel: &CancelToken) -> CallResult;

    /// Whether the server config lets the tool run without asking.
    fn is_auto_approved(&self, tool_id: &str) -> bool;
}

impl Mcp for Supervisor {
    fn tools(&self) -> Vec<McpTool> {
        Supervisor::tools(self)
    }

    fn call(&self, tool_id: &str, arguments: Value, cancel: &CancelToken) -> CallResult {
        Supervisor::call(self, tool_id, arguments, cancel)
    }

    fn is_auto_approved(&self, tool_id: &str) -> bool {
        Supervisor::is_auto_approved(self, tool_id)
    }
}
