//! The comment composer docked under the panel: Cmd/Ctrl+Enter posts, `@` completes agents,
//! orchestrators and pipelines of the catalog (a mention wakes them: the engine reads the
//! names in the text), images pasted, dropped or picked are stored before the comment can go
//! out, and the draft is kept per issue.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, IntoElement, PathPromptOptions, SharedString, Task, Window, div, prelude::*, px};
use hub::api::{Attachment as Uploaded, Origin, Runnable, RunnableKind, RunnableRef};
use hub::{CatalogModel, HubNotice};
use ui::prelude::*;
use ui_rich::{Attachment, AttachmentContent, AttachmentId, Completion, Composer, ComposerEvent, Trigger, TriggerKind, composer_view};

use super::memory::{self, Saved};
use super::{IssuePanel, PAD_X, pickers};

/// Rows offered for one `@` query.
const MENTION_LIMIT: usize = 6;
/// The hint before the key cap while a mention is typed.
const MENTION_HINT: &str = "Mentions wake agents when you post";

/// The composer side of the panel: images stored or being stored.
#[derive(Default)]
pub(super) struct InputState {
    /// Images of the draft already in the blob store, by chip.
    pub uploaded: HashMap<AttachmentId, Uploaded>,
    uploading: HashMap<AttachmentId, Task<()>>,
}

/// The composer, docked at the foot of the panel.
pub(super) fn dock(this: &IssuePanel, _cx: &mut Context<IssuePanel>) -> AnyElement {
    div()
        .flex_none()
        .w_full()
        .px(px(PAD_X))
        .pt(px(SPACE_3))
        .pb(px(SPACE_4))
        .border_t_1()
        .border_color(LINE)
        .bg(BG)
        .child(composer_view(&this.composer).width(this.content_width()))
        .into_any_element()
}

impl IssuePanel {
    /// Puts the issue's saved draft back into the composer and resumes its uploads.
    pub(super) fn restore_draft(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = memory::draft(&self.issue, cx) else { return };
        self.input.uploaded = saved.uploaded;
        self.composer.update(cx, |c, cx| c.restore(saved.draft, cx));
        self.sync_hint(cx);
        self.track_attachments(cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let saved = Saved { draft: self.composer.read(cx).draft(), uploaded: self.input.uploaded.clone() };
        memory::save_draft(&self.issue, saved, cx);
    }

    pub(super) fn on_composer_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit { text, attachments, .. } => self.post(text.clone(), attachments, cx),
            ComposerEvent::Stop => {}
            ComposerEvent::TriggerChanged(trigger) => self.complete(trigger.as_ref(), cx),
            ComposerEvent::DraftChanged => {
                self.sync_hint(cx);
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

    /// Posts the comment with the stored images of `attachments`; the engine wakes whoever
    /// it mentions.
    fn post(&mut self, text: String, attachments: &[Attachment], cx: &mut Context<Self>) {
        let images = attachments.iter().filter_map(|a| self.input.uploaded.remove(&a.id)).collect();
        hub::commands(cx).comment(&self.issue, &text, images, cx);
        memory::save_draft(&self.issue, Saved::default(), cx);
        self.input.uploaded.clear();
        self.input.uploading.clear();
        self.scroll.scroll_to_bottom();
        self.sync_hint(cx);
    }

    /// "Mentions wake agents when you post" while the text holds an `@`.
    fn sync_hint(&mut self, cx: &mut Context<Self>) {
        let mentions = self.composer.read(cx).text().contains('@');
        let hint = mentions.then(|| SharedString::from(MENTION_HINT));
        self.composer.update(cx, |c, cx| c.set_hint(hint, cx));
    }

    // ------------------------------------------------------------ completions

    /// The `@` rows for what is typed: from the catalog, no lookup needed.
    fn complete(&mut self, trigger: Option<&Trigger>, cx: &mut Context<Self>) {
        let Some(trigger) = trigger.filter(|t| t.kind == TriggerKind::Mention) else { return };
        let attached = self.current(cx).and_then(|issue| issue.runner.clone());
        let (rows, total) = mention_rows(self.catalog.read(cx), &trigger.query, attached.as_ref(), &self.issue.to_string());
        self.composer.update(cx, |c, cx| c.set_completions(rows, Some(total), cx));
    }

    // ------------------------------------------------------------ images

    /// Starts storing every attachment that is neither stored nor being stored, and forgets
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
                Ok((bytes, media)) => {
                    // Half way: the bytes are read, the store has them next.
                    this.update(cx, |this, cx| {
                        this.composer.update(cx, |c, cx| c.set_attachment_progress(id, Some(0.5), cx));
                    })
                    .ok();
                    match cx.update(|cx| commands.put_image(&name, &media, bytes, cx)) {
                        Ok(task) => task.await,
                        Err(_) => return,
                    }
                }
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

    /// The attach button: the platform's file picker.
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
                    let added =
                        this.composer.update(cx, |c, cx| c.add_attachment(Attachment::image_file(name, path, size), cx));
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

/// Runnables a comment can mention: agents, orchestrators and pipelines offered in issues.
pub(super) fn mentionable(catalog: &CatalogModel) -> impl Iterator<Item = &Runnable> {
    catalog.runnables().iter().filter(|r| !r.archived && r.uses.issues && !matches!(r.origin, Origin::Spawned { .. }))
}

/// The `@` rows for `query` and how many matched in all. Names that start with the query
/// come first, then names that contain it; within each, the attached runner, then
/// orchestrators, agents and pipelines.
pub(super) fn mention_rows(
    catalog: &CatalogModel,
    query: &str,
    attached: Option<&RunnableRef>,
    issue: &str,
) -> (Vec<Completion>, usize) {
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, &Runnable)> = mentionable(catalog)
        .filter_map(|r| {
            let name = r.name.to_lowercase();
            if name.starts_with(&q) {
                Some((0, r))
            } else if name.contains(&q) {
                Some((1, r))
            } else {
                None
            }
        })
        .collect();
    hits.sort_by_key(|(class, r)| (*class, attached != Some(&r.reference), kind_rank(r.kind), r.name.to_lowercase()));
    let total = hits.len();
    let rows = hits
        .into_iter()
        .take(MENTION_LIMIT)
        .map(|(_, r)| {
            let kind = pickers::kind_label(r.kind);
            let detail = if r.paused {
                format!("{kind} · paused · takes no new work")
            } else if attached == Some(&r.reference) {
                format!("{kind} · on this issue now · gets your comment as input")
            } else {
                format!("{kind} · starts a new run on {issue}")
            };
            let row = Completion::new(format!("runnable:{}", r.reference), r.name.clone(), format!("@{}", r.name))
                .detail(detail)
                .icon(pickers::kind_icon(r.kind));
            match catalog.activity(&r.reference) {
                Some(activity) => row.hint(pickers::state_label(&activity.state).0),
                None => row,
            }
        })
        .collect();
    (rows, total)
}

fn kind_rank(kind: RunnableKind) -> u8 {
    match kind {
        RunnableKind::Orchestrator => 0,
        RunnableKind::Agent => 1,
        RunnableKind::Pipeline => 2,
    }
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
