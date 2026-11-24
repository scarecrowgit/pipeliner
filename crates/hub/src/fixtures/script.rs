//! Scripted agent streams: what the fixture plays when a permission is answered, a message is
//! sent or a turn is stopped. Each follows the order the engine's agent loop emits:
//!
//! - every model request starts with `TurnStarted`, then text and thinking deltas, then
//!   `ToolCallStarted` / `ToolCallReady` per call;
//! - after the stream, `Usage`, then `MessageCommitted` for the assistant message;
//! - each call ends with `ToolFinished` (after `TodoUpdated` or `FilesChanged` when the tool
//!   changed those), and one `MessageCommitted` of the user role carries every result;
//! - the turn ends with `TurnFinished`; the session header changes around it.

use crate::api::{
    AgentEvent, ContentBlock, Event, Message, Role, SessionId, SessionState, StopReason, TodoItem,
    TodoStatus, ToolClass, ToolDisplay, Usage,
};

use super::World;
use super::data_transcript::{stored, text, tool_result, tool_use};

/// What the engine stores for a call that a stop cut short.
pub const INTERRUPTED: &str = "Interrupted before the tool finished. It may not have run; check before relying on it.";

/// Splits text into small chunks, as a model streams it.
pub fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_inclusive(' ') {
        current.push_str(word);
        if current.len() >= 12 {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Builds one session's events while changing the world the way the engine would.
struct Turn<'a> {
    world: &'a mut World,
    session: SessionId,
    events: Vec<Event>,
    /// Context tokens of the last model request, for the next header.
    context: Option<u64>,
}

impl<'a> Turn<'a> {
    fn new(world: &'a mut World, session: &SessionId) -> Self {
        Self { world, session: session.clone(), events: Vec::new(), context: None }
    }

    fn agent(&mut self, event: AgentEvent) {
        self.events.push(Event::Agent { session: self.session.clone(), event });
    }

    fn next_seq(&self) -> u64 {
        self.world.messages.get(&self.session).and_then(|messages| messages.last()).map_or(1, |message| message.seq + 1)
    }

    /// Stores a message and reports it.
    fn commit(&mut self, role: Role, content: Vec<ContentBlock>) {
        let model = (role == Role::Assistant).then(|| self.world.session(&self.session).map(|chat| chat.model.clone())).flatten();
        let message = stored(self.next_seq(), self.world.now, role, content, model);
        self.world.messages.entry(self.session.clone()).or_default().push(message.clone());
        if let Ok(chat) = self.world.session_mut(&self.session) {
            chat.message_count += 1;
        }
        self.agent(AgentEvent::MessageCommitted { message });
    }

    /// The session header after a change.
    fn header(&mut self, state: SessionState, cost: f64) {
        let now = self.world.now;
        let context = self.context;
        if let Ok(chat) = self.world.session_mut(&self.session) {
            chat.state = state;
            if let Some(tokens) = context {
                chat.context_tokens = tokens;
            }
            chat.cost_usd += cost;
            chat.updated = now;
            let session = chat.clone();
            self.events.push(Event::SessionChanged { session });
        }
    }

    /// The end of a model request: its usage, then the stored assistant message.
    fn reply(&mut self, content: Vec<ContentBlock>) {
        self.agent(AgentEvent::Usage {
            usage: Usage { input: 2_100, output: 180, cache_read: 46_000, cache_write: 600 },
            cost_usd: 0.02,
            context_tokens: 48_900,
            context_window: 200_000,
        });
        self.context = Some(48_900);
        self.commit(Role::Assistant, content);
    }

    /// A model request that only answers in text.
    fn streamed_reply(&mut self, reply: &str) {
        self.agent(AgentEvent::TurnStarted);
        for chunk in chunks(reply) {
            self.agent(AgentEvent::TextDelta { text: chunk });
        }
        self.reply(vec![text(reply)]);
    }

    /// The tool call's start and its complete input, as streamed.
    fn call(&mut self, id: &str, name: &str, class: ToolClass, display: ToolDisplay) {
        self.agent(AgentEvent::ToolCallStarted { id: id.to_string(), name: name.to_string() });
        self.agent(AgentEvent::ToolCallReady { id: id.to_string(), name: name.to_string(), class, display });
    }

    fn finished(&mut self, id: &str, display: ToolDisplay, is_error: bool) {
        self.agent(AgentEvent::ToolFinished { id: id.to_string(), display, is_error });
    }
}

/// The rest of the "Fix double charge on retry" turn once `bun test` is allowed: the command
/// runs, the agent ticks off the todo list, reports and ends the turn.
pub fn after_permission(world: &mut World, session: &SessionId) -> Vec<Event> {
    let request = world.permissions.iter().position(|request| &request.session == session).map(|i| world.permissions.remove(i));
    let mut turn = Turn::new(world, session);
    if let Some(request) = &request {
        turn.events.push(Event::PermissionResolved { id: request.id.clone() });
    }
    turn.header(SessionState::Running, 0.0);

    let output = "bun test v1.3.2\n\nsrc/charges/retry.test.ts:\n✓ retries reuse the idempotency key\n✓ a timeout does not charge twice\n\n212 pass\n0 fail";
    let command = |output: &str, exit_code| ToolDisplay::Command {
        command: "bun test src/charges".to_string(),
        output: output.to_string(),
        exit_code,
    };
    turn.finished("t8", command(output, Some(0)), false);
    turn.commit(Role::User, vec![tool_result("t8", output)]);

    let done = |text: &str| TodoItem { text: text.to_string(), status: TodoStatus::Done };
    let items = vec![
        done("Find every caller of retryCharge"),
        done("Add an idempotency key to retries"),
        done("Write a test that fails on main"),
        done("Run the charges test suite"),
    ];
    let todo_display = ToolDisplay::Text { title: "Todo list".to_string(), body: "4 of 4 done".to_string() };
    turn.agent(AgentEvent::TurnStarted);
    turn.call("t9", "todowrite", ToolClass::Internal, todo_display.clone());
    let input = r#"{"items":[{"text":"Find every caller of retryCharge","status":"done"},{"text":"Add an idempotency key to retries","status":"done"},{"text":"Write a test that fails on main","status":"done"},{"text":"Run the charges test suite","status":"done"}]}"#;
    turn.reply(vec![tool_use("t9", "todowrite", input)]);
    turn.agent(AgentEvent::TodoUpdated { items });
    turn.finished("t9", todo_display, false);
    turn.commit(Role::User, vec![tool_result("t9", "Todo list updated")]);

    turn.streamed_reply(
        "All 212 tests pass, including the new one that failed on main. Retries now reuse one idempotency key, so a timeout can no longer charge the card twice.",
    );
    turn.agent(AgentEvent::TurnFinished { stop: StopReason::EndTurn });
    turn.header(SessionState::Done, 0.06);
    turn.events
}

/// A sent message: stored, one file read, then a short streamed answer.
pub fn reply_to(world: &mut World, session: &SessionId, sent: &Message) -> Vec<Event> {
    let id = format!("r{}", world.next_id);
    world.next_id += 1;
    let mut turn = Turn::new(world, session);
    turn.header(SessionState::Running, 0.0);
    turn.commit(Role::User, sent.content.clone());

    let target = "AGENTS.md".to_string();
    let read = |result: &str| ToolDisplay::Line { verb: "read".into(), target: target.clone(), result: result.into() };
    let lead = "Let me check the project notes first.";
    turn.agent(AgentEvent::TurnStarted);
    for chunk in chunks(lead) {
        turn.agent(AgentEvent::TextDelta { text: chunk });
    }
    turn.call(&id, "read_file", ToolClass::Read, read(""));
    turn.reply(vec![text(lead), tool_use(&id, "read_file", r#"{"path":"AGENTS.md"}"#)]);
    turn.finished(&id, read("32 lines"), false);
    turn.commit(Role::User, vec![tool_result(&id, "# Agent notes\n(32 lines)")]);

    turn.streamed_reply("Got it. I checked the project notes; this is a preview, so I stop here.");
    turn.agent(AgentEvent::TurnFinished { stop: StopReason::EndTurn });
    turn.header(SessionState::Idle, 0.04);
    turn.events
}

/// A stop: a pending request is dropped, every call of the last assistant message that has
/// no result is answered as interrupted, and the turn ends cancelled.
pub fn interrupted(world: &mut World, session: &SessionId) -> Vec<Event> {
    let resolved: Vec<_> = world.permissions.iter().filter(|request| &request.session == session).map(|r| r.id.clone()).collect();
    world.permissions.retain(|request| &request.session != session);
    let dangling: Vec<(String, String)> = world
        .messages
        .get(session)
        .and_then(|messages| messages.last())
        .filter(|last| last.message.role == Role::Assistant)
        .map(|last| {
            last.message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mut turn = Turn::new(world, session);
    turn.events.extend(resolved.into_iter().map(|id| Event::PermissionResolved { id }));
    if !dangling.is_empty() {
        let mut results = Vec::new();
        for (id, name) in &dangling {
            turn.finished(id, ToolDisplay::Text { title: name.clone(), body: INTERRUPTED.to_string() }, true);
            results.push(ContentBlock::ToolResult { tool_use_id: id.clone(), content: INTERRUPTED.to_string(), is_error: true });
        }
        turn.commit(Role::User, results);
    }
    turn.agent(AgentEvent::TurnFinished { stop: StopReason::Cancelled });
    turn.header(SessionState::Idle, 0.0);
    turn.events
}
