//! The MCP servers: one supervisor for the servers of `config.toml` (and `~/.claude.json`),
//! shared by every session, plus one per workspace for its `.mcp.json`. Each starts the first
//! time a session needs it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base::cancel::CancelToken;
use base::paths::Paths;
use mcp::{CallResult, McpTool, OnChange, ServerSource, Supervisor};
use model::McpServerInfo;
use serde_json::Value;

use crate::bus::lock;

/// The supervisors, started on first use.
pub(crate) struct McpHub {
    paths: Paths,
    global: Mutex<Option<Arc<Supervisor>>>,
    local: Mutex<HashMap<PathBuf, Arc<Supervisor>>>,
}

impl McpHub {
    pub fn new(paths: Paths) -> Self {
        Self { paths, global: Mutex::new(None), local: Mutex::new(HashMap::new()) }
    }

    /// The servers a session in `workspace` uses, starting their supervisors when needed.
    /// `on_change` is called whenever one of them changes state.
    pub fn view(&self, workspace: Option<&Path>, on_change: &OnChange) -> McpView {
        let env = |name: &str| std::env::var(name).ok();
        let global = lock(&self.global)
            .get_or_insert_with(|| {
                let loaded = mcp::load_server_configs(&self.paths, None, &env);
                Arc::new(Supervisor::start(loaded.servers, on_change.clone()))
            })
            .clone();
        let local = workspace.map(|ws| {
            lock(&self.local)
                .entry(ws.to_path_buf())
                .or_insert_with(|| {
                    let loaded = mcp::load_server_configs(&self.paths, Some(ws), &env);
                    let own: Vec<_> =
                        loaded.servers.into_iter().filter(|s| s.source == ServerSource::McpJson).collect();
                    Arc::new(Supervisor::start(own, on_change.clone()))
                })
                .clone()
        });
        McpView { supervisors: std::iter::once(global).chain(local).collect() }
    }

    /// Stops every server.
    pub fn shutdown(&self) {
        if let Some(global) = lock(&self.global).take() {
            global.shutdown();
        }
        for (_, local) in lock(&self.local).drain() {
            local.shutdown();
        }
    }
}

/// The servers one session sees, as the agent loop uses them.
pub(crate) struct McpView {
    supervisors: Vec<Arc<Supervisor>>,
}

impl McpView {
    /// Every server with its state.
    pub fn servers(&self) -> Vec<McpServerInfo> {
        self.supervisors.iter().flat_map(|s| s.servers()).collect()
    }

    /// Restarts a server by name. Returns whether one was found.
    pub fn reconnect(&self, name: &str) -> bool {
        self.supervisors.iter().any(|s| s.reconnect(name))
    }

    fn owner(&self, tool_id: &str) -> Option<&Supervisor> {
        self.supervisors.iter().find(|s| s.tools().iter().any(|t| t.id == tool_id)).map(|s| &**s)
    }
}

impl agent::Mcp for McpView {
    fn tools(&self) -> Vec<McpTool> {
        let mut out: Vec<McpTool> = Vec::new();
        for tool in self.supervisors.iter().flat_map(|s| s.tools()) {
            if !out.iter().any(|t| t.id == tool.id) {
                out.push(tool);
            }
        }
        out
    }

    fn call(&self, tool_id: &str, arguments: Value, cancel: &CancelToken) -> CallResult {
        match self.owner(tool_id) {
            Some(owner) => owner.call(tool_id, arguments, cancel),
            // The global supervisor answers with its own "not found" result.
            None => self.supervisors[0].call(tool_id, arguments, cancel),
        }
    }

    fn is_auto_approved(&self, tool_id: &str) -> bool {
        self.owner(tool_id).is_some_and(|s| s.is_auto_approved(tool_id))
    }
}
