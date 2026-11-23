# Harness — agent pipeline editor (GPUI demo)

Native Rust/GPUI port of the "Harness — редактор пайплайнов агентов" mockup: five screens plus the
shared sidebar, Graphite dark design tokens, mock data and the mockup's interactions.

## Run

```bash
nix-shell shell.nix --run 'cargo run'
```

Start on a specific screen: `HARNESS_SCREEN=chat|home|editor|inbox|issues|agents` (default `chat`).
Each screen also runs standalone: `cargo run -p screen_editor --example screen_editor` (same for
`screen_home`, `screen_review`, `screen_chat`, `screen_issues`, `screen_agents`, `sidebar`). `Ctrl+Q` quits.

`Ctrl+N` (or "New chat" in the sidebar) starts a new chat; `Ctrl+K` (or "Open a chat…", "Chats", "View all" in the sidebar) opens the chat history picker: search over
titles, messages, files and folders with `in:` `agent:` `is:` `has:files` chips, status filters, time
groups, preview, Open / Fork / Pin / Archive (with Undo), and Select mode for bulk archive, Markdown
export (to `~/Downloads`) and delete. Pins and archive live in `~/.local/share/harness/chat_flags.json`.
The button next to the workspace name (or `Ctrl+\`) switches the sidebar between wide and icon-only; the choice is saved in `~/.local/share/harness/sidebar`.
Home shows the dashboard (mock pipeline data; running and waiting chats are real).

## Pipelines

Pipelines are configurable (`crates/pipeline`): any number of them, each a TOML file in
`~/.config/harness/pipelines/<id>.toml`, edited on the Pipelines screen (New pipeline — blank,
copy or from a template; Add step; Workdirs). A pipeline has **workdirs** you define (folder,
a fresh git worktree per run or the folder as is, base branch, branch prefix, setup command,
env) and **steps** run top to bottom:

| Step | Does |
| --- | --- |
| Agent | a model works in a workdir (read only / read and run / full tools, skills, instructions); with sub-agents it orchestrates them via `delegate` |
| Decision | a model answers a yes/no question; "no" jumps to a later step or the end |
| Render | renders the changed HTML files of a workdir (before / after PNGs, headless Firefox) |
| Approval | waits for you (board or Inbox): approve, or request changes to go back to a step |
| Command | runs a shell command (tests, builds) |
| Merge | merges the run's branch of a worktree into its base |
| Pull request | commits, pushes, opens a (draft) PR and posts it to the Inbox |
| Report | posts a run summary to the Inbox |

The built-in "Mockup → GPUI build" pipeline: Triage (Haiku decision) → Mockup (designer agent
with ui-ux-pro-max) → Render → Your approval → Merge mockup → Build (Opus orchestrator with Sonnet
coders, verifies per `AGENTS.md`) → Pull request. Issues pick a pipeline when created; runs keep the
pipeline version they started with. Data: `~/.local/share/harness/` (`issues.json`, `runs/`).
Headless runner: `PIPELINE=<id> cargo run -p pipeline --example run_issue -- new "Title" "Body"`,
`… approve HAR-1`, `… changes HAR-1 "notes"`, `… retry HAR-1`.

## Inbox

Agents hand their finished work to the Inbox: the chat agent has a `report_to_inbox` tool
(title, Markdown summary, outcome done / needs review / blocked / failed, links) and is told to
call it at the end of a task. Pull requests opened in a run (`gh pr create`, `glab mr create`,
MCP `create_pull_request`) are added to that report, or posted as a report of their own when the
agent made none. Reports live in `~/.local/share/harness/inbox.json` and appear on the Inbox
screen under "From agents" (above the mock pipeline reviews): live PR status from `gh pr view`
(state, checks, review, diff size, branches), other links, the summary, Open chat / Open pull
request / Mark unread / Archive. The sidebar badge counts unread reports plus waiting reviews.

## Issues

The issue detail panel has an Activity section with comments (saved with the issue in
`issues.json`). Typing `@` in the comment box opens an autocomplete of the configured pipelines
(↑ ↓ move, ↵ / Tab insert, Esc close); mentions show as accent chips and `` `code` `` in mono.
Hover a comment to delete it. Mentions are recorded but don't start runs yet.

## Motion

Animations follow the Graphite design system's motion spec (`crates/ui/src/motion.rs`):
hover/selection colours on buttons, tabs, nav items, menu and list rows fade over 150ms;
popovers and menus come out of their trigger (180ms in, 4px), the ⌘K history picker opens
as a dialog with a fading scrim (220ms, 8px), toasts rise 8px (240ms), confirms revealed
inside an open popup slide in (180ms), and every exit is 100ms. Only opacity and position
move (gpui has no div transforms, so the 0.98 → 1 scale is left out); popups on the mock
screens animate in only. `HARNESS_REDUCE_MOTION=1` (or GNOME `enable-animations` off) turns
motion off; `HARNESS_MOTION_SLOW=10` plays everything 10× slower for checking by eye.

## Real chat (LLM over API)

The Chat screen is a working coding agent (crates `llm` + `agent_tools`): streaming replies, tool
calls (read_file, list_dir, grep auto; edit_file, write_file, bash ask first), Build/Plan modes,
`!cmd`, `/new /clear /model /plan /build`, @file mentions, real token/cost panel, sessions saved to
`~/.local/share/harness/sessions` and listed under Chats in the sidebar (Pinned, Recent; click to switch, `+` for a new chat). Providers come from `~/.config/harness/config.toml`:

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

Test hooks: `HARNESS_CHAT_PICKER_OPEN=1`, `HARNESS_CHAT_FORCE=skill-a,skill-b`; `HARNESS_CHAT_PROMPT="..."` sends a prompt in a fresh session on start; `HARNESS_CHAT_OPEN=<id>` opens a saved session; `HARNESS_CHAT_HISTORY=1` opens the history picker (`HARNESS_CHAT_HISTORY_Q="in:repo text"` pre-fills it).
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
