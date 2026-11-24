//! Running the tool calls of one assistant message: in order, each behind the permission
//! gate, each answered exactly once, whatever happens to it.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use anyhow::Result;
use mcp::McpTool;
use model::{
    AgentEvent, ContentBlock, FileChange, Message, PermissionDecision, PermissionRequest, RequestId, Role,
    RuleDecision, RuleScope, ToolClass, Usage,
};
use tools::permission::{always_rule, policy_label};
use tools::{Tool, ToolOutput};

use crate::gate::{GateCall, Verdict};
use crate::history::INTERRUPTED;
use crate::session_tools::names::{SKILL, TOOL_SEARCH};
use crate::session_tools::{SessionTool, ask, finish, ok, report, search, skill, todo};
use crate::spec::Finish;
use crate::stream::Call;
use crate::toolbox::{Found, mcp_display};
use crate::turn::Session;

/// What a name in a tool call reaches, detached from the toolbox so the call can change it.
enum Target {
    Local(Arc<dyn Tool>),
    Mcp(McpTool),
    Session(SessionTool),
    Unknown,
}

/// The result of a call that may not run, boxed to keep `Result`s small.
pub(crate) type Refused = Box<ToolOutput>;

/// The result of a call that never ran or was cut off.
pub(crate) fn interrupted_output(name: &str) -> ToolOutput {
    ToolOutput::error(name, INTERRUPTED)
}

/// A panic's message, when it has one.
pub(crate) fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}

impl Session<'_> {
    /// Runs every call and stores one message with a result for each, in the calls' order.
    /// Consecutive `spawn_agent` calls run at the same time. After a cancel the remaining
    /// calls are answered as interrupted without running.
    ///
    /// Nothing that goes wrong with one call keeps the others from being answered: a panic
    /// anywhere under a call becomes its error result, and a failure to book a result is
    /// returned only after the results are stored.
    pub(crate) fn run_calls(&mut self, calls: &[Call]) -> Result<()> {
        let mut results: Vec<ContentBlock> = Vec::with_capacity(calls.len());
        let mut failure: Option<anyhow::Error> = None;
        let mut rest = calls;
        while let Some(first) = rest.first() {
            let spawns = rest.iter().take_while(|call| self.is_spawn(call)).count();
            let (batch, outputs) = if spawns > 0 && !self.cancel.is_cancelled() {
                let batch = &rest[..spawns];
                let outputs = catch_unwind(AssertUnwindSafe(|| self.spawn_batch(batch)))
                    .unwrap_or_else(|panic| batch.iter().map(|call| crashed(&call.name, &*panic)).collect());
                (batch, outputs)
            } else {
                let output = catch_unwind(AssertUnwindSafe(|| self.run_call(first)))
                    .unwrap_or_else(|panic| crashed(&first.name, &*panic));
                (&rest[..1], vec![output])
            };
            let mut outputs = outputs.into_iter();
            for call in batch {
                // One output per call is what both branches give; a missing one still gets an answer.
                let output = outputs.next().unwrap_or_else(|| interrupted_output(&call.name));
                results.push(self.finish_call(call, output, &mut failure));
            }
            rest = &rest[batch.len()..];
        }
        self.commit(Message { role: Role::User, content: results }, Usage::default(), 0.0, None)?;
        failure.map_or(Ok(()), Err)
    }

    fn is_spawn(&self, call: &Call) -> bool {
        call.error.is_none() && matches!(self.toolbox.find(&call.name), Some(Found::Session(SessionTool::Spawn)))
    }

    /// Books a finished call and turns its output into the result the model reads. A failure
    /// to book it goes to `failure` (the first one is kept); the result is returned regardless.
    fn finish_call(&mut self, call: &Call, output: ToolOutput, failure: &mut Option<anyhow::Error>) -> ContentBlock {
        self.totals.tool_calls += 1;
        if !output.files.is_empty()
            && let Err(error) = self.record_files(&output.files)
        {
            failure.get_or_insert(error);
        }
        if !output.is_error {
            self.note_prs(&output.content);
        }
        self.emit(AgentEvent::ToolFinished { id: call.id.clone(), display: output.display, is_error: output.is_error });
        ContentBlock::ToolResult {
            tool_use_id: call.id.clone(),
            content: self.clip(output.content),
            is_error: output.is_error,
        }
    }

    /// Runs one call that is not part of a `spawn_agent` batch.
    fn run_call(&mut self, call: &Call) -> ToolOutput {
        if self.cancel.is_cancelled() {
            return interrupted_output(&call.name);
        }
        if let Some(error) = &call.error {
            return ToolOutput::error(
                &call.name,
                format!(
                    "The input of `{}` could not be read, so the tool was not run: {error}. Call it again with a \
                     complete, valid JSON object.",
                    call.name
                ),
            );
        }
        let target = match self.toolbox.find(&call.name) {
            Some(Found::Local(tool)) => Target::Local(tool.clone()),
            Some(Found::Mcp(tool)) => Target::Mcp(tool.clone()),
            Some(Found::Session(kind)) => Target::Session(kind),
            None => Target::Unknown,
        };
        match target {
            Target::Local(tool) => self.local_call(tool.as_ref(), call),
            Target::Mcp(tool) => self.mcp_call(&tool, call),
            Target::Session(kind) => self.session_call(kind, call),
            Target::Unknown => ToolOutput::error(&call.name, self.unknown_tool(&call.name)),
        }
    }

    fn unknown_tool(&self, name: &str) -> String {
        if self.toolbox.deferred_tools().iter().any(|tool| tool.id == name) {
            return format!("`{name}` is not loaded yet. Call `{TOOL_SEARCH}` with `select:{name}` first, then call it.");
        }
        let restricted = self.env.read_only || self.env.mode == model::ChatMode::Plan;
        if restricted && tools::builtin(model::ToolAccess::Full).get(name).is_some() {
            return format!(
                "`{name}` is not available: this session changes nothing in its folder. Describe the change instead."
            );
        }
        format!("There is no tool `{name}`. The tools you can call: {}.", self.toolbox.names().join(", "))
    }

    fn local_call(&mut self, tool: &dyn Tool, call: &Call) -> ToolOutput {
        let Some(context) = self.tool_context() else {
            return ToolOutput::error(&call.name, format!("`{}` needs a working folder and this session has none.", call.name));
        };
        let class = tool.class();
        let subject = catch_unwind(AssertUnwindSafe(|| tool.subject(&context, &call.input))).unwrap_or_default();
        if let Err(refused) = self.permit(call, class, &subject, false) {
            return *refused;
        }
        catch_unwind(AssertUnwindSafe(|| tool.run(&context, &call.input))).unwrap_or_else(|panic| crashed(&call.name, &*panic))
    }

    fn mcp_call(&mut self, tool: &McpTool, call: &Call) -> ToolOutput {
        let Some(mcp) = self.deps.mcp else {
            return ToolOutput::error(&call.name, "The MCP servers are not available in this session.");
        };
        let auto_approved = mcp.is_auto_approved(&tool.id);
        if let Err(refused) = self.permit(call, ToolClass::External, "", auto_approved) {
            return *refused;
        }
        match catch_unwind(AssertUnwindSafe(|| mcp.call(&tool.id, call.input.clone(), self.cancel))) {
            Ok(result) => ToolOutput {
                display: mcp_display(tool, &result.text),
                content: result.text,
                is_error: result.is_error,
                files: Vec::new(),
            },
            Err(panic) => crashed(&call.name, &*panic),
        }
    }

    /// Asks the gate. `Err` carries the result of a call that may not run.
    fn permit(&self, call: &Call, class: ToolClass, subject: &str, auto_approved: bool) -> Result<(), Refused> {
        let name = call.name.as_str();
        let gate_call = GateCall {
            session: &self.env.session,
            tool: name,
            class,
            subject,
            repo: self.env.repo.as_deref(),
            auto_approved,
        };
        match self.deps.gate.check(&gate_call) {
            Verdict::Allow => Ok(()),
            Verdict::Deny { reason } => Err(Box::new(ToolOutput::error(
                name,
                format!("`{name}` was not run: {reason}. Do not retry it; find another way or tell the person."),
            ))),
            Verdict::Ask => {
                let context = self.tool_context();
                let (_, display) = self.toolbox.describe(name, &call.input, context.as_ref());
                let scope = match &self.env.repo {
                    Some(path) => RuleScope::Repo { path: path.clone() },
                    None => RuleScope::Session { session: self.env.session.clone() },
                };
                let request = PermissionRequest {
                    id: RequestId::new(base::id::new_id()),
                    session: self.env.session.clone(),
                    agent: self.spec.name.clone(),
                    tool: name.to_string(),
                    class,
                    display,
                    policy: policy_label(name, RuleDecision::Ask),
                    always_rule: always_rule(name, class, subject, scope),
                    at: self.now(),
                };
                self.ask(name, request)
            }
        }
    }

    /// Waits for a person's answer to `request`. `Err` carries the result of a refused call.
    pub(crate) fn ask(&self, name: &str, request: PermissionRequest) -> Result<(), Refused> {
        match self.deps.gate.ask(request, self.cancel) {
            None => Err(Box::new(interrupted_output(name))),
            // An approval that crossed the cancel on its way here starts nothing.
            Some(PermissionDecision::AllowOnce | PermissionDecision::AllowAlways { .. }) if self.cancel.is_cancelled() => {
                Err(Box::new(interrupted_output(name)))
            }
            Some(PermissionDecision::AllowOnce | PermissionDecision::AllowAlways { .. }) => Ok(()),
            Some(PermissionDecision::Reject { note }) => {
                let note = note.trim();
                let note = if note.is_empty() { String::new() } else { format!(" Their note: \"{note}\".") };
                Err(Box::new(ToolOutput::error(
                    name,
                    format!("The person rejected this action.{note} Do not repeat the same call; adapt."),
                )))
            }
        }
    }

    fn session_call(&mut self, kind: SessionTool, call: &Call) -> ToolOutput {
        let name = call.name.as_str();
        let failed = |message: String| ToolOutput::error(name, message);
        match kind {
            SessionTool::Todo => {
                let items = match todo::parse(&call.input) {
                    Ok(items) => items,
                    Err(message) => return failed(message),
                };
                if let Err(error) = self.deps.host.set_todos(&self.env.session, &items) {
                    return failed(format!("The task list could not be saved: {error:#}"));
                }
                self.todos = items.clone();
                let output = ok("Todo list", todo::summary(&items));
                self.emit(AgentEvent::TodoUpdated { items });
                output
            }
            SessionTool::Skill => match skill::parse(&call.input) {
                Ok(skill_name) => self.load_skill(&skill_name),
                Err(message) => failed(message),
            },
            SessionTool::ToolSearch => match search::parse(&call.input) {
                Ok((query, max)) => {
                    let (count, text) = self.toolbox.load(&query, max);
                    let mut output = ok(TOOL_SEARCH, text);
                    output.display = model::ToolDisplay::Line {
                        verb: TOOL_SEARCH.into(),
                        target: query,
                        result: base::fmt::plural(count as u64, "tool", "tools"),
                    };
                    output
                }
                Err(message) => failed(message),
            },
            SessionTool::Report => {
                let parsed = match report::parse(&call.input) {
                    Ok(parsed) => parsed,
                    Err(message) => return failed(message),
                };
                match self.deps.host.post_report(&self.env.session, &parsed) {
                    Ok(()) => {
                        let mut output = ok(&parsed.title, report::posted(&parsed));
                        output.display = crate::session_tools::text_display(&parsed.title, &parsed.summary);
                        output
                    }
                    Err(error) => failed(format!("The report could not be saved: {error:#}")),
                }
            }
            SessionTool::AskUser => match ask::parse(&call.input) {
                Ok(question) => {
                    let mut output = ok("Question", ask::ASKED);
                    output.display = crate::session_tools::text_display("Question", &question);
                    self.question = Some(question);
                    output
                }
                Err(message) => failed(message),
            },
            SessionTool::Finish => {
                let Finish::Tool(tool) = &self.spec.finish else {
                    return failed(format!("There is no tool `{name}`."));
                };
                match finish::parse(tool, &call.input) {
                    Ok(payload) => {
                        let mut output = ok(name, finish::RECORDED);
                        output.display = crate::session_tools::text_display(name, &crate::session_tools::pretty(&payload));
                        self.payload = Some(payload);
                        output
                    }
                    Err(message) => failed(message),
                }
            }
            // A lone spawn call that did not join a batch.
            SessionTool::Spawn => self
                .spawn_batch(std::slice::from_ref(call))
                .pop()
                .unwrap_or_else(|| interrupted_output(name)),
        }
    }

    fn load_skill(&mut self, name: &str) -> ToolOutput {
        let line = |result: &str| model::ToolDisplay::Line { verb: SKILL.into(), target: name.to_string(), result: result.into() };
        if self.spec.skills.forced.iter().any(|forced| forced == name) {
            let mut output = ok(SKILL, format!("Skill `{name}` is always loaded: its instructions are in your system prompt."));
            output.display = line("already loaded");
            return output;
        }
        if !self.offered_skills.iter().any(|skill| skill.name == name) {
            let names: Vec<&str> = self.offered_skills.iter().map(|s| s.name.as_str()).collect();
            return ToolOutput::error(SKILL, format!("There is no skill `{name}`. Available skills: {}.", names.join(", ")));
        }
        if self.loaded_skills.iter().any(|loaded| loaded == name) {
            let mut output = ok(SKILL, format!("Skill `{name}` is already loaded earlier in this conversation. Follow it."));
            output.display = line("already loaded");
            return output;
        }
        let Some(text) = self.deps.skills.skill_text(name) else {
            return ToolOutput::error(SKILL, format!("Skill `{name}` can no longer be read from disk."));
        };
        self.loaded_skills.push(name.to_string());
        self.emit(AgentEvent::SkillLoaded { name: name.to_string() });
        let mut output = ok(SKILL, text);
        output.display = line("loaded");
        output
    }

    /// Adds changed files to the session's list: line counts of one path add up.
    fn record_files(&mut self, changed: &[FileChange]) -> Result<()> {
        for change in changed {
            match self.files.iter_mut().find(|known| known.path == change.path) {
                Some(known) => {
                    known.added += change.added;
                    known.removed += change.removed;
                }
                None => self.files.push(change.clone()),
            }
        }
        self.deps.host.set_files(&self.env.session, &self.files)?;
        self.emit(AgentEvent::FilesChanged { files: self.files.clone() });
        Ok(())
    }

    /// A tool result cut to the size the model may read: the start and the end stay, the
    /// whole text goes to the blob store, and a note in the middle says so.
    fn clip(&self, content: String) -> String {
        let max = self.spec.limits.tool_output_max;
        if content.len() <= max {
            return content;
        }
        let mut head = max / 3 * 2;
        while !content.is_char_boundary(head) {
            head -= 1;
        }
        let mut tail = content.len() - max / 3;
        while !content.is_char_boundary(tail) {
            tail += 1;
        }
        let kept = match self.deps.host.put_blob(content.as_bytes()) {
            Ok(id) => format!("the full output is kept as blob {id}"),
            Err(_) => "the full output could not be kept".to_string(),
        };
        format!(
            "{}\n\n[… {} left out here: the output was {} long and only its start and end are shown; {kept}. \
             Narrow the call if you need the middle.]\n\n{}",
            &content[..head],
            base::fmt::bytes((tail - head) as u64),
            base::fmt::bytes(content.len() as u64),
            &content[tail..]
        )
    }
}

/// The result of a tool that panicked.
fn crashed(name: &str, panic: &(dyn Any + Send)) -> ToolOutput {
    ToolOutput::error(
        name,
        format!(
            "`{name}` crashed ({}). It may have done part of its work; check the state before you try again.",
            panic_text(panic)
        ),
    )
}
