//! Inspect configured MCP servers:
//! `cargo run -p mcp --example mcp_probe -- [--workspace DIR] [server-name]`
//!
//! Loads the configs (Harness config = `$HARNESS_CONFIG` or `~/.config/harness/config.toml`),
//! lists the servers (env KEYS only, never values), starts the selected server (or every
//! enabled one), prints its tools and shuts it down.

use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

fn harness_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_CONFIG") {
        return p.into();
    }
    let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
    });
    base.join("harness").join("config.toml")
}

fn usage() -> ! {
    eprintln!("usage: mcp_probe [--workspace DIR] [server-name]");
    std::process::exit(2);
}

fn main() -> anyhow::Result<()> {
    let mut workspace: Option<PathBuf> = None;
    let mut selected: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" | "-w" => workspace = Some(args.next().unwrap_or_else(|| usage()).into()),
            "-h" | "--help" => usage(),
            s if s.starts_with('-') => usage(),
            s if selected.is_none() => selected = Some(s.to_string()),
            _ => usage(),
        }
    }
    let workspace = match workspace {
        Some(w) => w,
        None => std::env::current_dir()?,
    };
    let harness = harness_config_path();
    println!("Harness config: {}", harness.display());
    println!("Workspace:      {}", workspace.display());

    let (servers, warnings) = mcp::load_server_configs(&harness, &workspace, &mcp::claude_json_path())?;
    for w in &warnings {
        println!("warning: {w}");
    }
    if servers.is_empty() {
        println!("No MCP servers configured.");
        return Ok(());
    }
    println!("\nServers:");
    for s in &servers {
        println!(
            "  {} [{:?}]{}",
            s.name,
            s.source,
            if s.enabled { "" } else { " (disabled)" }
        );
        println!("    command: {} {}", s.command, s.args.join(" "));
        if !s.env.is_empty() {
            let keys: Vec<_> = s.env.keys().map(String::as_str).collect();
            println!("    env keys: {}", keys.join(", "));
        }
        if let Some(cwd) = &s.cwd {
            println!("    cwd: {}", cwd.display());
        }
        if !s.auto_approve.is_empty() {
            println!("    auto-approve: {}", s.auto_approve.join(", "));
        }
    }

    let to_start: Vec<_> = match &selected {
        Some(name) => match servers.iter().find(|s| &s.name == name) {
            Some(s) => vec![s],
            None => anyhow::bail!("no MCP server named `{name}`"),
        },
        None => servers.iter().filter(|s| s.enabled).collect(),
    };
    for cfg in to_start {
        println!("\n== {} ==", cfg.name);
        let client = match mcp::Client::start(cfg, TIMEOUT) {
            Ok(c) => c,
            Err(e) => {
                println!("  failed to start: {e:#}");
                continue;
            }
        };
        let (name, version) = client.server_info();
        println!("  server: {name} {version}");
        match client.list_tools(TIMEOUT) {
            Ok(tools) => {
                println!("  {} tool(s):", tools.len());
                for t in tools {
                    let first = t.description.lines().next().unwrap_or_default();
                    println!("    {} ({}) - {first}", t.name, mcp::tool_id(&cfg.name, &t.name));
                }
            }
            Err(e) => println!("  tools/list failed: {e:#}"),
        }
        client.shutdown();
    }
    Ok(())
}
