//! The new-issue dialog (`new_issue()` of the mockups): title, description with pasted or
//! dropped images, status, priority, owner, type, repo, what it runs with and when that
//! starts. "Create issue" files it; "Create & start" files it and starts the attached runner.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths, IntoElement,
    ObjectFit, Render, SharedString, Subscription, Window, div, img, prelude::*, px,
};
use hub::Route;
use hub::api::{
    Attachment, IssueStatus, IssueType, NewIssue, Priority, ProjectId, RunnableKind, RunnableRef, StartTriggers,
};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::viewer::ImageThumb;
use ui_rich::{SubmitOn, TextArea, TextAreaEvent, TextInput, TextInputEvent};

use super::card::runner_kind;

/// The dialog's id.
pub(crate) const DIALOG: &str = "new-issue";
/// Probe of the title field.
pub(crate) const TITLE: &str = "new-issue-title";
/// Id and probe of the button that files the issue.
pub(crate) const CREATE: &str = "new-issue-create";
/// Id and probe of the button that files the issue and starts it.
pub(crate) const CREATE_START: &str = "new-issue-create-start";

/// When the attached runner starts, as the "Start" control offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StartWhen {
    /// Now: "Create & start" is the primary action.
    #[default]
    RightAway,
    /// When the issue moves to Todo.
    OnTodo,
    /// Only when a comment mentions it.
    OnMention,
}

impl StartWhen {
    const ALL: [StartWhen; 3] = [StartWhen::RightAway, StartWhen::OnTodo, StartWhen::OnMention];

    fn label(self) -> &'static str {
        match self {
            StartWhen::RightAway => "Right away",
            StartWhen::OnTodo => "When moved to Todo",
            StartWhen::OnMention => "Only on @mention",
        }
    }

    /// The triggers the issue is created with. Starting right away is an explicit start after
    /// creating, so no trigger fires on creation itself.
    pub(crate) fn triggers(self) -> StartTriggers {
        StartTriggers { on_assign: false, on_comment: false, on_mention: true, on_todo: self == StartWhen::OnTodo }
    }
}

/// What the dialog starts from.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seed {
    pub project: Option<ProjectId>,
    pub project_name: String,
    pub repos: Vec<String>,
    pub owners: Vec<String>,
    pub status: IssueStatus,
}

/// The dialog's body and state.
pub(crate) struct NewIssueForm {
    seed: Seed,
    pub(crate) title: Entity<TextInput>,
    pub(crate) body: Entity<TextArea>,
    pub(crate) status: IssueStatus,
    pub(crate) priority: Priority,
    pub(crate) owner: String,
    pub(crate) kind: IssueType,
    pub(crate) repo: Option<String>,
    pub(crate) runner: Option<RunnableRef>,
    pub(crate) start: StartWhen,
    pub(crate) images: Vec<Attachment>,
    uploading: usize,
    pub(crate) create_more: bool,
    pub(crate) error: Option<SharedString>,
    busy: bool,
    /// The title takes focus on the first frame, after the dialog focused its first control.
    focus_pending: bool,
    _subs: Vec<Subscription>,
}

impl NewIssueForm {
    pub(crate) fn new(seed: Seed, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| TextInput::new(cx).placeholder("Issue title"));
        let body = cx.new(|cx| {
            TextArea::new(cx).placeholder("Add a description…").submit_on(SubmitOn::Never).rows(3, 12)
        });
        let subs = vec![
            cx.subscribe_in(&title, window, |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Changed => {
                    this.error = None;
                    cx.notify();
                }
                TextInputEvent::Submit => this.submit(this.primary_starts(), window, cx),
                _ => {}
            }),
            cx.subscribe(&body, |this: &mut Self, _, event: &TextAreaEvent, cx| {
                if let TextAreaEvent::PasteImage(item) = event {
                    this.paste_images(item, cx);
                }
            }),
        ];
        let owner = seed.owners.first().cloned().unwrap_or_else(|| "You".to_string());
        Self {
            repo: seed.repos.first().cloned(),
            status: seed.status,
            seed,
            title,
            body,
            priority: Priority::Medium,
            owner,
            kind: IssueType::default(),
            runner: None,
            start: StartWhen::RightAway,
            images: Vec::new(),
            uploading: 0,
            create_more: false,
            error: None,
            busy: false,
            focus_pending: true,
            _subs: subs,
        }
    }

    /// Puts the caret in the title.
    pub(crate) fn focus_title(&self, window: &mut Window, cx: &App) {
        self.title.read(cx).focus(window);
    }

    /// Whether the primary button (and Cmd/Ctrl-Enter) also starts the runner.
    pub(crate) fn primary_starts(&self) -> bool {
        self.runner.is_some() && self.start == StartWhen::RightAway
    }

    /// The issue the buttons would create, or what is missing.
    pub(crate) fn build(&self, cx: &App) -> Result<NewIssue, SharedString> {
        let title = self.title.read(cx).text().trim().to_string();
        if title.is_empty() {
            return Err("Give the issue a title.".into());
        }
        if self.uploading > 0 {
            return Err("Wait until the images are attached.".into());
        }
        let Some(project) = self.seed.project.clone() else {
            return Err("There is no project to file it in.".into());
        };
        Ok(NewIssue {
            project,
            title,
            body: self.body.read(cx).text().trim().to_string(),
            status: self.status,
            priority: self.priority,
            kind: self.kind,
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            runner: self.runner.clone(),
            triggers: self.start.triggers(),
            images: self.images.clone(),
        })
    }

    /// Files the issue, and starts its runner when `start`. Closes the dialog and opens the
    /// issue, or with "Create more" clears the title and description for the next one.
    pub(crate) fn submit(&mut self, start: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let new = match self.build(cx) {
            Ok(new) => new,
            Err(error) => {
                self.error = Some(error);
                self.focus_title(window, cx);
                cx.notify();
                return;
            }
        };
        let start = start && new.runner.is_some();
        self.busy = true;
        self.error = None;
        cx.notify();
        let created = hub::commands(cx).create_issue(new, cx);
        cx.spawn_in(window, async move |this, cx| {
            let issue = created.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let Some(issue) = issue else {
                    this.error = Some("The issue was not created; the notice says why.".into());
                    cx.notify();
                    return;
                };
                if start {
                    hub::commands(cx).start_issue(&issue.key, cx);
                }
                if this.create_more {
                    this.title.update(cx, |input, cx| input.set_text("", cx));
                    this.body.update(cx, |area, cx| area.set_text("", cx));
                    this.images.clear();
                    this.focus_title(window, cx);
                    cx.notify();
                } else {
                    overlays(window, cx).close(DIALOG, window, cx);
                    hub::navigate(Route::Issues { issue: Some(issue.key) }, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ images

    fn paste_images(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) {
        for entry in item.entries() {
            if let ClipboardEntry::Image(image) = entry {
                let n = self.images.len() + self.uploading + 1;
                let ext = image.format.mime_type().rsplit('/').next().unwrap_or("png").replace("svg+xml", "svg");
                self.upload(format!("pasted-{n}.{ext}"), image.format.mime_type().to_string(), image.bytes.clone(), cx);
            }
        }
    }

    /// Image files dropped on the description: read off the UI thread, then stored.
    pub(crate) fn drop_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let Some(media_type) = media_type(&path) else {
                // As the comment box says it: a dropped PDF is refused, not lost silently.
                self.error = Some("Only images can be attached".into());
                continue;
            };
            let name = path.file_name().map_or_else(|| "image".to_string(), |n| n.to_string_lossy().into_owned());
            self.uploading += 1;
            let read = cx.background_spawn(async move { std::fs::read(&path) });
            cx.spawn(async move |this, cx| {
                let bytes = read.await;
                this.update(cx, |this, cx| {
                    this.uploading -= 1;
                    match bytes {
                        Ok(bytes) => this.upload(name, media_type.to_string(), bytes, cx),
                        Err(error) => this.error = Some(format!("Could not read {name}: {error}").into()),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn upload(&mut self, name: String, media_type: String, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.uploading += 1;
        let stored = hub::commands(cx).put_image(&name, &media_type, bytes, cx);
        cx.spawn(async move |this, cx| {
            let attachment = stored.await;
            this.update(cx, |this, cx| {
                this.uploading -= 1;
                if let Some(attachment) = attachment {
                    this.images.push(attachment);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ choices

    fn open_menu(&self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.entity();
        let anchor = Anchor::of(key, window, cx);
        overlays(window, cx).open_menu(anchor, Placement::below(), move |_, cx| menu(&form, key, cx), window, cx);
    }

    fn select(&self, key: &'static str, value: impl Into<SharedString>, glyph: Option<IconName>, cx: &mut Context<Self>) -> AnyElement {
        SelectBox::new(key, value)
            .when_some(glyph, SelectBox::icon)
            // The repo is a machine name, in mono as drawn.
            .when(key == "new-issue-repo", SelectBox::mono)
            .on_click(cx.listener(move |this, _, window, cx| this.open_menu(key, window, cx)))
            .popover_anchor(key)
            .into_any_element()
    }

    fn runs_with(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let catalog = hub::catalog(cx);
        let head = row()
            .gap(px(SPACE_2))
            .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child("Runs with"))
            .child(grow())
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("optional"));
        let chosen = self.runner.as_ref().map(|runner| {
            let (glyph, kind) = runner_kind(runner);
            let runnable = catalog.read(cx).runnable(runner).cloned();
            let name = runnable.as_ref().map_or_else(|| runner.id().to_string(), |r| r.name.clone());
            let about = runnable.map(|r| r.description).filter(|d| !d.is_empty());
            let sub = about.map_or_else(|| kind.to_string(), |about| format!("{kind} · {about}"));
            row()
                .mt(px(SPACE_1_5))
                .gap(px(SPACE_2_5))
                .child(Tile::new(glyph).tone(TileTone::Accent))
                .child(
                    col()
                        .flex_1()
                        .min_w_0()
                        .child(truncate(name).font_weight(gpui::FontWeight::MEDIUM))
                        .child(truncate_lh(sub, LABEL.line).type_style(LABEL).text_color(TEXT_MUTED)),
                )
                .child(
                    Button::new("new-issue-runner", "Change")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.open_menu("new-issue-runner", window, cx)))
                        .probe("new-issue-runner"),
                )
                .popover_anchor("new-issue-runner")
        });
        let attach = self.runner.is_none().then(|| {
            div().mt(px(SPACE_1_5)).flex().child(
                Button::new("new-issue-runner", "Attach agent, orchestrator or pipeline")
                    .secondary()
                    .small()
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.open_menu("new-issue-runner", window, cx)))
                    .popover_anchor("new-issue-runner")
                    .probe("new-issue-runner"),
            )
        });
        let start_ix = StartWhen::ALL.iter().position(|s| *s == self.start).unwrap_or(0);
        let start = self.runner.is_some().then(|| {
            let seg = StartWhen::ALL
                .iter()
                .fold(Segmented::new("new-issue-start").small(), |seg, when| seg.option(when.label()))
                .selected(start_ix)
                .on_select({
                    let form = cx.entity().downgrade();
                    move |ix, _, cx| {
                        form.update(cx, |this, cx| {
                            this.start = StartWhen::ALL[ix];
                            cx.notify();
                        })
                        .ok();
                    }
                });
            row()
                .mt(px(SPACE_2_5))
                .gap(px(SPACE_2))
                .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).mr(px(SPACE_1)).child("Start"))
                .child(seg)
        });
        col()
            .w_full()
            .px(px(SPACE_3_5))
            .py(px(SPACE_3))
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .child(head)
            .children(chosen)
            .children(attach)
            .children(start)
    }

    fn thumbs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let shown = crate::panel::viewer_images(&self.images, cx);
        let thumbs: Vec<AnyElement> = self
            .images
            .iter()
            .enumerate()
            .map(|(ix, image)| {
                let path = shown.get(ix).and_then(|i| i.path().map(|p| p.to_path_buf())).unwrap_or_default();
                col()
                    .flex_none()
                    .relative()
                    .child(
                        ImageThumb::new(SharedString::from(format!("new-issue-image-{ix}")), shown.clone(), ix)
                                                .flex()
                            .flex_col()
                            .w(px(112.))
                            .h(px(72.))
                            .p(px(SPACE_1_5))
                            .gap(px(SPACE_1))
                            .bg(SURFACE)
                            .rounded(px(RADIUS_MD))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded(px(RADIUS_SM))
                                    .overflow_hidden()
                                    .child(img(path).size_full().object_fit(ObjectFit::Cover)),
                            )
                            .child(truncate_lh(image.name.clone(), CAPTION.line).type_style(CAPTION).text_color(TEXT_SECONDARY)),
                    )
                    // Over the thumbnail: a press here removes the image and opens nothing.
                    .child(
                        div().absolute().top(px(SPACE_1)).right(px(SPACE_1)).occlude().child(
                            IconButton::new(("new-issue-image-remove", ix), IconName::X)
                                .small()
                                .tooltip(format!("Remove {}", image.name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if ix < this.images.len() {
                                        this.images.remove(ix);
                                        cx.notify();
                                    }
                                }))
                                .probe(SharedString::from(format!("new-issue-image-remove-{ix}"))),
                        ),
                    )
                    .into_any_element()
            })
            .collect();
        let hint = if self.uploading > 0 { "Attaching…".to_string() } else { "Paste or drop images".to_string() };
        row()
            .mt(px(SPACE_2_5))
            .gap(px(SPACE_2))
            .items_end()
            .children(thumbs)
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(hint))
    }
}

fn media_type(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// The menu a select opens.
fn menu(form: &Entity<NewIssueForm>, key: &'static str, cx: &mut App) -> Menu {
    if key == "new-issue-runner" {
        return runner_menu(form, cx);
    }
    let this = form.read(cx);
    let pick = |f: Box<dyn Fn(&mut NewIssueForm)>| {
        let form = form.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
            form.update(cx, |this, cx| {
                f(this);
                this.focus_title(window, cx);
                cx.notify();
            })
        }
    };
    match key {
        "new-issue-status" => Menu::new("new-issue-status-menu").width(200.).items(IssueStatus::ALL.into_iter().map(|s| {
            MenuItem::new(("status", s as usize), s.label())
                .check_column()
                .checked(s == this.status)
                .on_click(pick(Box::new(move |f| f.status = s)))
        })),
        "new-issue-priority" => {
            Menu::new("new-issue-priority-menu").width(200.).items([Priority::High, Priority::Medium, Priority::Low].map(|p| {
                MenuItem::new(("priority", p as usize), p.label())
                    .check_column()
                    .checked(p == this.priority)
                    .on_click(pick(Box::new(move |f| f.priority = p)))
            }))
        }
        "new-issue-owner" => {
            let mut owners = this.seed.owners.clone();
            if !owners.contains(&this.owner) {
                owners.push(this.owner.clone());
            }
            let mut menu = Menu::new("new-issue-owner-menu").width(220.);
            for (ix, owner) in owners.into_iter().enumerate() {
                let checked = owner == this.owner;
                let label = if owner.is_empty() { "No owner".to_string() } else { owner.clone() };
                menu = menu.item(
                    MenuItem::new(("owner", ix), label)
                        .check_column()
                        .checked(checked)
                        .on_click(pick(Box::new(move |f| f.owner = owner.clone()))),
                );
            }
            menu
        }
        "new-issue-type" => Menu::new("new-issue-type-menu").width(200.).items(IssueType::ALL.into_iter().map(|k| {
            MenuItem::new(("type", k as usize), k.label())
                .check_column()
                .checked(k == this.kind)
                .on_click(pick(Box::new(move |f| f.kind = k)))
        })),
        "new-issue-repo" => {
            let mut menu = Menu::new("new-issue-repo-menu").width(240.);
            for (ix, repo) in this.seed.repos.clone().into_iter().enumerate() {
                let checked = this.repo.as_deref() == Some(repo.as_str());
                menu = menu.item(
                    MenuItem::new(("repo", ix), repo.clone())
                        .icon(IconName::Folder)
                        .check_column()
                        .checked(checked)
                        .on_click(pick(Box::new(move |f| f.repo = Some(repo.clone())))),
                );
            }
            menu.item(
                MenuItem::new("repo-none", "No repo")
                    .check_column()
                    .checked(this.repo.is_none())
                    .on_click(pick(Box::new(|f| f.repo = None))),
            )
        }
        _ => Menu::new("new-issue-menu"),
    }
}

/// Every orchestrator, agent and pipeline that can take the issue.
fn runner_menu(form: &Entity<NewIssueForm>, cx: &mut App) -> Menu {
    let runnables: Vec<hub::api::Runnable> = hub::catalog(cx).read(cx).runnables().to_vec();
    let current = form.read(cx).runner.clone();
    let mut menu = Menu::new("new-issue-runner-menu").width(380.).max_height(420.);
    let groups = [
        (RunnableKind::Orchestrator, "Orchestrators"),
        (RunnableKind::Agent, "Agents"),
        (RunnableKind::Pipeline, "Pipelines"),
    ];
    for (kind, label) in groups {
        let of_kind: Vec<&hub::api::Runnable> =
            runnables.iter().filter(|r| r.kind == kind && !r.archived && !r.paused).collect();
        if of_kind.is_empty() {
            continue;
        }
        menu = menu.label(label);
        for runnable in of_kind {
            let reference = runnable.reference.clone();
            let (glyph, _) = runner_kind(&reference);
            let f = form.clone();
            let chosen = reference.clone();
            menu = menu.item(
                MenuItem::new(SharedString::from(format!("runner-{reference}")), runnable.name.clone())
                    .tile(Tile::new(glyph).medium())
                    .subline(runnable.description.clone())
                    .check_column()
                    .checked(current.as_ref() == Some(&reference))
                    .on_click(move |_, window, cx| {
                        f.update(cx, |this, cx| {
                            this.runner = Some(chosen.clone());
                            this.focus_title(window, cx);
                            cx.notify();
                        })
                    }),
            );
        }
    }
    if current.is_some() {
        let f = form.clone();
        menu = menu.separator().item(MenuItem::new("runner-none", "Nothing attached").check_column().on_click(
            move |_, window, cx| {
                f.update(cx, |this, cx| {
                    this.runner = None;
                    this.focus_title(window, cx);
                    cx.notify();
                })
            },
        ));
    }
    menu
}

impl Render for NewIssueForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            let title = self.title.clone();
            window.defer(cx, move |window, cx| title.read(cx).focus(window));
        }
        let focused = self.title.read(cx).is_focused(window);
        let error = self.error.clone();
        let title_input = self.title.clone();
        let title = div()
            .w_full()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| title_input.read(cx).focus(window))
            .child(
                InputFrame::new("new-issue-title-frame")
                    .focused(focused)
                    .danger(error.is_some())
                    .child(div().w_full().type_style(TITLE_SM).child(self.title.clone())),
            )
            .probe(TITLE);
        let body_focused = self.body.read(cx).is_focused(window);
        let body_area = self.body.clone();
        let body = div()
            .id("new-issue-body")
            .w_full()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| body_area.read(cx).focus(window))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.drop_files(paths.paths().to_vec(), cx)))
            .child(
                InputFrame::new("new-issue-body-frame")
                    .multiline()
                    .focused(body_focused)
                    .child(col().w_full().child(self.body.clone()).child(self.thumbs(cx))),
            )
            .probe("new-issue-body");
        let status = self.select("new-issue-status", self.status.label(), None, cx);
        let priority = self.select("new-issue-priority", format!("{} priority", self.priority.label()), None, cx);
        let owner_label = if self.owner.is_empty() { "No owner".to_string() } else { self.owner.clone() };
        let owner = self.select("new-issue-owner", owner_label, Some(IconName::Users), cx);
        let kind = self.select("new-issue-type", self.kind.label(), Some(IconName::Issues), cx);
        let repo = self.select("new-issue-repo", self.repo.clone().unwrap_or_else(|| "No repo".into()), Some(IconName::Folder), cx);
        // The dialog wraps its body children in a box that sizes to them: fill it, or the
        // fields measure at the width of their text.
        col()
            .flex_1()
            .w_full()
            .min_w_0()
            .gap(px(DIALOG_GAP))
            .child(
                col()
                    .w_full()
                    .gap(px(SPACE_1_5))
                    .child(title)
                    .when_some(error, |d, error| {
                        d.child(div().type_style(LABEL).text_color(DANGER).child(error).probe("new-issue-error"))
                    })
                    .child(body),
            )
            .child(row().gap(px(SPACE_1_5)).child(status).child(priority).child(owner).child(kind).child(repo))
            .child(self.runs_with(cx))
    }
}
