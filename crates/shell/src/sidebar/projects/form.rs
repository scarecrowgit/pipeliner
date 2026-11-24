//! The project form: "New project…" and "Project settings" (name, key prefix, repo folders),
//! and deleting a project after a confirmation.
//!
//! Probes: `project-form-name`, `project-form-prefix`, `project-form-path`,
//! `project-form-add`, `project-form-repo-<n>-remove`, `project-form-save`,
//! `project-form-delete`, `project-delete-confirm`.

use std::path::PathBuf;

use gpui::{AnyElement, App, AppContext as _, Context, ElementId, Entity, IntoElement, SharedString, Subscription, Window, div, prelude::*, px};
use model::{Project, ProjectId};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::switch_to;

/// Dialog id of the project form.
pub(crate) const FORM_DIALOG: &str = "project-form";
/// Dialog id of the delete confirmation.
pub(crate) const DELETE_DIALOG: &str = "project-delete";
/// Longest key prefix.
pub const PREFIX_MAX: usize = 6;

/// The project form's state.
pub(crate) struct ProjectForm {
    /// The project being edited; `None` for a new one.
    editing: Option<Project>,
    others: Vec<Project>,
    name: Entity<TextInput>,
    prefix: Entity<TextInput>,
    path: Entity<TextInput>,
    repos: Vec<PathBuf>,
    /// The prefix was typed by hand (a new project's prefix follows its name until then).
    prefix_typed: bool,
    path_error: Option<SharedString>,
    error: Option<SharedString>,
    checking: bool,
    _subscriptions: Vec<Subscription>,
}

/// Upper-case ASCII letters of `text`, at most [`PREFIX_MAX`].
pub fn clean_prefix(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_uppercase()).take(PREFIX_MAX).collect()
}

/// The prefix a new project's name suggests: the first letters of its words, else of the name.
pub fn suggest_prefix(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().any(|c| c.is_ascii_alphabetic())).collect();
    if words.len() >= 2 {
        clean_prefix(&words.iter().filter_map(|w| w.chars().find(char::is_ascii_alphabetic)).collect::<String>())
    } else {
        clean_prefix(name).chars().take(3).collect()
    }
}

/// What is wrong with a project as typed, if anything.
pub fn problem(name: &str, prefix: &str, others: &[Project]) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("Give the project a name");
    }
    if prefix.len() < 2 {
        return Some("The key prefix needs 2 to 6 letters");
    }
    if others.iter().any(|p| p.key_prefix.eq_ignore_ascii_case(prefix)) {
        return Some("Another project uses this key prefix");
    }
    None
}

impl ProjectForm {
    fn new(editing: Option<Project>, others: Vec<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (name_text, prefix_text, repos) = match &editing {
            Some(p) => (p.name.clone(), p.key_prefix.clone(), p.repos.clone()),
            None => (String::new(), String::new(), Vec::new()),
        };
        let name = cx.new(|cx| TextInput::new(cx).with_text(&name_text).placeholder("Payments"));
        let prefix = cx.new(|cx| TextInput::new(cx).with_text(&prefix_text).placeholder("PAY"));
        let path = cx.new(|cx| TextInput::new(cx).placeholder("~/code/payments-api"));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this: &mut Self, _, event, _, cx| {
                if *event == TextInputEvent::Changed && this.editing.is_none() && !this.prefix_typed {
                    let suggested = suggest_prefix(this.name.read(cx).text());
                    this.prefix.update(cx, |input, cx| input.set_text(&suggested, cx));
                }
                this.error = None;
                cx.notify();
            }),
            cx.subscribe_in(&prefix, window, |this: &mut Self, _, event, _, cx| {
                if *event == TextInputEvent::Changed {
                    this.prefix_typed = true;
                    let text = this.prefix.read(cx).text().to_string();
                    let clean = clean_prefix(&text);
                    if clean != text {
                        this.prefix.update(cx, |input, cx| input.set_text(&clean, cx));
                    }
                    this.error = None;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&path, window, |this: &mut Self, _, event, _, cx| match event {
                TextInputEvent::Submit => this.add_path(cx),
                TextInputEvent::Changed => {
                    this.path_error = None;
                    cx.notify();
                }
                _ => {}
            }),
        ];
        Self {
            editing,
            others,
            name,
            prefix,
            path,
            repos,
            prefix_typed: false,
            path_error: None,
            error: None,
            checking: false,
            _subscriptions: subscriptions,
        }
    }

    /// Check the typed folder and add it to the repos.
    fn add_path(&mut self, cx: &mut Context<Self>) {
        let typed = self.path.read(cx).text().trim().to_string();
        if typed.is_empty() || self.checking {
            return;
        }
        let path = hub::hub(cx).read(cx).paths().expand(&typed);
        if self.repos.contains(&path) {
            self.path_error = Some("This folder is already in the list".into());
            cx.notify();
            return;
        }
        self.checking = true;
        cx.notify();
        let exists = hub::queries(cx).folder_exists(&path, cx);
        cx.spawn(async move |this, cx| {
            let exists = exists.await;
            this.update(cx, |this, cx| {
                this.checking = false;
                if exists {
                    this.repos.push(path);
                    this.path.update(cx, |input, cx| input.set_text("", cx));
                } else {
                    this.path_error = Some(format!("No folder at {}", hub::display_path(&path, cx)).into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_repo(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.repos.len() {
            self.repos.remove(ix);
            cx.notify();
        }
    }

    /// Save the project; `false` (with the reason shown) when it is not valid yet.
    fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let name = self.name.read(cx).text().trim().to_string();
        let prefix = clean_prefix(self.prefix.read(cx).text());
        if let Some(problem) = problem(&name, &prefix, &self.others) {
            self.error = Some(problem.into());
            cx.notify();
            return false;
        }
        let id = match &self.editing {
            Some(project) => project.id.clone(),
            None => fresh_id(&name, &self.others),
        };
        let project = Project { id: id.clone(), name, key_prefix: prefix, repos: self.repos.clone() };
        hub::commands(cx).save_project(project, cx);
        if self.editing.is_none() {
            switch_to(&id, cx);
        }
        true
    }
}

/// An id for a new project: the slug of its name, made unique.
fn fresh_id(name: &str, others: &[Project]) -> ProjectId {
    let base = base::id::slug(name, 32);
    let taken = |id: &str| others.iter().any(|p| p.id.as_str() == id);
    if !taken(&base) {
        return ProjectId::new(base);
    }
    let n = (2..).find(|n| !taken(&format!("{base}-{n}"))).unwrap_or(2);
    ProjectId::new(format!("{base}-{n}"))
}

/// "New project…".
pub(crate) fn open_new(window: &mut Window, cx: &mut App) {
    let others = hub::ops(cx).read(cx).projects().value().map(|p| p.to_vec()).unwrap_or_default();
    open_form(None, others, window, cx);
}

/// "Project settings" for the current project.
pub(crate) fn open_edit(window: &mut Window, cx: &mut App) {
    let ops = hub::ops(cx);
    let ops = ops.read(cx);
    let Some(current) = ops.current_project().cloned() else { return };
    let others: Vec<Project> =
        ops.projects().value().map(|p| p.iter().filter(|p| p.id != current.id).cloned().collect()).unwrap_or_default();
    open_form(Some(current), others, window, cx);
}

fn open_form(editing: Option<Project>, others: Vec<Project>, window: &mut Window, cx: &mut App) {
    // The dialog focuses its first control, the name.
    let form = cx.new(|cx| ProjectForm::new(editing, others, window, cx));
    ui::overlays(window, cx).open_dialog(FORM_DIALOG, move |_, cx| render(&form, cx), window, cx);
}

fn render(form: &Entity<ProjectForm>, cx: &App) -> AnyElement {
    let this = form.read(cx);
    let editing = this.editing.clone();
    let prefix = clean_prefix(this.prefix.read(cx).text());
    let example = if prefix.is_empty() { "Issues get keys like PAY-12".to_string() } else { format!("Issues get keys like {prefix}-12") };
    let prefix_hint = match &editing {
        Some(project) if project.key_prefix != prefix && !prefix.is_empty() => {
            format!("{example}. Issues made before keep {}-", project.key_prefix)
        }
        _ => example,
    };
    let mut repos = col().gap(px(SPACE_1));
    for (ix, path) in this.repos.iter().enumerate() {
        let remove_id: SharedString = format!("project-form-repo-{ix}-remove").into();
        let remove = form.clone();
        repos = repos.child(
            row()
                .gap(px(SPACE_2))
                .h(px(CONTROL_HEIGHT_SM))
                .child(icon(IconName::Folder, 14., TEXT_MUTED))
                .child(truncate(hub::display_path(path, cx)).type_style(MONO))
                .child(
                    IconButton::new(ElementId::Name(remove_id.clone()), IconName::X)
                        .small()
                        .tooltip("Remove")
                        .on_click(move |_, _, cx| remove.update(cx, |this, cx| this.remove_repo(ix, cx)))
                        .probe(remove_id),
                ),
        );
    }
    let add = form.clone();
    let add_row = row()
        .gap(px(SPACE_2))
        .child(div().flex_1().child(text_field("project-form-path", &this.path).small().icon(IconName::Folder).probe("project-form-path")))
        .child(
            Button::new("project-form-add", if this.checking { "Checking…" } else { "Add folder" })
                .small()
                .disabled(this.checking)
                .on_click(move |_, _, cx| add.update(cx, |this, cx| this.add_path(cx)))
                .probe("project-form-add"),
        );
    let save = form.clone();
    let commit = move |window: &mut Window, cx: &mut App| {
        if save.update(cx, |this, cx| this.save(cx)) {
            ui::overlays(window, cx).close(FORM_DIALOG, window, cx);
        }
    };
    let commit_click = commit.clone();
    let title = match &editing {
        Some(project) => format!("{} settings", project.name),
        None => "New project".to_string(),
    };
    let mut dialog = Dialog::new(FORM_DIALOG, title)
        .width(520.)
        .subtitle("A project groups local repos and gives its issues their keys")
        .child(Field::new("Name").child(text_field("project-form-name", &this.name).probe("project-form-name")))
        .child(
            Field::new("Key prefix")
                .hint(prefix_hint)
                .child(text_field("project-form-prefix", &this.prefix).probe("project-form-prefix")),
        )
        .child(
            Field::new("Repo folders")
                .hint("Each folder is checked before it is added")
                .child(repos)
                .child(add_row)
                .children(this.path_error.clone().map(|e| div().type_style(LABEL).text_color(DANGER).child(e))),
        )
        .children(this.error.clone().map(|e| div().type_style(LABEL).text_color(DANGER).child(e)))
        .cancel_button()
        .action(
            Button::new("project-form-save", if editing.is_some() { "Save" } else { "Create project" })
                .primary()
                .kbd("⌘↵")
                .on_click(move |_, window, cx| commit_click(window, cx))
                .probe("project-form-save"),
        )
        .on_confirm(move |window, cx| commit(window, cx));
    if let Some(project) = editing {
        let last = this.others.is_empty();
        dialog = dialog.note(
            Button::new("project-form-delete", "Delete project…")
                .danger()
                .small()
                .disabled(last)
                .on_click(move |_, window, cx| open_delete(project.clone(), window, cx))
                .probe("project-form-delete"),
        );
    }
    dialog.into_any_element()
}

/// Ask before deleting a project; its issues stay in the store.
fn open_delete(project: Project, window: &mut Window, cx: &mut App) {
    ui::overlays(window, cx).open_dialog(
        DELETE_DIALOG,
        move |_, _| {
            let project = project.clone();
            Dialog::new(DELETE_DIALOG, format!("Delete {}?", project.name))
                .width(480.)
                .child(div().text_color(TEXT_SECONDARY).child(format!(
                    "The project leaves Harness. Its issues ({}-…) stay in the store, and its repo folders are not touched.",
                    project.key_prefix
                )))
                .cancel_button()
                .action(
                    Button::new("project-delete-confirm", "Delete project")
                        .danger()
                        .on_click(move |_, window, cx| delete(&project.id, window, cx))
                        .probe("project-delete-confirm"),
                )
                .into_any_element()
        },
        window,
        cx,
    );
}

/// Delete `id`; when it was the current project another one becomes current.
fn delete(id: &ProjectId, window: &mut Window, cx: &mut App) {
    let ops = hub::ops(cx);
    let next = ops
        .read(cx)
        .projects()
        .value()
        .and_then(|projects| projects.iter().find(|p| &p.id != id).map(|p| p.id.clone()));
    let was_current = ops.read(cx).current_project().is_some_and(|p| &p.id == id);
    hub::commands(cx).delete_project(id, cx);
    if was_current && let Some(next) = next {
        switch_to(&next, cx);
    }
    let overlays = ui::overlays(window, cx);
    overlays.close(DELETE_DIALOG, window, cx);
    overlays.close(FORM_DIALOG, window, cx);
}
