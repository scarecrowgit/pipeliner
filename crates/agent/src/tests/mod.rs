//! Tests of the loop against a scripted model, a memory host and temp workdirs. No network,
//! no real model, nothing outside the temp directory.

mod basic;
mod cancel;
mod context;
mod permission;
mod session;
mod subagents;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use base::cancel::CancelToken;
use base::paths::Paths;
use base::time::FixedClock;
use catalog::Catalog;
use llm::{BlobSource, ChatRequest, LlmClient, LlmError, ProviderInfo, ScriptedLlm, StreamEvent};
use mcp::{CallResult, McpTool};
use model::{
    AgentEvent, ChatMode, ContentBlock, Message, ModelRef, PermissionDecision, PermissionRequest, Role, RunnableRef,
    SessionId, ToolAccess, ToolClass, ToolDisplay, ToolGrants, ToolSpec,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tools::{Tool, ToolContext, ToolOutput, ToolRegistry};

use crate::sink::lock;
use crate::{
    AgentDeps, AgentEnv, AgentOutcome, AgentSink, AgentSpec, AllowAll, EventLog, Finish, Input, Limits, Mcp,
    MemoryHost, MemoryRules, PermissionGate, RuleGate, UserInput, run,
};

/// 23 November 2026, noon UTC.
const NOW: i64 = 1_795_435_200_000;

/// Everything a run needs, with a `Build`-like agent on a scripted model in a temp workdir.
pub(crate) struct Fixture {
    pub dir: TempDir,
    pub host: MemoryHost,
    pub llm: ScriptedLlm,
    pub clock: FixedClock,
    pub catalog: Catalog,
    pub spec: AgentSpec,
    pub env: AgentEnv,
    pub cancel: CancelToken,
    pub extra: ToolRegistry,
    pub mcp: Option<FakeMcp>,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_skills(&[])
    }

    /// A fixture with installed skills, each `(name, description, body)`.
    pub fn with_skills(skills: &[(&str, &str, &str)]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).expect("workdir");
        let paths = Paths::under(&dir.path().join("paths"));
        for (name, description, body) in skills {
            let folder = paths.skills_dir().join(name);
            std::fs::create_dir_all(&folder).expect("skill dir");
            let text = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
            std::fs::write(folder.join("SKILL.md"), text).expect("skill file");
        }
        let catalog = Catalog::load(&paths, None);
        assert_eq!(catalog.skills().len(), skills.len(), "{:?}", catalog.problems());
        Self {
            host: MemoryHost::new(),
            llm: ScriptedLlm::new(),
            clock: FixedClock::new(NOW),
            catalog,
            spec: AgentSpec {
                name: "Build".into(),
                runnable: RunnableRef::agent("build"),
                role: "You are Build, a coding agent.".into(),
                model: ModelRef::new("scripted", "scripted-model"),
                reasoning: Default::default(),
                tools: ToolGrants { access: ToolAccess::Full, ..Default::default() },
                skills: Default::default(),
                spawn: Default::default(),
                templates: Vec::new(),
                budget_usd: None,
                max_steps: 0,
                finish: Finish::Reply,
                limits: Limits {
                    retry_base: std::time::Duration::from_millis(1),
                    retry_max: std::time::Duration::from_millis(4),
                    ..Default::default()
                },
            },
            env: AgentEnv {
                session: SessionId::new("s1"),
                workdir: Some(work),
                mode: ChatMode::Build,
                ..Default::default()
            },
            cancel: CancelToken::new(),
            extra: ToolRegistry::new(),
            mcp: None,
            dir,
        }
    }

    pub fn session(&self) -> &SessionId {
        &self.env.session
    }

    pub fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    /// Writes a file into the workdir, creating its folders.
    pub fn write(&self, rel: &str, text: &str) {
        let path = self.work().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.work().join(rel)).expect("read")
    }

    pub fn deps<'a>(&'a self, llm: &'a dyn LlmClient, gate: &'a dyn PermissionGate) -> AgentDeps<'a> {
        AgentDeps {
            llm,
            host: &self.host,
            gate,
            mcp: self.mcp.as_ref().map(|mcp| mcp as &dyn Mcp),
            skills: &self.catalog,
            clock: &self.clock,
            tools: self.extra.clone(),
        }
    }

    /// Runs with every permission granted and returns the outcome and every event.
    pub fn run(&self, input: Input) -> (AgentOutcome, EventLog) {
        self.run_gated(&AllowAll, input)
    }

    pub fn run_gated(&self, gate: &dyn PermissionGate, input: Input) -> (AgentOutcome, EventLog) {
        let mut log = EventLog::new();
        let outcome = self.run_into(gate, input, &mut log);
        (outcome, log)
    }

    pub fn run_into(&self, gate: &dyn PermissionGate, input: Input, sink: &mut dyn AgentSink) -> AgentOutcome {
        run(&self.spec, &self.env, &self.deps(&self.llm, gate), input, sink, &self.cancel)
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.llm.requests()
    }

    /// The stored conversation as the model would be sent it.
    pub fn transcript(&self) -> Vec<Message> {
        self.host.transcript(self.session())
    }

    /// What a tool answers to `input` in a workdir holding the same files, without touching
    /// this one.
    pub fn twin_output(&self, tool: &dyn Tool, files: &[(&str, &str)], input: Value) -> ToolOutput {
        let twin = TempDir::new().expect("temp dir");
        for (rel, text) in files {
            let path = twin.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        let (env, cancel) = (BTreeMap::new(), CancelToken::new());
        tool.run(&ToolContext { root: twin.path(), env: &env, cancel: &cancel }, &input)
    }
}

pub(crate) fn say(text: &str) -> Input {
    Input::User(UserInput::text(text))
}

pub(crate) fn text(text: &str) -> ContentBlock {
    ContentBlock::text(text)
}

pub(crate) fn tool_use(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse { id: id.into(), name: name.into(), input }
}

pub(crate) fn result(id: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolResult { tool_use_id: id.into(), content: content.into(), is_error: false }
}

pub(crate) fn error(id: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolResult { tool_use_id: id.into(), content: content.into(), is_error: true }
}

pub(crate) fn user(content: Vec<ContentBlock>) -> Message {
    Message { role: Role::User, content }
}

pub(crate) fn assistant(content: Vec<ContentBlock>) -> Message {
    Message { role: Role::Assistant, content }
}

/// The content and error flag of the result that answers `id` in a conversation.
pub(crate) fn result_of(messages: &[Message], id: &str) -> (String, bool) {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, content, is_error } if tool_use_id == id => {
                Some((content.clone(), *is_error))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

/// Checks the invariant the loop exists to keep: every tool call is answered exactly once, by
/// the message right after it, and nothing answers a call that was not made.
pub(crate) fn assert_paired(messages: &[Message]) {
    for (index, message) in messages.iter().enumerate() {
        let calls: Vec<&str> = message.tool_uses().map(|(id, _, _)| id).collect();
        let next: Vec<&str> = messages
            .get(index + 1)
            .into_iter()
            .flat_map(|m| &m.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        if message.role == Role::Assistant {
            assert_eq!(calls, next, "calls of message {index} and the results after it");
        } else if index == 0 {
            let own = message.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }));
            assert!(!own, "the first message answers a call nobody made");
        }
        if message.role == Role::User && messages.get(index + 1).is_some_and(|m| m.role == Role::User) {
            assert!(next.is_empty(), "results in message {} do not follow a call", index + 1);
        }
    }
}

pub(crate) fn tool_names(request: &ChatRequest) -> Vec<&str> {
    request.tools.iter().map(|t| t.name.as_str()).collect()
}

/// The names of a session's events, for asserting on their order.
pub(crate) fn kinds(log: &EventLog, session: &SessionId) -> Vec<&'static str> {
    log.of(session)
        .into_iter()
        .map(|event| match event {
            AgentEvent::TurnStarted => "turn_started",
            AgentEvent::TextDelta { .. } => "text",
            AgentEvent::ThinkingDelta { .. } => "thinking",
            AgentEvent::MessageCommitted { .. } => "message",
            AgentEvent::ToolCallStarted { .. } => "tool_started",
            AgentEvent::ToolCallReady { .. } => "tool_ready",
            AgentEvent::ToolFinished { .. } => "tool_finished",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::TodoUpdated { .. } => "todo",
            AgentEvent::FilesChanged { .. } => "files",
            AgentEvent::SkillLoaded { .. } => "skill",
            AgentEvent::SubagentStarted { .. } => "subagent_started",
            AgentEvent::SubagentFinished { .. } => "subagent_finished",
            AgentEvent::CompactionStarted => "compaction_started",
            AgentEvent::Compacted { .. } => "compacted",
            AgentEvent::Retrying { .. } => "retrying",
            AgentEvent::PrDetected { .. } => "pr",
            AgentEvent::Notice { .. } => "notice",
            AgentEvent::TurnFinished { .. } => "turn_finished",
            AgentEvent::Failed { .. } => "failed",
        })
        .collect()
}

/// MCP servers that answer `<id> ok` and remember what they were asked.
#[derive(Default)]
pub(crate) struct FakeMcp {
    pub tools: Vec<McpTool>,
    pub auto: Vec<String>,
    pub calls: Mutex<Vec<(String, Value)>>,
}

impl FakeMcp {
    /// `count` tools `tool_0`… on the server `grafana`.
    pub fn with_tools(count: usize) -> Self {
        let tools = (0..count)
            .map(|n| {
                let name = format!("tool_{n}");
                let id = mcp::tool_id("grafana", &name);
                McpTool {
                    id: id.clone(),
                    server: "grafana".into(),
                    name,
                    spec: ToolSpec {
                        name: id,
                        description: format!("Does thing {n}"),
                        input_schema: json!({"type": "object"}),
                    },
                }
            })
            .collect();
        Self { tools, ..Default::default() }
    }
}

impl Mcp for FakeMcp {
    fn tools(&self) -> Vec<McpTool> {
        self.tools.clone()
    }

    fn call(&self, tool_id: &str, arguments: Value, _cancel: &CancelToken) -> CallResult {
        lock(&self.calls).push((tool_id.to_string(), arguments));
        CallResult { text: format!("{tool_id} ok"), is_error: false }
    }

    fn is_auto_approved(&self, tool_id: &str) -> bool {
        self.auto.iter().any(|id| id == tool_id)
    }
}

/// A model client that answers with a closure, for what a script cannot express: answers that
/// depend on the request, or a client that misbehaves.
pub(crate) struct FnLlm<F>(pub F);

impl<F> LlmClient for FnLlm<F>
where
    F: Fn(&ChatRequest, &CancelToken, &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> + Send + Sync,
{
    fn stream(
        &self,
        _model: &ModelRef,
        request: &ChatRequest,
        _blobs: &dyn BlobSource,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), LlmError> {
        (self.0)(request, cancel, on_event)
    }

    fn list_models(&self, provider: &str) -> Result<Vec<String>, LlmError> {
        Err(LlmError::UnknownModel(provider.to_string()))
    }

    fn providers(&self) -> Vec<ProviderInfo> {
        Vec::new()
    }

    fn default_model(&self) -> Option<ModelRef> {
        None
    }
}

/// A read-class tool that runs a closure, for tools that panic, cancel or count.
pub(crate) struct Probe<F> {
    pub name: &'static str,
    pub run: F,
}

impl<F: Fn(&ToolContext<'_>, &Value) -> String + Send + Sync> Tool for Probe<F> {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.name.into(), description: "A test tool".into(), input_schema: json!({"type": "object"}) }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Read
    }

    fn subject(&self, _ctx: &ToolContext<'_>, _input: &Value) -> String {
        String::new()
    }

    fn preview(&self, _ctx: &ToolContext<'_>, _input: &Value) -> ToolDisplay {
        ToolDisplay::Text { title: self.name.into(), body: String::new() }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        let content = (self.run)(ctx, input);
        ToolOutput { display: ToolDisplay::Text { title: self.name.into(), body: content.clone() }, content, is_error: false, files: Vec::new() }
    }
}

/// A rule gate over `rules` whose person answers with `answer` and counts the questions.
pub(crate) struct Asker<'a> {
    pub rules: &'a MemoryRules,
    pub asked: AtomicUsize,
    pub requests: Mutex<Vec<PermissionRequest>>,
}

impl<'a> Asker<'a> {
    pub fn new(rules: &'a MemoryRules) -> Self {
        Self { rules, asked: AtomicUsize::new(0), requests: Mutex::new(Vec::new()) }
    }

    /// The gate; `answer` is what the person says to each request.
    pub fn gate<'g>(
        &'g self,
        answer: impl Fn(&PermissionRequest) -> Option<PermissionDecision> + Send + Sync + 'g,
    ) -> RuleGate<'g> {
        RuleGate::new(self.rules, move |request, _cancel| {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let decision = answer(&request);
            lock(&self.requests).push(request);
            decision
        })
    }

    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}
