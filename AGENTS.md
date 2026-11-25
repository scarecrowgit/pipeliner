# Pipeliner — instructions for agents working on this repo

Pipeliner is a desktop harness for AI agent pipelines, written in Rust with GPUI.
This file is read by agents that improve the app itself (the "Design → GPUI build"
pipeline runs in a git worktree of this repo), so keep it accurate.

## Layout

```
crates/core     harness-core: domain model (model.rs), SQLite store + change feed (store.rs),
                config (config.rs), data paths (paths.rs), skills (skills.rs). No UI, no network.
crates/engine   harness-engine: LLM client (llm.rs), tools (tools.rs), agent loop (agent.rs),
                prompt stack (prompt.rs), chats (chat.rs), pipeline executor (pipeline.rs),
                git/gh (git.rs), HTML→PNG rendering (render.rs), starter templates (templates.rs),
                and a headless CLI (src/bin/pipeliner-cli.rs).
crates/app      pipeliner: the GPUI app. main.rs, theme.rs (design tokens), state.rs (AppState:
                engine handle, route, revision counter, toasts), shell.rs (sidebar + routing),
                ui/ (shared components), screens/ (one file per screen).
design/         Static HTML mockups (design/screens/*.html), tokens.css, renders/, render.sh.
                The source of truth for how the UI looks. See design/README.md.
vendor/xattr    Patched xattr 0.2.3 (libc ENOATTR removal). Leave it alone.
```

## Build, test, lint

Everything runs inside the nix shell (it provides cargo, clang, mold, wayland/x11 libs):

```sh
nix-shell shell.nix --run 'cargo check -p pipeliner'
nix-shell shell.nix --run 'cargo test --workspace'
nix-shell shell.nix --run 'cargo clippy --workspace -- -D warnings'
nix-shell shell.nix --run 'cargo build --release -p pipeliner'
```

Set `CARGO_TARGET_DIR` to a shared folder (pipelines do: `~/.cache/pipeliner/cargo-target`)
so worktrees don't rebuild GPUI from scratch. Don't run the GUI from an agent; check logic with
tests or the CLI (`cargo run -p harness-engine --bin pipeliner-cli -- help`).

## Rules

- No mock or sample data in the app. Every view reads the Store; when there is nothing, show an
  empty state. Starter templates contain structure and instructions only.
- UI colors, spacing, radii and type come from `crates/app/src/theme.rs` (mirrors design/tokens.css).
  Never hardcode hex colors in screens.
- Engine code never blocks the UI thread: the app calls `Engine` methods (they spawn threads) and
  re-renders on store changes (`AppState` bumps `revision`).
- Every state change goes through `Store::put_*` / `update_*` so views and the event log see it.
  Log user-visible happenings with `store.log(kind, run, task, text)`.
- Memory and Policies are not implemented in this release (screens say so). Don't fake them.
- Keep animations (route transitions, pulsing running dots, progress, toasts) when you touch UI.
- Prefer small focused changes in the existing style; add a unit test for logic you change.
- Don't commit or push from an agent step; the pipeline commits and opens the PR.

## Data on disk

- Config: `~/.config/pipeliner/config.toml` (`PIPELINER_CONFIG_DIR` overrides)
- Database, worktrees, renders: `~/.local/share/pipeliner/` (`PIPELINER_DATA_DIR` overrides)
