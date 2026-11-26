//! The composer: mode selector and its settings, the message box (Enter sends, Shift+Enter
//! adds a line), `/run <pipeline>` and `@agent` suggestions, Send / Stop.

use std::path::PathBuf;

use gpui::{
    img, Animation, AnimationExt as _, ClipboardEntry, Div, ExternalPaths, Focusable as _, Hsla,
    ObjectFit, PathPromptOptions, StyledImage as _,
};
use gpui_component::input::{IndentInline, MoveDown, MoveUp, Paste, Position};
use gpui_component::tooltip::Tooltip;
use harness_core::{Attachment, ChatMode, Status};

use super::messages::mode_blurb;
use super::{eid, ChatScreen};
use crate::screens::prelude::*;

gpui::actions!(
    chat_composer,
    [Submit, SuggestUp, SuggestDown, SuggestAccept, PasteImage]
);

/// Images per message.
pub(super) const MAX_ATTACHMENTS: usize = 5;

/// How many more images fit next to `n` attached ones.
pub(super) fn remaining_slots(n: usize) -> usize {
    MAX_ATTACHMENTS.saturating_sub(n)
}

/// "1 image", "2 images".
pub(super) fn count_noun(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Token counts the way the design shows them: `850`, `1.4k`, `12k`.
pub(super) fn tokens_short(t: u64) -> String {
    if t < 1000 {
        return t.to_string();
    }
    let tenths = (t + 50) / 100;
    if tenths >= 100 || tenths.is_multiple_of(10) {
        format!("{}k", tenths / 10)
    } else {
        format!("{}.{}k", tenths / 10, tenths % 10)
    }
}

/// Under a thumbnail: `≈ 1.4k`, or `pasted · ≈ 0.8k`.
pub(super) fn attachment_meta(a: &Attachment) -> String {
    let t = format!("≈ {}", tokens_short(a.tokens()));
    if a.pasted {
        format!("pasted · {t}")
    } else {
        t
    }
}

/// In the composer bar: `2 images · ≈ 2.2k tokens`.
fn images_summary(n: usize, tokens: u64) -> String {
    format!(
        "{} · ≈ {} tokens",
        count_noun(n, "image"),
        tokens_short(tokens)
    )
}

/// Under the message box: what the images add to each reply.
fn images_cost_label(model: &str, tokens: u64, cost: f64) -> String {
    if cost > 0.0 {
        let price = if cost < 0.01 {
            "under $0.01".to_string()
        } else {
            format!("≈ {}", fmt_usd(cost))
        };
        if model.is_empty() {
            format!("images {price} per reply")
        } else {
            format!("{model} · images {price} per reply")
        }
    } else {
        format!("≈ {} tokens of images per reply", tokens_short(tokens))
    }
}

/// One autocomplete entry.
pub(super) struct Suggestion {
    pub label: String,
    pub detail: String,
    /// The whole composer text after accepting.
    pub replacement: String,
}

impl ChatScreen {
    /// `/run` → the command; `/run bu` → matching pipelines; `@fi` → matching agents.
    pub(super) fn suggestions(&self, text: &str) -> Vec<Suggestion> {
        let mut out = vec![];
        if let Some(rest) = text.strip_prefix("/run ") {
            if rest.contains(char::is_whitespace) {
                return out;
            }
            let q = harness_engine::git::slug(rest);
            for p in &self.pickers.pipelines {
                let slug = harness_engine::git::slug(&p.name);
                if slug.starts_with(&q) || p.name.to_lowercase().starts_with(&rest.to_lowercase()) {
                    out.push(Suggestion {
                        label: slug.clone(),
                        detail: p.name.clone(),
                        replacement: format!("/run {slug} "),
                    });
                }
            }
        } else if text.starts_with('/') && !text.contains(char::is_whitespace) {
            if "/run".starts_with(text) {
                out.push(Suggestion {
                    label: "/run".into(),
                    detail: "start a pipeline with the rest as its input".into(),
                    replacement: "/run ".into(),
                });
            }
        } else if let Some(rest) = text.strip_prefix('@') {
            if rest.contains(char::is_whitespace) {
                return out;
            }
            let q = rest.to_lowercase();
            // The engine reads one word after `@`: names with spaces can't be addressed.
            for a in self
                .pickers
                .agents
                .iter()
                .filter(|a| !a.name.contains(char::is_whitespace))
            {
                if a.name.to_lowercase().starts_with(&q) {
                    out.push(Suggestion {
                        label: format!("@{}", a.name),
                        detail: if a.description.is_empty() {
                            super::data::short_model(&a.model)
                        } else {
                            a.description.clone()
                        },
                        replacement: format!("@{} ", a.name),
                    });
                }
            }
        }
        out.truncate(6);
        out
    }

    fn current_suggestions(&self, cx: &App) -> Vec<Suggestion> {
        let text = self.composer.read(cx).value();
        self.suggestions(&text)
    }

    fn accept_suggestion(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.current_suggestions(cx).into_iter().nth(ix) else {
            return;
        };
        let chars = s.replacement.chars().count() as u32;
        self.composer.update(cx, |i, cx| {
            i.set_value(s.replacement, window, cx);
            i.set_cursor_position(Position::new(0, chars), window, cx);
        });
        self.suggest_ix = 0;
        cx.notify();
    }

    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.current_suggestions(cx).is_empty() {
            self.send(window, cx);
        } else {
            self.accept_suggestion(self.suggest_ix, window, cx);
        }
    }

    fn on_suggest_up(&mut self, _: &SuggestUp, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.current_suggestions(cx).len();
        if n == 0 {
            window.dispatch_action(Box::new(MoveUp), cx);
        } else {
            self.suggest_ix = (self.suggest_ix + n - 1) % n;
            cx.notify();
        }
    }

    fn on_suggest_down(&mut self, _: &SuggestDown, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.current_suggestions(cx).len();
        if n == 0 {
            window.dispatch_action(Box::new(MoveDown), cx);
        } else {
            self.suggest_ix = (self.suggest_ix + 1) % n;
            cx.notify();
        }
    }

    fn on_suggest_accept(
        &mut self,
        _: &SuggestAccept,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current_suggestions(cx).is_empty() {
            window.dispatch_action(Box::new(IndentInline), cx);
        } else {
            self.accept_suggestion(self.suggest_ix, window, cx);
        }
    }

    // ---- images ----------------------------------------------------------------

    /// How many more images the next message can take (counting ones still being added).
    fn free_slots(&self) -> usize {
        remaining_slots(self.attachments.len() + self.adding)
    }

    fn warn_full(cx: &mut App) {
        state::toast(
            ToastKind::Warning,
            format!("Up to {MAX_ATTACHMENTS} images per message"),
            cx,
        );
    }

    /// Runs `job` on a background thread and attaches the image it saves.
    fn spawn_add(
        &mut self,
        job: impl FnOnce(&harness_engine::Engine) -> anyhow::Result<Attachment> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.adding += 1;
        cx.notify();
        let engine = state::engine(cx);
        let task = cx.background_spawn(async move { job(&engine) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.adding -= 1;
                match res {
                    Ok(a) => this.push_attachment(a, cx),
                    Err(e) => state::toast(ToastKind::Danger, format!("{e:#}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push_attachment(&mut self, a: Attachment, cx: &mut Context<Self>) {
        if self.attachments.len() >= MAX_ATTACHMENTS {
            Self::discard(a.path, cx);
            Self::warn_full(cx);
        } else {
            self.attachments.push(a);
        }
    }

    /// Deletes a never-sent image file in the background.
    fn discard(path: String, cx: &mut App) {
        state::run_engine(cx, move |e| {
            e.discard_attachment(&path);
            Ok(())
        })
        .detach();
    }

    fn remove_attachment(&mut self, path: &str, cx: &mut Context<Self>) {
        let before = self.attachments.len();
        self.attachments.retain(|a| a.path != path);
        if self.attachments.len() != before {
            Self::discard(path.to_string(), cx);
        }
        cx.notify();
    }

    /// Adds image files (picked or dropped), as many as fit.
    fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let free = self.free_slots();
        if free == 0 {
            Self::warn_full(cx);
            return;
        }
        if paths.len() > free {
            state::toast(
                ToastKind::Info,
                format!(
                    "Only {} more fit: the rest were skipped",
                    count_noun(free, "image")
                ),
                cx,
            );
        }
        for path in paths.into_iter().take(free) {
            self.spawn_add(move |e| e.add_attachment_file(&path), cx);
        }
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        if self.free_slots() == 0 {
            Self::warn_full(cx);
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add image".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }

    /// Ctrl/Cmd-V in the message box: an image on the clipboard becomes an attachment,
    /// anything else pastes as text.
    fn on_paste_image(&mut self, _: &PasteImage, window: &mut Window, cx: &mut Context<Self>) {
        let image = cx.read_from_clipboard().and_then(|item| {
            if item.text().is_some_and(|t| !t.is_empty()) {
                return None;
            }
            item.into_entries().find_map(|e| match e {
                ClipboardEntry::Image(i) => Some(i),
                _ => None,
            })
        });
        match image {
            Some(image) => {
                if self.free_slots() == 0 {
                    Self::warn_full(cx);
                    return;
                }
                let bytes = image.bytes;
                self.spawn_add(move |e| e.save_attachment_bytes(&bytes, None, true), cx);
            }
            None => window.dispatch_action(Box::new(Paste), cx),
        }
    }

    /// Sends the composer text and images. With no chat open, creates one with the draft
    /// settings first. While the chat's task runs, the engine turns the message into steering.
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.adding > 0 {
            state::toast(ToastKind::Info, "Still adding an image, one moment", cx);
            return;
        }
        let attachments = std::mem::take(&mut self.attachments);
        self.composer
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.feedback = false;
        self.force_bottom = true;
        self.scroll.scroll_to_bottom();
        cx.notify();

        let chat_id = self.detail.as_ref().map(|d| d.chat.id.clone());
        let draft = self.draft.clone();
        cx.spawn(async move |this, cx| {
            let id = match chat_id {
                Some(id) => id,
                None => {
                    let created = cx.update(|cx| {
                        state::run_engine(cx, move |e| {
                            let mut c = e.new_chat(draft.mode)?;
                            draft.apply(&mut c);
                            if c.model.is_empty() {
                                c.model = e.config().default_model;
                            }
                            e.store().put_chat(&c)?;
                            Ok(c.id)
                        })
                    });
                    let Ok(task) = created else { return };
                    match task.await {
                        Some(id) => {
                            let _ = this.update(cx, |this, cx| {
                                state::replace_route(
                                    Route::Chat {
                                        chat: Some(id.clone()),
                                    },
                                    cx,
                                );
                                this.select(Some(id.clone()), cx);
                            });
                            id
                        }
                        None => {
                            let _ = this.update(cx, |this, cx| {
                                this.restore_draft(text, attachments, cx);
                            });
                            return;
                        }
                    }
                }
            };
            let body = text.clone();
            let sent = attachments.clone();
            let Ok(task) =
                cx.update(|cx| state::run_engine(cx, move |e| e.send_with(&id, &body, sent)))
            else {
                return;
            };
            if task.await.is_none() {
                let _ = this.update(cx, |this, cx| {
                    this.restore_draft(text, attachments, cx);
                });
            }
        })
        .detach();
    }

    /// A send failed: put the text and images back.
    fn restore_draft(
        &mut self,
        text: String,
        attachments: Vec<Attachment>,
        cx: &mut Context<Self>,
    ) {
        self.restore = Some(text);
        let room = remaining_slots(self.attachments.len());
        self.attachments
            .splice(0..0, attachments.into_iter().take(room));
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self
            .detail
            .as_ref()
            .and_then(|d| d.task.as_ref())
            .map(|t| t.id.clone())
        else {
            return;
        };
        state::run_engine(cx, move |e| e.cancel_task(&task)).detach();
    }

    /// Placeholder for the composer, by mode and settings.
    pub(super) fn placeholder_text(&self) -> SharedString {
        let s = self.settings();
        let text = match s.mode {
            ChatMode::Direct => {
                let m = super::data::short_model(&s.model);
                if m.is_empty() {
                    "Message the model…".to_string()
                } else {
                    format!("Message {m}…")
                }
            }
            ChatMode::Agent => match self.agent_name(s.agent_id.as_ref()) {
                Some(a) => format!("Ask @{a}…"),
                None => "Pick an agent, or start with @name…".into(),
            },
            ChatMode::Orchestrator => "Describe the task; the orchestrator plans it first…".into(),
            ChatMode::Pipeline => match self.pipeline_name(s.pipeline_id.as_ref()) {
                Some(p) => format!("Describe the task for {p}…"),
                None => "Pick a pipeline, or type /run <pipeline> …".into(),
            },
        };
        text.into()
    }

    /// "Message the Orchestrator", "Message Fixer", ...
    fn target_label(&self) -> String {
        let s = self.settings();
        match s.mode {
            ChatMode::Direct => {
                let m = super::data::short_model(&s.model);
                if m.is_empty() {
                    "Message the model".into()
                } else {
                    format!("Message {m}")
                }
            }
            ChatMode::Agent => format!(
                "Message {}",
                self.agent_name(s.agent_id.as_ref()).unwrap_or("the agent")
            ),
            ChatMode::Orchestrator => "Message the Orchestrator".into(),
            ChatMode::Pipeline => match self.pipeline_name(s.pipeline_id.as_ref()) {
                Some(p) => format!("Start {p}"),
                None => "Start a pipeline".into(),
            },
        }
    }

    pub(super) fn render_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let s = self.settings();
        let status = self.task_status();
        let running = status == Some(Status::Running);
        let waiting = status == Some(Status::Waiting);
        let text = self.composer.read(cx).value();
        let has_text = !text.trim().is_empty();
        let has_input = has_text || !self.attachments.is_empty();
        let suggestions = self.suggestions(&text);
        let suggest_ix = self.suggest_ix.min(suggestions.len().saturating_sub(1));

        let modes: Vec<&str> = ChatMode::ALL.iter().map(|m| m.label()).collect();
        let mode_ix = ChatMode::ALL.iter().position(|m| *m == s.mode).unwrap_or(0);
        let weak = cx.entity().downgrade();
        let selector = ui::segmented("chat-mode", &modes, mode_ix, move |ix, _, cx| {
            let _ = weak.update(cx, |this, cx| this.set_mode(ChatMode::ALL[ix], cx));
        });

        // Hint about where the next message goes.
        let hint: Option<(Hsla, String)> = if waiting || self.feedback {
            Some((
                theme::WARNING,
                "A plan waits for you · your message goes to it as feedback".into(),
            ))
        } else if running {
            Some((
                theme::INFO_TEXT,
                "Working · your message steers it before its next step".into(),
            ))
        } else {
            None
        };

        let popup = (!suggestions.is_empty()).then(|| {
            // Anchored to the top edge of the message box, growing upwards.
            div()
                .absolute()
                .bottom(relative(1.))
                .left_0()
                .pb(px(6.))
                .child(
                    div()
                        .w(px(380.))
                        .flex()
                        .flex_col()
                        .p(theme::S1)
                        .rounded(theme::RADIUS_LG)
                        .border_1()
                        .border_color(theme::LINE_STRONG)
                        .bg(theme::SURFACE_RAISED)
                        .shadow_lg()
                        .child(
                            div()
                                .px(px(10.))
                                .pt(theme::S1)
                                .pb(theme::S1)
                                .text_label()
                                .fg_muted()
                                .child(if text.starts_with('@') {
                                    "Agents"
                                } else {
                                    "Pipelines"
                                }),
                        )
                        .children(suggestions.iter().enumerate().map(|(ix, sg)| {
                            div()
                                .id(ElementId::Name(format!("suggest-{ix}").into()))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .min_h(px(32.))
                                .px(px(10.))
                                .rounded(theme::RADIUS_MD)
                                .cursor_pointer()
                                .when(ix == suggest_ix, |d| d.bg(theme::SURFACE_HOVER))
                                .hover(|st| st.bg(theme::SURFACE_HOVER))
                                .child(div().text_mono().fg().child(sg.label.clone()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_label()
                                        .fg_muted()
                                        .ellipsis()
                                        .child(sg.detail.clone()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.accept_suggestion(ix, window, cx)
                                }))
                        }))
                        .with_animation(
                            "suggest-in",
                            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                            |d, t| d.opacity(t).mt(px(4.) * (1.0 - t)),
                        ),
                )
        });

        let images_label = (!self.attachments.is_empty()).then(|| {
            let tokens: u64 = self.attachments.iter().map(|a| a.tokens()).sum();
            let cost = state::engine(cx).config().cost(&s.model, tokens, 0);
            images_cost_label(&super::data::short_model(&s.model), tokens, cost)
        });

        let send_label = if running || waiting {
            "Send to it"
        } else {
            "Send"
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .child(cmd_chip(
                "chat-cmd-run",
                "/run",
                "a pipeline",
                cx.listener(|this, _, window, cx| this.insert_prefix("/run ", window, cx)),
            ))
            .child(cmd_chip(
                "chat-cmd-agent",
                "@agent",
                "talk to an agent",
                cx.listener(|this, _, window, cx| this.insert_prefix("@", window, cx)),
            ))
            .when_some(hint, |d, (color, h)| {
                d.child(div().text_label().text_color(color).ellipsis().child(h))
            })
            .when_some(images_label, |d, l| {
                d.child(div().text_label().fg_muted().ellipsis().child(l))
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S1)
                    .text_label()
                    .fg_muted()
                    .child(kbd("↵"))
                    .child("send")
                    .child(kbd("⇧↵"))
                    .child("new line"),
            )
            .when(running || waiting, |d| {
                d.child(
                    Button::new("chat-stop", "Stop")
                        .danger()
                        .icon(AppIcon::Pause)
                        .tooltip("Cancel this chat's task and its sub-agents")
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                )
            })
            .when(!(running || waiting) || has_input, |d| {
                d.child(
                    Button::new("chat-send", send_label)
                        .primary()
                        .icon(AppIcon::Send)
                        .disabled(!has_input)
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                )
            });

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(theme::S6)
            .pt(theme::S4)
            .pb(theme::S5)
            .border_t_1()
            .border_color(theme::LINE)
            .bg(theme::BG_SIDEBAR)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::S3)
                    .child(selector)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.))
                            .text_label()
                            .fg_muted()
                            .child(mode_blurb(s.mode)),
                    ),
            )
            .child(self.render_settings_row(cx))
            .child(div().text_label().fg_muted().child(self.target_label()))
            .child(
                div()
                    .relative()
                    .key_context("ChatComposer")
                    .on_action(cx.listener(Self::on_submit))
                    .on_action(cx.listener(Self::on_suggest_up))
                    .on_action(cx.listener(Self::on_suggest_down))
                    .on_action(cx.listener(Self::on_suggest_accept))
                    .on_action(cx.listener(Self::on_paste_image))
                    .child(self.render_compose_box(window, cx))
                    .children(popup),
            )
            .child(actions)
    }

    /// The message box: image tray, text, and the bar with "Add image".
    fn render_compose_box(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let focused = self.composer.focus_handle(cx).is_focused(window);
        let n = self.attachments.len();
        let free = self.free_slots();
        let tokens: u64 = self.attachments.iter().map(|a| a.tokens()).sum();
        let mod_key = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        };

        let tray = (n > 0).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .px(px(12.))
                .pt(px(10.))
                .pb(px(2.))
                .children(self.attachments.iter().map(|a| {
                    let path = a.path.clone();
                    let label = format!("Remove {}", a.name);
                    div()
                        .id(eid("att", &a.path))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .w(px(136.))
                        .child(
                            div()
                                .relative()
                                .h(px(84.))
                                .border_1()
                                .border_color(theme::LINE_STRONG)
                                .rounded(theme::RADIUS_MD)
                                .bg(theme::BG)
                                .overflow_hidden()
                                .child(
                                    img(PathBuf::from(&a.path))
                                        .size_full()
                                        .object_fit(ObjectFit::Cover),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(4.))
                                        .bottom(px(4.))
                                        .h(px(18.))
                                        .px(px(6.))
                                        .flex()
                                        .items_center()
                                        .rounded(theme::RADIUS_SM)
                                        .bg(theme::SCRIM)
                                        .text_color(theme::TEXT_SECONDARY)
                                        .text_mono_sm()
                                        .child(format!("{}×{}", a.width, a.height)),
                                )
                                .child(
                                    div()
                                        .id(eid("att-x", &a.path))
                                        .absolute()
                                        .top(px(4.))
                                        .right(px(4.))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(theme::RADIUS_SM)
                                        .bg(theme::SCRIM)
                                        .text_color(theme::TEXT)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme::SURFACE_HOVER))
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(label.clone()).build(window, cx)
                                        })
                                        .child(icon_sized(AppIcon::X, px(12.)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_attachment(&path, cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .overflow_hidden()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .ellipsis()
                                        .text_color(theme::TEXT)
                                        .child(a.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .text_color(theme::TEXT_MUTED)
                                        .child(attachment_meta(a)),
                                ),
                        )
                        .with_animation(
                            eid("att-in", &a.path),
                            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                            |d, t| d.opacity(t).mt(px(6.) * (1.0 - t)),
                        )
                }))
                .when(free > 0, |d| {
                    d.child(
                        div()
                            .id("chat-drop")
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(4.))
                            .w(px(136.))
                            .h(px(84.))
                            .px(px(4.))
                            .border_1()
                            .border_dashed()
                            .border_color(theme::LINE_STRONG)
                            .rounded(theme::RADIUS_MD)
                            .text_color(theme::TEXT_MUTED)
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_center()
                            .cursor_pointer()
                            .hover(|s| {
                                s.border_color(theme::ACCENT_RING)
                                    .text_color(theme::TEXT_SECONDARY)
                                    .bg(theme::ACCENT_GLOW)
                            })
                            .tooltip(|window, cx| Tooltip::new("Add an image").build(window, cx))
                            .child(icon_sized(AppIcon::Image, theme::ICON_SM))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child("Drop or paste")
                                    .child(kbd(mod_key))
                                    .child(kbd("V")),
                            )
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_color(theme::TEXT_DISABLED)
                                    .child(format!("PNG, JPG, WebP · {free} more")),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                    )
                })
        });

        let cbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(6.))
            .border_t_1()
            .border_color(theme::LINE)
            .child(
                Button::new("chat-add-image", "Add image")
                    .ghost()
                    .small()
                    .icon(AppIcon::Image)
                    .disabled(free == 0)
                    .tooltip("Choose PNG, JPG or WebP files")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
            )
            .child(div().flex_1())
            .when(n > 0, |d| {
                d.child(
                    div()
                        .text_label()
                        .fg_muted()
                        .child(images_summary(n, tokens)),
                )
            });

        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(if focused {
                theme::ACCENT_RING
            } else {
                theme::LINE_STRONG
            })
            .rounded(theme::RADIUS_LG)
            .bg(theme::SURFACE_RAISED)
            .drag_over::<ExternalPaths>(|s, _, _, _| {
                s.border_color(theme::ACCENT_RING).bg(theme::ACCENT_GLOW)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_files(paths.paths().to_vec(), cx)
            }))
            .children(tray)
            .child(ui::input::composer_field_bare(&self.composer))
            .child(cbar)
    }

    /// The mode's settings: model / agent / pipeline, and the working folder.
    fn render_settings_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.settings();
        let picker: AnyElement = match s.mode {
            ChatMode::Direct | ChatMode::Orchestrator => setting(
                "Model",
                ui::select::select_sm(&self.model_select)
                    .w(px(240.))
                    .into_any_element(),
            ),
            ChatMode::Agent if self.pickers.agents.is_empty() => empty_setting(
                "No agents yet",
                Button::new("chat-go-agents", "Create one in Agents")
                    .ghost()
                    .small()
                    .on_click(|_, _, cx| state::navigate(Route::Agents { agent: None }, cx)),
            ),
            ChatMode::Agent => setting(
                "Agent",
                ui::select::select_sm(&self.agent_select)
                    .w(px(240.))
                    .into_any_element(),
            ),
            ChatMode::Pipeline if self.pickers.pipelines.is_empty() => empty_setting(
                "No pipelines yet",
                Button::new("chat-go-pipelines", "Create one in Pipelines")
                    .ghost()
                    .small()
                    .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
            ),
            ChatMode::Pipeline => setting(
                "Pipeline",
                ui::select::select_sm(&self.pipeline_select)
                    .w(px(240.))
                    .into_any_element(),
            ),
        };
        let with_folder = matches!(
            s.mode,
            ChatMode::Agent | ChatMode::Orchestrator | ChatMode::Direct
        );
        let (hint_icon, hint_color, hint) =
            match (s.cwd.as_deref().filter(|c| !c.is_empty()), self.cwd_valid) {
                (None, _) if s.mode == ChatMode::Direct => (
                    AppIcon::Info,
                    theme::TEXT_MUTED,
                    "optional: lets it read files",
                ),
                (None, _) => (
                    AppIcon::Info,
                    theme::TEXT_MUTED,
                    "no folder: it can't read or change files",
                ),
                (Some(_), Some(true)) => (AppIcon::CircleCheck, theme::SUCCESS, "folder found"),
                (Some(_), Some(false)) => (
                    AppIcon::CircleX,
                    theme::DANGER,
                    "not a folder on this machine",
                ),
                (Some(_), None) => (AppIcon::Clock, theme::TEXT_MUTED, "checking…"),
            };
        let _ = cx;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S4)
            .child(picker)
            .when(with_folder, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .child(div().text_label().fg_muted().child("Folder"))
                        .child(
                            div()
                                .w(px(260.))
                                .child(ui::input::field_sm(&self.cwd_input)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::S1)
                                .text_label()
                                .text_color(hint_color)
                                .child(icon_sized(hint_icon, theme::ICON_SM))
                                .child(hint),
                        ),
                )
            })
    }

    fn insert_prefix(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).value().to_string();
        let next = if current.trim().is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}{}", current.trim_start())
        };
        let chars = prefix.chars().count() as u32;
        self.composer.update(cx, |s, cx| {
            s.set_value(next, window, cx);
            s.set_cursor_position(Position::new(0, chars), window, cx);
        });
        cx.notify();
    }
}

/// "Model [select]".
fn setting(name: &'static str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .child(div().text_label().fg_muted().child(name))
        .child(control)
        .into_any_element()
}

fn empty_setting(text: &'static str, action: Button) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .child(div().text_label().text_color(theme::WARNING).child(text))
        .child(action)
        .into_any_element()
}

/// A clickable command chip: `/run a pipeline`.
fn cmd_chip(
    id: &'static str,
    cmd: &'static str,
    what: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(24.))
        .px(theme::S2)
        .rounded(theme::RADIUS_MD)
        .border_1()
        .border_color(theme::LINE)
        .bg(theme::BG_SIDEBAR)
        .cursor_pointer()
        .hover(|s| s.border_color(theme::LINE_STRONG))
        .child(div().text_mono().fg_secondary().child(cmd))
        .child(div().text_label().fg_muted().child(what))
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(w: u32, h: u32, pasted: bool) -> Attachment {
        Attachment {
            path: "/tmp/a.png".into(),
            name: "a.png".into(),
            mime: "image/png".into(),
            width: w,
            height: h,
            pasted,
        }
    }

    #[test]
    fn formats_token_counts() {
        assert_eq!(tokens_short(1), "1");
        assert_eq!(tokens_short(850), "850");
        assert_eq!(tokens_short(999), "999");
        assert_eq!(tokens_short(1000), "1k");
        assert_eq!(tokens_short(1365), "1.4k");
        assert_eq!(tokens_short(800), "800");
        assert_eq!(tokens_short(2200), "2.2k");
        assert_eq!(tokens_short(9990), "10k");
        assert_eq!(tokens_short(12_400), "12k");
    }

    #[test]
    fn labels_attachments() {
        // 1280×800 → 1365 tokens.
        assert_eq!(attachment_meta(&att(1280, 800, false)), "≈ 1.4k");
        assert_eq!(attachment_meta(&att(980, 620, true)), "pasted · ≈ 810");
        assert_eq!(images_summary(1, 1365), "1 image · ≈ 1.4k tokens");
        assert_eq!(images_summary(2, 2200), "2 images · ≈ 2.2k tokens");
    }

    #[test]
    fn counts_free_slots_and_plurals() {
        assert_eq!(remaining_slots(0), 5);
        assert_eq!(remaining_slots(2), 3);
        assert_eq!(remaining_slots(9), 0);
        assert_eq!(count_noun(1, "screenshot"), "1 screenshot");
        assert_eq!(count_noun(3, "screenshot"), "3 screenshots");
    }

    #[test]
    fn labels_image_cost() {
        assert_eq!(
            images_cost_label("Opus 5.5", 2200, 0.0165),
            "Opus 5.5 · images ≈ $0.02 per reply"
        );
        assert_eq!(
            images_cost_label("Opus 5.5", 100, 0.001),
            "Opus 5.5 · images under $0.01 per reply"
        );
        assert_eq!(
            images_cost_label("", 1365, 0.0),
            "≈ 1.4k tokens of images per reply"
        );
    }
}
