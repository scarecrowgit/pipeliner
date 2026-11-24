//! What the shell's header shows for the editor: after the name the draft's version, the live
//! version, how far the draft is from it and whether it is saved; on the right undo and redo,
//! "Replay last 100", "Test run" and "Deploy".
//!
//! Probes: `editor-save-state` (its value is the words shown), `editor-undo`, `editor-redo`,
//! `editor-replay`, `editor-test-run`, `editor-deploy`.

use gpui::{AnyElement, App, Context, IntoElement, SharedString, div, prelude::*, px};
use hub::api::VersionState;
use hub::model::RunnableRef;
use ui::dev::Probe;
use ui::prelude::*;
use ui::tooltip::Tooltip;

use super::EditorView;
use super::load_save::SaveState;

/// The versions the header and the deploy dialog name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Versions {
    /// The version new runs use, if one was deployed.
    pub live: Option<u32>,
    /// The number the next deploy gets.
    pub next: u32,
}

impl EditorView {
    /// The live version and the number of the next deploy.
    pub(crate) fn versions(&self, cx: &App) -> Versions {
        let catalog = self.catalog.read(cx);
        let list = catalog.versions(&self.pipeline).and_then(|v| v.value().cloned());
        let list = list.as_deref().map(Vec::as_slice).unwrap_or_default();
        Versions {
            live: list.iter().filter(|v| v.state == VersionState::Live).map(|v| v.version).max(),
            next: list.iter().map(|v| v.version).max().unwrap_or(0) + 1,
        }
    }

    /// Items waiting for the live version to start.
    pub(crate) fn queued(&self, cx: &App) -> u32 {
        let reference = RunnableRef::pipeline(self.pipeline.as_str());
        self.catalog.read(cx).activity(&reference).map_or(0, |activity| activity.queued)
    }

    /// "2 changes since v14", once the changes are known and something is live.
    pub(crate) fn changes_label(&self, cx: &App) -> Option<SharedString> {
        let live = self.versions(cx).live?;
        let changes = self.doc.changes.as_ref()?;
        Some(format!("{} since v{live}", base::fmt::plural(changes.len() as u64, "change", "changes")).into())
    }

    pub(super) fn badges(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.parts.draft.read(cx).graph().is_none() {
            return Vec::new();
        }
        let versions = self.versions(cx);
        let mut badges = vec![
            div()
                .ml(px(SPACE_2))
                .child(Badge::new(format!("Draft v{}", versions.next)).tone(BadgeTone::Accent))
                .into_any_element(),
        ];
        badges.push(match versions.live {
            Some(live) => Badge::new(format!("v{live} live")).tone(BadgeTone::Success).dot(Status::Live).into_any_element(),
            None => Badge::new("Not deployed").into_any_element(),
        });
        if let Some(changes) = self.changes_label(cx) {
            badges.push(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(changes).into_any_element());
        }
        let state = self.save_state(cx);
        let label = state.label();
        let color = match state {
            SaveState::Failed(_) => DANGER,
            _ => TEXT_MUTED,
        };
        badges.push(
            div()
                .id("editor-save-state")
                .debug_selector(|| "editor-save-state".into())
                .min_w_0()
                .flex_shrink()
                .type_style(LABEL)
                .text_color(color)
                .when(matches!(state, SaveState::Failed(_)), |d| d.tooltip(Tooltip::with_kbd("Save again", "⌘S")))
                .child(truncate_lh(label.clone(), LABEL.line))
                .probe_value("editor-save-state", label)
                .into_any_element(),
        );
        badges
    }

    pub(super) fn actions(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let loaded = self.parts.draft.read(cx).graph().is_some();
        let (undo, redo) = {
            let draft = self.parts.draft.read(cx);
            (draft.can_undo(), draft.can_redo())
        };
        let versions = self.versions(cx);
        let history = |id: &'static str, verb: &'static str, what: Option<&'static str>, keys: &'static str| {
            let tip = match what {
                Some(what) if !what.is_empty() => format!("{verb} {}", what.to_lowercase()),
                _ => verb.to_string(),
            };
            (id, verb, what.is_none(), tip, keys)
        };
        let mut actions = Vec::new();
        for (id, verb, disabled, tip, keys) in
            [history("editor-undo", "Undo", undo, "⌘Z"), history("editor-redo", "Redo", redo, "⇧⌘Z")]
        {
            let glyph = if verb == "Undo" { IconName::Undo } else { IconName::Redo };
            // The button leaves focus on the canvas, so ⌘Z keeps working after a click here.
            let button = IconButton::new(id, glyph).focus_on_click(false).disabled(disabled || !loaded).on_click(
                cx.listener(move |this, _, _, cx| match id {
                    "editor-undo" => this.undo(cx),
                    _ => this.redo(cx),
                }),
            );
            actions.push(
                div()
                    .id(SharedString::from(format!("{id}-tip")))
                    .tooltip(Tooltip::with_kbd(tip, keys))
                    .child(button)
                    .probe(id)
                    .into_any_element(),
            );
        }
        actions.push(
            div()
                .id("editor-replay-tip")
                .tooltip(Tooltip::text("Coming soon"))
                .child(
                    Button::new("editor-replay", "Replay last 100")
                        .ghost()
                        .icon(IconName::Replay)
                        .focus_on_click(false)
                        .disabled(true),
                )
                .probe("editor-replay")
                .into_any_element(),
        );
        actions.push(
            Button::new("editor-test-run", "Test run")
                .secondary()
                .focus_on_click(false)
                .icon(IconName::Play)
                .disabled(!loaded)
                .on_click(cx.listener(|this, _, window, cx| this.test_run_clicked(window, cx)))
                .probe("editor-test-run")
                .into_any_element(),
        );
        actions.push(
            Button::new("editor-deploy", format!("Deploy v{}", versions.next))
                .primary()
                .focus_on_click(false)
                .disabled(!loaded)
                .on_click(cx.listener(|this, _, window, cx| this.open_deploy(window, cx)))
                .probe("editor-deploy")
                .into_any_element(),
        );
        actions
    }
}
