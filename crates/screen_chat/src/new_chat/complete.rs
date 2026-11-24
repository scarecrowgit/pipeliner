//! Path completion of the folder field: while the text reads as a path, the folders inside
//! the directory it names are listed, repos marked; Tab or a click completes the path.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::ChildDir;
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::NewChatView;
use crate::workdir::common::{display_path, expand_path, looks_like_path};

/// Folders listed under a typed path at most.
const COMPLETIONS_SHOWN: usize = 8;

/// The folders inside the directory of the typed path, for the newest request only: an
/// answer for text that has been changed since is dropped.
#[derive(Debug, Default)]
pub(crate) struct DirCompletions {
    generation: u64,
    rows: Vec<ChildDir>,
}

impl DirCompletions {
    /// Starts a request; returns the generation its answer must carry. The rows stay until
    /// the answer is in, so the list does not flicker while typing.
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Takes an answer; `false` (and nothing changes) when a newer request was made since.
    fn finish(&mut self, generation: u64, rows: Vec<ChildDir>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.rows = rows;
        true
    }

    /// Forgets the rows; answers still on their way are dropped.
    fn clear(&mut self) {
        self.generation += 1;
        self.rows.clear();
    }

    /// The folders found.
    pub(crate) fn rows(&self) -> &[ChildDir] {
        &self.rows
    }
}

/// The directory a typed path names and the start of the name inside it:
/// `~/code/ac` is (`~/code`, `ac`) and `~/code/` is (`~/code`, ``).
pub(crate) fn split_typed(text: &str, cx: &App) -> (PathBuf, String) {
    let text = text.trim();
    let full = expand_path(text, cx);
    if text == "~" || text.ends_with('/') {
        return (full, String::new());
    }
    let prefix = full.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    (full.parent().map_or_else(|| full.clone(), Path::to_path_buf), prefix)
}

impl NewChatView {
    /// Asks which folders complete the typed path, or forgets them when the text is no path.
    pub(super) fn complete_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.search.read(cx).text().to_string();
        if !looks_like_path(&text) {
            self.dirs.clear();
            return;
        }
        let (dir, prefix) = split_typed(&text, cx);
        let generation = self.dirs.begin();
        let found = hub::queries(cx).list_dir(&dir, &prefix, COMPLETIONS_SHOWN, cx);
        cx.spawn(async move |this, cx| {
            let rows = found.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.dirs.finish(generation, rows) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Completes the path in the field to `path` and looks beyond it: `~/code/acme/`.
    pub(crate) fn complete_to(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("{}/", display_path(path, cx));
        self.search.update(cx, |input, cx| {
            input.set_text(&text, cx);
            input.focus(window);
        });
        self.look_at_typed(cx);
        cx.notify();
    }

    /// The folders that complete the typed path, the first one Tab takes.
    pub(crate) fn dir_rows(&self) -> &[ChildDir] {
        self.dirs.rows()
    }
}

/// One folder under the typed path: its name, and "repo" when it is a git checkout. Clicking
/// completes the path with it.
pub(super) fn dir_row(
    ix: usize,
    dir: &ChildDir,
    window: &mut Window,
    cx: &mut Context<NewChatView>,
) -> AnyElement {
    let name: SharedString = format!("new-chat-dir-row-{ix}").into();
    let faded = motion::hover_fade_state(name.clone(), Look::bg(TRANSPARENT), Look::bg(SURFACE_RAISED), window, cx);
    let selector = name.to_string();
    let path = dir.path.clone();
    let el = row()
        .id(ElementId::Name(name.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .gap(px(SPACE_3))
        .h(px(36.))
        .px(px(SPACE_3))
        .rounded(px(RADIUS_LG))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| this.complete_to(&path, window, cx)));
    faded
        .apply(el)
        .child(icon(IconName::Folder, 14., TEXT_MUTED))
        .child(div().flex_1().min_w_0().type_style(MONO).child(crate::workdir::common::line_text(dir.name.clone(), MONO.line)))
        .when(dir.is_repo, |d| d.child(Badge::new("repo").square()))
        .probe(name)
        .into_any_element()
}
