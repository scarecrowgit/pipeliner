//! Static data for the chat screen: primary agents (Build / Plan), slash commands.

use gpui::Hsla;
use ui::theme::*;

/// Primary agent mode. Plan only offers read-only tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Build,
    Plan,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Build => "Build",
            Mode::Plan => "Plan",
        }
    }

    pub fn desc(self) -> &'static str {
        match self {
            Mode::Build => "Full access · edits and bash ask first",
            Mode::Plan => "Read-only · investigates and writes a plan",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Mode::Build => Mode::Plan,
            Mode::Plan => Mode::Build,
        }
    }
}

pub const MODES: [Mode; 2] = [Mode::Build, Mode::Plan];

/// Visual style of the agent selector (the mockup's "agent" kind).
pub struct KindStyle {
    pub icon: &'static str,
    pub label: &'static str,
    pub tile_bg: Hsla,
    pub tile_fg: Hsla,
    pub bg: Hsla,
    pub border: Hsla,
}

pub const AGENT_STYLE: KindStyle = KindStyle {
    icon: "agent",
    label: "agent",
    tile_bg: SURFACE_HOVER,
    tile_fg: TEXT_SECONDARY,
    bg: SURFACE_RAISED,
    border: LINE_STRONG,
};

/// Build: the accent variant of the agent selector.
pub const BUILD_STYLE: KindStyle = KindStyle {
    icon: "agent",
    label: "agent",
    tile_bg: ACCENT_SOFT,
    tile_fg: ACCENT_TEXT,
    bg: ACCENT_SOFT,
    border: ACCENT_RING,
};

pub struct Cmd {
    pub name: &'static str,
    pub desc: &'static str,
    pub key: &'static str,
}

const fn cmd(name: &'static str, desc: &'static str, key: &'static str) -> Cmd {
    Cmd { name, desc, key }
}

pub const CMDS: &[Cmd] = &[
    cmd("new", "Start a new session", ""),
    cmd("clear", "Clear this session's messages", ""),
    cmd("model", "Switch provider and model", ""),
    cmd("plan", "Switch to Plan (read-only)", "Tab"),
    cmd("build", "Switch to Build (full access)", "Tab"),
    cmd("skills", "List available skills", ""),
    cmd("mcp", "MCP servers, status and tools", ""),
];

/// Env vars the llm crate auto-detects, for the setup card.
pub const ENV_VARS: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "Anthropic"),
    (
        "OPENAI_API_KEY",
        "OpenAI (optional OPENAI_BASE_URL for compatible gateways)",
    ),
    ("OPENROUTER_API_KEY", "OpenRouter"),
    (
        "OLLAMA_HOST",
        "Ollama (or a server on localhost:11434, no key)",
    ),
];

pub const CONFIG_SNIPPET: &str = "default_model = \"anthropic/claude-sonnet-5-5\"

[[providers]]
id = \"anthropic\"
kind = \"anthropic\"
base_url = \"https://api.anthropic.com\"
api_key_env = \"ANTHROPIC_API_KEY\"";
