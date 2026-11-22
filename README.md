# Harness — agent pipeline editor (GPUI demo)

Native Rust/GPUI port of the "Harness — редактор пайплайнов агентов" mockup: five screens plus the
shared sidebar, Graphite dark design tokens, mock data and the mockup's interactions.

## Run

```bash
nix-shell shell.nix --run 'cargo run'
```

Start on a specific screen: `HARNESS_SCREEN=chat|editor|inbox|issues|agents` (default `chat`).
Each screen also runs standalone: `cargo run -p screen_editor --example screen_editor` (same for
`screen_review`, `screen_chat`, `screen_issues`, `screen_agents`, `sidebar`). `Ctrl+Q` quits.

## Real chat (LLM over API)

The Chat screen is a working coding agent (crates `llm` + `agent_tools`): streaming replies, tool
calls (read_file, list_dir, grep auto; edit_file, write_file, bash ask first), Build/Plan modes,
`!cmd`, `/new /clear /model /plan /build`, @file mentions, real token/cost panel, sessions saved to
`~/.local/share/harness/sessions` and listed under Chat in the sidebar (click to switch, "New session"). Providers come from `~/.config/harness/config.toml`:

```toml
default_model = "nr/cc/claude-opus-5-5"   # "<provider id>/<model id>" (split at the first '/')

[[providers]]
id = "nr"
kind = "openai"            # or "anthropic"
base_url = "http://host/v1"
api_key_env = "NR_API_KEY" # env var NAME; or api_key = "..."
```

Env auto-detect: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` (+`OPENAI_BASE_URL`), `OPENROUTER_API_KEY`,
Ollama on localhost. The agent works in `$HARNESS_WORKSPACE` or the current directory.
MCP servers (stdio): `[[mcp_servers]]` in `config.toml` (name, command, args, env, cwd, enabled,
`auto_approve = ["tool" | "*"]`), the project's `.mcp.json` (Claude Code format, `autoApprove`), and
`~/.claude.json` servers when `import_claude_code = true`. Tools appear as `mcp__<server>__<tool>`
and ask before running unless auto-approved; `/mcp` shows status. With more than 15 MCP tools
their schemas are deferred: the model sees only the names and loads definitions on demand with
`tool_search` (`select:mcp__server__tool` or keywords); loaded tools stay loaded for the session. Skills: `SKILL.md` folders from
`~/.claude/skills`, `~/.config/harness/skills`, `<workspace>/.claude/skills`; the model loads them with
the `skill` tool; `/skills` lists them. Test MCP server: `cargo run -p mcp --example echo_server`;
probe configs with `cargo run -p mcp --example mcp_probe`.

Agent picker ("Talk to"): Build / Plan (Tab) plus Claude Code subagents from `~/.claude/agents` and
`<workspace>/.claude/agents` (their prompt, `tools:` and exact-match `model:`). Its Skills section can
*force* skills: their full text is in every request of the session (chips next to the agent button,
"Skills in context" in the panel); other skills load on demand.

Test hooks: `HARNESS_CHAT_PICKER_OPEN=1`, `HARNESS_CHAT_FORCE=skill-a,skill-b`; `HARNESS_CHAT_PROMPT="..."` sends a prompt in a fresh session on start; `HARNESS_CHAT_OPEN=<id>` opens a saved session.
CLI check without the GUI: `cargo run -p llm --example chat -- [--tools] provider/model "prompt"`.

## Screens

| Screen | Crate | What works |
| --- | --- | --- |
| Pipeline editor | `screen_editor` | node canvas (drag by title, connect ports, zoom/Fit, add nodes), per-type inspector with live edits, simulated Test run with timeline/log/problems |
| Inbox · review | `screen_review` | queue, A/B/C variants with diff, Ship / Skip / Send back, keys J/K, 1–3, R, Esc |
| Chat session | `screen_chat` | transcript with tool calls, sub-agents, diff, permission prompt; composer with `/` and `@` popovers, `!` shell mode, Build⇄Plan (Tab), streamed replies |
| Issue board | `screen_issues` | board/list, filters, drag cards between columns, detail panel, status menu, runner picker with search, comments with @mentions |
| Agents & pipelines | `screen_agents` | filtered card grid, detail panel per kind, toggles, steppers, team edits, "New" menu |
| Sidebar | `sidebar` | full 232px / compact 56px rail (editor), navigation between screens |

## Layout

- `crates/ui` — tokens (`theme.rs`), components (`components.rs`), `TextInput`, navigation (`nav.rs`), app bootstrap.
- `crates/screen_*`, `crates/sidebar` — one crate per screen; `src/main.rs` — app shell.
- `assets/icons` — lucide-style stroke icons embedded via `rust-embed`.
- `reference/` — the mockup's markup and screenshots used for the port.

Known gaps: no multi-line text input (multi-line fields edit as a single line), no CSS-like letter
spacing, no pulse animations.
