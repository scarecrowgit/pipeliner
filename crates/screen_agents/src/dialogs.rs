//! The dialogs and toasts: delete (`agents-delete-dialog.html`), archive with Undo
//! (`agents-archived-toast.html`), and the small ones the menus open: rename, import from a
//! file, export, and new from a template.
//!
//! A dialog's build closure runs every frame and reads the view, so typing into its input
//! redraws it (the delete button enables when the name is typed).

use gpui::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, SharedString, Subscription, Window, div, prelude::*, px};
use hub::api::{AgentDef, Origin, Place, Runnable, RunnableKind, Uses};
use hub::model::RunnableRef;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use crate::edit::EditForm;
use crate::rows;
use crate::view::{AgentsView, open_editor};

const DELETE_DIALOG: &str = "agents-delete-dialog";
const RENAME_DIALOG: &str = "agents-rename-dialog";
const IMPORT_DIALOG: &str = "agents-import-dialog";
const EXPORT_DIALOG: &str = "agents-export-dialog";
const TEMPLATE_DIALOG: &str = "agents-template-dialog";
/// Most characters of a name in a dialog's title or button: the kit's title and buttons do
/// not shrink, so a long name is cut here (the type-to-confirm field still wants it whole).
pub(crate) const DIALOG_NAME_CHARS: usize = 40;

/// `name` cut to [`DIALOG_NAME_CHARS`] for a dialog's title or button.
pub(crate) fn short_name(name: &str) -> String {
    base::fmt::truncate_chars(name, DIALOG_NAME_CHARS)
}

/// The inputs of the open dialog.
pub(crate) enum Form {
    Delete { reference: RunnableRef, name: String, input: Entity<TextInput> },
    Rename { reference: RunnableRef, input: Entity<TextInput> },
    Import { input: Entity<TextInput> },
    Export { reference: RunnableRef, input: Entity<TextInput> },
    /// New from a template: of one kind (with a blank first for agents and orchestrators), or
    /// any.
    Template { kind: Option<RunnableKind>, choice: usize },
    Edit(EditForm),
}

/// A choice of the template dialog.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Choice {
    Blank(RunnableKind),
    Template { id: String, name: String, description: String, kind: RunnableKind },
}

impl AgentsView {
    pub(crate) fn set_form(&mut self, form: Form, subs: Vec<Subscription>) {
        self.form = Some(form);
        self.form_subs = subs;
    }

    /// A one-line input whose Enter runs `submit` and whose edits redraw the dialog.
    fn form_input(
        &mut self,
        text: &str,
        placeholder: &'static str,
        dialog: &'static str,
        submit: fn(&mut AgentsView, &mut Window, &mut Context<AgentsView>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TextInput>, Subscription) {
        let input = cx.new(|cx| TextInput::new(cx).with_text(text).placeholder(placeholder));
        input.update(cx, |input, cx| input.select_all(cx));
        let sub = cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => cx.notify(),
            TextInputEvent::Submit => submit(this, window, cx),
            TextInputEvent::Cancel => ui::overlays(window, cx).close(dialog, window, cx),
            _ => {}
        });
        (input, sub)
    }

    /// The more menu's delete line: "Used by PAY triage and 2 issues".
    pub(crate) fn used_by_line(&self, reference: &RunnableRef, cx: &App) -> String {
        let Some(used) = self.used_by.get(reference) else { return "Checking what uses it…".into() };
        let catalog = self.catalog.read(cx);
        let mut parts: Vec<String> = used
            .pipelines
            .iter()
            .map(|id| catalog.runnable(&RunnableRef::pipeline(id.as_str())).map_or_else(|| id.to_string(), |r| r.name.clone()))
            .collect();
        if !used.issues.is_empty() {
            parts.push(base::fmt::plural(used.issues.len() as u64, "issue", "issues"));
        }
        match parts.len() {
            0 => "Nothing uses it".into(),
            1 => format!("Used by {}", parts[0]),
            n => format!("Used by {} and {}", parts[..n - 1].join(", "), parts[n - 1]),
        }
    }

    // ------------------------------------------------------------ delete

    pub(crate) fn open_delete(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runnable) = self.catalog.read(cx).runnable(reference).cloned() else { return };
        if rows::removal_blocked(&runnable).is_some() {
            return;
        }
        if !self.used_by.contains_key(reference) {
            self.load_used_by(reference, cx);
        }
        let (input, sub) = self.form_input("", "", DELETE_DIALOG, |this, window, cx| this.confirm_delete(window, cx), window, cx);
        input.update(cx, |i, cx| i.set_placeholder(runnable.name.clone(), cx));
        self.set_form(Form::Delete { reference: reference.clone(), name: runnable.name.clone(), input: input.clone() }, vec![sub]);
        let view = cx.entity();
        // The dialog focuses its first control (here "Archive instead") once it is drawn,
        // unless focus moved meanwhile: move it to the name field first.
        let first = std::cell::Cell::new(true);
        ui::overlays(window, cx).open_dialog(
            DELETE_DIALOG,
            move |window, cx| {
                if first.replace(false) {
                    let input = input.clone();
                    window.defer(cx, move |window, cx| input.read(cx).focus(window));
                }
                delete_dialog(&view, cx)
            },
            window,
            cx,
        );
    }

    /// Whether the delete dialog's typed name matches.
    fn delete_armed(&self, cx: &App) -> bool {
        matches!(&self.form, Some(Form::Delete { name, input, .. }) if input.read(cx).text().trim() == name.as_str())
    }

    pub(crate) fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.delete_armed(cx) {
            return;
        }
        let Some(Form::Delete { reference, .. }) = self.form.take() else { return };
        hub::commands(cx).delete_runnable(&reference, cx);
        ui::overlays(window, cx).close(DELETE_DIALOG, window, cx);
        if self.selected.as_ref() == Some(&reference) {
            self.select(None, cx);
        }
    }

    // ------------------------------------------------------------ archive

    /// Archives and offers Undo in a toast.
    pub(crate) fn archive(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runnable) = self.catalog.read(cx).runnable(reference).cloned() else { return };
        hub::commands(cx).set_runnable_archived(reference, true, cx);
        let orphans = runnable
            .agent
            .as_ref()
            .map_or(0, |a| a.team.iter().filter(|m| m.scheme() == "pipeline").count());
        let mut toast = Toast::new(format!("{} archived", short_name(&runnable.name)));
        if runnable.kind == RunnableKind::Orchestrator && orphans > 0 {
            let pipelines = base::fmt::plural(orphans as u64, "pipeline", "pipelines");
            toast = toast.detail(format!("{pipelines} {} no owner now", if orphans == 1 { "has" } else { "have" }));
        }
        let view = cx.entity().downgrade();
        let undo = reference.clone();
        toast = toast.undo(move |_, cx| {
            hub::commands(cx).set_runnable_archived(&undo, false, cx);
            view.update(cx, |this, cx| this.select(Some(undo.clone()), cx)).ok();
        });
        ui::overlays(window, cx).toast(toast, cx);
        if self.selected.as_ref() == Some(reference) {
            self.select(None, cx);
        }
    }

    /// Brings an archived runnable back.
    pub(crate) fn restore(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.catalog.read(cx).runnable(reference).map(|r| r.name.clone()).unwrap_or_default();
        hub::commands(cx).set_runnable_archived(reference, false, cx);
        ui::overlays(window, cx).toast(Toast::new(format!("{} restored", short_name(&name))), cx);
    }

    // ------------------------------------------------------------ rename

    pub(crate) fn open_rename(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.catalog.read(cx).runnable(reference).map(|r| r.name.clone()) else { return };
        let (input, sub) = self.form_input(&name, "Name", RENAME_DIALOG, |this, window, cx| this.commit_rename(window, cx), window, cx);
        self.set_form(Form::Rename { reference: reference.clone(), input: input.clone() }, vec![sub]);
        let view = cx.entity();
        ui::overlays(window, cx).open_dialog(
            RENAME_DIALOG,
            move |_, cx| {
                let Some(Form::Rename { input, .. }) = view.read(cx).form.as_ref() else { return div().into_any_element() };
                let save = view.clone();
                let empty = input.read(cx).text().trim().is_empty();
                Dialog::new(RENAME_DIALOG, "Rename")
                    .width(440.)
                    .child(text_field("agents-rename-input", input).probe("agents-rename-input"))
                    .cancel_button()
                    .action(
                        Button::new("agents-rename-save", "Rename")
                            .primary()
                            .kbd("⌘↵")
                            .disabled(empty)
                            .on_click(move |_, window, cx| save.update(cx, |this, cx| this.commit_rename(window, cx)))
                            .probe("agents-rename-save"),
                    )
                    .on_confirm({
                        let view = view.clone();
                        move |window, cx| view.update(cx, |this, cx| this.commit_rename(window, cx))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        input.read(cx).focus(window);
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An empty name is no name: the dialog stays open (its button is disabled meanwhile).
        let Some(Form::Rename { input, .. }) = self.form.as_ref() else { return };
        let name = input.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(Form::Rename { reference, .. }) = self.form.take() else { return };
        ui::overlays(window, cx).close(RENAME_DIALOG, window, cx);
        let task = hub::commands(cx).rename_runnable(&reference, &name, cx);
        cx.spawn(async move |this, cx| {
            if let Some(renamed) = task.await {
                this.update(cx, |this, cx| this.select(Some(renamed), cx)).ok();
            }
        })
        .detach();
    }

    // ------------------------------------------------------------ import and export

    pub(crate) fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (input, sub) =
            self.form_input("", "~/agents/reviewer.yaml", IMPORT_DIALOG, |this, window, cx| this.commit_import(window, cx), window, cx);
        self.set_form(Form::Import { input: input.clone() }, vec![sub]);
        let view = cx.entity();
        ui::overlays(window, cx).open_dialog(
            IMPORT_DIALOG,
            move |_, cx| {
                let Some(Form::Import { input }) = view.read(cx).form.as_ref() else { return div().into_any_element() };
                let empty = input.read(cx).text().trim().is_empty();
                let go = view.clone();
                Dialog::new(IMPORT_DIALOG, "Import from file")
                    .width(520.)
                    .subtitle("An agent definition (.yaml) or an AGENTS.md file")
                    .child(
                        Field::new("File")
                            .hint("It is copied into your library; the file itself is not changed")
                            .child(text_field("agents-import-path", input).icon(IconName::File).probe("agents-import-path")),
                    )
                    .cancel_button()
                    .action(
                        Button::new("agents-import-confirm", "Import")
                            .primary()
                            .kbd("⌘↵")
                            .disabled(empty)
                            .on_click(move |_, window, cx| go.update(cx, |this, cx| this.commit_import(window, cx)))
                            .probe("agents-import-confirm"),
                    )
                    .on_confirm({
                        let view = view.clone();
                        move |window, cx| view.update(cx, |this, cx| this.commit_import(window, cx))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        input.read(cx).focus(window);
    }

    fn commit_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Form::Import { input }) = self.form.as_ref() else { return };
        let text = input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.form = None;
        let path = hub::hub(cx).read(cx).paths().expand(&text);
        ui::overlays(window, cx).close(IMPORT_DIALOG, window, cx);
        let task = hub::commands(cx).import_runnable(&path, cx);
        cx.spawn(async move |this, cx| {
            if let Some(imported) = task.await {
                this.update(cx, |this, cx| this.select(Some(imported), cx)).ok();
            }
        })
        .detach();
    }

    pub(crate) fn open_export(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.catalog.read(cx).runnable(reference).map(|r| r.name.clone()) else { return };
        let home = hub::home(cx);
        let start = hub::display_path(&home, cx);
        let (input, sub) = self.form_input(&start, "~/Downloads", EXPORT_DIALOG, |this, window, cx| this.commit_export(window, cx), window, cx);
        self.set_form(Form::Export { reference: reference.clone(), input: input.clone() }, vec![sub]);
        let view = cx.entity();
        let title = format!("Export {} as YAML", short_name(&name));
        ui::overlays(window, cx).open_dialog(
            EXPORT_DIALOG,
            move |_, cx| {
                let Some(Form::Export { input, .. }) = view.read(cx).form.as_ref() else { return div().into_any_element() };
                let go = view.clone();
                Dialog::new(EXPORT_DIALOG, title.clone())
                    .width(520.)
                    .child(Field::new("Folder").child(text_field("agents-export-path", input).icon(IconName::Folder).probe("agents-export-path")))
                    .cancel_button()
                    .action(
                        Button::new("agents-export-confirm", "Export")
                            .primary()
                            .kbd("⌘↵")
                            .on_click(move |_, window, cx| go.update(cx, |this, cx| this.commit_export(window, cx)))
                            .probe("agents-export-confirm"),
                    )
                    .on_confirm({
                        let view = view.clone();
                        move |window, cx| view.update(cx, |this, cx| this.commit_export(window, cx))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        input.read(cx).focus(window);
    }

    fn commit_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Form::Export { reference, input }) = self.form.take() else { return };
        let text = input.read(cx).text().trim().to_string();
        let dir = hub::hub(cx).read(cx).paths().expand(if text.is_empty() { "~" } else { &text });
        ui::overlays(window, cx).close(EXPORT_DIALOG, window, cx);
        let task = hub::commands(cx).export_runnable(&reference, &dir, cx);
        let handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            if let Some(path) = task.await {
                handle
                    .update(cx, |_, window, cx| {
                        let shown = hub::display_path(&path, cx);
                        ui::overlays(window, cx).toast(Toast::new("Exported").detail(shown), cx);
                    })
                    .ok();
            }
        })
        .detach();
    }

    // ------------------------------------------------------------ new from a template

    /// The choices of the template dialog for `kind` (`None`: every template).
    pub(crate) fn choices(&self, kind: Option<RunnableKind>) -> Vec<Choice> {
        let mut out = Vec::new();
        if let Some(kind @ (RunnableKind::Agent | RunnableKind::Orchestrator)) = kind {
            out.push(Choice::Blank(kind));
        }
        out.extend(self.templates.iter().filter(|t| kind.is_none_or(|k| t.kind == k)).map(|t| Choice::Template {
            id: t.id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            kind: t.kind,
        }));
        out
    }

    pub(crate) fn open_templates(&mut self, kind: Option<RunnableKind>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_form(Form::Template { kind, choice: 0 }, Vec::new());
        let view = cx.entity();
        ui::overlays(window, cx).open_dialog(TEMPLATE_DIALOG, move |_, cx| template_dialog(&view, cx), window, cx);
    }

    fn pick_template(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(Form::Template { choice, .. }) = self.form.as_mut() {
            *choice = ix;
            cx.notify();
        }
    }

    fn commit_template(&mut self, cx: &mut Context<Self>) {
        let Some(Form::Template { kind, choice }) = self.form.take() else { return };
        let Some(chosen) = self.choices(kind).get(choice).cloned() else { return };
        match chosen {
            Choice::Blank(kind) => {
                let runnable = self.blank(kind, cx);
                let reference = runnable.reference.clone();
                hub::commands(cx).save_runnable(Place::Library, runnable, cx);
                self.select(Some(reference), cx);
            }
            Choice::Template { id, kind, .. } => {
                let task = hub::commands(cx).create_from_template(&id, cx);
                cx.spawn(async move |this, cx| {
                    let Some(made) = task.await else { return };
                    if kind == RunnableKind::Pipeline {
                        cx.update(|cx| open_editor(&made, cx)).ok();
                    } else {
                        this.update(cx, |this, cx| this.select(Some(made), cx)).ok();
                    }
                })
                .detach();
            }
        }
    }

    /// A new, empty agent or orchestrator for the library, named so it clashes with nothing.
    fn blank(&self, kind: RunnableKind, cx: &App) -> Runnable {
        let (slug, name) = match kind {
            RunnableKind::Orchestrator => ("new-orchestrator", "New orchestrator"),
            _ => ("new-agent", "New agent"),
        };
        let catalog = self.catalog.read(cx);
        let make = |id: &str| match kind {
            RunnableKind::Orchestrator => RunnableRef::orchestrator(id),
            _ => RunnableRef::agent(id),
        };
        let n = (1..).find(|n| {
            let id = if *n == 1 { slug.to_string() } else { format!("{slug}-{n}") };
            catalog.runnable(&make(&id)).is_none()
        });
        let n = n.unwrap_or(1);
        let (id, name) = if n == 1 { (slug.to_string(), name.to_string()) } else { (format!("{slug}-{n}"), format!("{name} {n}")) };
        Runnable {
            reference: make(&id),
            kind,
            name,
            description: String::new(),
            origin: Origin::Library,
            uses: Uses::default(),
            paused: false,
            archived: false,
            agent: Some(AgentDef::default()),
            path: None,
        }
    }
}

fn hit(ix: usize, icon_name: IconName, what: impl Into<SharedString>, detail: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    row()
        .id(("agents-delete-hit", ix))
        .debug_selector(move || format!("agents-delete-hit-{ix}"))
        .min_h(px(36.))
        .gap(px(SPACE_2_5))
        .px(px(SPACE_3))
        .when(ix > 0, |d| d.border_t_1().border_color(LINE_SUBTLE))
        .child(Tile::new(icon_name))
        .child(crate::list::fit(what.into()).type_style(BODY_STRONG))
        .child(div().flex_1().min_w_0().type_style(LABEL).text_color(color).child(truncate(detail.into())))
        .into_any_element()
}

fn delete_dialog(view: &Entity<AgentsView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let Some(Form::Delete { reference, name, input }) = this.form.as_ref() else { return div().into_any_element() };
    let catalog = this.catalog.read(cx);
    let Some(runnable) = catalog.runnable(reference) else { return div().into_any_element() };
    let used = this.used_by.get(reference).cloned().unwrap_or_default();
    let running = rows::running_issue(catalog.activity(reference));

    let mut hits = Vec::new();
    if let Some(key) = &running {
        hits.push(hit(hits.len(), IconName::Issues, key.to_string(), "running now · stops and goes back to Todo", WARNING));
    }
    for id in &used.pipelines {
        let pipeline = catalog.runnable(&RunnableRef::pipeline(id.as_str()));
        let name = pipeline.map_or_else(|| id.to_string(), |r| r.name.clone());
        let version = pipeline
            .and_then(|p| catalog.activity(&p.reference))
            .and_then(|a| match a.state {
                hub::api::RunnableState::Live { version } => Some(format!("v{version} keeps running without it")),
                _ => None,
            })
            .unwrap_or_else(|| "keeps its other nodes".to_string());
        hits.push(hit(hits.len(), IconName::Pipelines, name, format!("loses a team member · {version}"), TEXT_SECONDARY));
    }
    for created in &used.created {
        let name = catalog.runnable(created).map_or_else(|| created.id().to_string(), |r| r.name.clone());
        hits.push(hit(hits.len(), IconName::Spawn, name, "agent it created · stops with it", SPAWN_TEXT));
    }
    let others: Vec<String> = used.issues.iter().filter(|k| Some(*k) != running.as_ref()).map(ToString::to_string).collect();
    if !others.is_empty() {
        let n = base::fmt::plural(others.len() as u64, "issue", "issues");
        let verb = if others.len() == 1 { "becomes" } else { "become" };
        hits.push(hit(hits.len(), IconName::Clock, n, format!("{} · {verb} unattached", others.join(", ")), TEXT_SECONDARY));
    }
    let issue_count = others.len() + usize::from(running.is_some());
    let mut places = Vec::new();
    if !used.pipelines.is_empty() {
        places.push(base::fmt::plural(used.pipelines.len() as u64, "pipeline", "pipelines"));
    }
    if issue_count > 0 {
        places.push(base::fmt::plural(issue_count as u64, "issue", "issues"));
    }
    let sub = if places.is_empty() {
        format!("{} · nothing uses it", rows::kind_word(runnable.kind))
    } else {
        format!("{} · used in {}", rows::kind_word(runnable.kind), places.join(" and "))
    };
    let what = if runnable.kind == RunnableKind::Pipeline { "Its graph, versions" } else { "Its prompt, skills, limits" };
    let armed = this.delete_armed(cx);
    let (archive, confirm) = (view.clone(), view.clone());
    let archive_ref = reference.clone();

    let short = short_name(name);
    Dialog::new(DELETE_DIALOG, format!("Delete {short}?"))
        .width(560.)
        .subtitle(sub)
        .child(div().w(px(520.)).text_color(TEXT_SECONDARY).child(format!(
            "{what} and run history are removed for everyone. Chats and issues it worked on stay, with its name greyed out."
        )))
        .when(!hits.is_empty(), |d| {
            d.child(
                col()
                    .gap(px(SPACE_1_5))
                    .child(div().type_style(LABEL_STRONG).text_color(TEXT_MUTED).child("What changes"))
                    .child(col().border_1().border_color(LINE).rounded(px(RADIUS_LG)).overflow_hidden().children(hits)),
            )
        })
        .child(
            Banner::new()
                .child(icon(IconName::Archive, 16., TEXT_MUTED))
                .child(
                    col()
                        .flex_1()
                        .min_w_0()
                        .child(div().type_style(BODY_STRONG).child("Archive keeps all of this"))
                        .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child("It stops taking work, and you can restore it with its history.")),
                )
                .child(
                    Button::new("agents-delete-archive", "Archive instead")
                        .secondary()
                        .small()
                        .on_click(move |_, window, cx| {
                            ui::overlays(window, cx).close(DELETE_DIALOG, window, cx);
                            let reference = archive_ref.clone();
                            archive.update(cx, |this, cx| {
                                this.form = None;
                                this.archive(&reference, window, cx)
                            })
                        })
                        .probe("agents-delete-archive"),
                ),
        )
        .child(ConfirmInput::new("agents-delete-name", name.clone()).child(input.clone()).probe("agents-delete-name"))
        .note(row().gap(px(SPACE_1_5)).text_color(DANGER).child(icon(IconName::Alert, 14., DANGER)).child("This can’t be undone"))
        .cancel_button()
        .action(
            Button::new("agents-delete-confirm", format!("Delete {short}"))
                .danger()
                .icon(IconName::Trash)
                .disabled(!armed)
                .on_click(move |_, window, cx| confirm.update(cx, |this, cx| this.confirm_delete(window, cx)))
                .probe("agents-delete-confirm"),
        )
        .on_confirm({
            let view = view.clone();
            move |window, cx| view.update(cx, |this, cx| this.confirm_delete(window, cx))
        })
        .into_any_element()
}

fn template_dialog(view: &Entity<AgentsView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let Some(Form::Template { kind, choice }) = this.form.as_ref() else { return div().into_any_element() };
    let (kind, choice) = (*kind, *choice);
    let choices = this.choices(kind);
    let title = match kind {
        Some(RunnableKind::Agent) => "New agent",
        Some(RunnableKind::Orchestrator) => "New orchestrator",
        Some(RunnableKind::Pipeline) => "New pipeline",
        None => "New from a template",
    };
    let pick = view.clone();
    let create = view.clone();
    let group = choices.iter().fold(OptionGroup::new("agents-templates"), |g, c| {
        let (id, name, description): (SharedString, String, String) = match c {
            Choice::Blank(kind) => (
                format!("agents-template-blank-{}", rows::kind_word(*kind).to_lowercase()).into(),
                format!("Blank {}", rows::kind_word(*kind).to_lowercase()),
                "Start from nothing and write its instructions".to_string(),
            ),
            Choice::Template { id, name, description, kind } => {
                (format!("agents-template-{id}").into(), name.clone(), format!("{} · {description}", rows::kind_word(*kind)))
            }
        };
        g.option(OptionCard::new(id, name).description(description))
    });
    let empty = choices.is_empty();
    Dialog::new(TEMPLATE_DIALOG, title)
        .width(560.)
        .subtitle("It is written to your library; edit it from there")
        .when(empty, |d| d.child(Note::new("No templates of this kind yet.")))
        .when(!empty, |d| {
            d.child(group.selected(choice.min(choices.len().saturating_sub(1))).on_select(move |ix, _, cx| {
                pick.update(cx, |this, cx| this.pick_template(ix, cx))
            }))
        })
        .cancel_button()
        .action(
            Button::new("agents-template-create", "Create")
                .primary()
                .kbd("⌘↵")
                .disabled(empty)
                .on_click(move |_, window, cx| {
                    ui::overlays(window, cx).close(TEMPLATE_DIALOG, window, cx);
                    create.update(cx, |this, cx| this.commit_template(cx))
                })
                .probe("agents-template-create"),
        )
        .on_confirm({
            let view = view.clone();
            move |window, cx| {
                ui::overlays(window, cx).close(TEMPLATE_DIALOG, window, cx);
                view.update(cx, |this, cx| this.commit_template(cx))
            }
        })
        .into_any_element()
}
