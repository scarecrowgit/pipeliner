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

Pipeline steps get their own `CARGO_TARGET_DIR` per run (`~/.cache/pipeliner/runs/run-N/target`);
keep it, don't point it at a shared folder: parallel runs build different code under the same
crate names and break each other's builds. Never install or replace the app binary from an agent
(`~/.local/share/pipeliner/bin/pipeliner` is what the user runs). Don't run the GUI from an agent;
check logic with tests or the CLI (`cargo run -p harness-engine --bin pipeliner-cli -- help`).

## Look at what you changed

Tests don't see layout. After any UI change, take screenshots of the real app and look at
them with `read_file` (it shows PNGs to you; no OCR or other tricks needed), and include
what you saw in your report:

```sh
scripts/shoot.sh -o /tmp/shots                     # every screen
scripts/shoot.sh -o /tmp/shots board review:@review_pr
```

It runs the app headless (a private cage compositor, software Vulkan, grim): no window
appears on the user's screen and the user's data is never used. The data is a fixture from
`pipeliner-fixture` with deliberately awkward content: long titles, a 10k-line diff, a
minified one-line file, unbroken URLs, nested sub-agents, non-Latin text. `<out>/compare/`
has each screen next to its mockup render. One shot takes ~15 s.

Layout rules that the screenshots keep catching:

- Text in a flex row needs `min_w_0()` on itself or its parent, plus `ellipsis()` or a
  width, or a long word pushes the row (and the whole column) wider.
- Cut one line with `ellipsis()` from `theme::TextStyles`, never gpui's `truncate()`: in
  gpui 0.2.2 it keeps the first, unconstrained measurement, so text is clipped mid-letter
  with no "…". For several lines use `ui::Clamped::new(id, text, n)`: gpui's
  `line_clamp(n)` drops the extra lines (and its "…") silently.
- gpui-component's `Input` sets its height from its size class after your styles; pin it
  with `min_h`/`max_h` (see `ui::input::field`).
- A readable column in a wide window is `.w(px(N)).max_w_full()`, not
  `.w_full().max_w(px(N))` or a bare `max_w`: with only a cap, gpui sizes the column from
  its content, so inputs collapse to squares, wrapping rows grow tall and text runs past
  the edge. Check new screens at `scripts/shoot.sh -s 1920x1080` too.
- Plain `div` text can't be selected. Errors, commands and anything else worth copying go
  through `ui::selectable_text(id, text, window, cx)` (mono, keeps lines, Ctrl+C copies).
- Text that should wrap needs a width (`w_full()`/`flex_1()`); words without spaces never
  wrap. Clip or truncate long tokens (paths, URLs, hashes).
- Never render an unbounded list or diff in full: GPUI re-renders the whole view on every
  animation frame. Fold, paginate or virtualize (`uniform_list`/`list`) past a few hundred
  rows.
- Fixed-width inputs and stat tiles must not shrink below their content; give them
  `flex_none()` or a `min_w`.

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
