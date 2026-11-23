//! Scrollable transcript: setup card / welcome, user messages, assistant turns (markdown,
//! tool calls, diffs, bash output, permission prompts), shell runs, errors and notices.

use agent_tools::skills::SKILL_TOOL;
use agent_tools::{DiffKind, DiffLine, ToolDisplay};
use gpui::{
    AnyElement, Context, ElementId, FontWeight, IntoElement, SharedString, div, prelude::*, px,
};
use serde_json::Value;
use ui::{Badge, Button, ButtonVariant, Status, Tone, icon, status_dot, theme::*};

use crate::agent::Decision;
use crate::data::{CONFIG_SNIPPET, ENV_VARS, Mode};
use crate::markdown;
use crate::models;
use crate::session::{
    Item, ShellRun, ToolCall, ToolResult, ToolStatus, Turn, TurnBlock, TurnState, UserMsg, truncate,
};
use crate::todo::{self, TODO_TOOL, TodoStatus};
use crate::widgets::*;
use crate::{ChatScreen, ConfigState};

/// Diff lines rendered per block at most.
const MAX_DIFF_LINES: usize = 400;
/// Tail of bash output rendered.
const MAX_OUTPUT_LINES: usize = 80;
/// Head of an expanded MCP result rendered.
const MAX_MCP_LINES: usize = 300;
const MAX_MCP_CHARS: usize = 20_000;

fn id(prefix: &str, key: impl std::fmt::Display) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}-{key}")))
}

/// Tool input as an object (some gateways send it JSON-encoded as a string).
fn input_obj(v: &Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        v => v.clone(),
    }
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn short_name(tool: &str) -> String {
    match tool {
        "read_file" => "read".into(),
        "list_dir" => "list".into(),
        "edit_file" => "edit".into(),
        "write_file" => "write".into(),
        TODO_TOOL => "todo".into(),
        crate::inbox::REPORT_TOOL => "inbox".into(),
        crate::deferred::TOOL_SEARCH => "tools".into(),
        other => other.into(),
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// What a call operates on, for its row.
fn arg_summary(call: &ToolCall) -> String {
    let display = call.result().and_then(|r| r.display.as_ref());
    match display {
        Some(ToolDisplay::Read { path, .. } | ToolDisplay::List { path, .. }) => {
            return path.clone();
        }
        Some(ToolDisplay::Diff { path, .. }) => return path.clone(),
        Some(ToolDisplay::Bash { command, .. }) => return first_line(command).to_string(),
        _ => {}
    }
    let Some(input) = call.input.as_ref().map(input_obj) else {
        return String::new();
    };
    match call.name.as_str() {
        "read_file" => {
            let path = str_field(&input, "path").unwrap_or_default();
            match input.get("offset").and_then(Value::as_u64) {
                Some(o) => format!("{path}:{o}"),
                None => path,
            }
        }
        "list_dir" => str_field(&input, "path")
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".".into()),
        "grep" => {
            let pattern = str_field(&input, "pattern").unwrap_or_default();
            match str_field(&input, "path").or_else(|| str_field(&input, "glob")) {
                Some(p) => format!("{pattern}  in {p}"),
                None => pattern,
            }
        }
        "edit_file" | "write_file" => str_field(&input, "path").unwrap_or_default(),
        "bash" => str_field(&input, "command")
            .map(|c| first_line(&c).to_string())
            .unwrap_or_default(),
        TODO_TOOL => match todo::parse(&input) {
            Ok(items) => format!("{} items", items.len()),
            Err(_) => "invalid list".into(),
        },
        SKILL_TOOL => str_field(&input, "name").unwrap_or_default(),
        crate::inbox::REPORT_TOOL => str_field(&input, "title").unwrap_or_default(),
        crate::deferred::TOOL_SEARCH => str_field(&input, "query").unwrap_or_default(),
        _ => truncate(&input.to_string(), 120),
    }
}

fn error_text(r: &ToolResult) -> String {
    let s = match &r.display {
        Some(ToolDisplay::Error { message }) => message.clone(),
        Some(ToolDisplay::Bash {
            timed_out: true, ..
        }) => "timed out".into(),
        Some(ToolDisplay::Bash {
            exit_code: Some(c), ..
        }) => format!("exit {c}"),
        _ => first_line(&r.content).to_string(),
    };
    truncate(&s, 90)
}

fn result_meta(call: &ToolCall, r: &ToolResult) -> String {
    match &r.display {
        Some(ToolDisplay::Read { lines, .. }) => format!("{lines} lines"),
        Some(ToolDisplay::List { entries, .. }) => format!("{entries} entries"),
        Some(ToolDisplay::Grep { matches, files, .. }) => {
            format!("{matches} matches · {files} files")
        }
        Some(ToolDisplay::Diff { added, removed, .. }) => format!("+{added} −{removed}"),
        Some(ToolDisplay::Bash { exit_code: Some(c), .. }) => format!("exit {c}"),
        Some(ToolDisplay::Bash { exit_code: None, .. }) => "done".into(),
        _ if call.name == TODO_TOOL => "updated".into(),
        _ if call.name == crate::inbox::REPORT_TOOL => "posted to Inbox".into(),
        _ if call.name == crate::deferred::TOOL_SEARCH => {
            let n = r.content.lines().filter(|l| l.starts_with("## ")).count();
            format!("loaded {n} tool{}", if n == 1 { "" } else { "s" })
        }
        _ if call.name == SKILL_TOOL => format!("loaded · {} lines", r.content.lines().count()),
        _ => "done".into(),
    }
}

/// Unified diff lines; a gap marker where line numbers jump (new hunk).
fn diff_body(lines: &[DiffLine], dim: bool) -> gpui::Div {
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut prev_new = 0usize;
    for l in lines.iter().take(MAX_DIFF_LINES) {
        if l.kind != DiffKind::Del {
            if prev_new != 0 && l.line_no > prev_new + 1 {
                rows.push(
                    div()
                        .px(px(12.))
                        .text_color(TEXT_DISABLED)
                        .child("⋯")
                        .into_any_element(),
                );
            }
            prev_new = l.line_no;
        }
        let (bg, fg, sign) = match l.kind {
            DiffKind::Add => (DIFF_ADD_BG, DIFF_ADD_TEXT, "+"),
            DiffKind::Del => (DIFF_DEL_BG, DIFF_DEL_TEXT, "−"),
            DiffKind::Context => (gpui::transparent_black(), TEXT_SECONDARY, " "),
        };
        rows.push(
            div()
                .flex()
                .gap(px(10.))
                .px(px(12.))
                .bg(bg)
                .text_color(fg)
                .child(
                    div()
                        .w(px(32.))
                        .flex_none()
                        .text_right()
                        .text_color(TEXT_MUTED)
                        .child(l.line_no.to_string()),
                )
                .child(div().w(px(10.)).flex_none().child(sign))
                .child(div().flex_1().min_w_0().child(l.text.clone()))
                .into_any_element(),
        );
    }
    if lines.len() > MAX_DIFF_LINES {
        rows.push(
            div()
                .px(px(12.))
                .text_color(TEXT_MUTED)
                .child(format!("… {} more lines", lines.len() - MAX_DIFF_LINES))
                .into_any_element(),
        );
    }
    div()
        .flex()
        .flex_col()
        .py(px(6.))
        .bg(BG_SIDEBAR)
        .font_family(FONT_MONO)
        .text_size(px(12.))
        .line_height(px(20.))
        .overflow_hidden()
        .when(dim, |d| d.opacity(0.6))
        .children(rows)
}

/// Full text of an MCP result, capped (mono, wraps).
fn mcp_body(text: &str, is_error: bool) -> gpui::Div {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let mut shown: String = lines
        .iter()
        .take(MAX_MCP_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let mut more = lines.len().saturating_sub(MAX_MCP_LINES);
    if shown.len() > MAX_MCP_CHARS {
        let mut cut = MAX_MCP_CHARS;
        while !shown.is_char_boundary(cut) {
            cut -= 1;
        }
        more += shown[cut..].lines().count();
        shown.truncate(cut);
    }
    div()
        .px(px(12.))
        .py(px(8.))
        .border_t_1()
        .border_color(LINE_SUBTLE)
        .bg(BG_SIDEBAR)
        .font_family(FONT_MONO)
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(if is_error { DANGER } else { TEXT_SECONDARY })
        .child(shown)
        .when(more > 0, |d| {
            d.child(
                div()
                    .text_color(TEXT_MUTED)
                    .child(format!("… {more} more lines")),
            )
        })
}

/// Last `MAX_OUTPUT_LINES` of command output.
fn output_body(output: &str) -> gpui::Div {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let skip = lines.len().saturating_sub(MAX_OUTPUT_LINES);
    let text = lines[skip..].join("\n");
    div()
        .overflow_hidden()
        .px(px(12.))
        .py(px(8.))
        .border_t_1()
        .border_color(LINE_SUBTLE)
        .bg(BG_SIDEBAR)
        .font_family(FONT_MONO)
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(TEXT_SECONDARY)
        .when(skip > 0, |d| {
            d.child(
                div()
                    .text_color(TEXT_MUTED)
                    .child(format!("… {skip} earlier lines")),
            )
        })
        .child(ui::selectable::plain(text, "\n\n"))
}

fn exit_meta(display: Option<&ToolDisplay>, is_error: bool) -> gpui::Div {
    let (text, color) = match display {
        Some(ToolDisplay::Bash {
            timed_out: true, ..
        }) => ("timed out".to_string(), DANGER),
        Some(ToolDisplay::Bash {
            exit_code: Some(c), ..
        }) => (
            format!("exit {c}"),
            if *c == 0 && !is_error {
                SUCCESS
            } else {
                DANGER
            },
        ),
        _ if is_error => ("failed".into(), DANGER),
        _ => ("done".into(), SUCCESS),
    };
    row_meta(text, color)
}

fn bash_output(r: &ToolResult) -> String {
    match &r.display {
        Some(ToolDisplay::Bash { output, .. }) => output.clone(),
        _ => r.content.clone(),
    }
}

/// How a tool call is drawn.
enum Look {
    Row,
    Diff,
    Bash,
    Todo(Vec<todo::TodoItem>),
    Ask,
    /// MCP call: `(server, tool)`.
    Mcp((String, String)),
}

fn look(call: &ToolCall, mcp: Option<(String, String)>) -> Look {
    if call.status == ToolStatus::Asking {
        return Look::Ask;
    }
    if let Some(target) = mcp {
        return Look::Mcp(target);
    }
    if call.name == TODO_TOOL
        && let Some(Ok(items)) = call.input.as_ref().map(todo::parse)
        && call.result().is_some_and(|r| !r.is_error)
    {
        return Look::Todo(items);
    }
    match call.result() {
        Some(r) if !r.is_error && matches!(r.display, Some(ToolDisplay::Diff { .. })) => Look::Diff,
        Some(r) if matches!(r.display, Some(ToolDisplay::Bash { .. })) => Look::Bash,
        _ => Look::Row,
    }
}

impl ChatScreen {
    fn look(&self, call: &ToolCall) -> Look {
        look(call, self.mcp_label(&call.name))
    }

    pub(crate) fn render_transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.selection.borrow_mut().begin_frame();
        let build = || {
            if self.items.is_empty() {
                vec![self.empty_state(cx)]
            } else {
                self.item_elements(cx)
            }
        };
        let items = ui::selectable::scope(&self.selection, build);
        div()
            .id("transcript")
            .track_focus(&self.transcript_focus)
            // Runs after the text blocks' own handlers: a press that no block took clears.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selection.borrow_mut().press_outside() {
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                if (k.modifiers.control || k.modifiers.platform) && k.key == "a" {
                    this.selection.borrow_mut().select_all();
                    cx.notify();
                    cx.stop_propagation();
                } else if (k.modifiers.control || k.modifiers.platform) && k.key == "c" {
                    if let Some(text) = this.selection.borrow().selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                }
            }))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(24.))
            .child(
                div()
                    .w_full()
                    .max_w(px(780.))
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    // flex_none: a flex column taller than the viewport would otherwise shrink
                    // its items (squashing e.g. a two-line user bubble to one line).
                    .children(
                        items
                            .into_iter()
                            .map(|el| div().flex().flex_col().flex_shrink_0().min_w_0().child(el)),
                    ),
            )
    }

    // ── Empty state ─────────────────────────────────────────────────────

    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.config {
            ConfigState::Loading => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_color(TEXT_MUTED)
                .child(spinner("config-spin", 14., TEXT_MUTED))
                .child("Loading providers…")
                .into_any_element(),
            ConfigState::Failed(e) => self.setup_card(Some(e.clone()), cx),
            ConfigState::Ready(c) if c.providers.is_empty() => self.setup_card(None, cx),
            ConfigState::Ready(_) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .pt(px(40.))
                .items_center()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(TEXT)
                        .child(format!(
                            "Ask {} about {}",
                            self.agent_name(),
                            self.repo_name()
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child("@ attaches a file · / for commands · ! runs a shell command · Tab switches Build ⇄ Plan"),
                )
                .into_any_element(),
        }
    }

    fn setup_card(&self, error: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let path = llm::Config::path().display().to_string();
        block()
            .px(px(16.))
            .py(px(14.))
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(tile(
                        "policy",
                        24.,
                        RADIUS_MD,
                        14.,
                        ACCENT_SOFT,
                        ACCENT_TEXT,
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(TEXT)
                            .child("Connect an LLM provider"),
                    ),
            )
            .children(error.map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(DANGER)
                    .child(format!("Could not load the config: {e}"))
            }))
            .child(para(rich(vec![
                Seg::text(
                    "Harness talks to models over their APIs. Add providers to ",
                    TEXT_SECONDARY,
                ),
                Seg::mono(path, TEXT),
                Seg::text(":", TEXT_SECONDARY),
            ])))
            .child(markdown::code_block("toml", CONFIG_SNIPPET))
            .child(para(
                "Or set one of these environment variables before starting Harness:",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .children(ENV_VARS.iter().map(|(var, what)| {
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(
                                div()
                                    .w(px(170.))
                                    .flex_none()
                                    .font_family(FONT_MONO)
                                    .text_color(ACCENT_TEXT)
                                    .child(*var),
                            )
                            .child(div().text_color(TEXT_MUTED).child(*what))
                    })),
            )
            .child(
                div().flex().child(
                    Button::new("reload-config", "Reload config")
                        .small()
                        .primary()
                        .icon("replay")
                        .on_click(cx.listener(|this, _, _, cx| this.load_config(cx))),
                ),
            )
            .into_any_element()
    }

    // ── Items ───────────────────────────────────────────────────────────

    fn item_elements(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let n = self.items.len();
        let is_turn = |i: Option<&Item>| matches!(i, Some(Item::Turn(_)));
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                ui::selectable::set_prefix(i.to_string());
                match item {
                Item::User(u) => self.user_item(i, u),
                Item::Shell(s) => self.shell_item(i, s, cx),
                Item::Turn(t) => {
                    let first = i == 0 || !is_turn(self.items.get(i - 1));
                    let last = !is_turn(self.items.get(i + 1));
                    self.turn_item(i, t, first, last, cx)
                }
                Item::Error(e) => self.error_card(e, i + 1 == n, cx),
                Item::Notice(text) => div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(text.clone())
                    .into_any_element(),
                Item::Divider(d) => divider(&d.text),
                }
            })
            .collect()
    }

    fn user_item(&self, ix: usize, u: &UserMsg) -> AnyElement {
        // ~7px per char at 13px Inter; 85% of the 780px column ≈ 640px of text.
        let wide = u.text.lines().any(|l| l.chars().count() * 7 > 600)
            || u.text.contains('\n')
            || u.images.len() > 1;
        user_bubble(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(!u.images.is_empty(), |d| {
                    d.child(
                        div().flex().flex_wrap().gap(px(8.)).mb(px(2.)).children(
                            u.images.iter().enumerate().map(|(i, a)| {
                                ui::images::image_card(id(&format!("msg-img-{ix}"), i), a, 200., 120.)
                            }),
                        ),
                    )
                })
                .when(!u.text.is_empty(), |d| d.child(user_text(&u.text, TEXT)))
                .children(u.attachments.iter().map(|a| {
                    let (meta, color) = match (&a.content, &a.error) {
                        (_, Some(e)) => (truncate(e, 60), DANGER),
                        (Some(_), None) => (format!("{} lines", a.lines), TEXT_MUTED),
                        (None, None) => ("reading…".into(), TEXT_MUTED),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child(icon("file", 12., TEXT_MUTED))
                        .child(div().font_family(FONT_MONO).child(a.path.clone()))
                        .child(div().text_color(color).child(format!("· {meta}")))
                })),
            wide,
        )
        .into_any_element()
    }

    fn shell_item(&self, ix: usize, s: &ShellRun, _cx: &mut Context<Self>) -> AnyElement {
        let row = tool_row(
            "shell",
            format!("$ {}", s.command),
            match &s.output {
                None => running_meta(id("shell-spin", ix), "running…"),
                Some(r) => exit_meta(r.display.as_ref(), r.is_error),
            },
        );
        let output = s
            .output
            .as_ref()
            .map(bash_output)
            .filter(|o| !o.trim().is_empty());
        block()
            .child(row.border_b_0())
            .children(output.map(|o| output_body(&o)))
            .into_any_element()
    }

    fn error_card(&self, message: &str, last: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(RADIUS_XL))
            .bg(DANGER_SOFT)
            .border_1()
            .border_color(DANGER.opacity(0.35))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_dot(Status::Error, false))
                    .child(
                        div()
                            .text_color(DANGER)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Request failed"),
                    ),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_SECONDARY)
                    .child(truncate(message, 2000)),
            )
            .when(last, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            Button::new("retry", "Retry")
                                .small()
                                .icon("replay")
                                .disabled(self.busy())
                                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                        )
                        .child(
                            Button::new("error-reload", "Reload config")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.load_config(cx))),
                        ),
                )
            })
            .into_any_element()
    }

    // ── Assistant turns ─────────────────────────────────────────────────

    fn turn_item(
        &self,
        ix: usize,
        t: &Turn,
        first: bool,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let streaming = t.state == TurnState::Streaming;
        let agent_name = t.agent.clone().unwrap_or_else(|| t.mode.name().to_string());
        let badge =
            Badge::new(agent_name.clone()).tone(if t.mode == Mode::Build && t.agent.is_none() {
                Tone::Accent
            } else {
                Tone::Neutral
            });

        // Body: text blocks and tool calls in order; consecutive compact rows share a block.
        let mut body: Vec<AnyElement> = Vec::new();
        let mut rows: Vec<AnyElement> = Vec::new();
        let flush = |rows: &mut Vec<AnyElement>, body: &mut Vec<AnyElement>| {
            if !rows.is_empty() {
                body.push(block().children(std::mem::take(rows)).into_any_element());
            }
        };
        for b in &t.blocks {
            match b {
                TurnBlock::Text(tb) => {
                    if tb.text.trim().is_empty() {
                        continue;
                    }
                    flush(&mut rows, &mut body);
                    body.push(markdown::render(&tb.md));
                }
                TurnBlock::Tool(call) => match self.look(call) {
                    Look::Mcp(target) if call.result().is_some() => {
                        flush(&mut rows, &mut body);
                        body.push(self.mcp_block(call, &target, cx));
                    }
                    Look::Mcp(target) => rows.push(self.mcp_row(call, &target).into_any_element()),
                    Look::Row => rows.push(self.tool_row_el(call).into_any_element()),
                    other => {
                        flush(&mut rows, &mut body);
                        body.push(match other {
                            Look::Diff => self.diff_block(call, cx),
                            Look::Bash => self.bash_block(call, cx),
                            Look::Todo(items) => todo_block(&items),
                            Look::Ask => self.permission_card(call, t.mode, cx),
                            Look::Row | Look::Mcp(_) => unreachable!(),
                        });
                    }
                },
            }
        }
        flush(&mut rows, &mut body);

        let thinking = (!t.thinking.trim().is_empty()).then(|| {
            let open = t.thinking_open;
            let text: String = {
                let s = t.thinking.trim();
                let skip = s.chars().count().saturating_sub(4000);
                s.chars().skip(skip).collect()
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .id(id("thinking", ix))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .cursor_pointer()
                        .hover(|s| s.text_color(TEXT_SECONDARY))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(Item::Turn(t)) = this.items.get_mut(ix) {
                                t.thinking_open = !t.thinking_open;
                                cx.notify();
                            }
                        }))
                        .child(icon(
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            12.,
                            TEXT_MUTED,
                        ))
                        .child("Thinking")
                        .when(streaming && t.blocks.is_empty(), |d| {
                            d.child(spinner(id("think-spin", ix), 12., TEXT_MUTED))
                        }),
                )
                .when(open, |d| {
                    d.child(
                        div()
                            .pl(px(18.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(TEXT_MUTED)
                            .child(text),
                    )
                })
        });

        let waiting = streaming && t.blocks.is_empty() && t.thinking.trim().is_empty();
        let footer = match t.state {
            TurnState::Interrupted => Some(("Interrupted · Esc".to_string(), TEXT_MUTED)),
            TurnState::Failed => Some(("Failed · partial output not sent".to_string(), DANGER)),
            TurnState::Done if last && !self.busy() && t.usage != llm::Usage::default() => Some((
                format!(
                    "{} · {} · in {} · out {}",
                    agent_name,
                    t.model,
                    models::fmt_tokens(
                        t.usage.input_tokens
                            + t.usage.cache_read_tokens
                            + t.usage.cache_write_tokens
                    ),
                    models::fmt_tokens(t.usage.output_tokens)
                ),
                TEXT_MUTED,
            )),
            _ => None,
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .when(first, |d| d.child(agent_line(badge, t.model.clone())))
            .children(thinking)
            .when(waiting, |d| {
                d.child(running_meta(id("wait-spin", ix), "Thinking…").text_size(px(13.)))
            })
            .children(body)
            .children(footer.map(|(text, color)| {
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(color)
                    .child(text)
            }))
            .into_any_element()
    }

    fn tool_row_el(&self, call: &ToolCall) -> gpui::Div {
        let meta = match &call.status {
            ToolStatus::Streaming => running_meta(
                id("tool-spin", &call.id),
                if call.partial > 0 {
                    format!("writing… {}", fmt_bytes(call.partial))
                } else {
                    "preparing…".into()
                },
            ),
            ToolStatus::Queued => row_meta("queued", TEXT_MUTED),
            ToolStatus::Asking => row_meta("waiting for you", WARNING),
            ToolStatus::Running => running_meta(id("tool-spin", &call.id), "running…"),
            ToolStatus::Cancelled => row_meta("cancelled", TEXT_MUTED),
            ToolStatus::Rejected(_) => row_meta("rejected by you", DANGER),
            ToolStatus::Done(r) if r.is_error => row_meta(error_text(r), DANGER),
            ToolStatus::Done(r) => row_meta(result_meta(call, r), TEXT_MUTED),
        };
        tool_row(short_name(&call.name), arg_summary(call), meta)
    }

    /// `server · tool ……… status` while an MCP call is pending.
    fn mcp_row(&self, call: &ToolCall, (server, tool): &(String, String)) -> gpui::Div {
        let meta = match &call.status {
            ToolStatus::Done(_) | ToolStatus::Asking | ToolStatus::Queued => {
                row_meta("queued", TEXT_MUTED)
            }
            ToolStatus::Streaming => running_meta(id("tool-spin", &call.id), "preparing…"),
            ToolStatus::Running => running_meta(id("tool-spin", &call.id), "running…"),
            ToolStatus::Cancelled => row_meta("cancelled", TEXT_MUTED),
            ToolStatus::Rejected(_) => row_meta("rejected by you", DANGER),
        };
        mcp_head(server, tool, div().flex_1(), meta)
    }

    /// Finished MCP call: `server · tool  first line of the result  ›`, click to expand.
    fn mcp_block(
        &self,
        call: &ToolCall,
        (server, tool): &(String, String),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(r) = call.result() else {
            return self
                .mcp_row(call, &(server.clone(), tool.clone()))
                .into_any_element();
        };
        let text = r.content.trim();
        let first = first_line(text);
        let expandable = text.lines().nth(1).is_some() || first.chars().count() > 80;
        let open = call.open && expandable;
        let tool_id = call.id.clone();
        let summary = ellipsis_cell(if first.is_empty() {
            "(no output)".to_string()
        } else {
            first.to_string()
        })
        .font_family(FONT_MONO)
        .text_color(if r.is_error { DANGER } else { TEXT_SECONDARY });
        let chevron = if expandable {
            icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                14.,
                TEXT_MUTED,
            )
            .into_any_element()
        } else {
            div().into_any_element()
        };
        block()
            .child(
                mcp_head(server, tool, summary, chevron)
                    .border_b_0()
                    .id(id("mcp-toggle", &call.id))
                    .when(expandable, |d| {
                        d.cursor_pointer().hover(|s| s.bg(SURFACE_RAISED)).on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_tool(&tool_id, cx)),
                        )
                    }),
            )
            .when(open, |d| d.child(mcp_body(text, r.is_error)))
            .into_any_element()
    }

    fn diff_block(&self, call: &ToolCall, cx: &mut Context<Self>) -> AnyElement {
        let Some(ToolResult {
            display:
                Some(ToolDisplay::Diff {
                    path,
                    added,
                    removed,
                    lines,
                    created,
                }),
            ..
        }) = call.result()
        else {
            return self.tool_row_el(call).into_any_element();
        };
        let open = call.open;
        let tool_id = call.id.clone();
        let head = div()
            .id(id("diff-toggle", &call.id))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .min_h(px(36.))
            .px(px(12.))
            .py(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|s| s.bg(SURFACE_RAISED))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(&tool_id, cx)))
            .child(tool_name(short_name(&call.name)))
            .child(
                ellipsis_cell(path.clone())
                    .font_family(FONT_MONO)
                    .text_color(TEXT_SECONDARY),
            )
            .when(*created, |d| {
                d.child(div().text_color(TEXT_MUTED).child("new file"))
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(6.))
                    .font_family(FONT_MONO)
                    .child(div().text_color(SUCCESS).child(format!("+{added}")))
                    .child(div().text_color(DANGER).child(format!("−{removed}"))),
            )
            .child(icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                14.,
                TEXT_MUTED,
            ));
        block()
            .child(head)
            .when(open && !lines.is_empty(), |d| {
                d.child(
                    diff_body(lines, false)
                        .border_t_1()
                        .border_color(LINE_SUBTLE),
                )
            })
            .into_any_element()
    }

    fn bash_block(&self, call: &ToolCall, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = call.result() else {
            return self.tool_row_el(call).into_any_element();
        };
        let output = bash_output(r);
        let has_output = !output.trim().is_empty();
        let open = call.open;
        let tool_id = call.id.clone();
        block()
            .child(
                tool_row(
                    "bash",
                    arg_summary(call),
                    exit_meta(r.display.as_ref(), r.is_error),
                )
                .border_b_0()
                .id(id("bash-toggle", &call.id))
                .when(has_output, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(SURFACE_RAISED))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(&tool_id, cx)))
                }),
            )
            .when(open && has_output, |d| d.child(output_body(&output)))
            .into_any_element()
    }

    fn toggle_tool(&mut self, tool_id: &str, cx: &mut Context<Self>) {
        for item in &mut self.items {
            if let Item::Turn(t) = item
                && let Some(c) = t.tool_mut(tool_id)
            {
                c.open = !c.open;
                cx.notify();
                return;
            }
        }
    }

    /// Inline permission prompt for an `Ask` tool, with the call's preview.
    fn permission_card(&self, call: &ToolCall, mode: Mode, cx: &mut Context<Self>) -> AnyElement {
        let input = call.input.as_ref().map(input_obj).unwrap_or(Value::Null);
        let path = str_field(&input, "path").unwrap_or_default();
        let mcp_target = self.mcp_label(&call.name);
        let what = match (call.name.as_str(), &call.preview, &mcp_target) {
            (_, _, Some((server, tool))) => format!("call {tool} on the MCP server {server}"),
            ("edit_file", _, _) => format!("edit {path}"),
            ("write_file", Some(ToolDisplay::Diff { created: true, .. }), _) => {
                format!("create {path}")
            }
            ("write_file", _, _) => format!("write {path}"),
            ("bash", _, _) => "run a command".into(),
            (other, _, _) => format!("use {other}"),
        };
        let preview: AnyElement = match (&call.preview, &mcp_target) {
            (_, Some((server, tool))) => mcp_args(server, tool, &input),
            (
                Some(ToolDisplay::Diff {
                    lines,
                    added,
                    removed,
                    ..
                }),
                None,
            ) => div()
                .flex()
                .flex_col()
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(LINE)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(6.))
                        .bg(BG)
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .text_color(TEXT_SECONDARY)
                                .child(path.clone()),
                        )
                        .child(div().text_color(SUCCESS).child(format!("+{added}")))
                        .child(div().text_color(DANGER).child(format!("−{removed}"))),
                )
                .child(diff_body(lines, false))
                .into_any_element(),
            (Some(ToolDisplay::Bash { command, .. }), None) => div()
                .px(px(12.))
                .py(px(8.))
                .rounded(px(RADIUS_LG))
                .bg(BG)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(TEXT)
                .child(format!("$ {command}"))
                .into_any_element(),
            (Some(ToolDisplay::Error { message }), None) => div()
                .text_size(px(12.))
                .text_color(DANGER)
                .child(format!("Preview failed: {message}"))
                .into_any_element(),
            _ => div()
                .px(px(12.))
                .py(px(8.))
                .rounded(px(RADIUS_LG))
                .bg(BG)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(TEXT_SECONDARY)
                .child(truncate(&input.to_string(), 600))
                .into_any_element(),
        };
        let listener = |d: Decision| {
            let tool_id = call.id.clone();
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, window, cx| {
                this.answer(&tool_id, d, cx);
                this.focus_input(window, cx);
            })
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(RADIUS_XL))
            .bg(WARNING_SOFT)
            .border_1()
            .border_color(hex(0x4a3f17))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_dot(Status::Waiting, false))
                    .child(
                        div()
                            .text_color(WARNING)
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{} wants to {what}", mode.name())),
                    ),
            )
            .child(preview)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(BG)
                    .text_size(px(12.))
                    .text_color(TEXT)
                    .child(div().flex_1().h(px(18.)).child(self.note_input.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new(id("allow-once", &call.id), "Allow once")
                            .small()
                            .primary()
                            .on_click(listener(Decision::Once)),
                    )
                    .child(
                        Button::new(
                            id("allow-always", &call.id),
                            match &mcp_target {
                                Some((server, tool)) => {
                                    format!("Always allow {server} · {tool} for this session")
                                }
                                None => format!("Always allow {} for this session", call.name),
                            },
                        )
                        .small()
                        .on_click(listener(Decision::Always)),
                    )
                    .child(
                        Button::new(id("reject", &call.id), "Reject")
                            .small()
                            .variant(ButtonVariant::Danger)
                            .on_click(listener(Decision::Reject)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child("policy:")
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .child(format!("{} = ask", call.name)),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Row head of an MCP call: mono `server · tool`, then `middle`, then `meta`.
fn mcp_head(
    server: &str,
    tool: &str,
    middle: impl IntoElement,
    meta: impl IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .w_full()
        .min_h(px(36.))
        .px(px(12.))
        .py(px(6.))
        .border_b_1()
        .border_color(LINE_SUBTLE)
        .text_size(px(12.))
        .child(
            div()
                .flex_none()
                .max_w(px(260.))
                .truncate()
                .whitespace_nowrap()
                .font_family(FONT_MONO)
                .child(rich(vec![
                    Seg::mono(server.to_string(), TEXT_MUTED),
                    Seg::mono(" · ", TEXT_DISABLED),
                    Seg::mono(tool.to_string(), TEXT_SECONDARY),
                ])),
        )
        .child(middle)
        .child(meta)
}

/// Permission preview of an MCP call: server, tool and pretty-printed arguments.
fn mcp_args(server: &str, tool: &str, input: &Value) -> AnyElement {
    let args = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    let lines: Vec<&str> = args.lines().collect();
    let more = lines.len().saturating_sub(60);
    let shown = truncate(&lines[..lines.len().min(60)].join("\n"), 6000);
    div()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(LINE)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .gap(px(6.))
                .px(px(12.))
                .py(px(6.))
                .bg(BG)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .child(div().text_color(TEXT_MUTED).child("server"))
                .child(div().text_color(TEXT).child(server.to_string()))
                .child(div().ml(px(8.)).text_color(TEXT_MUTED).child("tool"))
                .child(div().text_color(TEXT).child(tool.to_string())),
        )
        .child(
            div()
                .px(px(12.))
                .py(px(8.))
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .bg(BG_SIDEBAR)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(TEXT_SECONDARY)
                .child(shown)
                .when(more > 0, |d| {
                    d.child(
                        div()
                            .text_color(TEXT_MUTED)
                            .child(format!("… {more} more lines")),
                    )
                }),
        )
        .into_any_element()
}

/// System divider: `──── 📁 Moved to x · clean on main ────`.
fn divider(text: &str) -> AnyElement {
    let rule = || div().flex_1().min_w(px(24.)).h(px(1.)).bg(LINE);
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(TEXT_MUTED)
        .child(rule())
        .child(ui::icon("folder", 12., TEXT_MUTED))
        .child(
            div()
                .flex_none()
                .max_w(gpui::relative(0.8))
                .child(ui::selectable::text(gpui::StyledText::new(text.to_string()), "\n\n")),
        )
        .child(rule())
        .into_any_element()
}

fn todo_block(items: &[todo::TodoItem]) -> AnyElement {
    block()
        .px(px(12.))
        .py(px(10.))
        .gap(px(6.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(div().font_family(FONT_MONO).child("todowrite"))
                .child(format!(
                    "{} of {} done",
                    todo::done_count(items),
                    items.len()
                )),
        )
        .children(items.iter().map(|t| {
            let done = t.status == TodoStatus::Completed;
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(status_dot(todo_status(t.status), true))
                .child(
                    div()
                        .text_color(if done { TEXT_MUTED } else { TEXT })
                        .when(done, |d| d.line_through())
                        .child(t.content.clone()),
                )
        }))
        .into_any_element()
}

fn fmt_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.)
    }
}
