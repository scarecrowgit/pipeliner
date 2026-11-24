//! The deploy dialog: what changed since the live version, how the last test run went, the
//! problems that block a deploy (errors) or are worth a look (warnings), where the new version
//! rolls out to, and a note for the changelog. Deploy saves the draft first, then asks the
//! engine for the next version and says which one it got.
//!
//! Probes: `deploy-change-<n>`, `deploy-open-run`, `deploy-blocked`, `deploy-note`,
//! `deploy-confirm`.

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, IntoElement, Subscription, WeakEntity, Window,
    div, prelude::*, px,
};
use hub::api::{GraphChange, GraphDef, NodeKind, RunState};
use hub::model::{NodeId, Problem, Severity};
use ui::dev::Probe;
use ui::prelude::*;
use ui::tooltip::Tooltip;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::EditorView;
use super::dock::DockTab;
use super::problems::sorted;

/// The dialog's overlay id.
pub(crate) const DIALOG_ID: &str = "deploy-dialog";
/// Width and distance from the top, as drawn.
const DIALOG_WIDTH: f32 = 600.0;
const DIALOG_TOP: f32 = 150.0;
/// Height of a change row.
const CHANGE_ROW_HEIGHT: f32 = 36.0;

/// The dialog's own state; everything else is read from the editor each frame.
pub(crate) struct DeployDialog {
    editor: WeakEntity<EditorView>,
    pub(crate) note: Entity<TextInput>,
    /// The note took focus after the dialog's own first focus.
    focused: bool,
    _subscriptions: Vec<Subscription>,
}

impl DeployDialog {
    fn new(editor: WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let note = cx.new(|cx| TextInput::new(cx).placeholder("What changed, in a line"));
        let subscriptions = vec![cx.subscribe_in(&note, window, |this, _, event: &TextInputEvent, window, cx| {
            if *event == TextInputEvent::Submit {
                this.confirm(window, cx);
            }
        })];
        Self { editor, note, focused: false, _subscriptions: subscriptions }
    }

    /// Deploys with the note and closes, unless something blocks the deploy.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = self.note.read(cx).text().trim().to_string();
        let Some(editor) = self.editor.upgrade() else { return };
        if editor.read(cx).deploy_blocked(cx) {
            return;
        }
        editor.update(cx, |editor, cx| editor.deploy(note, window, cx));
        overlays(window, cx).close(DIALOG_ID, window, cx);
    }

    /// The dialog for this frame. Built inside the overlay host's layer.
    fn dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.focused {
            // The dialog focuses its first control once drawn; the note takes it a turn later.
            self.focused = true;
            let note = self.note.clone();
            window.defer(cx, move |window, cx| {
                window.defer(cx, move |window, cx| note.read(cx).focus(window));
            });
        }
        let Some(editor) = self.editor.upgrade() else { return div().into_any_element() };
        let editor = editor.read(cx);
        let versions = editor.versions(cx);
        let name = editor.title(cx);
        let draft = editor.parts.draft.read(cx);
        let graph = draft.graph().cloned();
        let problems = sorted(draft.problems());
        let current = draft.problems_are_current();
        let blocked = editor.deploy_blocked(cx);
        let changes = editor.doc.changes.clone();
        let queued = editor.queued(cx);
        let run = editor.shown_run(cx);

        let mut body: Vec<AnyElement> = Vec::new();
        let since = match versions.live {
            Some(live) => format!("Changes since v{live}"),
            None => "What is in it".to_string(),
        };
        body.push(
            col()
                .gap(px(SPACE_1_5))
                .child(
                    row()
                        .child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child(since))
                        .child(
                            div()
                                .id("deploy-compare")
                                .type_style(LABEL)
                                .text_color(TEXT_MUTED)
                                .tooltip(Tooltip::text("Coming soon"))
                                .child("Compare canvases"),
                        ),
                )
                .child(changes_card(changes.as_deref(), graph.as_ref()))
                .into_any_element(),
        );
        body.push(self.run_row(run.as_deref(), cx));
        if let Some(problems_block) = problems_banner(&problems, current, graph.as_ref()) {
            body.push(problems_block);
        }
        let all = if queued > 0 {
            format!("{} start on v{} as soon as it’s live", base::fmt::plural(queued.into(), "queued issue", "queued issues"), versions.next)
        } else {
            format!("New issues start on v{} as soon as it’s live", versions.next)
        };
        let staged_desc = match versions.live {
            Some(live) => format!("The rest stay on v{live}. Promote or roll back from Versions"),
            None => "The rest wait. Promote or roll back from Versions".to_string(),
        };
        body.push(
            Field::new("Roll out to")
                .child(
                    col()
                        .gap(px(SPACE_1_5))
                        .child(OptionCard::new("deploy-rollout-all", "All new issues").description(all).selected(true).on_select(|_, _, _| {}))
                        .child(
                            div().opacity(0.5).child(
                                OptionCard::new("deploy-rollout-staged", "1 in 10 new issues for 24 hours")
                                    .description(staged_desc)
                                    .right(Badge::new("Coming soon")),
                            ),
                        ),
                )
                .into_any_element(),
        );
        body.push(
            Field::new("Note for the changelog")
                .child(
                    div()
                        .debug_selector(|| "deploy-note".into())
                        .child(text_field("deploy-note-field", &self.note))
                        .probe("deploy-note"),
                )
                .into_any_element(),
        );

        let confirm = Button::new("deploy-confirm", format!("Deploy v{}", versions.next))
            .primary()
            .kbd("⌘↵")
            .disabled(blocked)
            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx)));
        let key = cx.entity().downgrade();
        let subtitle = match versions.live {
            Some(live) => format!("Replaces v{live} for new issues"),
            None => format!("The first version of {name}"),
        };
        let mut dialog = Dialog::new(DIALOG_ID, format!("Deploy v{} of {name}", versions.next))
            .width(DIALOG_WIDTH)
            .top(DIALOG_TOP)
            .subtitle(subtitle)
            .children(body)
            .cancel_button()
            .action(div().child(confirm).probe("deploy-confirm"))
            .on_confirm(move |window, cx| {
                key.update(cx, |this, cx| this.confirm(window, cx)).ok();
            });
        if let Some(live) = versions.live {
            dialog = dialog.note(format!("Started runs stay on v{live}"));
        }
        dialog.into_any_element()
    }

    /// How the test run on show went, with a way to it.
    fn run_row(&self, run: Option<&hub::api::Run>, cx: &mut Context<Self>) -> AnyElement {
        let (mark, tone, title, detail) = match run {
            Some(run) => {
                let secs = ((run.finished.unwrap_or_else(base::time::now_ms) - run.started) / 1000).max(0) as u64;
                let facts = format!("{} · {}", base::fmt::duration_words(secs), base::fmt::cost(run.cost_usd));
                let (mark, color, verb) = match run.state {
                    RunState::Done => (IconName::Check, SUCCESS, "passed"),
                    RunState::Failed | RunState::Interrupted => (IconName::Alert, DANGER, "failed"),
                    RunState::Cancelled => (IconName::X, TEXT_MUTED, "was stopped"),
                    _ => (IconName::Clock, INFO, "is running"),
                };
                (mark, color, format!("Test run {verb} on {}", run.title), facts)
            }
            None => (
                IconName::Play,
                TEXT_MUTED,
                "No test run of this draft yet".to_string(),
                "A test run tries the draft on one item with real agents.".to_string(),
            ),
        };
        let editor = self.editor.clone();
        let link = if run.is_some() { "Open run" } else { "Test run" };
        row()
            .gap(px(SPACE_2_5))
            .px(px(SPACE_3))
            .py(px(SPACE_2_5))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_LG))
            .child(div().size(px(24.)).flex().flex_none().items_center().justify_center().rounded(px(RADIUS_MD)).bg(SURFACE_HOVER).child(icon(mark, 14., tone)))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().child(title))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(detail)),
            )
            .child(
                div()
                    .type_style(LABEL)
                    .child(Link::new("deploy-open-run", link).on_click(cx.listener(move |_, _, window, cx| {
                        editor
                            .update(cx, |editor, cx| {
                                editor.open_tab(DockTab::TestRun, cx);
                                if editor.test.shown.is_none() {
                                    editor.test.title.read(cx).focus(window);
                                }
                            })
                            .ok();
                        overlays(window, cx).close(DIALOG_ID, window, cx);
                    })))
                    .probe("deploy-open-run"),
            )
            .into_any_element()
    }
}

/// The icon of a node, by kind.
fn kind_icon(kind: &NodeKind) -> IconName {
    match kind {
        NodeKind::Source(_) => IconName::Source,
        NodeKind::Router(_) => IconName::Router,
        NodeKind::Agent(_) => IconName::Agent,
        NodeKind::Orchestrator(_) => IconName::Orchestrator,
        NodeKind::Subagent(_) => IconName::Spawn,
        NodeKind::File(_) => IconName::File,
        NodeKind::Review(_) => IconName::Inbox,
        NodeKind::Action(_) => IconName::Zap,
        NodeKind::Pipeline(_) => IconName::Pipelines,
    }
}

/// The rows of "Changes since vN".
fn changes_card(changes: Option<&[GraphChange]>, graph: Option<&GraphDef>) -> AnyElement {
    let frame = col().bg(SURFACE).border_1().border_color(LINE).rounded(px(RADIUS_XL)).overflow_hidden();
    let Some(changes) = changes else {
        return frame.child(change_note("Reading the changes…")).into_any_element();
    };
    if changes.is_empty() {
        return frame.child(change_note("Nothing changed since the live version")).into_any_element();
    }
    let node_icon = |node: &NodeId| graph.and_then(|g| g.node(node)).map_or(IconName::Trash, |n| kind_icon(&n.kind));
    let rows = changes.iter().enumerate().map(|(ix, change)| {
        let (sign, color, mark, name, what) = match change {
            GraphChange::Added { node, name, what } => ("+", SUCCESS, node_icon(node), name.clone(), what.clone()),
            GraphChange::Removed { node, name } => ("−", DANGER, node_icon(node), name.clone(), "removed".to_string()),
            GraphChange::Changed { node, name, what } => ("~", WARNING, node_icon(node), name.clone(), what.clone()),
            GraphChange::Wiring { what } => ("·", TEXT_MUTED, IconName::Branch, "Wiring".to_string(), what.clone()),
        };
        row()
            .id(ElementId::Name(format!("deploy-change-{ix}").into()))
            .debug_selector(move || format!("deploy-change-{ix}"))
            .min_h(px(CHANGE_ROW_HEIGHT))
            .px(px(SPACE_3))
            .gap(px(SPACE_2_5))
            .when(ix > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
            .child(div().w(px(14.)).flex_none().flex().justify_center().type_style(MONO).text_color(color).child(sign))
            .child(div().size(px(20.)).flex().flex_none().items_center().justify_center().rounded(px(RADIUS_MD)).bg(SURFACE_HOVER).child(icon(mark, 12., TEXT_SECONDARY)))
            .child(div().flex_none().type_style(BODY_STRONG).child(name))
            .child(div().flex_1().min_w_0().type_style(LABEL).text_color(TEXT_SECONDARY).child(truncate_lh(what, LABEL.line)))
            .probe(format!("deploy-change-{ix}"))
    });
    frame.children(rows).into_any_element()
}

fn change_note(text: &'static str) -> impl IntoElement {
    row().min_h(px(CHANGE_ROW_HEIGHT)).px(px(SPACE_3)).type_style(LABEL).text_color(TEXT_MUTED).child(text)
}

/// Errors that block the deploy and warnings worth a look; a note while checking.
fn problems_banner(problems: &[Problem], current: bool, graph: Option<&GraphDef>) -> Option<AnyElement> {
    if !current {
        return Some(
            div()
                .debug_selector(|| "deploy-blocked".into())
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Checking the draft…")
                .probe("deploy-blocked")
                .into_any_element(),
        );
    }
    if problems.is_empty() {
        return None;
    }
    let errors = problems.iter().filter(|p| p.severity == Severity::Error).count();
    let (tone, color, title) = if errors > 0 {
        (BannerTone::Danger, DANGER, format!("Fix {} before deploying", base::fmt::plural(errors as u64, "error", "errors")))
    } else {
        (BannerTone::Warning, WARNING, base::fmt::plural(problems.len() as u64, "warning", "warnings"))
    };
    let line = |p: &Problem| {
        let name = match (&p.node, graph) {
            (Some(node), Some(graph)) => graph.node(node).map_or_else(|| node.to_string(), |n| n.name.clone()),
            (Some(node), None) => node.to_string(),
            (None, _) => "Pipeline".to_string(),
        };
        let word = if p.severity == Severity::Error { "error" } else { "warning" };
        format!("{name}: {} ({word})", p.message)
    };
    Some(
        div()
            .debug_selector(|| "deploy-blocked".into())
            .child(Banner::new()
            .tone(tone)
            .child(icon(IconName::Alert, 16., color))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().type_style(BODY_STRONG).child(title))
                    .children(problems.iter().map(|p| div().type_style(LABEL).text_color(TEXT_SECONDARY).child(line(p)))),
            ))
            .probe("deploy-blocked")
            .into_any_element(),
    )
}

impl EditorView {
    /// Errors in the draft, or a check still running, keep it from being deployed.
    pub(crate) fn deploy_blocked(&self, cx: &App) -> bool {
        let draft = self.parts.draft.read(cx);
        draft.graph().is_none()
            || !draft.problems_are_current()
            || draft.problems().iter().any(|p| p.severity == Severity::Error)
    }

    /// Saves and opens the deploy dialog.
    pub(super) fn open_deploy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.parts.draft.read(cx).graph().is_none() {
            return;
        }
        self.keep_mine(cx);
        self.save_now(cx);
        self.refresh_changes(cx);
        if !self.parts.draft.read(cx).problems_are_current() {
            self.schedule_check(cx);
        }
        let editor = cx.entity().downgrade();
        let dialog = cx.new(|cx| DeployDialog::new(editor, window, cx));
        self.deploy = Some(dialog.clone());
        overlays(window, cx).open_dialog(
            DIALOG_ID,
            move |window, cx| dialog.update(cx, |dialog, cx| dialog.dialog(window, cx)),
            window,
            cx,
        );
    }

    /// Closes the deploy dialog if it is open.
    pub(super) fn close_deploy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deploy.take().is_some() {
            overlays(window, cx).close(DIALOG_ID, window, cx);
        }
    }

    /// The deploy dialog's state, while it is open.
    #[cfg(test)]
    pub(crate) fn deploy_dialog(&self) -> Option<&Entity<DeployDialog>> {
        self.deploy.as_ref()
    }

    /// Saves, then deploys the draft as the next version with `note`; a toast names it.
    pub(crate) fn deploy(&mut self, note: String, window: &mut Window, cx: &mut Context<Self>) {
        self.deploy = None;
        self.save_now(cx);
        let name = self.title(cx);
        let deployed = hub::commands(cx).deploy_pipeline(&self.pipeline, &note, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(version) = deployed.await else { return };
            this.update_in(cx, |this, window, cx| {
                overlays(window, cx).toast(Toast::new(format!("Deployed v{}", version.version)).detail(name.to_string()), cx);
                this.refresh_changes(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
