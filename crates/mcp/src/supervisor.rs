//! The owner of every MCP server of one workspace.
//!
//! Servers start, die and restart on the supervisor's own threads; the calls the engine makes
//! ([`Supervisor::servers`], [`Supervisor::tools`], [`Supervisor::reconnect`]) only read or
//! flip state under a short lock, so no caller waits on a slow server. Only
//! [`Supervisor::call`] blocks, for the length of the tool call (bounded by its timeout and
//! cancel token), and [`Supervisor::shutdown`], for the time it takes to stop the processes.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use model::chat::ToolSpec;
use model::workspace::{McpServerInfo, McpState};
use serde_json::{Value, json};

use crate::client::{CallResult, Client, McpError, ToolInfo};
use crate::config::ServerConfig;
use crate::rpc::{lock, seconds};
use crate::{sanitize, tool_id};

/// Called after a server changed state (started, failed, reconnecting), on the thread that
/// changed it and never under the supervisor's lock.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// How long things may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Start-up of one server: spawn, `initialize` and `tools/list` together.
    pub start: Duration,
    /// One `tools/call`.
    pub call: Duration,
    /// How often ready servers are checked for having died.
    pub liveness: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            start: Duration::from_secs(20),
            call: Duration::from_secs(120),
            liveness: Duration::from_secs(2),
        }
    }
}

/// A tool of a ready server, as the model sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    /// `mcp__<server>__<tool>`, sanitised; also `spec.name`.
    pub id: String,
    /// The server's configured name.
    pub server: String,
    /// The tool's own name on the server.
    pub name: String,
    pub spec: ToolSpec,
}

struct Server {
    config: ServerConfig,
    state: McpState,
    error: Option<String>,
    tools: Vec<McpTool>,
    client: Option<Arc<Client>>,
    /// Bumped by every (re)start, so a late result of an older start is dropped.
    generation: u64,
    /// Cancels the start in flight.
    cancel: CancelToken,
}

struct State {
    servers: Vec<Server>,
    shut_down: bool,
}

struct Inner {
    state: Mutex<State>,
    on_change: OnChange,
    timeouts: Timeouts,
    /// Cancelled by shutdown; every start token is a child of it.
    stop: CancelToken,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// Owns the MCP servers of one workspace. Dropping it stops them all.
pub struct Supervisor {
    inner: Arc<Inner>,
}

impl Supervisor {
    /// Takes over `configs` and starts every enabled server in parallel, returning at once.
    /// A name that appears twice keeps its first config.
    pub fn start(configs: Vec<ServerConfig>, on_change: OnChange) -> Supervisor {
        Self::start_with(configs, on_change, Timeouts::default())
    }

    /// [`Supervisor::start`] with other timeouts (tests use short ones).
    pub fn start_with(configs: Vec<ServerConfig>, on_change: OnChange, timeouts: Timeouts) -> Supervisor {
        let supervisor = Self::configure_with(configs, on_change, timeouts);
        supervisor.inner.start_idle();
        supervisor
    }

    /// Takes over `configs` without starting anything: enabled servers stay
    /// [`McpState::Idle`] until [`Supervisor::start_idle`] or a reconnect starts them.
    pub fn configure(configs: Vec<ServerConfig>, on_change: OnChange) -> Supervisor {
        Self::configure_with(configs, on_change, Timeouts::default())
    }

    /// [`Supervisor::configure`] with other timeouts.
    pub fn configure_with(configs: Vec<ServerConfig>, on_change: OnChange, timeouts: Timeouts) -> Supervisor {
        let stop = CancelToken::new();
        let mut servers: Vec<Server> = Vec::new();
        for config in configs {
            if servers.iter().any(|s| s.config.name == config.name) {
                continue;
            }
            let state = if config.enabled { McpState::Idle } else { McpState::Disabled };
            servers.push(Server {
                config,
                state,
                error: None,
                tools: Vec::new(),
                client: None,
                generation: 0,
                cancel: stop.child(),
            });
        }
        let inner = Arc::new(Inner {
            state: Mutex::new(State { servers, shut_down: false }),
            on_change,
            timeouts,
            stop,
            threads: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        let interval = timeouts.liveness;
        let spawned = std::thread::Builder::new().name("mcp-liveness".into()).spawn(move || {
            while stop.sleep(interval) {
                match weak.upgrade() {
                    Some(inner) => inner.check_liveness(),
                    None => return,
                }
            }
        });
        if let Ok(handle) = spawned {
            lock(&inner.threads).push(handle);
        }
        Supervisor { inner }
    }

    /// Starts every server that is still [`McpState::Idle`], in parallel, returning at once.
    /// Returns how many starts began; calls `on_change` when there was one.
    pub fn start_idle(&self) -> usize {
        let started = self.inner.start_idle();
        if started > 0 {
            (self.inner.on_change)();
        }
        started
    }

    /// Every configured server with its state, tool count and, for a failed one, the error
    /// with the end of its stderr.
    pub fn servers(&self) -> Vec<McpServerInfo> {
        let state = lock(&self.inner.state);
        state
            .servers
            .iter()
            .map(|s| McpServerInfo {
                name: s.config.name.clone(),
                state: s.state,
                tools: u32::try_from(s.tools.len()).unwrap_or(u32::MAX),
                error: s.error.clone(),
                source: s.config.source.label().to_string(),
            })
            .collect()
    }

    /// The tools of every ready server, in config order. When two tools map to one id, the
    /// first keeps it and the other is left out.
    pub fn tools(&self) -> Vec<McpTool> {
        let state = lock(&self.inner.state);
        let mut out: Vec<McpTool> = Vec::new();
        for tool in ready_tools(&state) {
            if !out.iter().any(|t| t.id == tool.id) {
                out.push(tool.clone());
            }
        }
        out
    }

    /// Whether the config lets the tool behind `tool_id` run without asking.
    pub fn is_auto_approved(&self, tool_id: &str) -> bool {
        let state = lock(&self.inner.state);
        let found = state
            .servers
            .iter()
            .filter(|s| s.state == McpState::Ready)
            .find_map(|s| s.tools.iter().find(|t| t.id == tool_id).map(|t| (s, t)));
        found.is_some_and(|(server, tool)| server.config.auto_approves(&tool.name))
    }

    /// Runs the tool behind `tool_id`, blocking until it answers, the call timeout passes or
    /// `cancel` fires. Every failure comes back as an `is_error` result the model can read.
    /// A server found dead here is marked failed.
    pub fn call(&self, tool_id: &str, arguments: Value, cancel: &CancelToken) -> CallResult {
        let target = {
            let state = lock(&self.inner.state);
            let found = state.servers.iter().filter(|s| s.state == McpState::Ready).find_map(|s| {
                let tool = s.tools.iter().find(|t| t.id == tool_id)?;
                Some((s.config.clone(), s.client.clone()?, tool.name.clone(), s.generation))
            });
            match found {
                Some(target) => target,
                None => return failed(not_available(&state, tool_id)),
            }
        };
        let (config, client, tool, generation) = target;
        let arguments = normalize_arguments(arguments);
        match client.call_tool(&tool, arguments, cancel, self.inner.timeouts.call) {
            Ok(result) => result,
            Err(McpError::Cancelled) => failed(format!("MCP tool `{tool}` was cancelled.")),
            Err(McpError::Timeout(_)) => failed(format!(
                "MCP tool `{tool}` timed out after {} and was cancelled.",
                seconds(self.inner.timeouts.call)
            )),
            Err(e @ McpError::Closed(_)) => {
                client.shutdown();
                let text = failure_text(&config, &client, &e);
                self.inner.mark_failed(&config.name, generation, text.clone());
                failed(format!("MCP server `{}` stopped: {text}", config.name))
            }
            Err(e) => failed(config.redact(&format!("MCP tool `{tool}` failed: {e}"))),
        }
    }

    /// Stops the server `name` (if it runs) and starts it again. Returns `false` for an
    /// unknown or disabled server, or after shutdown.
    pub fn reconnect(&self, name: &str) -> bool {
        let start = {
            let mut state = lock(&self.inner.state);
            if state.shut_down {
                return false;
            }
            let Some(server) = state.servers.iter_mut().find(|s| s.config.name == name) else {
                return false;
            };
            if server.state == McpState::Disabled {
                return false;
            }
            server.cancel.cancel();
            server.cancel = self.inner.stop.child();
            server.generation += 1;
            server.state = McpState::Starting;
            server.error = None;
            server.tools.clear();
            (server.generation, server.cancel.clone(), server.client.take())
        };
        let (generation, cancel, old) = start;
        self.inner.spawn_start(name.to_string(), generation, cancel, old);
        (self.inner.on_change)();
        true
    }

    /// Stops every server and the supervisor's threads, waiting (a couple of seconds at
    /// most) until the processes are gone. Does not call `on_change`. Safe to call again.
    pub fn shutdown(&self) {
        let clients: Vec<Arc<Client>> = {
            let mut state = lock(&self.inner.state);
            if state.shut_down {
                return;
            }
            state.shut_down = true;
            self.inner.stop.cancel();
            let mut clients = Vec::new();
            for server in &mut state.servers {
                clients.extend(server.client.take());
                server.tools.clear();
                if matches!(server.state, McpState::Starting | McpState::Ready) {
                    server.state = McpState::Failed;
                    server.error = Some("Stopped".into());
                }
            }
            clients
        };
        std::thread::scope(|scope| {
            for client in &clients {
                let spawned = std::thread::Builder::new()
                    .name("mcp-stop".into())
                    .spawn_scoped(scope, || client.shutdown());
                if spawned.is_err() {
                    client.shutdown();
                }
            }
        });
        let handles = std::mem::take(&mut *lock(&self.inner.threads));
        let me = std::thread::current().id();
        for handle in handles {
            // A callback running on one of our threads may be the one shutting down.
            if handle.thread().id() != me {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Inner {
    /// Moves every idle server to `Starting` and brings each up on its own thread. Returns
    /// how many it started; nothing after shutdown.
    fn start_idle(self: &Arc<Self>) -> usize {
        let starts: Vec<(String, u64, CancelToken)> = {
            let mut state = lock(&self.state);
            if state.shut_down {
                return 0;
            }
            state
                .servers
                .iter_mut()
                .filter(|s| s.state == McpState::Idle)
                .map(|s| {
                    s.state = McpState::Starting;
                    (s.config.name.clone(), s.generation, s.cancel.clone())
                })
                .collect()
        };
        let count = starts.len();
        for (name, generation, cancel) in starts {
            self.spawn_start(name, generation, cancel, None);
        }
        count
    }

    /// Brings a server up on its own thread. `old` is the client it replaces, stopped first.
    fn spawn_start(self: &Arc<Self>, name: String, generation: u64, cancel: CancelToken, old: Option<Arc<Client>>) {
        let inner = self.clone();
        let thread_name = format!("mcp-start-{}", sanitize(&name));
        let task_name = name.clone();
        let spawned = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || inner.run_start(&task_name, generation, &cancel, old));
        match spawned {
            Ok(handle) => {
                let mut threads = lock(&self.threads);
                threads.retain(|h| !h.is_finished());
                threads.push(handle);
            }
            Err(e) => self.mark_failed(&name, generation, format!("could not start a thread: {e}")),
        }
    }

    fn run_start(&self, name: &str, generation: u64, cancel: &CancelToken, old: Option<Arc<Client>>) {
        if let Some(old) = old {
            old.shutdown();
        }
        let config = {
            let state = lock(&self.state);
            match state.servers.iter().find(|s| s.config.name == name && s.generation == generation) {
                Some(s) => s.config.clone(),
                None => return,
            }
        };
        let outcome = bring_up(&config, self.timeouts.start, cancel);
        let stale = {
            let mut state = lock(&self.state);
            let current = !state.shut_down && !cancel.is_cancelled();
            let server = state.servers.iter_mut().find(|s| s.config.name == name && s.generation == generation);
            match (server, outcome) {
                (Some(server), outcome) if current => {
                    match outcome {
                        Ok((client, tools)) => {
                            server.tools = tools.into_iter().map(|t| make_tool(name, t)).collect();
                            server.client = Some(client);
                            server.state = McpState::Ready;
                            server.error = None;
                        }
                        Err(text) => {
                            server.state = McpState::Failed;
                            server.error = Some(text);
                        }
                    }
                    None
                }
                (_, Ok((client, _))) => Some(client),
                (_, Err(_)) => return,
            }
        };
        match stale {
            // Superseded by a reconnect or a shutdown while starting.
            Some(client) => client.shutdown(),
            None => (self.on_change)(),
        }
    }

    /// Marks a ready or starting server of this `generation` failed and stops its client.
    fn mark_failed(&self, name: &str, generation: u64, error: String) {
        let client = {
            let mut state = lock(&self.state);
            if state.shut_down {
                return;
            }
            let Some(server) = state.servers.iter_mut().find(|s| s.config.name == name) else { return };
            if server.generation != generation || !matches!(server.state, McpState::Ready | McpState::Starting) {
                return;
            }
            server.state = McpState::Failed;
            server.error = Some(error);
            server.tools.clear();
            server.client.take()
        };
        if let Some(client) = client {
            client.shutdown();
        }
        (self.on_change)();
    }

    /// Marks ready servers whose process died or whose connection closed as failed.
    fn check_liveness(&self) {
        let ready: Vec<(String, u64, Arc<Client>, ServerConfig)> = {
            let state = lock(&self.state);
            state
                .servers
                .iter()
                .filter(|s| s.state == McpState::Ready)
                .filter_map(|s| Some((s.config.name.clone(), s.generation, s.client.clone()?, s.config.clone())))
                .collect()
        };
        for (name, generation, client, config) in ready {
            if client.is_alive() {
                continue;
            }
            client.shutdown();
            let error = McpError::Closed(client.closed_reason().unwrap_or_else(|| "the server stopped".into()));
            self.mark_failed(&name, generation, failure_text(&config, &client, &error));
        }
    }
}

type Started = Result<(Arc<Client>, Vec<ToolInfo>), String>;

/// Spawn, `initialize` and `tools/list` within `timeout`. A failure is the error text the
/// UI shows.
fn bring_up(config: &ServerConfig, timeout: Duration, cancel: &CancelToken) -> Started {
    let deadline = Instant::now() + timeout;
    let client = Client::spawn(config).map_err(|e| config.redact(&e.to_string()))?;
    let result = client
        .initialize(timeout, cancel)
        .and_then(|_| client.list_tools(deadline.saturating_duration_since(Instant::now()), cancel));
    match result {
        Ok(tools) => Ok((Arc::new(client), tools)),
        Err(e) => {
            client.shutdown();
            Err(failure_text(config, &client, &e))
        }
    }
}

/// `error (exited with …)` and the stderr tail, with secrets hidden. Call after the client
/// was shut down so the exit status and the last of stderr are in.
fn failure_text(config: &ServerConfig, client: &Client, error: &McpError) -> String {
    let mut text = error.to_string();
    if matches!(error, McpError::Closed(_))
        && let Some(status) = client.exit_status()
    {
        text = format!("{text} ({status})");
    }
    let tail = client.stderr_tail();
    if !tail.is_empty() {
        text = format!("{text}\n--- stderr ---\n{tail}");
    }
    config.redact(&text)
}

fn make_tool(server: &str, info: ToolInfo) -> McpTool {
    let id = tool_id(server, &info.name);
    let description = match info.description.trim() {
        "" => format!("Tool `{}` of the MCP server `{server}`.", info.name),
        text => format!("[MCP server `{server}`] {text}"),
    };
    McpTool {
        spec: ToolSpec { name: id.clone(), description, input_schema: info.input_schema },
        id,
        server: server.to_string(),
        name: info.name,
    }
}

fn ready_tools(state: &State) -> impl Iterator<Item = &McpTool> {
    state.servers.iter().filter(|s| s.state == McpState::Ready).flat_map(|s| s.tools.iter())
}

/// Why `tool_id` cannot run: its server is not ready, or nothing has that id.
fn not_available(state: &State, tool_id: &str) -> String {
    let server = state
        .servers
        .iter()
        .find(|s| tool_id.starts_with(&format!("mcp__{}__", sanitize(&s.config.name))));
    let Some(server) = server else {
        return format!("Unknown MCP tool `{tool_id}`.");
    };
    let name = &server.config.name;
    match server.state {
        McpState::Starting => format!("MCP server `{name}` is still starting; try again shortly."),
        McpState::Idle => format!("MCP server `{name}` has not been started yet; try again shortly."),
        McpState::Disabled => format!("MCP server `{name}` is disabled in its config."),
        McpState::Failed => {
            let why = server.error.as_deref().map(base::fmt::first_line).unwrap_or("unknown error");
            format!("MCP server `{name}` is not running (failed: {why}).")
        }
        McpState::Ready => format!("MCP server `{name}` has no tool `{tool_id}`."),
    }
}

/// Some providers send tool input as a JSON string; MCP wants an object.
fn normalize_arguments(arguments: Value) -> Value {
    match arguments {
        Value::Null => json!({}),
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v @ Value::Object(_)) => v,
            _ => Value::String(s),
        },
        other => other,
    }
}

fn failed(text: String) -> CallResult {
    CallResult { text, is_error: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_normalised() {
        assert_eq!(normalize_arguments(Value::Null), json!({}));
        assert_eq!(normalize_arguments(json!("{\"a\": 1}")), json!({"a": 1}));
        assert_eq!(normalize_arguments(json!("[1]")), json!("[1]"));
        assert_eq!(normalize_arguments(json!({"b": 2})), json!({"b": 2}));
    }

    #[test]
    fn tools_carry_their_id_as_spec_name() {
        let tool = make_tool(
            "my server",
            ToolInfo { name: "read.file".into(), description: " Reads. ".into(), input_schema: json!({"type": "object"}) },
        );
        assert_eq!(tool.id, "mcp__my_server__read_file");
        assert_eq!(tool.spec.name, tool.id);
        assert_eq!(tool.spec.description, "[MCP server `my server`] Reads.");
        assert_eq!((tool.server.as_str(), tool.name.as_str()), ("my server", "read.file"));
    }

    #[test]
    fn unknown_and_disabled_tools_explain_themselves() {
        let on_change: OnChange = Arc::new(|| {});
        let mut off = ServerConfig::new("off", "/nonexistent");
        off.enabled = false;
        let sup = Supervisor::start(vec![off], on_change);
        assert_eq!(sup.servers()[0].state, McpState::Disabled);
        let r = sup.call("mcp__off__x", Value::Null, &CancelToken::new());
        assert!(r.is_error && r.text.contains("disabled"), "{r:?}");
        let r = sup.call("mcp__nope__x", Value::Null, &CancelToken::new());
        assert!(r.is_error && r.text.contains("Unknown MCP tool"), "{r:?}");
        assert!(!sup.is_auto_approved("mcp__off__x"));
        assert!(!sup.reconnect("off") && !sup.reconnect("nope"));
    }
}
