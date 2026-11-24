//! Changing definitions: the one save path every panel control uses, the commands that need
//! no dialog (duplicate, pause, chat with it, run history), and the "Edit prompt and tools"
//! dialog.

use gpui::{App, AppContext as _, Context, Entity, IntoElement, Window, div, prelude::*};
use hub::api::{ChatMode, ModelRef, NewSession, Origin, Place, Runnable, RunnableKind, ToolAccess};
use hub::model::RunnableRef;
use hub::Route;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextArea, TextAreaEvent, TextInput, TextInputEvent, text_area_field, text_field};

use crate::dialogs::Form;
use crate::view::{AgentsView, open_editor};

/// Dialog id of "Edit prompt and tools".
pub(crate) const EDIT_DIALOG: &str = "agents-edit-dialog";

/// Where a definition is saved back to; `None` for those that cannot be saved here.
pub(crate) fn place_of(origin: &Origin) -> Option<Place> {
    match origin {
        Origin::Library => Some(Place::Library),
        Origin::Project { repo } => Some(Place::Project(repo.clone())),
        Origin::Builtin | Origin::ClaudeCode { .. } | Origin::Spawned { .. } => None,
    }
}

/// The inputs of the edit dialog.
pub(crate) struct EditForm {
    pub reference: RunnableRef,
    pub description: Entity<TextInput>,
    pub model: Entity<TextInput>,
    pub role: Entity<TextArea>,
    pub access: ToolAccess,
}

const ACCESS: [(ToolAccess, &str); 3] =
    [(ToolAccess::ReadOnly, "Read only"), (ToolAccess::ReadAndRun, "Read and run"), (ToolAccess::Full, "Full access")];

impl AgentsView {
    /// Applies `f` to a copy of the definition and saves it where it lives. Definitions that
    /// cannot be saved here are left alone (their controls are drawn inert).
    pub(crate) fn save(&mut self, reference: &RunnableRef, cx: &mut Context<Self>, f: impl FnOnce(&mut Runnable)) {
        let Some(mut runnable) = self.catalog.read(cx).runnable(reference).cloned() else { return };
        let Some(place) = place_of(&runnable.origin) else { return };
        f(&mut runnable);
        hub::commands(cx).save_runnable(place, runnable, cx);
    }

    /// Copies a runnable and selects the copy.
    pub(crate) fn duplicate(&mut self, reference: &RunnableRef, cx: &mut Context<Self>) {
        let task = hub::commands(cx).duplicate_runnable(reference, cx);
        cx.spawn(async move |this, cx| {
            if let Some(copy) = task.await {
                this.update(cx, |this, cx| this.select(Some(copy), cx)).ok();
            }
        })
        .detach();
    }

    /// Pauses or resumes.
    pub(crate) fn set_paused(&mut self, reference: &RunnableRef, paused: bool, cx: &mut Context<Self>) {
        hub::commands(cx).set_runnable_paused(reference, paused, cx);
    }

    /// Starts a chat with the runnable as the agent and opens it.
    pub(crate) fn chat_with(&mut self, reference: &RunnableRef, cx: &mut Context<Self>) {
        let new = NewSession {
            runnable: Some(reference.clone()),
            mode: ChatMode::Build,
            model: None,
            workdir: None,
            forced_skills: Vec::new(),
            issue: None,
        };
        let task = hub::commands(cx).new_session(new, cx);
        cx.spawn(async move |_, cx| {
            if let Some(session) = task.await {
                cx.update(|cx| hub::navigate(Route::Chat { session: session.id }, cx)).ok();
            }
        })
        .detach();
    }

    /// "Run history": a pipeline's runs are in the editor; an agent's recent work is at the
    /// end of its panel.
    pub(crate) fn show_history(&mut self, reference: &RunnableRef, cx: &mut Context<Self>) {
        if reference.scheme() == "pipeline" {
            open_editor(reference, cx);
            return;
        }
        if self.selected.as_ref() != Some(reference) {
            self.select(Some(reference.clone()), cx);
        }
        // The scroll is applied when the panel is next laid out.
        self.panel_scroll.scroll_to_item(self.panel_sections.saturating_sub(1));
        cx.notify();
    }

    /// "Edit prompt and tools" for an agent or orchestrator; a pipeline opens in the editor.
    pub(crate) fn open_edit(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runnable) = self.catalog.read(cx).runnable(reference).cloned() else { return };
        if runnable.kind == RunnableKind::Pipeline {
            open_editor(reference, cx);
            return;
        }
        if place_of(&runnable.origin).is_none() {
            return;
        }
        let agent = runnable.agent.clone().unwrap_or_default();
        let description = cx.new(|cx| TextInput::new(cx).with_text(&runnable.description).placeholder("What it is for, in one line"));
        let model = cx.new(|cx| TextInput::new(cx).with_text(&agent.model.to_string()).placeholder("The configured default"));
        let role = cx.new(|cx| TextArea::new(cx).with_text(&agent.role).rows(8, 16).placeholder("What it does and how"));
        // The fields hold the keys while the dialog is open: Escape closes it, Enter in a line
        // and Cmd/Ctrl-Enter in the instructions save it.
        let line = |this: &mut Self, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>| match event {
            TextInputEvent::Submit => this.save_edit(window, cx),
            TextInputEvent::Cancel => ui::overlays(window, cx).close(EDIT_DIALOG, window, cx),
            _ => {}
        };
        let subs = vec![
            cx.subscribe_in(&description, window, move |this, _, event, window, cx| line(this, event, window, cx)),
            cx.subscribe_in(&model, window, move |this, _, event, window, cx| line(this, event, window, cx)),
            cx.subscribe_in(&role, window, |this, _, event: &TextAreaEvent, window, cx| match event {
                TextAreaEvent::Submit => this.save_edit(window, cx),
                TextAreaEvent::Cancel => ui::overlays(window, cx).close(EDIT_DIALOG, window, cx),
                _ => {}
            }),
        ];
        self.set_form(Form::Edit(EditForm { reference: reference.clone(), description, model, role, access: agent.tools.access }), subs);
        let view = cx.entity();
        let title = format!("Edit {}", crate::dialogs::short_name(&runnable.name));
        ui::overlays(window, cx).open_dialog(
            EDIT_DIALOG,
            move |_, cx| edit_dialog(&view, &title, cx),
            window,
            cx,
        );
    }

    /// Saves the edit dialog.
    pub(crate) fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some(Form::Edit(form)) = self.form.take() else { return };
        let description = form.description.read(cx).text().trim().to_string();
        let model = form.model.read(cx).text().trim().to_string();
        let role = form.role.read(cx).text().to_string();
        let access = form.access;
        self.save(&form.reference, cx, move |r| {
            r.description = description;
            let agent = r.agent.get_or_insert_with(Default::default);
            agent.model = if model.is_empty() { ModelRef::default() } else { ModelRef::parse(&model) };
            agent.role = role;
            agent.tools.access = access;
        });
    }

    /// Saves the edit dialog and closes it.
    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        ui::overlays(window, cx).close(EDIT_DIALOG, window, cx);
    }

    fn set_access(&mut self, access: ToolAccess, cx: &mut Context<Self>) {
        if let Some(Form::Edit(form)) = self.form.as_mut() {
            form.access = access;
            cx.notify();
        }
    }
}

fn edit_dialog(view: &Entity<AgentsView>, title: &str, cx: &App) -> gpui::AnyElement {
    let Some(Form::Edit(form)) = view.read(cx).form.as_ref() else {
        return div().into_any_element();
    };
    let selected = ACCESS.iter().position(|(a, _)| *a == form.access).unwrap_or(2);
    let pick = view.clone();
    let save = view.clone();
    let segmented = ACCESS
        .iter()
        .fold(Segmented::new("agents-edit-tools").full_width(), |s, (_, label)| s.option(*label))
        .selected(selected)
        .on_select(move |ix, _, cx| pick.update(cx, |this, cx| this.set_access(ACCESS[ix].0, cx)));
    Dialog::new(EDIT_DIALOG, title.to_string())
        .width(640.)
        .subtitle("Saved to its file; running work keeps the old definition")
        .child(Field::new("Description").child(text_field("agents-edit-description", &form.description).probe("agents-edit-description")))
        .child(
            Field::new("Model")
                .hint("provider/model, or empty for the default")
                .child(text_field("agents-edit-model", &form.model).probe("agents-edit-model")),
        )
        .child(Field::new("Tools").child(segmented))
        .child(Field::new("Instructions").hint("Markdown").child(text_area_field("agents-edit-role", &form.role).probe("agents-edit-role")))
        .cancel_button()
        .primary_action(
            Button::new("agents-edit-save", "Save").primary().kbd("⌘↵"),
            move |_, cx| save.update(cx, |this, cx| this.commit_edit(cx)),
        )
        .into_any_element()
}
