//! The issue's comments in the detail panel: the feed (`@mentions` as accent chips,
//! `` `code` `` in mono) and the comment box with its `@mention` autocomplete.
//! Mentionable names are the configured pipelines.

use gpui::{
    AnyElement, App, Context, Focusable, FontWeight, Hsla, IntoElement, SharedString, StyledText, TextRun,
    Window, deferred, div, font, prelude::*, px,
};
use pipeline::{Comment, Issue};
use ui::theme::*;
use ui::{Button, icon, kbd};

use crate::IssuesScreen;
use crate::comments::{self, Span};
use crate::widgets::{Ellipsis, small};

/// One row of the `@mention` list.
pub(crate) struct MentionRow {
    pub name: String,
    pub matched: std::ops::Range<usize>,
    pub sub: String,
    /// `running` / `this issue` / empty.
    pub status: &'static str,
}

impl IssuesScreen {
    /// Names that can be `@mentioned`: the configured pipelines.
    pub(crate) fn mention_names(&self, cx: &App) -> Vec<String> {
        self.pipes.read(cx).pipelines.iter().map(|p| p.name.clone()).collect()
    }

    /// The `@token` being typed in the comment box, if any.
    pub(crate) fn mention_token(&self, cx: &App) -> Option<(usize, String)> {
        comments::query(self.comment_input.read(cx).text()).map(|(at, t)| (at, t.to_string()))
    }

    /// Whether the `@mention` list shows (also with no matches: empty state).
    pub(crate) fn mention_open(&self, cx: &App) -> bool {
        !self.mention_dismissed && self.mention_token(cx).is_some()
    }

    pub(crate) fn mention_rows(&self, cx: &App) -> Vec<MentionRow> {
        let Some((_, q)) = self.mention_token(cx) else { return Vec::new() };
        let p = self.pipes.read(cx);
        let own = self
            .selected
            .as_ref()
            .and_then(|k| p.issue(k))
            .and_then(|i| p.pipeline_of(i))
            .map(|d| d.id.clone());
        let names = self.mention_names(cx);
        comments::matches(&q, &names)
            .into_iter()
            .map(|(ix, matched)| {
                let def = &p.pipelines[ix];
                let steps = def.steps.len();
                let mut sub = format!("Pipeline · {steps} {}", if steps == 1 { "step" } else { "steps" });
                if !def.description.trim().is_empty() {
                    sub = format!("{sub} · {}", def.description.trim());
                }
                let status = if own.as_deref() == Some(def.id.as_str()) {
                    "this issue"
                } else if p.active_in(&def.id) > 0 {
                    "running"
                } else {
                    ""
                };
                MentionRow { name: names[ix].clone(), matched, sub, status }
            })
            .collect()
    }

    pub(crate) fn move_mention(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.mention_open(cx) {
            return;
        }
        let n = self.mention_rows(cx).len();
        if n > 0 {
            self.mention_ix = (self.mention_ix as isize + delta).rem_euclid(n as isize) as usize;
        }
        cx.notify();
    }

    /// Replace the `@token` with `@Name ` and keep typing after it.
    pub(crate) fn insert_mention(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((at, _)) = self.mention_token(cx) else { return };
        let Some(r) = self.mention_rows(cx).into_iter().nth(row) else { return };
        let head = self.comment_input.read(cx).text()[..at].to_string();
        let text = format!("{head}@{} ", r.name);
        self.comment_input.update(cx, |i, cx| i.set_text(text, cx));
        self.mention_ix = 0;
        self.mention_dismissed = false;
        window.focus(&self.comment_input.focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn post_comment(&mut self, cx: &mut Context<Self>) {
        let text = self.comment_input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(key) = self.selected.clone() else { return };
        let mentions = comments::mentions(&text, &self.mention_names(cx));
        let res = self.pipes.update(cx, |p, cx| p.add_comment(&key, &text, mentions, cx));
        match res {
            Ok(()) => {
                self.comment_error = None;
                self.mention_ix = 0;
                self.mention_dismissed = false;
                self.comment_input.update(cx, |i, cx| i.set_text("", cx));
            }
            Err(e) => self.comment_error = Some(format!("Could not post: {e}").into()),
        }
        cx.notify();
    }

    fn delete_comment(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else { return };
        self.pipes.update(cx, |p, cx| p.delete_comment(&key, id, cx));
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────

    pub(crate) fn render_activity(&self, issue: &Issue, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.comment_input.read(cx);
        let empty = input.text().trim().is_empty();
        let focused = input.is_focused(window);
        let names = self.mention_names(cx);
        let now = ui::time::now_secs();
        let offset = ui::time::utc_offset(now);
        let count = match issue.comments.len() {
            0 => String::new(),
            1 => "1 comment".into(),
            n => format!("{n} comments"),
        };
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(12.))
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Activity"))
                    .child(small(count, TEXT_MUTED)),
            )
            .children(issue.comments.iter().map(|c| self.comment_item(c, &names, now, offset, cx)))
            .child(
                div()
                    .relative()
                    .children(self.render_mention_list(cx))
                    .child(
                        div()
                            .id("comment-box")
                            .flex()
                            .items_start()
                            .min_h(px(58.))
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(RADIUS_LG))
                            .border_1()
                            .border_color(if focused { ACCENT_STRONG } else { LINE_STRONG })
                            .when(focused, |d| d.shadow(selected_glow()))
                            .bg(SURFACE_RAISED)
                            .text_color(TEXT)
                            .cursor_text()
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.comment_input.focus_handle(cx));
                            }))
                            .child(self.comment_input.clone()),
                    ),
            )
            .when_some(self.comment_error.clone(), |d, e| d.child(small(e, DANGER)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().text_size(px(12.)).text_color(TEXT_MUTED).child("Type @ to mention a pipeline"))
                    .child(kbd("↵"))
                    .child(
                        Button::new("post-comment", "Comment")
                            .small()
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.post_comment(cx))),
                    ),
            )
    }

    fn comment_item(&self, c: &Comment, names: &[String], now: u64, offset: i64, cx: &mut Context<Self>) -> AnyElement {
        let id = c.id;
        let group: SharedString = format!("comment-{id}").into();
        let initials: String = c
            .author
            .split(|ch: char| !ch.is_alphanumeric())
            .filter_map(|w| w.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect();
        let avatar = div()
            .size(px(22.))
            .flex_none()
            .rounded_full()
            .bg(SURFACE_HOVER)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(9.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(TEXT_SECONDARY)
            .child(SharedString::from(initials));
        let when = format!("{} · comment", ui::time::when_label(c.created, now, offset));
        let header = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(TEXT).child(SharedString::from(c.author.clone())))
            .child(div().flex_1().ellipsis().text_color(TEXT_MUTED).child(SharedString::from(when)))
            .child(
                div()
                    .id(SharedString::from(format!("comment-delete-{id}")))
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .p(px(2.))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_comment(id, cx)))
                    .child(icon("x", 12., TEXT_MUTED)),
            );
        div()
            .id(group.clone())
            .group(group)
            .flex()
            .gap(px(10.))
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .child(header)
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(RADIUS_LG))
                            .border_1()
                            .border_color(LINE)
                            .bg(SURFACE)
                            .flex()
                            .flex_col()
                            .children(c.text.lines().map(|l| {
                                div().min_h(px(LINE_BODY)).child(rich_text(l, names, TEXT_SECONDARY))
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The `@mention` list above the comment box (nothing when closed).
    fn render_mention_list(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.mention_open(cx) {
            return None;
        }
        let (_, query) = self.mention_token(cx)?;
        let rows = self.mention_rows(cx);
        let total = self.pipes.read(cx).pipelines.len();
        let selected = self.mention_ix.min(rows.len().saturating_sub(1));
        let header = if total == 0 {
            "No pipelines to mention yet".to_string()
        } else if rows.is_empty() {
            format!("No pipeline matches “{query}”")
        } else if query.is_empty() {
            format!("Pipelines · {total}")
        } else {
            format!("Matching “{query}” · {} of {total}", rows.len())
        };
        let list = div()
            .id("mention-list")
            .absolute()
            .left_0()
            .right_0()
            .bottom_full()
            .mb(px(4.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE)
            .shadow(popover_shadow())
            .occlude()
            .child(div().px(px(8.)).pt(px(6.)).pb(px(4.)).text_size(px(12.)).text_color(TEXT_MUTED).child(header))
            .children(rows.into_iter().enumerate().map(|(ix, r)| self.mention_row(ix, r, ix == selected, cx)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(10.))
                    .gap_y(px(4.))
                    .mt(px(4.))
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .border_t_1()
                    .border_color(LINE_SUBTLE)
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(hint(vec![kbd("↑"), kbd("↓")], "move"))
                    .child(hint(vec![kbd("↵")], "insert"))
                    .child(hint(vec![kbd("esc")], "close")),
            );
        Some(
            deferred(ui::motion::pop_in("mention-list-in", ui::motion::Pop::Above, list))
                .with_priority(1)
                .into_any_element(),
        )
    }

    fn mention_row(&self, ix: usize, r: MentionRow, highlighted: bool, cx: &mut Context<Self>) -> AnyElement {
        let parts = [
            (r.name[..r.matched.start].to_string(), TEXT),
            (r.name[r.matched.clone()].to_string(), ACCENT_TEXT),
            (r.name[r.matched.end..].to_string(), TEXT),
        ];
        let status_color = if r.status == "running" { INFO } else { TEXT_MUTED };
        div()
            .id(("mention", ix))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(44.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .when(highlighted, |d| d.bg(SURFACE_HOVER))
            .hover(|s| s.bg(SURFACE_HOVER))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.mention_ix != ix {
                    this.mention_ix = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.insert_mention(ix, window, cx)))
            .child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(INFO_SOFT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("pipelines", 12., INFO)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(colored_text(&parts, FontWeight::MEDIUM))
                    .child(div().ellipsis().text_size(px(12.)).line_height(px(16.)).text_color(TEXT_MUTED).child(r.sub)),
            )
            .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).text_color(status_color).child(r.status))
            .into_any_element()
    }
}

/// `kbd` keys followed by a label.
fn hint(keys: Vec<gpui::Div>, label: &'static str) -> gpui::Div {
    div().flex().items_center().gap(px(4.)).children(keys).child(label)
}

fn text_run(len: usize, family: &str, weight: FontWeight, color: Hsla, bg: Option<Hsla>) -> TextRun {
    let mut f = font(family.to_string());
    f.weight = weight;
    TextRun { len, font: f, color, background_color: bg, underline: None, strikethrough: None }
}

/// One text made of coloured pieces (empty pieces are dropped).
fn colored_text(parts: &[(String, Hsla)], weight: FontWeight) -> StyledText {
    let parts: Vec<_> = parts.iter().filter(|(s, _)| !s.is_empty()).collect();
    let text: String = parts.iter().map(|(s, _)| s.as_str()).collect();
    let runs = parts.iter().map(|(s, c)| text_run(s.len(), FONT_SANS, weight, *c, None)).collect();
    StyledText::new(text).with_runs(runs)
}

/// Comment text: `@mentions` as accent chips, `code` in mono, the rest in `base`.
fn rich_text(text: &str, names: &[String], base: Hsla) -> StyledText {
    let mut out = String::new();
    let mut runs = Vec::new();
    for span in comments::spans(text, names) {
        let (range, run) = match span {
            Span::Text(r) => (r.clone(), text_run(r.len(), FONT_SANS, FontWeight::NORMAL, base, None)),
            Span::Mention(r, _) => {
                (r.clone(), text_run(r.len(), FONT_SANS, FontWeight::MEDIUM, ACCENT_TEXT, Some(ACCENT_SOFT)))
            }
            Span::Code(r) => (r.clone(), text_run(r.len(), FONT_MONO, FontWeight::NORMAL, TEXT, None)),
        };
        out.push_str(&text[range]);
        runs.push(run);
    }
    StyledText::new(out).with_runs(runs)
}
