//! OpenAI-compatible chat completions client with streaming and tool calls.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use harness_core::config::Provider;
use serde_json::{json, Value};

/// A message in the wire format.
#[derive(Clone, Debug)]
pub enum Msg {
    System(String),
    User(String),
    Assistant {
        content: String,
        tool_calls: Vec<Call>,
    },
    Tool {
        call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub content: String,
    pub calls: Vec<Call>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub finish_reason: String,
}

fn msg_json(m: &Msg) -> Value {
    match m {
        Msg::System(s) => json!({"role":"system","content":s}),
        Msg::User(s) => json!({"role":"user","content":s}),
        Msg::Assistant {
            content,
            tool_calls,
        } => {
            let mut v = json!({"role":"assistant","content":content});
            if !tool_calls.is_empty() {
                v["tool_calls"] = tool_calls
                    .iter()
                    .map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}}))
                    .collect();
            }
            v
        }
        Msg::Tool { call_id, content } => {
            json!({"role":"tool","tool_call_id":call_id,"content":content})
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(300))
        .build()
}

/// Streams one completion. `on_text` receives text deltas as they arrive.
/// `cancel` is polled between chunks.
pub fn complete(
    provider: &Provider,
    model: &str,
    messages: &[Msg],
    tools: &[ToolSpec],
    max_tokens: Option<u32>,
    cancel: &dyn Fn() -> bool,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let mut body = json!({
        "model": model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages.iter().map(msg_json).collect::<Vec<_>>(),
    });
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}}))
            .collect();
    }

    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            for _ in 0..(2u64.pow(attempt) * 10) {
                if cancel() {
                    bail!("cancelled");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let resp = agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {}", provider.key()))
            .set("Content-Type", "application/json")
            .send_json(body.clone());
        match resp {
            Ok(r) => match read_stream(r, cancel, on_text) {
                Ok(reply) => return Ok(reply),
                // A dropped stream is retried unless text was already shown or we were cancelled.
                Err(e) if !cancel() && format!("{e}").contains("ended early") => {
                    last_err = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            },
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                let err = anyhow!(
                    "model call failed ({code}): {}",
                    text.chars().take(400).collect::<String>()
                );
                if code == 429 || code >= 500 {
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }
            Err(e) => {
                last_err = Some(anyhow!("model call failed: {e}"));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("model call failed")))
}

fn read_stream(
    resp: ureq::Response,
    cancel: &dyn Fn() -> bool,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply> {
    let ct = resp.header("content-type").unwrap_or("").to_string();
    let reader = BufReader::new(resp.into_reader());
    let mut reply = Reply::default();
    if ct.contains("application/json") {
        // Non-streaming fallback.
        let v: Value = serde_json::from_reader(reader)?;
        return parse_full(&v);
    }
    // tool call index -> (id, name, args)
    let mut calls: Vec<Call> = vec![];
    let mut done = false;
    for line in reader.lines() {
        if cancel() {
            bail!("cancelled");
        }
        let line = line.context("reading model stream")?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            done = true;
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(err) = v.get("error") {
            bail!("model error: {err}");
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            reply.tokens_in = u["prompt_tokens"].as_u64().unwrap_or(reply.tokens_in);
            reply.tokens_out = u["completion_tokens"].as_u64().unwrap_or(reply.tokens_out);
        }
        let Some(choice) = v["choices"].get(0) else {
            continue;
        };
        if let Some(fr) = choice["finish_reason"].as_str() {
            reply.finish_reason = fr.to_string();
            done = true;
        }
        let delta = &choice["delta"];
        if let Some(t) = delta["content"].as_str() {
            if !t.is_empty() {
                reply.content.push_str(t);
                on_text(t);
            }
        }
        if let Some(tcs) = delta["tool_calls"].as_array() {
            for tc in tcs {
                let id = tc["id"].as_str().filter(|s| !s.is_empty());
                // A new id starts a new call; otherwise continue the call at `index` (or the last one).
                let idx = match (id, tc["index"].as_u64()) {
                    (Some(id), _) if !calls.iter().any(|c| c.id == id) => {
                        let at = tc["index"]
                            .as_u64()
                            .map(|i| i as usize)
                            .filter(|&i| i >= calls.len() || calls[i].id.is_empty());
                        match at {
                            Some(i) => i,
                            None => calls.len(),
                        }
                    }
                    (Some(id), _) => calls.iter().position(|c| c.id == id).unwrap_or(0),
                    (None, Some(i)) => i as usize,
                    (None, None) => calls.len().saturating_sub(1),
                };
                while calls.len() <= idx {
                    calls.push(Call {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });
                }
                let c = &mut calls[idx];
                if let Some(id) = tc["id"].as_str() {
                    c.id = id.to_string();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    c.name.push_str(n);
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    c.arguments.push_str(a);
                }
            }
        }
    }
    if !done {
        bail!("model stream ended early");
    }
    if reply.finish_reason == "length" && !calls.is_empty() {
        bail!("model output was cut off (max tokens) in the middle of a tool call");
    }
    for (i, c) in calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
        if c.arguments.trim().is_empty() {
            c.arguments = "{}".into();
        }
    }
    reply.calls = calls.into_iter().filter(|c| !c.name.is_empty()).collect();
    Ok(reply)
}

fn parse_full(v: &Value) -> Result<Reply> {
    let m = &v["choices"][0]["message"];
    let mut r = Reply {
        content: m["content"].as_str().unwrap_or("").to_string(),
        tokens_in: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        tokens_out: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        finish_reason: v["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        calls: vec![],
    };
    if let Some(tcs) = m["tool_calls"].as_array() {
        for (i, tc) in tcs.iter().enumerate() {
            r.calls.push(Call {
                id: tc["id"]
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| format!("call_{i}")),
                name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                arguments: tc["function"]["arguments"]
                    .as_str()
                    .unwrap_or("{}")
                    .to_string(),
            });
        }
    }
    Ok(r)
}

/// Lists model ids the provider serves (`GET /models`). Empty if unsupported.
pub fn list_models(provider: &Provider) -> Result<Vec<String>> {
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    let r = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", provider.key()))
        .call()?;
    let v: Value = r.into_json()?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// One short non-tool completion (titles, summaries). Returns trimmed text.
pub fn quick(
    provider: &Provider,
    model: &str,
    system: &str,
    user: &str,
) -> Result<(String, u64, u64)> {
    let r = complete(
        provider,
        model,
        &[Msg::System(system.into()), Msg::User(user.into())],
        &[],
        Some(400),
        &|| false,
        &mut |_| {},
    )?;
    Ok((r.content.trim().to_string(), r.tokens_in, r.tokens_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_non_stream() {
        let v = json!({"choices":[{"message":{"content":"hi","tool_calls":[{"id":"x","function":{"name":"f","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":4}});
        let r = parse_full(&v).unwrap();
        assert_eq!(r.content, "hi");
        assert_eq!(r.calls[0].name, "f");
        assert_eq!(r.tokens_out, 4);
    }
}
