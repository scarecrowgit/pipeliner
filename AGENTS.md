# Agent notes for Pipeliner (Harness)

Rust 2024 workspace, GPUI 0.2.2 desktop app: a harness for coding agents (chat, issues that run
through agent pipelines in git worktrees, an Inbox, an agent library, a pipeline editor, Home).
This branch (`recode`) is a from-scratch rewrite to the `screens-v2` design. The goal is an app
that is comfortable to edit with itself.

## Layers

Dependencies point down only. `crates/devtools/tests/layers.rs` enforces this.

| Layer | Crates | Rule |
| --- | --- | --- |
| Domain | `base`, `model`, `store`, `llm`, `mcp`, `tools`, `vcs`, `catalog`, `agent`, `issues`, `pipeline`, `engine`, `cli` | No `gpui`, no UI crates. Blocking code on the engine's own threads. |
| UI kit | `ui` (tokens, components, overlays, input), `ui_rich` (text area, markdown, diff, composer, charts, canvas) | No domain crates (`ui_rich` may use `base` for formatting). |
| Bridge | `hub` | Turns engine events into GPUI entities; owns the `Router` and the `Commands` handle. |
| Views | `shell`, `palette`, `screen_home`, `screen_chat`, `screen_inbox`, `screen_issues`, `screen_agents`, `screen_editor` | May use only `ui`, `ui_rich`, `hub`, `model`, `base`. Never another screen, never domain logic. |
| Binaries | `app` (`harness`), `cli` (`harness-cli`), `devtools` | Wiring only. |

`model` holds the plain data types every layer shares. Screens never define their own copy of a
run, session or issue; they render `model` types handed to them by `hub`.

## Build, test, lint

Cargo is only available inside the Nix shell, and stdin must be closed:

```bash
nix-shell shell.nix --run 'cargo build' </dev/null
nix-shell shell.nix --run 'cargo test --workspace' </dev/null
nix-shell shell.nix --run 'cargo clippy --workspace --all-targets' </dev/null
```

While iterating build one crate: `cargo test -p <crate>`. Domain crates build faster in their
own target dir, away from the GPUI builds:

```bash
nix-shell shell.nix --run 'CARGO_TARGET_DIR=target/domain cargo test -p <crate>' </dev/null
```

A change is done when the build is clean, tests pass and clippy has no warnings.

## Rules for subagents

You own the crates or modules named in your brief, nothing else.

- Edit only your files. If you need a change in another crate or in `model`, describe it in your
  report; do not make it.
- Do not commit, switch branches, or touch the root `Cargo.toml` or `Cargo.lock`. No `cargo clean`,
  no `cargo update`. Use only dependencies already listed in your crate's `Cargo.toml`; if you need
  another one, say so in the report.
- Build and test only your crate (`-p <crate>`). "Blocking waiting for file lock" is another agent
  compiling: wait.
- Do not open GUI windows unless your brief says so.
- Hand in: tests pass, clippy clean for your crate, and a report of at most 300 words: what is
  done, what is not, which contract changes you need.

## Conventions

- Keep modules under ~800 lines; split by area. Public items get a doc comment that says what
  the thing is for, not how it is implemented.
- Errors: `anyhow::Result` at boundaries, typed enums where the caller branches on the kind.
  No `unwrap`/`expect` outside tests unless the invariant is local and stated in a comment.
- No blocking IO on the UI thread. No domain logic in a view.
- Paths, ids, time and number formatting come from `base`. Do not re-implement them.
- Tests use temp directories (`tempfile`), in-memory databases and scripted fakes; they never
  touch `~/.config/harness` or `~/.local/share/harness`, and never call a real model.
- UI values come from `ui` tokens (`TEXT`, `SURFACE`, `LINE`, `SPACE_4`, `RADIUS_LG`…), never raw
  hex or ad-hoc pixel sizes for type. Labels are English, sentence case, with the exact wording of
  the mockups.
- Hover and selection colours fade (150ms); popovers, dialogs and toasts enter and exit through
  the overlay host. Follow `GRAPHITE.md`.

## References

- Mockups (the visual source of truth): `~/Pictures/designs/pipeliner-mockups/screens-v2/`.
  Read the generator sources, they are compact: `_build/lib.py` (shell, shared CSS, icons),
  `screens_a.py` (sidebar, home, chat), `screens_b.py` (inbox, issues, agents, editor),
  `screens_c.py` (overlays). Design system: `../GRAPHITE.md`, `../tokens.css`, `../Graphite/`.
- Old code, for hard-won logic only (do not copy its structure):
  `~/.harness/worktrees/pipeliner/legacy/` (tag `v1-legacy`).
- GPUI 0.2.2 source for API checks:
  `/mnt/storage/projects/rice/ash-fm/fm-handoff/fm/vendor/gpui/src`.
- Icons: `assets/icons/*.svg`, generated from the mockups' icon table (24 grid, 2px stroke).

## GPUI 0.2.2 quirks seen in this codebase

- `div()` is a flex row by default; use `.flex().flex_col()` for columns. Children of a column in a
  scroll container shrink unless `.flex_shrink_0()`.
- Text inside a shrink-to-fit or row container is measured at one-line width. For single-line
  ellipsis use the kit's truncate component, not ad-hoc styles.
- A `flex_wrap()` row makes its parent column measure wrapped text siblings at min-content width.
  Avoid `flex_wrap` on rows that share a column with wrapping text.
- Grid exists but only with equal tracks: `grid().grid_cols(n)` and `col_span(n)`. Use fixed
  widths or flex ratios for anything else.
- There are no element transforms: zoom by multiplying layout units, not by scaling.
- Element ids must be unique among siblings; use `("name", index)` tuples in lists.
- The background executor has one thread per core; long agent work never runs there.
