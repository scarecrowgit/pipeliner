# Pipeliner

A desktop harness for AI agent pipelines, built with Rust and [GPUI](https://gpui.rs).

Start in a chat, turn good prompts into agents and good plans into pipelines, watch every
run on the Board and Timeline, and step in where it matters: steer an agent mid-task, pick
between variants at a gate, approve the pull request at the end.

## What's in it

- **Chat** — one composer, four modes: *Direct* (you and a model), *Agent* (a saved agent with
  its role, tools and skills), *Orchestrator* (writes a plan, you approve, it delegates to
  sub-agents), *Pipeline* (starts a saved pipeline; also `/run <pipeline> <task>` anywhere).
- **Pipelines** — a graph of nodes: Source, Decision (an LLM answers yes/no), Router (rules),
  Agent step (with fan-out into N variants, each in its own git worktree, and an optional
  `delegate` sub-agent), Render (HTML mockups → PNG), Gate (you pick / approve / send back),
  Tool step (shell command, pull request, merge). Edits are drafts until you deploy a new version.
- **Review queue** — everything waiting for you: variants side by side with renders, diffs and
  cost; pick, send back with a note (it becomes prompt layer 6 on the retry), or reject.
- **Board / Timeline / Task threads** — every chat, step and spawned sub-agent is a task; open
  any task's thread to read its tool calls and steer it while it runs.
- **Agents** — the library of saved roles (prompt, model, tool access, skills).
- **Home** — spend, runs, decisions and live activity from the event log.
- **Settings** — providers (any OpenAI-compatible endpoint), models, workspace rules (prompt
  layer 1), budgets, render command, and updating the app itself.

Memory and Policies are designed (see `design/`) but not implemented in this release.

## Improving itself

The **Design → GPUI build** template points at this repository: triage decides whether the
mockups in `design/screens/` need to change; if so two designer variants are rendered for you
to pick; an orchestrator and a coder implement the change in a worktree; a pull request opens
for your approval, and approving merges it. *Settings › Update app* pulls `main`, builds a
release and restarts into it. Agents follow [AGENTS.md](AGENTS.md).

## Build

```sh
nix-shell shell.nix --run 'cargo run --release -p pipeliner'
```

Headless driver for scripts and tests: `cargo run -p harness-engine --bin pipeliner-cli`.

Screenshots of every screen without a display, on generated test data: `scripts/shoot.sh -o /tmp/shots`
(see AGENTS.md). `PIPELINER_ROUTE=review` opens the app on a screen; `PIPELINER_STILL=1` shows a data dir
without starting the engine.

Configuration lives in `~/.config/pipeliner/config.toml`; data (SQLite, worktrees, renders) in
`~/.local/share/pipeliner/`.

## Launcher

The release binary runs outside nix-shell (runtime library paths are baked in). Install it to
`~/.local/share/pipeliner/bin/pipeliner` (what the launcher and Settings → Update & restart use, so
other builds never replace the running app) and add a menu entry:

```sh
export CARGO_TARGET_DIR=~/.cache/pipeliner/cargo-target
nix-shell shell.nix --run 'cargo build --release -p pipeliner -p harness-engine'
$CARGO_TARGET_DIR/release/pipeliner-cli install
sed -e "s|@BIN@|$HOME/.local/share/pipeliner/bin/pipeliner|" \
    -e "s|@ICON@|$PWD/packaging/pipeliner.svg|" \
    packaging/pipeliner.desktop > ~/.local/share/applications/pipeliner.desktop
```
