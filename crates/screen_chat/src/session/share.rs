//! The share popover (`chat-share.html`): who can open a hosted copy (coming soon), what an
//! export includes, and the two local exports (`.md`, `.jsonl`) written through the API to a
//! path picked in the platform's save dialog.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{ExportFormat, ExportOptions};
use ui::components::PopSeparator;
use ui::dev::Probe;
use ui::prelude::*;

use super::SessionView;
use super::header::SHARE;

/// What the exports include, as the popover's toggles set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ShareOptions {
    pub subagents: bool,
    pub tool_output: bool,
    pub images: bool,
    pub local_paths: bool,
}

impl Default for ShareOptions {
    fn default() -> Self {
        Self { subagents: true, tool_output: true, images: true, local_paths: false }
    }
}

impl From<ShareOptions> for ExportOptions {
    fn from(o: ShareOptions) -> Self {
        ExportOptions { subagents: o.subagents, tool_output: o.tool_output, images: o.images, local_paths: o.local_paths }
    }
}

/// The user's home, for showing paths as `~/…` and as the export folder of a chat with none.
fn home() -> PathBuf {
    base::paths::Paths::from_env().home
}

/// `path` with the home folder written as `~`.
pub(super) fn display_path(path: &Path) -> String {
    base::paths::Paths::from_env().display(path)
}

impl SessionView {
    /// Opens the share popover under the header's Share button.
    pub(super) fn open_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of(SHARE, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_popover(anchor, Placement::below().end(), move |_, cx| popover(&view, cx), window, cx);
    }

    fn set_share(&mut self, f: impl FnOnce(&mut ShareOptions), cx: &mut Context<Self>) {
        f(&mut self.share);
        cx.notify();
    }

    /// Asks where to write the transcript, then has the engine write it there.
    fn export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        ui::overlays(window, cx).close(SHARE, window, cx);
        let dir = self.header_session(cx).and_then(|s| s.workdir.as_ref()).map_or_else(home, |w| w.path.clone());
        let ext = match format {
            ExportFormat::Markdown => "md",
            ExportFormat::Jsonl => "jsonl",
        };
        let name = format!("{}.{ext}", base::id::slug(&self.title(cx), 40));
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        let (id, options) = (self.session.clone(), ExportOptions::from(self.share));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            let Ok(written) = cx.update(|_, cx| hub::commands(cx).export_session(&id, format, options, &path, cx)) else {
                return;
            };
            if let Some(written) = written.await {
                this.update_in(cx, |_, window, cx| {
                    let toast = Toast::new("Chat exported").detail(display_path(&written));
                    ui::overlays(window, cx).toast(toast, cx);
                })
                .ok();
            }
        })
        .detach();
    }
}

fn popover(view: &Entity<SessionView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let options = this.share;
    let children: Vec<String> = this
        .model
        .read(cx)
        .detail()
        .map(|d| d.children.iter().map(|c| format!("@{}", c.runnable.id())).collect())
        .unwrap_or_default();
    let subagents = if children.is_empty() {
        "No sub-agent sessions".to_string()
    } else {
        format!("{} · {}", children.join(", "), base::fmt::plural(children.len() as u64, "session", "sessions"))
    };
    let toggle = |id: &'static str, label: &'static str, sub: SharedString, on: bool, set: fn(&mut ShareOptions, bool)| {
        let v = view.clone();
        row()
            .min_h(px(36.))
            .gap(px(SPACE_2_5))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(label)
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(sub)),
            )
            .child(
                Toggle::new(id, on)
                    .on_change(move |on, _, cx| v.update(cx, |this, cx| this.set_share(|o| set(o, on), cx)))
                    .probe(id),
            )
    };
    let include = col()
        .px(px(SPACE_2_5))
        .pt(px(SPACE_1_5))
        .pb(px(SPACE_2))
        .child(div().mb(px(SPACE_0_5)).type_style(LABEL_STRONG).text_color(TEXT_SECONDARY).child("Include"))
        .child(toggle("chat-share-subagents", "Sub-agent sessions", subagents.into(), options.subagents, |o, on| o.subagents = on))
        .child(toggle(
            "chat-share-tool-output",
            "Tool output and diffs",
            "Secrets and tokens are redacted".into(),
            options.tool_output,
            |o, on| o.tool_output = on,
        ))
        .child(toggle("chat-share-images", "Images", "Pasted and attached images".into(), options.images, |o, on| o.images = on))
        .child(toggle(
            "chat-share-local-paths",
            "Local paths",
            "~/.harness/… paths show as the repo path".into(),
            options.local_paths,
            |o, on| o.local_paths = on,
        ))
        .child(
            row()
                .mt(px(SPACE_2_5))
                .gap(px(SPACE_1_5))
                .child(
                    div().flex_1().min_w_0().child(
                        InputFrame::new("chat-share-link-field")
                            .small()
                            .sunken()
                            .icon(IconName::Lock)
                            .placeholder("Hosted links are coming soon"),
                    ),
                )
                .child(Button::new("chat-share-copy", "Copy link").secondary().small().disabled(true)),
        );
    let export = |id: &'static str, label: &'static str, hint: &'static str, icon: IconName, format: ExportFormat| {
        let v = view.clone();
        MenuItem::new(id, label)
            .icon(icon)
            .hint(hint)
            .on_click(move |_, window, cx| v.update(cx, |this, cx| this.export(format, window, cx)))
            .probe(id)
    };
    Popover::new()
        .width(400.)
        .title("Share this chat")
        .subtitle("A read-only copy of the thread as it is now. New messages show up as you send them.")
        .child(MenuItem::new("chat-share-only-me", "Only me").subline("Nobody else can open it").checked(true))
        .child(MenuItem::new("chat-share-workspace", "People in your workspace").subline("Can read and fork, can’t reply").check_column().coming_soon())
        .child(MenuItem::new("chat-share-anyone", "Anyone with the link").subline("Read-only, no sign-in").check_column().coming_soon())
        .child(PopSeparator)
        .child(include)
        .child(PopSeparator)
        .child(export("chat-share-markdown", "Export as Markdown", ".md", IconName::File, ExportFormat::Markdown))
        .child(export("chat-share-jsonl", "Export transcript", ".jsonl · with tool calls", IconName::Download, ExportFormat::Jsonl))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_map_one_to_one() {
        let o = ShareOptions { subagents: false, tool_output: true, images: false, local_paths: true };
        let e = ExportOptions::from(o);
        assert_eq!((e.subagents, e.tool_output, e.images, e.local_paths), (false, true, false, true));
    }
}
