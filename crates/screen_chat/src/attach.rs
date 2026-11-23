//! Image attachments in the composer: picking, pasting and dropping files, the pending
//! thumbnails row, and the limits.

use std::path::PathBuf;

use gpui::{
    AppContext as _, Context, IntoElement, PathPromptOptions, SharedString, Task, div, prelude::*,
    px,
};
use ui::images::{self, ImageAttachment, MAX_IMAGES};
use ui::theme::*;

use crate::ChatScreen;

/// An image waiting to be sent with the next message.
pub(crate) enum Pending {
    /// Being read from disk; dropping it cancels the read.
    Loading {
        id: u64,
        name: SharedString,
        task: Task<()>,
    },
    Ready {
        id: u64,
        image: ImageAttachment,
    },
}

impl Pending {
    fn id(&self) -> u64 {
        match self {
            Pending::Loading { id, .. } | Pending::Ready { id, .. } => *id,
        }
    }
}

const TOO_MANY: &str = "Up to 10 images per message";

impl ChatScreen {
    /// Images still being read.
    pub(crate) fn images_loading(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| matches!(p, Pending::Loading { .. }))
            .count()
    }

    /// Ready images, leaving the pending list empty.
    pub(crate) fn take_images(&mut self) -> Vec<ImageAttachment> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|p| match p {
                Pending::Ready { image, .. } => Some(image),
                Pending::Loading { .. } => None,
            })
            .collect()
    }

    fn next_image_id(&mut self) -> u64 {
        self.image_seq += 1;
        self.image_seq
    }

    fn has_room(&self) -> bool {
        self.pending.len() < MAX_IMAGES
    }

    pub(crate) fn remove_pending(&mut self, id: u64, cx: &mut Context<Self>) {
        self.pending.retain(|p| p.id() != id);
        cx.notify();
    }

    /// Images from the clipboard.
    pub(crate) fn add_pasted(&mut self, pasted: Vec<gpui::Image>, cx: &mut Context<Self>) {
        for image in pasted {
            if !self.has_room() {
                self.notice(TOO_MANY, cx);
                break;
            }
            self.paste_seq += 1;
            match images::from_clipboard_image(image, self.paste_seq) {
                Ok(image) => {
                    let id = self.next_image_id();
                    self.pending.push(Pending::Ready { id, image });
                }
                Err(e) => self.notice(e, cx),
            }
        }
        cx.notify();
    }

    /// Files from the picker or dropped onto the chat: non-images are rejected, the rest is
    /// read in the background.
    pub(crate) fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if images::format_from_path(&path).is_none() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.notice(format!("{name} is not an image"), cx);
                continue;
            }
            if !self.has_room() {
                self.notice(TOO_MANY, cx);
                break;
            }
            self.load_path(path, cx);
        }
        cx.notify();
    }

    fn load_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let id = self.next_image_id();
        let name: SharedString = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let task = cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { images::load_image(&path) })
                .await;
            this.update(cx, |this, cx| this.finish_load(id, res, cx)).ok();
        });
        self.pending.push(Pending::Loading { id, name, task });
    }

    fn finish_load(
        &mut self,
        id: u64,
        res: Result<ImageAttachment, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.pending.iter().position(|p| p.id() == id) else {
            return;
        };
        let old = match res {
            Ok(image) => std::mem::replace(&mut self.pending[ix], Pending::Ready { id, image }),
            Err(e) => {
                self.notice(e, cx);
                self.pending.remove(ix)
            }
        };
        // This is the finishing task itself: let it end instead of cancelling it.
        if let Pending::Loading { task, .. } = old {
            task.detach();
        }
        cx.notify();
    }

    /// The attach button: pick image files.
    pub(crate) fn pick_images(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach images".into()),
        });
        cx.spawn(async move |this, cx| {
            match rx.await {
                Ok(Ok(Some(paths))) => {
                    this.update(cx, |this, cx| this.add_paths(paths, cx)).ok();
                }
                Ok(Err(e)) => {
                    let msg = format!("Could not open the file picker: {e:#}");
                    this.update(cx, |this, cx| this.notice(msg, cx)).ok();
                }
                _ => {}
            }
        })
        .detach();
    }

    /// Thumbnails + caption above the text field; `None` when nothing is attached.
    pub(crate) fn pending_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.pending.is_empty() {
            return None;
        }
        let thumbs = self.pending.iter().map(|p| {
            let id = p.id();
            let remove = cx.listener(move |this, _, _, cx| this.remove_pending(id, cx));
            match p {
                Pending::Ready { image, .. } => {
                    images::image_thumb(("pending-image", id), image, 64., 48., 20., remove)
                        .into_any_element()
                }
                Pending::Loading { name, .. } => {
                    images::loading_thumb(("pending-image", id), name.clone(), 64., 48., 20., remove)
                        .into_any_element()
                }
            }
        });
        let caption = images::caption(self.pending.len(), self.images_loading());
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_end()
                .gap(px(8.))
                .pt(px(12.))
                .px(px(14.))
                .text_color(TEXT_MUTED)
                .children(thumbs)
                .child(caption.element()),
        )
    }
}
