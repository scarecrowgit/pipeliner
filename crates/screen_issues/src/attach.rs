//! Image attachments in the issue detail: description images and pending comment images —
//! picking, pasting and dropping files, loading, and the thumbnail rows.

use std::path::{Path, PathBuf};

use gpui::{
    AppContext as _, Context, IntoElement, PathPromptOptions, SharedString, Task, div, prelude::*,
    px,
};
use ui::images::{self, ImageAttachment, MAX_IMAGES};
use ui::theme::*;

use crate::IssuesScreen;
use crate::data::Issue;

/// Where an added image ends up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The pending images of the comment being written.
    Comment,
    /// The description images of the issue with this key.
    Desc(SharedString),
}

/// An image being read from disk; dropping it cancels the read.
pub(crate) struct Loading {
    id: u64,
    target: Target,
    name: SharedString,
    task: Task<()>,
}

/// The comment can be posted: there is text or ready images, and nothing is still loading.
pub(crate) fn can_post(text_empty: bool, images: usize, loading: usize) -> bool {
    loading == 0 && (!text_empty || images > 0)
}

fn limit_error(target: &Target) -> String {
    let what = match target {
        Target::Comment => "comment",
        Target::Desc(_) => "description",
    };
    format!("Up to {MAX_IMAGES} images per {what}")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn error_line(text: &str) -> gpui::Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(DANGER)
        .child(text.to_string())
}

impl IssuesScreen {
    // ── State ───────────────────────────────────────────────────────────

    fn loading_for(&self, target: &Target) -> usize {
        self.loading.iter().filter(|l| &l.target == target).count()
    }

    /// Comment images still being read.
    pub(crate) fn comment_loading(&self) -> usize {
        self.loading_for(&Target::Comment)
    }

    /// Ready + loading images of the target.
    fn count(&self, target: &Target) -> usize {
        let ready = match target {
            Target::Comment => self.comment_images.len(),
            Target::Desc(key) => self
                .issues
                .iter()
                .find(|i| &i.key == key)
                .map_or(0, |i| i.images.len()),
        };
        ready + self.loading_for(target)
    }

    fn next_image_id(&mut self) -> u64 {
        self.image_seq += 1;
        self.image_seq
    }

    fn error_slot(&mut self, target: &Target) -> &mut Option<String> {
        match target {
            Target::Comment => &mut self.comment_error,
            Target::Desc(_) => &mut self.desc_error,
        }
    }

    /// Show an error unless one from the same batch is already shown.
    fn note_error(&mut self, target: &Target, msg: impl Into<String>) {
        let slot = self.error_slot(target);
        if slot.is_none() {
            *slot = Some(msg.into());
        }
    }

    /// Drop everything pending for the comment (selection changed, comment posted).
    pub(crate) fn reset_pending(&mut self) {
        self.comment_images.clear();
        self.loading.retain(|l| l.target != Target::Comment);
        self.comment_error = None;
        self.desc_error = None;
    }

    pub(crate) fn remove_comment_image(&mut self, id: u64, cx: &mut Context<Self>) {
        self.comment_images.retain(|(i, _)| *i != id);
        cx.notify();
    }

    pub(crate) fn cancel_loading(&mut self, id: u64, cx: &mut Context<Self>) {
        self.loading.retain(|l| l.id != id);
        cx.notify();
    }

    fn push_ready(&mut self, target: Target, image: ImageAttachment) {
        match target {
            Target::Comment => {
                let id = self.next_image_id();
                self.comment_images.push((id, image));
            }
            Target::Desc(key) => {
                if let Some(issue) = self.issue_mut(&key) {
                    issue.images.push(image);
                }
            }
        }
    }

    // ── Adding ──────────────────────────────────────────────────────────

    /// Images from the clipboard go to the comment.
    pub(crate) fn add_pasted(&mut self, pasted: Vec<gpui::Image>, cx: &mut Context<Self>) {
        let target = Target::Comment;
        *self.error_slot(&target) = None;
        for image in pasted {
            if self.count(&target) >= MAX_IMAGES {
                self.note_error(&target, limit_error(&target));
                break;
            }
            self.paste_seq += 1;
            match images::from_clipboard_image(image, self.paste_seq) {
                Ok(image) => self.push_ready(target.clone(), image),
                Err(e) => self.note_error(&target, e),
            }
        }
        cx.notify();
    }

    /// Files from the picker or dropped: non-images are rejected, the rest is read in the
    /// background.
    pub(crate) fn add_paths(
        &mut self,
        target: Target,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        *self.error_slot(&target) = None;
        for path in paths {
            if images::format_from_path(&path).is_none() {
                self.note_error(&target, format!("{} is not an image", file_name(&path)));
                continue;
            }
            if self.count(&target) >= MAX_IMAGES {
                self.note_error(&target, limit_error(&target));
                break;
            }
            self.load_path(target.clone(), path, cx);
        }
        cx.notify();
    }

    fn load_path(&mut self, target: Target, path: PathBuf, cx: &mut Context<Self>) {
        let id = self.next_image_id();
        let name: SharedString = file_name(&path).into();
        let task = cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { images::load_image(&path) })
                .await;
            this.update(cx, |this, cx| this.finish_load(id, res, cx))
                .ok();
        });
        self.loading.push(Loading {
            id,
            target,
            name,
            task,
        });
    }

    fn finish_load(
        &mut self,
        id: u64,
        res: Result<ImageAttachment, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.loading.iter().position(|l| l.id == id) else {
            return;
        };
        let l = self.loading.remove(ix);
        // This is the finishing task itself: let it end instead of cancelling it.
        l.task.detach();
        match res {
            Ok(image) => self.push_ready(l.target, image),
            Err(e) => self.note_error(&l.target, e),
        }
        cx.notify();
    }

    /// The attach button / "Add" tile: pick image files.
    pub(crate) fn pick_images(&mut self, target: Target, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach images".into()),
        });
        cx.spawn(async move |this, cx| match rx.await {
            Ok(Ok(Some(paths))) => {
                this.update(cx, |this, cx| this.add_paths(target, paths, cx))
                    .ok();
            }
            Ok(Err(e)) => {
                let msg = format!("Could not open the file picker: {e:#}");
                this.update(cx, |this, cx| {
                    *this.error_slot(&target) = Some(msg);
                    cx.notify();
                })
                .ok();
            }
            _ => {}
        })
        .detach();
    }

    // ── Rendering ───────────────────────────────────────────────────────

    /// Description images (96×64 previews, loading placeholders, the "Add" tile) + error line.
    pub(crate) fn desc_images(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let target = Target::Desc(issue.key.clone());
        let full = self.count(&target) >= MAX_IMAGES;
        let previews = issue.images.iter().enumerate().map(|(ix, a)| {
            images::image_preview(("desc-image", ix as u64), a, 96., 64.).into_any_element()
        });
        let loading = self.loading.iter().filter(|l| l.target == target).map(|l| {
            let id = l.id;
            images::loading_thumb(
                ("desc-loading", id),
                l.name.clone(),
                96.,
                64.,
                18.,
                cx.listener(move |this, _, _, cx| this.cancel_loading(id, cx)),
            )
            .into_any_element()
        });
        let add_target = target.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .mt(px(10.))
                    .children(previews)
                    .children(loading)
                    .when(!full, |d| {
                        d.child(
                            images::add_image_tile("desc-add-image").on_click(cx.listener(
                                move |this, _, _, cx| this.pick_images(add_target.clone(), cx),
                            )),
                        )
                    }),
            )
            .when_some(self.desc_error.as_deref(), |d, e| {
                d.child(error_line(e).mt(px(6.)))
            })
    }

    /// Thumbnails + caption inside the comment box; `None` when nothing is attached.
    pub(crate) fn comment_pending_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let loading = self.comment_loading();
        if self.comment_images.is_empty() && loading == 0 {
            return None;
        }
        let ready = self.comment_images.iter().map(|(id, image)| {
            let id = *id;
            images::image_thumb(
                ("comment-image", id),
                image,
                56.,
                40.,
                18.,
                cx.listener(move |this, _, _, cx| this.remove_comment_image(id, cx)),
            )
            .into_any_element()
        });
        let loading_thumbs = self
            .loading
            .iter()
            .filter(|l| l.target == Target::Comment)
            .map(|l| {
                let id = l.id;
                images::loading_thumb(
                    ("comment-loading", id),
                    l.name.clone(),
                    56.,
                    40.,
                    18.,
                    cx.listener(move |this, _, _, cx| this.cancel_loading(id, cx)),
                )
                .into_any_element()
            });
        let caption = images::caption(self.count(&Target::Comment), loading);
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_end()
                .gap(px(8.))
                .px(px(10.))
                .pb(px(8.))
                .text_color(TEXT_MUTED)
                .children(ready)
                .children(loading_thumbs)
                .child(caption.element()),
        )
    }

    /// Error line under the comment box.
    pub(crate) fn comment_error_line(&self) -> Option<gpui::Div> {
        self.comment_error.as_deref().map(error_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posting_rules() {
        assert!(!can_post(true, 0, 0));
        assert!(can_post(false, 0, 0));
        assert!(can_post(true, 1, 0));
        assert!(!can_post(false, 1, 1));
        assert!(!can_post(true, 2, 1));
    }

    #[test]
    fn limit_errors() {
        assert_eq!(limit_error(&Target::Comment), "Up to 10 images per comment");
        assert_eq!(
            limit_error(&Target::Desc("PAY-1".into())),
            "Up to 10 images per description"
        );
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name(Path::new("/a/b/c.png")), "c.png");
    }
}
