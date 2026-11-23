//! Shows the MCP servers the real config yields and the tools they offer:
//! `cargo run -p mcp --example mcp_probe -- [--workspace DIR] [--no-start]`
//!
//! Reads the app's `config.toml` (`$HARNESS_CONFIG` or `~/.config/harness/config.toml`), the
//! workspace's `.mcp.json` (the current directory by default) and, when imported,
//! `~/.claude.json`. Prints env keys but never their values, and hides secrets in args.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base::paths::Paths;
use model::workspace::McpState;

fn usage() -> ! {
    eprintln!("usage: mcp_probe [--workspace DIR] [--no-start]");
    std::process::exit(2);
}

fn main() {
    let mut workspace: Option<PathBuf> = None;
    let mut start = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" | "-w" => workspace = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage())),
            "--no-start" => start = false,
            _ => usage(),
        }
    }
    let workspace = workspace.or_else(|| std::env::current_dir().ok());
    let paths = Paths::from_env();
    println!("Config:    {}", paths.display(&paths.config_file));
    if let Some(ws) = &workspace {
        println!("Workspace: {}", paths.display(ws));
    }

    let loaded = mcp::load_server_configs(&paths, workspace.as_deref(), &|name| std::env::var(name).ok());
    for warning in &loaded.warnings {
        println!("warning: {warning}");
    }
    if loaded.servers.is_empty() {
        println!("No MCP servers configured.");
        return;
    }
    println!("\nServers:");
    for s in &loaded.servers {
        let off = if s.enabled { "" } else { " (disabled)" };
        println!("  {} [{}]{off}", s.name, s.source.label());
        let args: Vec<String> = s.args.iter().map(|a| s.redact(a)).collect();
        println!("    command: {} {}", s.command, args.join(" "));
        if !s.env.is_empty() {
            let keys: Vec<&str> = s.env.keys().map(String::as_str).collect();
            println!("    env keys: {}", keys.join(", "));
        }
        if let Some(cwd) = &s.cwd {
            println!("    cwd: {}", paths.display(cwd));
        }
        if !s.auto_approve.is_empty() {
            println!("    auto-approve: {}", s.auto_approve.join(", "));
        }
    }
    if !start {
        return;
    }

    let supervisor = mcp::Supervisor::start(loaded.servers, Arc::new(|| {}));
    let end = Instant::now() + Duration::from_secs(25);
    while supervisor.servers().iter().any(|s| s.state == McpState::Starting) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    let tools = supervisor.tools();
    for server in supervisor.servers() {
        println!("\n== {} ({:?}) ==", server.name, server.state);
        if let Some(error) = &server.error {
            println!("  {}", error.replace('\n', "\n  "));
        }
        for tool in tools.iter().filter(|t| t.server == server.name) {
            println!("  {} - {}", tool.id, base::fmt::first_line(&tool.spec.description));
        }
    }
    supervisor.shutdown();
}
