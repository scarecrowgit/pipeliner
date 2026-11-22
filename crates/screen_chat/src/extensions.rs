//! Skills and MCP servers: discovery and startup on background threads, the model-facing
//! tool specs, dispatch of `skill` / MCP calls, liveness checks and the `/skills` and
//! `/mcp` notices.
//!
//! MCP work is blocking (process spawn, JSON-RPC round trips with long timeouts), so it
//! runs on dedicated `std::thread`s rather than the shared background executor; results
//! come back over a `smol::channel` to a foreground task that updates the entity.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use agent_tools::skills::{self, Skill};
use gpui::{AppContext as _, Context, Task};
use serde_json::Value;

use crate::ChatScreen;
use crate::session::{ToolResult, truncate};

/// `Client::start` and the first `tools/list`.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// One `tools/call`.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How often running servers are checked for an unexpected exit.
const LIVENESS_INTERVAL: Duration = Duration::from_secs(5);
/// MCP result text sent to the model at most (head kept).
const MAX_RESULT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ServerStatus {
    Starting,
    Ready,
    Failed { error: String, stderr: String },
    Disabled,
}

pub(crate) struct McpServer {
    pub config: mcp::ServerConfig,
    pub status: ServerStatus,
    pub client: Option<Arc<mcp::Client>>,
    pub tools: Vec<mcp::McpTool>,
    /// Error details expanded in the panel.
    pub open: bool,
    /// Bumped on every (re)start so a late result of an older start is dropped.
    generation: u64,
}

pub(crate) enum McpPhase {
    Loading,
    Ready,
    Failed(String),
}

/// `(server name, tool name)` of a model-facing MCP tool id.
pub(crate) type McpTarget = (String, String);

pub(crate) struct McpState {
    pub phase: McpPhase,
    pub servers: Vec<McpServer>,
    pub warnings: Vec<String>,
    /// Model-facing tool id → (server, tool), for every tool of a ready server.
    pub index: BTreeMap<String, McpTarget>,
    load_task: Option<Task<()>>,
    start_tasks: BTreeMap<String, Task<()>>,
    liveness_task: Option<Task<()>>,
}

impl McpState {
    pub fn new() -> Self {
        Self {
            phase: McpPhase::Loading,
            servers: Vec::new(),
            warnings: Vec::new(),
            index: BTreeMap::new(),
            load_task: None,
            start_tasks: BTreeMap::new(),
            liveness_task: None,
        }
    }

    pub fn server(&self, name: &str) -> Option<&McpServer> {
        self.servers.iter().find(|s| s.config.name == name)
    }

    fn server_mut(&mut self, name: &str) -> Option<&mut McpServer> {
        self.servers.iter_mut().find(|s| s.config.name == name)
    }

    pub fn auto_approves(&self, (server, tool): &McpTarget) -> bool {
        self.server(server)
            .is_some_and(|s| s.config.auto_approves(tool))
    }

    /// Rebuild the id → (server, tool) table. A tool whose id collides with an earlier
    /// one (sanitising/truncation can map two names to one id) is left out.
    fn rebuild_index(&mut self) {
        self.index.clear();
        for s in &self.servers {
            if s.status != ServerStatus::Ready {
                continue;
            }
            for t in &s.tools {
                let id = mcp::tool_id(&s.config.name, &t.name);
                if self.index.contains_key(&id) {
                    eprintln!(
                        "harness: MCP tool {}/{} skipped: its id `{id}` collides with another tool",
                        s.config.name, t.name
                    );
                    continue;
                }
                self.index
                    .insert(id, (s.config.name.clone(), t.name.clone()));
            }
        }
    }

    /// Specs for every indexed MCP tool, in server/tool order.
    pub fn tool_specs(&self) -> Vec<llm::ToolSpec> {
        let mut out = Vec::new();
        for s in &self.servers {
            if s.status != ServerStatus::Ready {
                continue;
            }
            for t in &s.tools {
                let id = mcp::tool_id(&s.config.name, &t.name);
                if self.index.get(&id) != Some(&(s.config.name.clone(), t.name.clone())) {
                    continue;
                }
                let desc = t.description.trim();
                let description = if desc.is_empty() {
                    format!("Tool `{}` of the MCP server `{}`.", t.name, s.config.name)
                } else {
                    format!("[MCP server `{}`] {desc}", s.config.name)
                };
                let input_schema = match &t.input_schema {
                    Value::Object(_) => t.input_schema.clone(),
                    _ => serde_json::json!({ "type": "object" }),
                };
                out.push(llm::ToolSpec {
                    name: id,
                    description,
                    input_schema,
                });
            }
        }
        out
    }

    pub fn has_tools(&self) -> bool {
        !self.index.is_empty()
    }

    /// Clients of all servers (for shutdown).
    pub fn take_clients(&mut self) -> Vec<Arc<mcp::Client>> {
        self.servers
            .iter_mut()
            .filter_map(|s| s.client.take())
            .collect()
    }
}

impl Drop for McpState {
    /// The screen is gone: stop the server processes off the UI thread.
    fn drop(&mut self) {
        release(self.take_clients());
    }
}

/// Drop clients on a worker thread: `Client::drop` shuts the process down and may block
/// for a second or more, which must never happen on the UI thread.
fn release(clients: Vec<Arc<mcp::Client>>) {
    if clients.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mcp-release".into())
        .spawn(move || drop(clients));
    if let Err(e) = spawned {
        eprintln!("harness: could not spawn a thread to stop MCP servers: {e}");
    }
}

/// Run `f` on its own thread; the receiver yields its result, or the panic payload.
fn on_thread<T: Send + 'static>(
    name: String,
    f: impl FnOnce() -> T + Send + 'static,
) -> smol::channel::Receiver<std::thread::Result<T>> {
    let (tx, rx) = smol::channel::bounded(1);
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        // A closed channel (the screen is gone) drops the value here, e.g. a fresh client.
        let _ = tx.send_blocking(r);
    });
    if let Err(e) = spawned {
        eprintln!("harness: could not spawn a thread: {e}");
    }
    rx
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// `Ok(value)`, or an error text for a panic / a thread that never answered.
async fn recv<T>(
    rx: smol::channel::Receiver<std::thread::Result<T>>,
    what: &str,
) -> Result<T, String> {
    match rx.recv().await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(p)) => Err(format!(
            "{what}: internal error ({})",
            panic_text(p.as_ref())
        )),
        Err(_) => Err(format!("{what}: the worker thread did not answer")),
    }
}

type Started = Result<(Arc<mcp::Client>, Vec<mcp::McpTool>), (String, String)>;

fn start_blocking(config: &mcp::ServerConfig) -> Started {
    let client =
        mcp::Client::start(config, START_TIMEOUT).map_err(|e| (format!("{e:#}"), String::new()))?;
    match client.list_tools(START_TIMEOUT) {
        Ok(tools) => Ok((Arc::new(client), tools)),
        Err(e) => Err((format!("tools/list failed: {e:#}"), client.stderr_tail())),
    }
}

/// Some providers send tool input JSON-encoded as a string; MCP wants an object.
fn input_object(input: &Value) -> Value {
    match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => v,
            _ => input.clone(),
        },
        Value::Null => Value::Object(Default::default()),
        v => v.clone(),
    }
}

fn cap_result(mut text: String) -> String {
    if text.len() <= MAX_RESULT_BYTES {
        return text;
    }
    let mut cut = MAX_RESULT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = text.len() - cut;
    text.truncate(cut);
    text.push_str(&format!("\n[… output truncated: {dropped} more bytes]"));
    text
}

fn result(content: impl Into<String>, is_error: bool) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error,
        display: None,
    }
}

/// Size and full text of a skill, read once at discovery.
pub(crate) struct SkillMeta {
    /// Rough token estimate: `SKILL.md` characters / 4.
    pub tokens: usize,
    /// [`skills::load`] output (what the `skill` tool returns / a forced skill adds).
    pub text: Option<String>,
}

impl SkillMeta {
    fn read(skill: &Skill) -> Self {
        let chars = std::fs::read_to_string(skill.dir.join("SKILL.md"))
            .map(|t| t.chars().count())
            .unwrap_or(0);
        Self {
            tokens: chars / 4,
            text: skills::load(skill).ok(),
        }
    }
}

/// `3.2k` — tokens in thousands with one decimal (the picker / panel format).
pub(crate) fn fmt_k(tokens: usize) -> String {
    format!("{:.1}k", tokens as f64 / 1000.)
}

/// The `skill` tool: the skill's instructions, or an error for an unknown name.
pub(crate) fn run_skill(skills: &[Skill], input: &Value) -> ToolResult {
    let input = input_object(input);
    let Some(name) = input.get("name").and_then(Value::as_str) else {
        return result("skill: missing string field `name`", true);
    };
    match skills.iter().find(|s| s.name == name) {
        Some(s) => match skills::load(s) {
            Ok(text) => result(text, false),
            Err(e) => result(format!("skill `{name}`: {e:#}"), true),
        },
        None => {
            let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
            result(
                format!(
                    "Unknown skill `{name}`. Available skills: {}.",
                    names.join(", ")
                ),
                true,
            )
        }
    }
}

/// Display-only split of an MCP tool id (for rows restored from disk whose server is not
/// running); dispatch always goes through the lookup table.
fn guess_target(id: &str) -> Option<McpTarget> {
    let rest = id.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then(|| (server.to_string(), tool.to_string()))
}

/// Outcome of a `tools/call` on the worker thread.
enum CallOutcome {
    Done(mcp::CallResult),
    /// Transport broken: the server is dead.
    Dead {
        error: String,
        stderr: String,
    },
}

impl ChatScreen {
    // ── Skills ──────────────────────────────────────────────────────────

    pub(crate) fn discover_skills(&mut self, cx: &mut Context<Self>) {
        let root = self.workspace.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let (found, meta) = cx
                .background_spawn(async move {
                    let found = skills::discover(&root);
                    let meta = found
                        .iter()
                        .map(|s| (s.name.clone(), SkillMeta::read(s)))
                        .collect::<BTreeMap<_, _>>();
                    (found, meta)
                })
                .await;
            this.update(cx, |this, cx| {
                this.skills = Arc::new(found);
                this.skill_meta = Arc::new(meta);
                cx.notify();
            })
            .ok();
        });
        self.skills_task = Some(task);
    }

    pub(crate) fn discover_agents(&mut self, cx: &mut Context<Self>) {
        let root = self.workspace.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { agent_tools::agents::discover(&root) })
                .await;
            this.update(cx, |this, cx| {
                this.agents = Arc::new(found);
                this.agents_loaded = true;
                this.validate_agent(cx);
                cx.notify();
            })
            .ok();
        });
        self.agents_task = Some(task);
    }

    pub(crate) fn skills_notice(&self) -> String {
        if self.skills.is_empty() {
            return "No skills found. Skills are folders with a SKILL.md in ~/.claude/skills, \
                    ~/.config/harness/skills or <workspace>/.claude/skills."
                .into();
        }
        let mut s = format!(
            "Skills ({}) — the agent loads one with the `skill` tool when a task matches:",
            self.skills.len()
        );
        for sk in self.skills.iter() {
            let source = match sk.source {
                skills::SkillSource::User => "user",
                skills::SkillSource::Harness => "harness",
                skills::SkillSource::Project => "project",
            };
            let desc: String = sk
                .description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let source = if self.forced.contains(&sk.name) {
                format!("{source}, forced")
            } else {
                source.to_string()
            };
            s.push_str(&format!(
                "\n• {} ({source}) — {}",
                sk.name,
                truncate(&desc, 110)
            ));
        }
        s
    }

    // ── MCP startup ─────────────────────────────────────────────────────

    pub(crate) fn load_mcp(&mut self, cx: &mut Context<Self>) {
        let root = self.workspace.root.clone();
        let rx = on_thread("mcp-config".into(), move || {
            mcp::load_server_configs(&llm::Config::path(), &root, &mcp::claude_json_path())
                .map_err(|e| format!("{e:#}"))
        });
        let task = cx.spawn(async move |this, cx| {
            let res = recv(rx, "MCP config").await.and_then(|r| r);
            this.update(cx, |this, cx| this.on_mcp_configs(res, cx))
                .ok();
        });
        self.mcp.load_task = Some(task);
    }

    fn on_mcp_configs(
        &mut self,
        res: Result<(Vec<mcp::ServerConfig>, Vec<String>), String>,
        cx: &mut Context<Self>,
    ) {
        let (configs, warnings) = match res {
            Ok(r) => r,
            Err(e) => {
                self.mcp.phase = McpPhase::Failed(e);
                cx.notify();
                return;
            }
        };
        self.mcp.phase = McpPhase::Ready;
        self.mcp.warnings = warnings;
        // Reloads (another workdir) keep servers whose config is unchanged running — the
        // user / global ones — and only (re)start or stop what differs (`.mcp.json`).
        let (servers, names, removed) = merge_servers(std::mem::take(&mut self.mcp.servers), configs);
        self.mcp.servers = servers;
        for mut s in removed {
            self.mcp.start_tasks.remove(&s.config.name);
            release(s.client.take().into_iter().collect());
        }
        self.mcp.rebuild_index();
        for name in names {
            self.start_server(name, cx);
        }
        if !self.mcp.servers.is_empty() && self.mcp.liveness_task.is_none() {
            self.start_liveness(cx);
        }
        cx.notify();
    }

    /// (Re)start one server: spawn, initialize, list its tools.
    pub(crate) fn start_server(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(s) = self.mcp.server_mut(&name) else {
            return;
        };
        s.generation += 1;
        let generation = s.generation;
        s.status = ServerStatus::Starting;
        release(s.client.take().into_iter().collect());
        s.tools.clear();
        s.open = false;
        let config = s.config.clone();
        self.mcp.rebuild_index();
        let rx = on_thread(format!("mcp-{name}"), move || start_blocking(&config));
        let key = name.clone();
        let task = cx.spawn(async move |this, cx| {
            let res = match recv(rx, "MCP start").await {
                Ok(r) => r,
                Err(e) => Err((e, String::new())),
            };
            this.update(cx, |this, cx| {
                this.on_server_started(name, generation, res, cx)
            })
            .ok();
        });
        self.mcp.start_tasks.insert(key, task);
        cx.notify();
    }

    fn on_server_started(
        &mut self,
        name: String,
        generation: u64,
        res: Started,
        cx: &mut Context<Self>,
    ) {
        self.mcp.start_tasks.remove(&name);
        let Some(s) = self
            .mcp
            .server_mut(&name)
            .filter(|s| s.generation == generation)
        else {
            // Superseded by a retry (or the server vanished): stop the stale process.
            if let Ok((client, _)) = res {
                release(vec![client]);
            }
            return;
        };
        match res {
            Ok((client, tools)) => {
                s.client = Some(client);
                s.tools = tools;
                s.status = ServerStatus::Ready;
            }
            Err((error, stderr)) => {
                s.status = ServerStatus::Failed { error, stderr };
            }
        }
        self.mcp.rebuild_index();
        cx.notify();
    }

    fn mark_server_dead(
        &mut self,
        name: &str,
        error: String,
        stderr: String,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.mcp.server_mut(name) else {
            return;
        };
        if s.status != ServerStatus::Ready {
            return;
        }
        s.status = ServerStatus::Failed { error, stderr };
        release(s.client.take().into_iter().collect());
        s.tools.clear();
        self.mcp.rebuild_index();
        cx.notify();
    }

    /// Every few seconds: running servers whose process exited are marked failed.
    fn start_liveness(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LIVENESS_INTERVAL).await;
                let Ok(clients) = this.update(cx, |this, _| {
                    this.mcp
                        .servers
                        .iter()
                        .filter_map(|s| {
                            Some((s.config.name.clone(), s.generation, s.client.clone()?))
                        })
                        .collect::<Vec<_>>()
                }) else {
                    return;
                };
                if clients.is_empty() {
                    continue;
                }
                let dead = cx
                    .background_spawn(async move {
                        clients
                            .into_iter()
                            .filter_map(|(name, generation, c)| {
                                let alive =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        c.is_alive()
                                    }))
                                    .unwrap_or(true);
                                (!alive).then(|| (name, generation, c.stderr_tail()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                if dead.is_empty() {
                    continue;
                }
                let gone = this.update(cx, |this, cx| {
                    for (name, generation, stderr) in dead {
                        if this
                            .mcp
                            .server(&name)
                            .is_some_and(|s| s.generation == generation)
                        {
                            this.mark_server_dead(
                                &name,
                                "the server process exited".into(),
                                stderr,
                                cx,
                            );
                        }
                    }
                });
                if gone.is_err() {
                    return;
                }
            }
        });
        self.mcp.liveness_task = Some(task);
    }

    /// Stop every server process (app quit), each on its own thread. The returned future
    /// resolves when all have stopped; the app waits for it only briefly, but closing stdin
    /// happens first, so servers exit on EOF even if the app is gone before the kill.
    pub(crate) fn shutdown_mcp(&mut self) -> impl std::future::Future<Output = ()> + use<> {
        let clients = self.mcp.take_clients();
        self.mcp.index.clear();
        let n = clients.len();
        let (tx, rx) = smol::channel::bounded::<()>(n.max(1));
        for c in clients {
            let tx = tx.clone();
            let spawned = std::thread::Builder::new()
                .name("mcp-shutdown".into())
                .spawn(move || {
                    c.shutdown();
                    drop(c);
                    let _ = tx.send_blocking(());
                });
            if let Err(e) = spawned {
                eprintln!("harness: could not spawn a thread to stop an MCP server: {e}");
            }
        }
        drop(tx);
        async move {
            for _ in 0..n {
                if rx.recv().await.is_err() {
                    break;
                }
            }
        }
    }

    // ── MCP calls ───────────────────────────────────────────────────────

    /// `(server, tool)` for a row label: the live table, else a best-effort split of the
    /// id (sessions restored while the server is not running).
    pub(crate) fn mcp_label(&self, tool_id: &str) -> Option<McpTarget> {
        self.mcp
            .index
            .get(tool_id)
            .cloned()
            .or_else(|| guess_target(tool_id))
    }

    /// Run `tools/call` for `target` on a worker thread; `on_done` gets the tool result.
    pub(crate) fn call_mcp(
        &mut self,
        target: McpTarget,
        input: &Value,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        let (server, tool) = target;
        let client = match self.mcp.server(&server) {
            Some(McpServer {
                status: ServerStatus::Ready,
                client: Some(c),
                ..
            }) => c.clone(),
            Some(McpServer {
                status: ServerStatus::Failed { error, .. },
                ..
            }) => {
                let msg = format!(
                    "MCP server `{server}` is not running (failed: {}).",
                    first_line(error)
                );
                return Task::ready(result(msg, true));
            }
            _ => {
                return Task::ready(result(
                    format!("MCP server `{server}` is not running."),
                    true,
                ));
            }
        };
        let arguments = input_object(input);
        let tool_name = tool.clone();
        let rx = on_thread(format!("mcp-call-{server}"), move || {
            match client.call_tool(&tool_name, arguments, cancel, CALL_TIMEOUT) {
                Ok(r) => CallOutcome::Done(r),
                Err(e) => CallOutcome::Dead {
                    error: format!("{e:#}"),
                    stderr: client.stderr_tail(),
                },
            }
        });
        let generation = self.mcp.server(&server).map_or(0, |s| s.generation);
        cx.spawn(async move |this, cx| match recv(rx, "MCP call").await {
            Ok(CallOutcome::Done(r)) => {
                let text = if r.text.trim().is_empty() {
                    "(no output)".to_string()
                } else {
                    r.text
                };
                result(cap_result(text), r.is_error)
            }
            Ok(CallOutcome::Dead { error, stderr }) => {
                let msg = format!("MCP server `{server}` stopped responding: {error}");
                this.update(cx, |this, cx| {
                    if this
                        .mcp
                        .server(&server)
                        .is_some_and(|s| s.generation == generation)
                    {
                        this.mark_server_dead(&server, error, stderr, cx);
                    }
                })
                .ok();
                result(msg, true)
            }
            Err(e) => result(format!("{tool}: {e}"), true),
        })
    }

    // ── /mcp ────────────────────────────────────────────────────────────

    pub(crate) fn mcp_notice(&self) -> String {
        let where_ = format!(
            "Configure servers with [[mcp_servers]] in {}, a .mcp.json in the workspace, or \
             import_claude_code = true to use {}.",
            llm::Config::path().display(),
            mcp::claude_json_path().display()
        );
        let mut s = match &self.mcp.phase {
            McpPhase::Loading => return "MCP servers are still loading…".into(),
            McpPhase::Failed(e) => format!("Could not load the MCP server config: {e}\n{where_}"),
            McpPhase::Ready if self.mcp.servers.is_empty() => {
                format!("No MCP servers configured. {where_}")
            }
            McpPhase::Ready => {
                let mut s = format!("MCP servers ({}):", self.mcp.servers.len());
                for srv in &self.mcp.servers {
                    let source = match srv.config.source {
                        mcp::ServerSource::HarnessConfig => "harness config",
                        mcp::ServerSource::ProjectMcpJson => ".mcp.json",
                        mcp::ServerSource::ClaudeCode => "~/.claude.json",
                    };
                    let state = match &srv.status {
                        ServerStatus::Starting => "starting…".to_string(),
                        ServerStatus::Ready => {
                            let n = srv.tools.len();
                            let names: Vec<&str> =
                                srv.tools.iter().take(12).map(|t| t.name.as_str()).collect();
                            let more = if n > 12 {
                                format!(", +{}", n - 12)
                            } else {
                                String::new()
                            };
                            format!(
                                "{n} tool{}{}",
                                if n == 1 { "" } else { "s" },
                                if n == 0 {
                                    String::new()
                                } else {
                                    format!(": {}{more}", names.join(", "))
                                }
                            )
                        }
                        ServerStatus::Failed { error, .. } => {
                            format!("failed: {}", truncate(first_line(error), 160))
                        }
                        ServerStatus::Disabled => "disabled".into(),
                    };
                    s.push_str(&format!("\n• {} ({source}) — {state}", srv.config.name));
                }
                s
            }
        };
        if !self.mcp.warnings.is_empty() {
            s.push_str("\nWarnings:");
            for w in &self.mcp.warnings {
                s.push_str(&format!("\n• {w}"));
            }
        }
        s
    }
}

/// New server list from `configs`: an existing server with an identical config is kept
/// as it is; others are fresh (a same-named one keeps its generation so a late start
/// result of the old process is dropped). Returns the list, the names to start, and the
/// servers no longer configured (to stop).
fn merge_servers(
    mut old: Vec<McpServer>,
    configs: Vec<mcp::ServerConfig>,
) -> (Vec<McpServer>, Vec<String>, Vec<McpServer>) {
    let mut servers = Vec::with_capacity(configs.len());
    let mut start = Vec::new();
    let mut replaced = Vec::new();
    for config in configs {
        if let Some(pos) = old.iter().position(|s| s.config == config) {
            servers.push(old.remove(pos));
            continue;
        }
        let generation = match old.iter().position(|s| s.config.name == config.name) {
            Some(pos) => {
                let prev = old.remove(pos);
                let g = prev.generation;
                replaced.push(prev);
                g
            }
            None => 0,
        };
        if config.enabled {
            start.push(config.name.clone());
        }
        servers.push(McpServer {
            status: if config.enabled {
                ServerStatus::Starting
            } else {
                ServerStatus::Disabled
            },
            config,
            client: None,
            tools: Vec::new(),
            open: false,
            generation,
        });
    }
    old.extend(replaced);
    (servers, start, old)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, dir: &std::path::Path) -> Skill {
        Skill {
            name: name.into(),
            description: "d".into(),
            dir: dir.to_path_buf(),
            source: skills::SkillSource::User,
        }
    }

    #[test]
    fn skill_tool_loads_or_errors() {
        let dir = std::env::temp_dir().join(format!("screen_chat_skill_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(
            dir.join("a/SKILL.md"),
            "---\nname: a\ndescription: d\n---\nDo A.\n",
        )
        .unwrap();
        let skills = vec![skill("a", &dir.join("a"))];
        let ok = run_skill(&skills, &serde_json::json!({ "name": "a" }));
        assert!(!ok.is_error);
        assert!(ok.content.contains("Do A."));
        let as_string = run_skill(&skills, &Value::String("{\"name\":\"a\"}".into()));
        assert!(!as_string.is_error);
        let unknown = run_skill(&skills, &serde_json::json!({ "name": "b" }));
        assert!(unknown.is_error);
        assert!(unknown.content.contains("Unknown skill `b`"));
        assert!(run_skill(&skills, &Value::Null).is_error);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Manual: `cargo build -p mcp --example echo_server`, then
    /// `cargo test -p screen_chat -- --ignored echo_server`.
    #[test]
    #[ignore]
    fn echo_server_start_and_call() {
        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/examples/echo_server");
        let config = mcp::ServerConfig {
            name: "echo".into(),
            command: bin.display().to_string(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            auto_approve: Vec::new(),
            source: mcp::ServerSource::HarnessConfig,
        };
        let (client, tools) = start_blocking(&config).map_err(|(e, _)| e).unwrap();
        assert!(tools.iter().any(|t| t.name == "echo"));
        let cancel = Arc::new(AtomicBool::new(false));
        let r = client
            .call_tool(
                "echo",
                input_object(&Value::String("{\"text\":\"hi\"}".into())),
                cancel,
                CALL_TIMEOUT,
            )
            .unwrap();
        assert_eq!((r.text.as_str(), r.is_error), ("hi", false));
        release(vec![client]);
    }

    #[test]
    fn merge_keeps_unchanged_servers() {
        let cfg = |name: &str, cmd: &str| mcp::ServerConfig {
            name: name.into(),
            command: cmd.into(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            auto_approve: Vec::new(),
            source: mcp::ServerSource::HarnessConfig,
        };
        let server = |c: mcp::ServerConfig| McpServer {
            config: c,
            status: ServerStatus::Ready,
            client: None,
            tools: Vec::new(),
            open: false,
            generation: 3,
        };
        let old = vec![
            server(cfg("global", "g")),
            server(cfg("proj", "p1")),
            server(cfg("gone", "x")),
        ];
        let (servers, start, removed) = merge_servers(
            old,
            vec![cfg("global", "g"), cfg("proj", "p2"), cfg("new", "n")],
        );
        assert_eq!(start, ["proj", "new"]);
        assert_eq!(servers[0].status, ServerStatus::Ready);
        assert_eq!(servers[1].status, ServerStatus::Starting);
        assert_eq!(servers[1].generation, 3);
        assert_eq!(servers[2].generation, 0);
        let gone: Vec<&str> = removed.iter().map(|s| s.config.name.as_str()).collect();
        assert_eq!(gone, ["gone", "proj"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(
            guess_target("mcp__gh__search_issues"),
            Some(("gh".into(), "search_issues".into()))
        );
        assert_eq!(guess_target("read_file"), None);
        assert_eq!(input_object(&Value::Null), serde_json::json!({}));
        let big = "é".repeat(MAX_RESULT_BYTES);
        let capped = cap_result(big);
        assert!(capped.len() < MAX_RESULT_BYTES + 100);
        assert!(capped.contains("output truncated"));
    }

    #[test]
    fn index_and_specs_skip_collisions() {
        let cfg = |name: &str| mcp::ServerConfig {
            name: name.into(),
            command: "x".into(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            auto_approve: vec!["safe".into()],
            source: mcp::ServerSource::HarnessConfig,
        };
        let tool = |name: &str| mcp::McpTool {
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({ "type": "object" }),
        };
        let mut st = McpState::new();
        st.servers = vec![
            McpServer {
                config: cfg("a.b"),
                status: ServerStatus::Ready,
                client: None,
                tools: vec![tool("safe"), tool("x")],
                open: false,
                generation: 1,
            },
            McpServer {
                // `a_b` sanitises to the same ids as `a.b`.
                config: cfg("a_b"),
                status: ServerStatus::Ready,
                client: None,
                tools: vec![tool("x"), tool("y")],
                open: false,
                generation: 1,
            },
            McpServer {
                config: cfg("down"),
                status: ServerStatus::Failed {
                    error: "e".into(),
                    stderr: String::new(),
                },
                client: None,
                tools: vec![tool("z")],
                open: false,
                generation: 1,
            },
        ];
        st.rebuild_index();
        assert_eq!(
            st.index.get("mcp__a_b__x"),
            Some(&("a.b".into(), "x".into()))
        );
        assert_eq!(
            st.index.get("mcp__a_b__y"),
            Some(&("a_b".into(), "y".into()))
        );
        assert!(!st.index.contains_key("mcp__down__z"));
        let specs = st.tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["mcp__a_b__safe", "mcp__a_b__x", "mcp__a_b__y"]);
        assert!(st.auto_approves(&("a.b".into(), "safe".into())));
        assert!(!st.auto_approves(&("a.b".into(), "x".into())));
    }
}
