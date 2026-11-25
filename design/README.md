# design/

Static, editable HTML mockups of the Pipeliner ("Harness") app. They are:

1. the visual reference for engineers implementing the GPUI app, and
2. the source files the AI designer agent edits; each edit is re-rendered to PNG for human approval.

## Layout

```
design/
  tokens.css           design tokens (colors, spacing, radius, type, durations, easing)
  render.sh            render one HTML file to PNG with headless chromium
  screens/<slug>.html  home chat pipeline-editor sidebar board timeline review
                       task-thread agents memory policies settings
  explanation/<slug>.html
                       system-map prompt-stack how-things-link run-step-by-step
                       chat-modes where-you-change-what
  renders/             <slug>.png for screens, explanation-<slug>.png for explanations
```

Each HTML file is self-contained: one `<style>` block in the head (tokens first, then component
styles, then screen-specific styles) and plain markup in the body. The intended render size is in
`<meta name="design:size" content="WxH">`.

## Rules for editing

- Static HTML and CSS only. No `<script>`, no network assets (no web fonts, CDN, remote images,
  data-URI blobs). Fonts are the stacks `Inter, system-ui, sans-serif` and the monospace stack from the tokens.
- Keep the design tokens. Use `var(--...)` for colors, spacing, radius, type and motion; do not
  hard-code a value that has a token. Add a new token to `tokens.css` and to the inline `:root` block of the
  files that use it, never only one of them.
- Change only what the issue needs. Do not reformat, rename classes or reorder unrelated markup.
- Keep the file self-contained (CSS inline in the head) and keep `design:size` correct if the height changes.
- After every edit, re-render the PNG into `renders/` and look at it before asking for approval.
- Reference only: `--sidebar-width` is 232px in the tokens, but these mockups render the sidebar column
  at about 277px (pinned in `screens/*.html`). Implement against the token unless told otherwise.

## Render

One file (size comes from the `design:size` meta; W and H are optional overrides):

```
design/render.sh design/screens/board.html design/renders/board.png
design/render.sh design/explanation/system-map.html design/renders/explanation-system-map.png
design/render.sh design/screens/sidebar.html /tmp/out.png 232 800
```

It uses `chromium`, `chromium-browser` or `google-chrome` from PATH and falls back to
`nix-shell -p chromium --run ...`. It always unsets `LD_LIBRARY_PATH` for the browser. The
underlying command is:

```
nix-shell -p chromium --run "env -u LD_LIBRARY_PATH chromium --headless=new --disable-gpu --no-sandbox \
  --hide-scrollbars --force-device-scale-factor=1 --window-size=W,H --virtual-time-budget=5000 \
  --screenshot=OUT.png file://IN.html"
```
