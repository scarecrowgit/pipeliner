//! The one agent loop used by chat and pipelines: turns, tools, sub-agents, compaction.
//!
//! [`run`] takes what the agent is ([`AgentSpec`]), where it runs ([`AgentEnv`]), what it may
//! call ([`AgentDeps`]) and what it is asked ([`Input`]); it blocks the calling thread until
//! the agent stops and says why ([`AgentOutcome`]). A chat turn, a pipeline node and a
//! sub-agent are the same code with different specs.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`spec`] | The arguments and the result of [`run`] |
//! | [`host`] | [`Host`]: the session's storage and services, implemented by the engine |
//! | [`gate`] | [`PermissionGate`], [`AllowAll`], and [`RuleGate`] over a [`RuleStore`] |
//! | [`sink`] | [`AgentSink`]: where [`model::AgentEvent`]s go |
//! | [`mcp`] | [`Mcp`]: the MCP servers as the loop uses them |
//! | [`memory`] | [`MemoryHost`], a host for tests |
//! | [`notes`] | The marker of messages the loop writes itself |
//!
//! What the loop guarantees about the stored conversation:
//!
//! - Every tool call is answered by exactly one result in the message right after it: also
//!   after a cancel, a panicking tool, a rejected permission, input that was not valid JSON
//!   (answered with an error, never run) and a session whose last run died mid-call (answered
//!   as interrupted before anything else happens).
//! - Text that streamed before a cancel is kept; the conversation stays valid for the next
//!   request.
//! - A compaction never separates a tool call from its result.
//! - The system prompt is the same text for every conversation request of a run. (The request
//!   that writes a compaction's summary is not one: it has no tools and its own short prompt.)
//! - Tool call ids are unique among the messages still sent to the model, also when the
//!   provider repeats them or sends none.
//! - Results are stored in the order of their calls, also when sub-agents created by one
//!   message run at the same time and finish in another order.

mod calls;
mod compact;
pub mod gate;
mod history;
pub mod host;
pub mod mcp;
pub mod memory;
pub mod notes;
mod prompt;
mod session_tools;
pub mod sink;
pub mod spec;
mod stream;
mod subagents;
mod toolbox;
mod turn;

#[cfg(test)]
mod tests;

use base::cancel::CancelToken;

pub use gate::{AllowAll, AskFn, GateCall, MemoryRules, PermissionGate, RuleGate, RuleStore, Verdict};
pub use host::{ChildSession, Host, Report};
pub use mcp::Mcp;
pub use memory::MemoryHost;
pub use session_tools::names as tool_names;
pub use sink::{AgentSink, EventLog};
pub use spec::{
    AgentDeps, AgentEnv, AgentOutcome, AgentSpec, DEFAULT_MAX_STEPS, Finish, FinishTool, Input, Limits, StopCause,
    SubagentTemplate, Totals, UserInput,
};

/// Runs the agent on the calling thread until it stops: the model answered without calling a
/// tool (or called the finish tool), asked a person something, was cancelled, hit the step
/// limit or the budget, or failed.
///
/// The input is stored first, then the loop alternates model calls and tool calls. Everything
/// it does is reported to `sink` as it happens, under the session's id (a sub-agent's events
/// under its own); everything it keeps goes through [`Host`]. Cancelling `cancel` stops the
/// model stream, the running tool and every sub-agent, and leaves a conversation the next
/// `run` can continue.
pub fn run(
    spec: &AgentSpec,
    env: &AgentEnv,
    deps: &AgentDeps<'_>,
    input: Input,
    sink: &mut dyn AgentSink,
    cancel: &CancelToken,
) -> AgentOutcome {
    let shared = sink::SharedSink::new(sink);
    turn::run_shared(spec, env, deps, input, &shared, cancel)
}
