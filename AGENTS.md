# Agent notes for Pipeliner (Harness)

Rust 2024 workspace, GPUI 0.2.2 desktop app. The binary is `src/main.rs` (app shell: sidebar +
screens); everything else lives in `crates/`:

| Crate | What |
| --- | --- |
| `ui` | Design tokens (`theme.rs`), components (`Button`, `Badge`, `Tabs`, `Stat`…), text input, navigation (`nav.rs`), motion (`motion.rs`), time helpers |
| `sidebar` | App sidebar and compact rail |
| `screen_home`, `screen_chat`, `screen_review` (Inbox), `screen_issues` (board), `screen_agents`, `screen_editor` | One crate per screen; each has `examples/<crate>.rs` for a standalone preview |
| `llm`, `agent_tools`, `mcp`, `pipeline` | Model client, coding tools, MCP client, the agent pipeline |

## Build, test, lint

Cargo is only available inside the Nix shell, and stdin must be closed:

```bash
nix-shell shell.nix --run 'cargo build' </dev/null
nix-shell shell.nix --run 'cargo test --workspace' </dev/null
nix-shell shell.nix --run 'cargo clippy --workspace --all-targets' </dev/null
```

Build one crate with `-p <crate>` while iterating; finish with the three commands above. A change
is done when the build is clean, tests pass and clippy has no warnings.

## Conventions

- Follow the style of the file you edit; keep modules under ~800 lines (split by area).
- UI values come from `ui::theme` tokens (`TEXT`, `SURFACE`, `LINE`, `SPACE_4`, `RADIUS_LG`…),
  never raw hex. Labels are English, sentence case.
- Hover/selection colours fade via `ui::motion` (`hover_fade`, `fade_row`); popups use
  `ui::motion::presence` / `pop_in`.
- Navigation between screens goes through `ui::nav` (`navigate`, `request_chat`…).

## GPUI 0.2.2 quirks seen in this codebase

- `div()` is a flex row by default; use `.flex().flex_col()` for columns. Children of a column in a
  scroll container shrink unless `.flex_shrink_0()`.
- Text inside a shrink-to-fit or row container is measured at one-line width. For single-line
  ellipsis use an absolutely positioned box inside a `relative().flex_1().min_w_0()` cell
  (see `ellipsis_cell` in `crates/screen_chat/src/widgets.rs`) or `line_clamp(1)`.
- A `flex_wrap()` row makes its parent column measure wrapped text siblings at min-content width
  (the column ends up too tall, leaving a gap). Avoid `flex_wrap` on rows that share a column with
  wrapping text, or give the row a fixed layout.
- There is no CSS grid; emulate columns with fixed widths or flex ratios.
- Element ids must be unique among siblings; use `("name", index)` tuples in lists.
