//! The tools one session offers its model: built-ins and extras after grants and mode,
//! session tools, and MCP tools, eager or loaded on demand.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use mcp::McpTool;
use model::{ChatMode, Skill, StoredMessage, ToolAccess, ToolClass, ToolDisplay, ToolSpec};
use serde_json::Value;
use tools::{Tool, ToolContext, ToolRegistry};

use crate::history::successful_calls;
use crate::session_tools::names::{SKILL, TOOL_SEARCH};
use crate::session_tools::spawn::pattern_matches;
use crate::session_tools::{SessionTool, ask, finish, pretty, report, search, skill, spawn, text_display, todo};
use crate::spec::{AgentDeps, AgentEnv, AgentSpec, Finish};

/// What a tool name stands for in this session.
pub(crate) enum Found<'t> {
    /// A built-in or an extra tool: runs in the workdir, behind the gate.
    Local(&'t Arc<dyn Tool>),
    Mcp(&'t McpTool),
    Session(SessionTool),
}

pub(crate) struct Toolbox {
    local: ToolRegistry,
    session: BTreeMap<String, (SessionTool, ToolSpec)>,
    /// Every MCP tool the grants allow.
    mcp: Vec<McpTool>,
    /// Too many MCP tools to send every schema: only loaded ones are offered.
    deferred: bool,
    /// Ids of deferred tools loaded with `tool_search`, in the order they were loaded.
    loaded: Vec<String>,
}

impl Toolbox {
    /// The tools of a run. `offered_skills` are the skills the `skill` tool may load.
    pub fn build(spec: &AgentSpec, env: &AgentEnv, deps: &AgentDeps<'_>, offered_skills: &[Skill]) -> Self {
        let denied = |name: &str| spec.tools.deny.iter().any(|pattern| pattern_matches(pattern, name));
        let restricted = env.read_only || env.mode == ChatMode::Plan;

        let mut local = ToolRegistry::new();
        if env.workdir.is_some() {
            local = tools::builtin(spec.tools.access);
            // An extra tool is held to the access level like a built-in of its class.
            for tool in deps.tools.names().filter_map(|name| deps.tools.get(name)) {
                if access_covers(spec.tools.access, tool.class()) {
                    local.register(tool.clone());
                }
            }
            let remove: Vec<String> = local
                .names()
                .filter(|name| {
                    let changes_things = local
                        .get(name)
                        .is_some_and(|tool| matches!(tool.class(), ToolClass::Write | ToolClass::Execute));
                    denied(name) || (restricted && changes_things)
                })
                .map(str::to_string)
                .collect();
            for name in remove {
                local.remove(&name);
            }
        }

        let mcp: Vec<McpTool> = deps
            .mcp
            .map(|mcp| mcp.tools())
            .unwrap_or_default()
            .into_iter()
            .filter(|tool| spec.tools.allow.iter().any(|pattern| pattern_matches(pattern, &tool.id)))
            .filter(|tool| !denied(&tool.id))
            .collect();
        let deferred = mcp.len() > search::EAGER_LIMIT;

        let mut session: Vec<(SessionTool, ToolSpec)> = vec![(SessionTool::Todo, todo::spec())];
        if !offered_skills.is_empty() {
            session.push((SessionTool::Skill, skill::spec(offered_skills)));
        }
        if spec.spawn.enabled {
            session.push((SessionTool::Spawn, spawn::spec(&spec.templates)));
        }
        if deferred {
            session.push((SessionTool::ToolSearch, search::spec()));
        }
        if env.depth == 0 {
            session.push((SessionTool::AskUser, ask::spec()));
            if spec.finish == Finish::Reply {
                session.push((SessionTool::Report, report::spec()));
            }
        }
        if let Finish::Tool(tool) = &spec.finish {
            session.push((SessionTool::Finish, finish::spec(tool)));
        }
        let session = session
            .into_iter()
            // The finish tool is how the run ends: grants cannot take it away.
            .filter(|(kind, spec)| *kind == SessionTool::Finish || !denied(&spec.name))
            .map(|(kind, spec)| (spec.name.clone(), (kind, spec)))
            .collect();

        Self { local, session, mcp, deferred, loaded: Vec::new() }
    }

    /// Loads again what earlier `tool_search` calls of the stored conversation loaded.
    pub fn restore(&mut self, stored: &[StoredMessage]) {
        if !self.deferred {
            return;
        }
        for input in successful_calls(stored, TOOL_SEARCH) {
            if let Ok((query, max)) = search::parse(input) {
                self.load(&query, max);
            }
        }
    }

    /// Runs a `tool_search` query and offers what it finds from now on. Returns how many
    /// tools it found and the text the model reads.
    pub fn load(&mut self, query: &str, max_results: usize) -> (usize, String) {
        let found = search::search(&self.mcp, query, max_results);
        for tool in &found {
            if !self.loaded.contains(&tool.id) {
                self.loaded.push(tool.id.clone());
            }
        }
        (found.len(), search::render(&found))
    }

    /// The MCP tools whose names go in the system prompt instead of their schemas.
    pub fn deferred_tools(&self) -> &[McpTool] {
        if self.deferred { &self.mcp } else { &[] }
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    pub fn has_mcp(&self) -> bool {
        !self.mcp.is_empty()
    }

    /// The tool a call by this name reaches; `None` when it is not offered right now.
    pub fn find(&self, name: &str) -> Option<Found<'_>> {
        if let Some((kind, _)) = self.session.get(name) {
            return Some(Found::Session(*kind));
        }
        if let Some(tool) = self.local.get(name) {
            return Some(Found::Local(tool));
        }
        self.mcp
            .iter()
            .find(|tool| tool.id == name && (!self.deferred || self.loaded.iter().any(|id| id == name)))
            .map(Found::Mcp)
    }

    /// The specs sent to the model: local and session tools by name, then MCP tools in server
    /// order (or, deferred, in the order they were loaded, so loading one keeps the prefix).
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut out: Vec<ToolSpec> =
            self.local.specs().into_iter().filter(|spec| !self.session.contains_key(&spec.name)).collect();
        out.extend(self.session.values().map(|(_, spec)| spec.clone()));
        out.sort_by(|a, b| a.name.cmp(&b.name));
        if self.deferred {
            out.extend(self.loaded.iter().filter_map(|id| self.mcp.iter().find(|t| &t.id == id)).map(|t| t.spec.clone()));
        } else {
            out.extend(self.mcp.iter().map(|tool| tool.spec.clone()));
        }
        out
    }

    /// Names of every tool offered right now.
    pub fn names(&self) -> Vec<String> {
        self.specs().into_iter().map(|spec| spec.name).collect()
    }

    /// The class and the preview of a call, for the transcript and for a permission request.
    /// Never panics: a tool whose preview does becomes plain text.
    pub fn describe(&self, name: &str, input: &Value, ctx: Option<&ToolContext<'_>>) -> (ToolClass, ToolDisplay) {
        match self.find(name) {
            Some(Found::Local(tool)) => {
                let preview = ctx.and_then(|ctx| catch_unwind(AssertUnwindSafe(|| tool.preview(ctx, input))).ok());
                (tool.class(), preview.unwrap_or_else(|| text_display(name, &pretty(input))))
            }
            Some(Found::Mcp(tool)) => (ToolClass::External, mcp_display(tool, &pretty(input))),
            Some(Found::Session(kind)) => (ToolClass::Internal, session_display(kind, name, input)),
            None => (ToolClass::Internal, text_display(name, &pretty(input))),
        }
    }
}

/// Whether an access level offers local tools of a class: reading always, running commands
/// from `ReadAndRun` on, writing and reaching outside only with `Full`. (MCP tools are not
/// local: each needs a grant by name.)
fn access_covers(access: ToolAccess, class: ToolClass) -> bool {
    match class {
        ToolClass::Read | ToolClass::Internal => true,
        ToolClass::Execute => matches!(access, ToolAccess::ReadAndRun | ToolAccess::Full),
        ToolClass::Write | ToolClass::External => access == ToolAccess::Full,
    }
}

/// How an MCP call is shown: its server and tool, then the arguments or the result.
pub(crate) fn mcp_display(tool: &McpTool, body: &str) -> ToolDisplay {
    text_display(&format!("{} · {}", tool.server, tool.name), body)
}

fn session_display(kind: SessionTool, name: &str, input: &Value) -> ToolDisplay {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    match kind {
        SessionTool::Todo => match todo::parse(input) {
            Ok(items) => text_display("Todo list", &todo::render(&items)),
            Err(_) => text_display("Todo list", &pretty(input)),
        },
        SessionTool::Skill => ToolDisplay::Line { verb: SKILL.into(), target: field("name"), result: String::new() },
        SessionTool::ToolSearch => {
            ToolDisplay::Line { verb: TOOL_SEARCH.into(), target: field("query"), result: String::new() }
        }
        SessionTool::Spawn => {
            let who = [field("label"), field("agent")].into_iter().find(|s| !s.is_empty());
            text_display(&format!("Sub-agent · {}", who.unwrap_or_else(|| spawn::GENERAL.to_string())), &field("task"))
        }
        SessionTool::Report => text_display(&field("title"), &field("summary")),
        SessionTool::AskUser => text_display("Question", &field("question")),
        SessionTool::Finish => text_display(name, &pretty(input)),
    }
}
