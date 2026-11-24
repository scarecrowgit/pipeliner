//! The composer's wiring: what a submit sends, Stop, messages sent while the agent is busy
//! (the API queues them), the draft kept per session, `@` and `/` completions fed
//! asynchronously, and images pasted, dropped or picked, uploaded to the blob store before
//! the message can go out.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, PathPromptOptions, SharedString, Task, Window, div, prelude::*, px,
};
use hub::api::{Attachment as Uploaded, RunnableKind, UserMessage};
use hub::{CatalogModel, HubNotice};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{
    Attachment, AttachmentContent, AttachmentId, Completion, Composer, ComposerEvent, TextAreaEvent, Trigger,
    TriggerKind,
};

use super::memory::{self, Saved};
use super::{SessionView, pickers};

/// Files offered for one `@` query.
const FILE_LIMIT: usize = 12;
/// Agents or skills offered for one `@` query.
const NAME_LIMIT: usize = 6;

/// The composer side of a session view: uploads and the completion lookup in flight.
#[derive(Default)]
pub(super) struct InputState {
    /// Images of the draft already in the blob store, by chip.
    pub uploaded: HashMap<AttachmentId, Uploaded>,
    uploading: HashMap<AttachmentId, Task<()>>,
    lookup: Option<Task<()>>,
    generation: u64,
    /// Completion answers applied so far; a stale answer is never applied.
    pub applied: u64,
}

/// What the composer was last told, so it is only told about changes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Applied {
    busy: bool,
    disabled: Option<SharedString>,
    placeholder: SharedString,
    hint: Option<SharedString>,
}

impl SessionView {
    /// Brings the composer in step with the session: busy, disabled, placeholder, queue hint.
    pub(super) fn sync(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let queued = model.queued().len() as u64;
        let next = Applied {
            busy: model.is_busy(),
            disabled: super::disabled_reason(model),
            placeholder: format!("Message {}…", pickers::current_label(self, cx)).into(),
            hint: (queued > 0).then(|| format!("{} queued", base::fmt::plural(queued, "message", "messages")).into()),
        };
        let last = self.applied.replace(next.clone());
        self.composer.update(cx, |c, cx| {
            if last.as_ref().map(|l| l.busy) != Some(next.busy) {
                c.set_busy(next.busy, cx);
            }
            if last.as_ref().map(|l| &l.disabled) != Some(&next.disabled) {
                c.set_disabled(next.disabled.clone(), cx);
            }
            if last.as_ref().map(|l| &l.placeholder) != Some(&next.placeholder) {
                c.set_placeholder(next.placeholder.clone(), cx);
            }
            if last.as_ref().map(|l| &l.hint) != Some(&next.hint) {
                c.set_hint(next.hint.clone(), cx);
            }
        });
        cx.notify();
    }

    /// Puts the session's saved draft back into the composer and resumes its uploads.
    pub(super) fn restore_draft(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = memory::draft(&self.session, cx) else { return };
        self.input.uploaded = saved.uploaded;
        self.composer.update(cx, |c, cx| c.restore(saved.draft, cx));
        self.track_attachments(cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let saved = Saved { draft: self.composer.read(cx).draft(), uploaded: self.input.uploaded.clone() };
        memory::save_draft(&self.session, saved, cx);
    }

    pub(super) fn on_composer_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit { text, attachments, .. } => self.send(text.clone(), attachments, cx),
            ComposerEvent::Stop => hub::commands(cx).interrupt(&self.session, cx),
            ComposerEvent::TriggerChanged(trigger) => self.lookup(trigger.clone(), cx),
            ComposerEvent::DraftChanged => {
                self.track_attachments(cx);
                self.save_draft(cx);
            }
            ComposerEvent::AttachRequested => self.pick_files(window, cx),
            ComposerEvent::FilesDropped(paths) => self.add_files(paths.clone(), window, cx),
            ComposerEvent::AttachmentRemoved(id) => {
                self.input.uploaded.remove(id);
                self.input.uploading.remove(id);
                self.save_draft(cx);
            }
            ComposerEvent::AttachmentRejected(why) => {
                ui::overlays(window, cx).toast(Toast::new(why.clone()).error(), cx);
            }
        }
    }

    /// Enter while a turn runs: the composer itself refuses to send, but the API queues a
    /// message for the next turn, so it goes out from here.
    pub(super) fn on_area_event(
        &mut self,
        _: &Entity<ui_rich::TextArea>,
        event: &TextAreaEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, TextAreaEvent::Submit) {
            return;
        }
        let c = self.composer.read(cx);
        if !c.is_busy() || c.disabled_reason().is_some() || c.menu_open() {
            return;
        }
        if c.attachments().iter().any(|a| a.progress.is_some()) {
            return;
        }
        let draft = c.draft();
        let text = draft.text.trim_end().to_string();
        if text.trim().is_empty() && draft.attachments.is_empty() {
            return;
        }
        self.composer.update(cx, |c, cx| c.clear(cx));
        self.send(text, &draft.attachments, cx);
    }

    /// Sends a message with the uploaded images of `attachments`; a busy session queues it.
    fn send(&mut self, text: String, attachments: &[Attachment], cx: &mut Context<Self>) {
        let images = attachments.iter().filter_map(|a| self.input.uploaded.remove(&a.id)).collect();
        hub::commands(cx).send(&self.session, UserMessage { text, images }, cx);
        memory::save_draft(&self.session, Saved::default(), cx);
        self.transcript.update(cx, |t, cx| t.scroll_to_end(cx));
    }

    // ------------------------------------------------------------ completions

    /// Looks up the rows for the trigger being typed; an answer for an older trigger is dropped.
    fn lookup(&mut self, trigger: Option<Trigger>, cx: &mut Context<Self>) {
        self.input.generation += 1;
        let generation = self.input.generation;
        let Some(trigger) = trigger else {
            self.input.lookup = None;
            return;
        };
        let queries = hub::queries(cx);
        let query = trigger.query.clone();
        let task = match trigger.kind {
            TriggerKind::Mention => {
                let names = name_rows(self.catalog.read(cx), &query);
                let files = queries.mention_files(&self.session, &query, FILE_LIMIT, cx);
                cx.spawn(async move |this, cx| {
                    let mut rows = file_rows(files.await);
                    rows.extend(names);
                    this.update(cx, |this, cx| this.apply_completions(generation, &trigger, rows, None, cx)).ok();
                })
            }
            TriggerKind::Command => {
                let commands = queries.slash_commands(&self.session, cx);
                cx.spawn(async move |this, cx| {
                    let all = commands.await;
                    let total = all.len();
                    let rows = all
                        .into_iter()
                        .filter(|c| matches(&c.name, &query))
                        .map(|c| {
                            let row = Completion::new(format!("command:{}", c.name), format!("/{}", c.name), format!("/{}", c.name))
                                .detail(c.description)
                                .icon(IconName::Terminal);
                            if c.from_project { row.hint("project") } else { row }
                        })
                        .collect();
                    this.update(cx, |this, cx| this.apply_completions(generation, &trigger, rows, Some(total), cx)).ok();
                })
            }
        };
        self.input.lookup = Some(task);
    }

    fn apply_completions(
        &mut self,
        generation: u64,
        trigger: &Trigger,
        rows: Vec<Completion>,
        total: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.input.generation || self.composer.read(cx).trigger() != Some(trigger) {
            return;
        }
        self.input.applied += 1;
        self.composer.update(cx, |c, cx| c.set_completions(rows, total, cx));
    }

    // ------------------------------------------------------------ images

    /// Starts uploading every attachment that is neither uploaded nor uploading, and forgets
    /// those that left the composer.
    fn track_attachments(&mut self, cx: &mut Context<Self>) {
        let attachments = self.composer.read(cx).attachments().to_vec();
        let present: HashSet<AttachmentId> = attachments.iter().map(|a| a.id).collect();
        self.input.uploaded.retain(|id, _| present.contains(id));
        self.input.uploading.retain(|id, _| present.contains(id));
        for attachment in attachments {
            if !self.input.uploaded.contains_key(&attachment.id) && !self.input.uploading.contains_key(&attachment.id) {
                self.upload(attachment, cx);
            }
        }
    }

    fn upload(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        let id = attachment.id;
        self.composer.update(cx, |c, cx| c.set_attachment_progress(id, Some(0.), cx));
        let commands = hub::commands(cx);
        let task = cx.spawn(async move |this, cx| {
            let name = attachment.name.to_string();
            let read = match attachment.content {
                AttachmentContent::Image(image) => Ok((image.bytes.clone(), image.format.mime_type().to_string())),
                AttachmentContent::ImageFile(path) | AttachmentContent::File(path) => {
                    cx.background_executor()
                        .spawn(async move { std::fs::read(&path).map(|bytes| (bytes, media_type(&path).to_string())) })
                        .await
                }
            };
            let stored = match read {
                Ok((bytes, media)) => match cx.update(|cx| commands.put_image(&name, &media, bytes, cx)) {
                    Ok(task) => task.await,
                    Err(_) => return,
                },
                Err(error) => {
                    cx.update(|cx| {
                        let notice = HubNotice::failed("attach the image", &error.into());
                        hub::hub(cx).update(cx, |hub, cx| hub.notify_user(notice, cx));
                    })
                    .ok();
                    None
                }
            };
            this.update(cx, |this, cx| this.uploaded(id, stored, cx)).ok();
        });
        self.input.uploading.insert(id, task);
    }

    fn uploaded(&mut self, id: AttachmentId, stored: Option<Uploaded>, cx: &mut Context<Self>) {
        self.input.uploading.remove(&id);
        match stored {
            Some(stored) => {
                self.input.uploaded.insert(id, stored);
                self.composer.update(cx, |c, cx| c.set_attachment_progress(id, None, cx));
            }
            None => {
                self.composer.update(cx, |c, cx| c.remove_attachment(id, cx));
            }
        }
        self.save_draft(cx);
    }

    /// The attach button: the platform's file picker, images only.
    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update_in(cx, |this, window, cx| this.add_files(paths, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Adds image files as attachments; their sizes are read off the UI thread.
    fn add_files(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let sized = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    (path, size)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let sized = sized.await;
            this.update_in(cx, |this, window, cx| {
                let mut refused = None;
                for (path, size) in sized {
                    if !is_image(&path) {
                        refused = Some(SharedString::from("Only images can be attached"));
                        continue;
                    }
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let added = this.composer.update(cx, |c, cx| c.add_attachment(Attachment::image_file(name, path, size), cx));
                    if let Err(why) = added {
                        refused = Some(why);
                    }
                }
                if let Some(why) = refused {
                    ui::overlays(window, cx).toast(Toast::new(why).error(), cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// The controls at the end of the composer's row: the session's forced skills (a lock and
/// the name, as drawn), and the context use while the panel is folded away.
pub(super) fn trailing(this: &SessionView, cx: &App) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(session) = this.header_session(cx) {
        for (ix, skill) in session.forced_skills.iter().enumerate() {
            out.push(
                row()
                    .id(("chat-forced-skill", ix))
                    .flex_none()
                    .gap(px(SPACE_1))
                    .h(px(20.))
                    .px(px(SPACE_1_5))
                    .rounded(px(RADIUS_MD))
                    .bg(SURFACE_HOVER)
                    .text_color(TEXT_SECONDARY)
                    .type_style(LABEL_TIGHT)
                    .whitespace_nowrap()
                    .child(icon(IconName::Lock, 10., TEXT_MUTED))
                    .child(skill.clone())
                    .probe(format!("chat-forced-skill-{ix}"))
                    .into_any_element(),
            );
        }
    }
    if !this.panel_open {
        let usage = this.model.read(cx).context();
        if usage.window > 0 {
            let pct = base::fmt::percent(usage.tokens as f64, usage.window as f64);
            out.push(
                div()
                    .flex_none()
                    .type_style(LABEL)
                    .tabular()
                    .text_color(TEXT_MUTED)
                    .child(format!("{pct}% context"))
                    .probe_value("chat-context", format!("{pct}"))
                    .into_any_element(),
            );
        }
    }
    out
}

/// The line under the composer: what the current agent may do, and its keys.
pub(super) fn hint_row(this: &SessionView, cx: &mut Context<SessionView>) -> impl IntoElement {
    let keys = KeyHints::new().hint(["Tab"], "Build ⇄ Plan").hint(["Esc"], "interrupt").hint(["⌘K"], "palette");
    row()
        .mt(px(SPACE_2))
        .px(px(SPACE_1))
        .gap(px(SPACE_3))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(truncate_lh(pickers::mode_line(this, cx), LABEL.line))
        .child(
            div()
                .id("chat-mode-toggle")
                .debug_selector(|| "chat-mode-toggle".into())
                .flex_none()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx)))
                .child(keys)
                .probe("chat-mode-toggle"),
        )
}

/// `@` rows that need no lookup: agents and orchestrators of the catalog, then skills.
fn name_rows(catalog: &CatalogModel, query: &str) -> Vec<Completion> {
    let mut rows: Vec<Completion> = pickers::chat_runnables(catalog)
        .filter(|r| matches(&r.name, query))
        .take(NAME_LIMIT)
        .map(|r| {
            let (kind, icon) = match r.kind {
                RunnableKind::Orchestrator => ("Orchestrator", IconName::Orchestrator),
                _ => ("Agent", IconName::Agent),
            };
            Completion::new(format!("runnable:{}", r.reference), r.name.clone(), format!("@{}", r.name.replace(' ', "-")))
                .detail(format!("{kind} · {}", r.description))
                .icon(icon)
        })
        .collect();
    if let Some(skills) = catalog.skills().value() {
        rows.extend(skills.iter().filter(|s| matches(&s.name, query)).take(NAME_LIMIT).map(|s| {
            Completion::new(format!("skill:{}", s.name), s.name.clone(), format!("@{}", s.name))
                .detail(format!("Skill · {}", s.description))
                .icon(IconName::Zap)
        }));
    }
    rows
}

fn file_rows(files: Vec<String>) -> Vec<Completion> {
    files
        .into_iter()
        .map(|f| Completion::new(format!("file:{f}"), f.clone(), format!("@{f}")).icon(IconName::File))
        .collect()
}

fn matches(label: &str, query: &str) -> bool {
    label.to_lowercase().contains(&query.to_lowercase())
}

const IMAGE_TYPES: [(&str, &str); 8] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
];

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase()
}

fn is_image(path: &Path) -> bool {
    let ext = extension(path);
    IMAGE_TYPES.iter().any(|(e, _)| *e == ext)
}

/// The media type of an image file, from its extension.
fn media_type(path: &Path) -> &'static str {
    let ext = extension(path);
    IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map_or("application/octet-stream", |(_, t)| t)
}
