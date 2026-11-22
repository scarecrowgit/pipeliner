//! Live check for the `llm` crate.
//!
//! ```sh
//! cargo run -p llm --example chat -- [provider/model] "prompt"   # stream a reply
//! cargo run -p llm --example chat -- --tools anthropic/claude-sonnet-5-5 "What time is it?"
//! cargo run -p llm --example chat -- --models [provider]          # list models
//! ```
//!
//! Without `provider/model`, uses `default_model` from the config. Without a prompt, reads
//! stdin. `--tools` offers a `current_time` tool and runs the tool loop (max 4 turns).
//! API keys are never printed.

use std::io::{Read as _, Write as _};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use llm::{
    ChatRequest, Config, ContentBlock, Message, Role, StopReason, StreamEvent, ToolSpec, Usage,
};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |args: &mut Vec<String>, name: &str| {
        let found = args.iter().any(|a| a == name);
        args.retain(|a| a != name);
        found
    };
    let list = flag(&mut args, "--models");
    let tools = flag(&mut args, "--tools");

    let config = Config::load()?;
    eprintln!("config: {}", Config::path().display());
    for p in &config.providers {
        let key = if p.api_key.is_some() {
            "key set"
        } else {
            "no key"
        };
        eprintln!(
            "  provider {:<12} {:?} {} ({key})",
            p.id, p.kind, p.base_url
        );
    }
    if let Some(default) = &config.default_model {
        eprintln!("  default_model {default}");
    }

    if list {
        for p in config
            .providers
            .iter()
            .filter(|p| args.is_empty() || args.contains(&p.id))
        {
            match llm::list_models(p) {
                Ok(models) => models.iter().for_each(|m| println!("{}/{m}", p.id)),
                Err(e) => eprintln!("{}: {e:#}", p.id),
            }
        }
        return Ok(());
    }

    // The first arg is a model ref if it looks like "<known provider>/<model>".
    let is_model_ref = |a: &str| {
        llm::config::split_model_ref(a).is_some_and(|(p, _)| config.provider(p).is_some())
    };
    let model_ref = match args.first() {
        Some(a) if is_model_ref(a) => args.remove(0),
        _ => config
            .default_model
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no model given and no default_model in config"))?,
    };
    let (provider_id, model) = llm::config::split_model_ref(&model_ref)
        .ok_or_else(|| anyhow::anyhow!("model must be \"provider/model\", got {model_ref:?}"))?;
    let provider = config
        .provider(provider_id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider {provider_id:?}"))?;

    let prompt = if args.is_empty() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        args.join(" ")
    };

    let mut request = ChatRequest {
        model: model.to_string(),
        system: Some("You are a concise assistant.".into()),
        messages: vec![Message::user_text(prompt)],
        tools: if tools {
            vec![ToolSpec {
                name: "current_time".into(),
                description: "Current time as seconds since the Unix epoch (UTC).".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            }]
        } else {
            Vec::new()
        },
        max_tokens: 2048,
        temperature: None,
    };
    eprintln!("--- {provider_id}/{model}");

    for _turn in 0..4 {
        let mut text = String::new();
        let mut calls = Vec::new();
        let mut usage = Usage::default();
        let mut stop = None;
        let mut thinking = false;
        llm::stream_chat(
            provider,
            &request,
            Arc::new(AtomicBool::new(false)),
            &mut |ev| match ev {
                StreamEvent::TextDelta(t) => {
                    if std::mem::take(&mut thinking) {
                        eprintln!();
                    }
                    print!("{t}");
                    let _ = std::io::stdout().flush();
                    text.push_str(&t);
                }
                StreamEvent::ThinkingDelta(t) => {
                    thinking = true;
                    eprint!("\x1b[2m{t}\x1b[0m");
                }
                StreamEvent::ToolUseStart { id, name } => eprintln!("\n[tool start {name} {id}]"),
                StreamEvent::ToolUseInputDelta { .. } => {}
                StreamEvent::ToolUse { id, name, input } => {
                    eprintln!("[tool call {name}({input})]");
                    calls.push((id, name, input));
                }
                StreamEvent::Usage(u) => usage = u,
                StreamEvent::Stop(reason) => stop = Some(reason),
            },
        )?;
        println!();
        eprintln!(
            "--- stop: {:?}; usage: in {} out {} cache_read {} cache_write {}",
            stop,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens
        );
        if stop != Some(StopReason::ToolUse) || calls.is_empty() {
            break;
        }
        let mut assistant = Vec::new();
        if !text.is_empty() {
            assistant.push(ContentBlock::Text(text));
        }
        let mut results = Vec::new();
        for (id, name, input) in calls {
            let (content, is_error) = match name.as_str() {
                "current_time" => {
                    let secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (secs.to_string(), false)
                }
                other => (format!("unknown tool {other}"), true),
            };
            eprintln!("[tool result {content}]");
            assistant.push(ContentBlock::ToolUse {
                id: id.clone(),
                name,
                input,
            });
            results.push(ContentBlock::ToolResult {
                tool_use_id: id,
                content,
                is_error,
            });
        }
        request.messages.push(Message {
            role: Role::Assistant,
            content: assistant,
        });
        request.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
    Ok(())
}
