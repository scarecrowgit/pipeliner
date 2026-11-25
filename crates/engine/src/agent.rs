//! Agent runtime: the model ↔ tools loop. Streams output into a thread, reads your
//! steering messages between steps, records tokens and cost on the task.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use harness_core::*;
use serde_json::Value;

use crate::llm::{self, Call, Msg, ToolSpec};
use crate::prompt::Stack;
use crate::tools::{self, ToolContext};
use crate::Engine;

/// What a custom tool handler returns.
pub enum Handled {
    /// Result text for the model; keep going.
    Continue(String),
    /// Result text; stop the loop and return `value` as the outcome's value.
    Stop(String, Value),
}

pub struct AgentRun<'a> {
    pub engine: &'a Engine,
    /// Task that accrues cost and can be cancelled.
    pub task_id: Id,
    /// Thread that shows the conversation (task id or chat id).
    pub thread_id: Id,
    /// Name shown as message author.
    pub actor: String,
    pub model: String,
    pub stack: Stack,
    pub tools: ToolContext,
    /// Extra tools handled by `handler` (delegate, answer, propose_plan...).
    pub extra_tools: Vec<ToolSpec>,
    /// Earlier turns (chats).
    pub history: Vec<Msg>,
    pub max_steps: u32,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub text: String,
    pub value: Value,
    pub steps: u32,
}

const STEER_PREFIX: &str = "Message from the user while you work (follow it):\n";

impl<'a> AgentRun<'a> {
    pub fn run(
        mut self,
        handler: &mut (dyn FnMut(&AgentRun<'a>, &Call, &Value) -> Option<Handled> + Send),
    ) -> Result<Outcome> {
        let engine = self.engine;
        let store = engine.store().clone();
        let config = engine.config();
        let (provider, model_id) = config
            .resolve(&self.model)
            .map(|(p, m)| (p.clone(), m))
            .context("no model provider configured (Settings › Models)")?;
        let cancel = self.tools.cancel.clone();

        let mut msgs: Vec<Msg> = vec![Msg::System(self.stack.system(&config))];
        msgs.append(&mut self.history);
        msgs.push(Msg::User(self.stack.first_turn()));
        // Messages already in the thread were part of the first turn.
        let _ = store.take_undelivered(&self.thread_id);

        let mut specs = tools::specs(&self.tools);
        specs.extend(self.extra_tools.iter().cloned());

        let mut out = Outcome::default();
        let mut step = 0u32;
        let mut wrap_up = false;
        loop {
            if cancel.load(Ordering::SeqCst) {
                bail!("cancelled");
            }
            for m in store.take_undelivered(&self.thread_id) {
                msgs.push(Msg::User(format!("{STEER_PREFIX}{}", m.content)));
            }
            if step >= self.max_steps && !wrap_up {
                wrap_up = true;
                msgs.push(Msg::User(
                    "You have reached your step limit. Stop using tools and reply with a summary: what you did, what works, and what is left."
                        .into(),
                ));
            }
            compact(&mut msgs);
            step += 1;

            // Stream into a partial assistant message.
            let mut m = Message::new(&self.thread_id, Role::Assistant, "");
            m.author = self.actor.clone();
            m.partial = true;
            store.put_message(&m)?;
            let mut last_flush = Instant::now();
            let cancel_fn = || cancel.load(Ordering::SeqCst);
            // Keep sending tool specs (some gateways reject tool history without them);
            // during wrap-up any calls are ignored.
            let tools_now: &[ToolSpec] = &specs;
            let reply = llm::complete(
                &provider,
                &model_id,
                &msgs,
                tools_now,
                None,
                &cancel_fn,
                &mut |delta| {
                    m.content.push_str(delta);
                    if last_flush.elapsed() > Duration::from_millis(120) {
                        let _ = store.put_message(&m);
                        last_flush = Instant::now();
                    }
                },
            );
            let reply = match reply {
                Ok(r) => r,
                Err(e) => {
                    m.partial = false;
                    if m.content.is_empty() {
                        m.role = Role::System;
                        m.content = format!("Model call failed: {e:#}");
                    }
                    let _ = store.put_message(&m);
                    return Err(e);
                }
            };
            let cost = config.cost(&self.model, reply.tokens_in, reply.tokens_out);
            let run_id = store
                .update_task(&self.task_id, |t| {
                    t.tokens_in += reply.tokens_in;
                    t.tokens_out += reply.tokens_out;
                    t.cost_usd += cost;
                    t.steps += 1;
                })
                .ok()
                .and_then(|t| t.run_id);
            if let Some(run_id) = run_id {
                let _ = store.update_run(&run_id, |r| r.cost_usd += cost);
            }

            m.content = reply.content.clone();
            m.partial = false;
            m.tool_calls = reply
                .calls
                .iter()
                .map(|c| ToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect();
            if m.content.trim().is_empty() && m.tool_calls.is_empty() {
                m.content = "(no reply)".into();
            }
            store.put_message(&m)?;
            msgs.push(Msg::Assistant {
                content: reply.content.clone(),
                tool_calls: reply.calls.clone(),
            });

            if reply.calls.is_empty() || wrap_up {
                if wrap_up && !reply.calls.is_empty() {
                    // Drop the unanswered calls from the stored message.
                    m.tool_calls.clear();
                    if m.content.trim().is_empty() {
                        m.content = "(stopped at the step limit)".into();
                    }
                    store.put_message(&m)?;
                }
                // Late steering: if you posted while it was answering, keep going.
                let pending = store.take_undelivered(&self.thread_id);
                if !pending.is_empty() && !wrap_up {
                    for p in pending {
                        msgs.push(Msg::User(format!("{STEER_PREFIX}{}", p.content)));
                    }
                    continue;
                }
                out.text = reply.content;
                out.steps = step;
                return Ok(out);
            }

            // Tool calls, in order.
            let results = self.execute_calls(&reply.calls, handler);
            let mut stop: Option<Value> = None;
            for (call, (text, stop_value)) in reply.calls.iter().zip(results) {
                let mut tm = Message::new(
                    &self.thread_id,
                    Role::Tool,
                    tools::truncate_middle(&text, 6000),
                );
                tm.author = call.name.clone();
                tm.tool_call_id = Some(call.id.clone());
                tm.tool_calls = vec![ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                }];
                store.put_message(&tm)?;
                msgs.push(Msg::Tool {
                    call_id: call.id.clone(),
                    content: tools::truncate_middle(&text, 20_000),
                });
                if let Some(v) = stop_value {
                    stop = Some(v);
                }
            }
            if let Some(v) = stop {
                out.value = v;
                out.text = m.content.clone();
                out.steps = step;
                return Ok(out);
            }
        }
    }

    fn execute_calls(
        &self,
        calls: &[Call],
        handler: &mut (dyn FnMut(&AgentRun<'a>, &Call, &Value) -> Option<Handled> + Send),
    ) -> Vec<(String, Option<Value>)> {
        let parse = |c: &Call| serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null);
        let mut results: Vec<Option<(String, Option<Value>)>> = vec![None; calls.len()];
        for (i, c) in calls.iter().enumerate() {
            if results[i].is_some() {
                continue;
            }
            let args = parse(c);
            if (c.arguments.trim().is_empty() || args.is_null() && c.arguments.trim() != "null")
                && serde_json::from_str::<Value>(&c.arguments).is_err()
            {
                results[i] = Some((
                    format!("Error: arguments are not valid JSON: {}", c.arguments),
                    None,
                ));
                continue;
            }
            let r = handler(self, c, &args)
                .map(|h| handled(Some(h)))
                .unwrap_or_else(|| (tools::execute(&self.tools, &c.name, &args), None));
            results[i] = Some(r);
        }
        results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| ("Error: tool did not run".into(), None)))
            .collect()
    }
}

fn handled(h: Option<Handled>) -> (String, Option<Value>) {
    match h {
        Some(Handled::Continue(s)) => (s, None),
        Some(Handled::Stop(s, v)) => (s, Some(v)),
        None => ("Error: unknown tool".into(), None),
    }
}

/// Keeps the context bounded: older tool results are shortened once the
/// conversation grows past ~300k characters.
fn compact(msgs: &mut [Msg]) {
    let total: usize = msgs
        .iter()
        .map(|m| match m {
            Msg::System(s) | Msg::User(s) => s.len(),
            Msg::Assistant {
                content,
                tool_calls,
            } => content.len() + tool_calls.iter().map(|c| c.arguments.len()).sum::<usize>(),
            Msg::Tool { content, .. } => content.len(),
        })
        .sum();
    if total < 300_000 {
        return;
    }
    let n = msgs.len();
    for (i, m) in msgs.iter_mut().enumerate() {
        if i + 16 >= n {
            break;
        }
        if let Msg::Tool { content, .. } = m {
            if content.len() > 600 {
                let head: String = content.chars().take(400).collect();
                *content = format!("{head}\n… [older tool output shortened to save context]");
            }
        }
    }
}

/// Spec for the `delegate` tool.
pub fn delegate_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: "delegate",
        description: format!(
            "Hand one focused task to a {name} sub-agent that works in the same folder with full tools. \
             Give exact files, the change, and acceptance criteria. Returns its report. \
             Delegated tasks run one at a time."
        ),
        parameters: serde_json::json!({
            "type":"object",
            "properties":{
                "title":{"type":"string","description":"short task title for the board"},
                "task":{"type":"string","description":"full instructions"}
            },
            "required":["title","task"]
        }),
    }
}
