//! Composer preview: the chat composer (agent and model pickers in the slots, `@` files and
//! agents, `/` commands, Enter sends, Stop while a fake turn runs) next to the issue
//! composer (`@` agents and pipelines, Ctrl/Cmd+Enter comments), fed by fake items, with a
//! log of every event.
//!
//! Dev-driver probes: `chat-text`, `chat-send`, `chat-stop`, `chat-attach`, `chat-chip-<n>`,
//! `chat-chip-<n>-remove`, `chat-menu-<n>`, the same with `issue-`, and the demo's buttons
//! `demo-busy`, `demo-disable`, `demo-image`.
//!
//! `cargo run -p ui_rich --example composer`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Entity, Image, ImageFormat, IntoElement, Render, SharedString, Subscription,
    Task, Window, div, prelude::*, px,
};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{
    Attachment, Completion, Composer, ComposerEvent, Trigger, TriggerKind, composer_view,
};

/// A 4 × 3 accent-coloured PNG, so pasted-in thumbnails have something to show.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x03, 0x08, 0x02, 0x00, 0x00, 0x00, 0x3b,
    0x96, 0x39, 0x91, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x88,
    0xcb, 0xba, 0x04, 0x47, 0x0c, 0x38, 0x39, 0x00, 0x71, 0x8a, 0x13, 0x39, 0x22, 0x16, 0x30,
    0x2b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

const FILES: &[&str] = &[
    "src/charges/retry.ts",
    "src/charges/retry.test.ts",
    "src/gateway/client.ts",
    "AGENTS.md",
    "package.json",
];

/// (name, kind, what a mention does, state)
const AGENTS: &[(&str, &str, &str, &str)] = &[
    ("Reproducer", "Agent", "on this issue now · gets your comment as input", "running"),
    ("Release captain", "Orchestrator", "starts a new run on PAY-1427", "idle"),
    ("Fixer", "Agent", "picks up the issue", "idle"),
    ("Codex", "Agent", "writes a variant in parallel", "idle"),
    ("PAY triage", "Pipeline", "routes the issue again", "live"),
];

const COMMANDS: &[(&str, &str)] = &[
    ("compact", "Summarise the session to free context"),
    ("model", "Switch the model for this session"),
    ("release-check", "From .harness/commands"),
    ("bump-sdk", "From .harness/commands"),
];

fn matches(label: &str, query: &str) -> bool {
    label.to_lowercase().contains(&query.to_lowercase())
}

fn agent_rows(query: &str) -> Vec<Completion> {
    AGENTS
        .iter()
        .filter(|(n, ..)| matches(n, query))
        .map(|(name, kind, what, state)| {
            let icon = match *kind {
                "Orchestrator" => IconName::Orchestrator,
                "Pipeline" => IconName::Pipelines,
                _ => IconName::Agent,
            };
            Completion::new(*name, *name, format!("@{}", name.replace(' ', "-")))
                .detail(format!("{kind} · {what}"))
                .icon(icon)
                .hint(*state)
        })
        .collect()
}

/// The fake catalogue the screens would ask the hub for: (rows, how many in all).
fn lookup(issue: bool, t: &Trigger) -> (Vec<Completion>, usize) {
    match t.kind {
        TriggerKind::Mention if issue => (agent_rows(&t.query), AGENTS.len()),
        TriggerKind::Mention => {
            let mut rows: Vec<Completion> = FILES
                .iter()
                .filter(|f| matches(f, &t.query))
                .map(|f| Completion::new(*f, *f, format!("@{f}")).icon(IconName::File))
                .collect();
            rows.extend(agent_rows(&t.query));
            (rows, FILES.len() + AGENTS.len())
        }
        TriggerKind::Command => (
            COMMANDS
                .iter()
                .filter(|(c, _)| matches(c, &t.query))
                .map(|(c, d)| {
                    Completion::new(*c, format!("/{c}"), format!("/{c}"))
                        .detail(*d)
                        .icon(IconName::Terminal)
                })
                .collect(),
            COMMANDS.len(),
        ),
    }
}

struct Demo {
    chat: Entity<Composer>,
    issue: Entity<Composer>,
    log: Vec<SharedString>,
    shots: u64,
    _subs: Vec<Subscription>,
    tasks: Vec<Task<()>>,
}

impl Demo {
    fn new(cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| Composer::chat(cx).name("chat").placeholder("Message Build…"));
        let issue = cx.new(|cx| Composer::issue(cx).name("issue"));
        let subs = vec![
            cx.subscribe(&chat, |this, c, ev, cx| this.on_event(false, c, ev, cx)),
            cx.subscribe(&issue, |this, c, ev, cx| this.on_event(true, c, ev, cx)),
        ];
        Self { chat, issue, log: Vec::new(), shots: 0, _subs: subs, tasks: Vec::new() }
    }

    fn on_event(
        &mut self,
        issue: bool,
        composer: Entity<Composer>,
        ev: &ComposerEvent,
        cx: &mut Context<Self>,
    ) {
        let who = if issue { "issue" } else { "chat" };
        match ev {
            ComposerEvent::TriggerChanged(Some(t)) => {
                let (rows, total) = lookup(issue, t);
                composer.update(cx, |c, cx| c.set_completions(rows, Some(total), cx));
            }
            ComposerEvent::Submit { text, attachments, mentions } => {
                let names: Vec<&str> = mentions.iter().map(|m| m.text.as_ref()).collect();
                self.push(
                    format!("{who}: sent {text:?} · {} attached · mentions {names:?}", attachments.len()),
                    cx,
                );
                if !issue {
                    self.fake_turn(cx);
                }
                return;
            }
            ComposerEvent::Stop => composer.update(cx, |c, cx| c.set_busy(false, cx)),
            ComposerEvent::AttachRequested => self.add_image(&composer, cx),
            ComposerEvent::FilesDropped(paths) => self.add_files(&composer, paths.clone(), cx),
            ComposerEvent::DraftChanged => return,
            _ => {}
        }
        self.push(format!("{who}: {ev:?}"), cx);
    }

    /// Pretend a turn runs for four seconds.
    fn fake_turn(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |c, cx| c.set_busy(true, cx));
        let chat = self.chat.clone();
        self.tasks.push(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            chat.update(cx, |c, cx| c.set_busy(false, cx)).ok();
        }));
    }

    /// What the file picker would add: an image that uploads over two seconds.
    fn add_image(&mut self, composer: &Entity<Composer>, cx: &mut Context<Self>) {
        self.shots += 1;
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, PNG.to_vec()));
        let att = Attachment::image(format!("gateway-trace-{}.png", self.shots), image).uploading(0.);
        let id = match composer.update(cx, |c, cx| c.add_attachment(att, cx)) {
            Ok(id) => id,
            Err(why) => return self.push(format!("refused: {why}"), cx),
        };
        let composer = composer.clone();
        self.tasks.push(cx.spawn(async move |_, cx| {
            for step in 1..=10 {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let p = (step < 10).then_some(step as f32 / 10.);
                if composer.update(cx, |c, cx| c.set_attachment_progress(id, p, cx)).is_err() {
                    return;
                }
            }
        }));
    }

    /// Dropped files: sizes are read off the UI thread.
    fn add_files(&mut self, composer: &Entity<Composer>, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let composer = composer.clone();
        self.tasks.push(cx.spawn(async move |this, cx| {
            let sized = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                            (p, size)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            for (path, size) in sized {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let image = matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("png" | "jpg" | "jpeg" | "gif" | "webp")
                );
                let att = if image {
                    Attachment::image_file(name, path, size)
                } else {
                    Attachment::file(name, path, size)
                };
                let res = composer.update(cx, |c, cx| c.add_attachment(att, cx));
                if let Ok(Err(why)) = res {
                    this.update(cx, |this, cx| this.push(format!("refused: {why}"), cx)).ok();
                }
            }
        }));
    }

    fn push(&mut self, line: String, cx: &mut Context<Self>) {
        self.log.push(line.into());
        if self.log.len() > 16 {
            self.log.remove(0);
        }
        cx.notify();
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = self.chat.clone();
        let issue = self.issue.clone();
        let (busy, off) = {
            let c = self.chat.read(cx);
            (c.is_busy(), c.disabled_reason().is_some())
        };
        let toggles = row()
            .gap(px(SPACE_2))
            .child(
                Button::new("demo-busy", if busy { "Finish turn" } else { "Start turn" })
                    .small()
                    .on_click({
                        let chat = chat.clone();
                        move |_, _, cx| chat.update(cx, |c, cx| c.set_busy(!busy, cx))
                    })
                    .probe("demo-busy"),
            )
            .child(
                Button::new("demo-disable", if off { "Enable chat" } else { "Make read-only" })
                    .small()
                    .on_click({
                        let chat = chat.clone();
                        move |_, _, cx| {
                            let reason = (!off).then(|| SharedString::from("Read-only session · fork it to reply"));
                            chat.update(cx, |c, cx| c.set_disabled(reason, cx))
                        }
                    })
                    .probe("demo-disable"),
            )
            .child(
                Button::new("demo-image", "Attach an image")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let chat = this.chat.clone();
                        this.add_image(&chat, cx);
                    }))
                    .probe("demo-image"),
            );
        let chat_box = composer_view(&chat)
            .leading(SelectBox::new("chat-agent", "Build").icon(IconName::Agent).small())
            .leading(SelectBox::new("chat-model", "opus-5.5").small())
            .trailing(Tag::new("payments-retries"));
        let footer = row()
            .mt(px(SPACE_2))
            .px(px(SPACE_1))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child("Build · full access · bash asks first by policy")
            .child(grow())
            .child(KeyHints::new().hint(["Tab"], "Build ⇄ Plan").hint(["Esc"], "interrupt"));
        row()
            .size_full()
            .items_start()
            .gap(px(SPACE_6))
            .p(px(SPACE_6))
            .child(
                col()
                    .flex_1()
                    .h_full()
                    .gap(px(SPACE_4))
                    .child(div().type_style(HEADING).child("Chat composer"))
                    .child(toggles)
                    .child(grow())
                    .child(col().child(chat_box).child(footer)),
            )
            .child(
                col()
                    .w(px(520.))
                    .gap(px(SPACE_3))
                    .child(div().type_style(HEADING).child("Issue composer"))
                    .child(composer_view(&issue))
                    .child(div().type_style(HEADING).pt(px(SPACE_4)).child("Events"))
                    .children(self.log.iter().map(|l| {
                        div().type_style(MONO).text_color(TEXT_MUTED).child(l.clone())
                    })),
            )
    }
}

fn main() {
    ui::run_app("Composer", 1280., 860., |_, cx| {
        ui_rich::bind_keys(cx);
        cx.new(Demo::new)
    });
}
