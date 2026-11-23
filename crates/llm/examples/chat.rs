//! Streams one answer to stdout using the real config:
//!
//! ```text
//! cargo run -p llm --example chat -- provider/model "prompt"
//! ```

use std::io::Write as _;

use base::cancel::CancelToken;
use base::paths::Paths;
use llm::{ChatRequest, Config, LlmClient, NoBlobs, Providers, StreamEvent};
use model::{Message, ModelRef};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(model), Some(prompt)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: chat <provider/model> <prompt>");
    };
    let client = Providers::new(Config::load(&Paths::from_env())?);
    let model = ModelRef::parse(&model);
    let request = ChatRequest { messages: vec![Message::user(prompt)], ..Default::default() };
    let mut stdout = std::io::stdout();
    let mut in_thinking = false;
    client.stream(&model, &request, &NoBlobs, &CancelToken::new(), &mut |event| {
        match event {
            StreamEvent::ThinkingDelta(t) => {
                if !in_thinking {
                    in_thinking = true;
                    let _ = write!(stdout, "[thinking] ");
                }
                let _ = write!(stdout, "{t}");
            }
            StreamEvent::TextDelta(t) => {
                if in_thinking {
                    in_thinking = false;
                    let _ = writeln!(stdout, "\n");
                }
                let _ = write!(stdout, "{t}");
            }
            StreamEvent::ToolUse { name, input, .. } => {
                let _ = writeln!(stdout, "\n[tool {name}] {input}");
            }
            StreamEvent::Usage(u) => eprintln!(
                "\n[usage] in {} out {} cache read {} write {}",
                u.input, u.output, u.cache_read, u.cache_write
            ),
            StreamEvent::Stop(reason) => {
                let _ = writeln!(stdout);
                eprintln!("[stop] {reason:?}");
            }
            _ => {}
        }
        let _ = stdout.flush();
    })?;
    Ok(())
}
