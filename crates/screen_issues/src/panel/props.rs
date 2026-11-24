//! The top of the panel: the title (edited in place), the description with its images, and the
//! properties as `props()` draws them: status, priority, owner and "Runs with" (the attach
//! button, or the runner card with its run, "Start when" and the runner's actions).

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, ElementId, Entity, Hsla, IntoElement, ObjectFit, SharedString, Window, div, img,
    prelude::*, px,
};
use hub::api::{Attachment, Issue, IssuePatch, Priority, PipelineId, Run, RunnableKind, RunnableRef, StartTriggers};
use hub::Route;
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::pickers::{self, OWNER, PRIORITY, RUNNER, STATUS};
use super::{Named, IssuePanel, run, section};

/// Width of the key column of the properties, px (`grid-template-columns:96px 1fr`).
const KEY_WIDTH: f32 = 96.;
/// A thumbnail of an attached image, px (`.thumb`).
const THUMB: (f32, f32) = (132., 84.);

/// Title, description and the description's images.
pub(super) fn title_section(
    this: &IssuePanel,
    issue: &Arc<Issue>,
    _window: &mut Window,
    cx: &mut Context<IssuePanel>,
) -> AnyElement {
    let cw = this.content_width();
    let title: AnyElement = if this.editing_title {
        div()
            .mt(px(SPACE_1))
            .w(px(cw))
            .child(text_field("issue-title-field", &this.title))
            .named("issue-title-field")
            .into_any_element()
    } else {
        div()
            .id("issue-title")
            .debug_selector(|| "issue-title".into())
            .flex_none()
            .w(px(cw))
            .mt(px(SPACE_0_5))
            .type_style(TITLE_LG)
            .cursor_text()
            .child(issue.title.clone())
            .on_click(cx.listener(|this, _, window, cx| this.edit_title(window, cx)))
            .probe("issue-title")
            .into_any_element()
    };
    let mut sec = section()
        .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child("Title"))
        .child(title);
    if !issue.body.trim().is_empty() {
        sec = sec.child(
            div().flex_none().w(px(cw)).mt(px(SPACE_2)).child(this.description.clone()).named("issue-description"),
        );
    }
    if !issue.images.is_empty() {
        let thumbs = issue.images.iter().enumerate().map(|(ix, image)| thumbnail(("issue-image", ix), image, cx));
        sec = sec.child(row().flex_none().flex_wrap().gap(px(SPACE_2)).mt(px(SPACE_2)).children(thumbs));
    }
    sec.into_any_element()
}

/// An attached image: the picture over a caption with its name and size.
pub(super) fn thumbnail(id: impl Into<ElementId>, image: &Attachment, cx: &App) -> AnyElement {
    let path = hub::queries(cx).blob_path(&image.blob);
    let (w, h) = THUMB;
    let url = path.clone();
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(LINE_STRONG)
        .bg(BG_SIDEBAR)
        .overflow_hidden()
        .cursor_pointer()
        .on_click(move |_, _, cx| cx.open_url(&format!("file://{}", url.display())))
        .child(
            div()
                .absolute()
                .top(px(SPACE_2))
                .left(px(SPACE_2))
                .right(px(SPACE_2))
                .bottom(px(26.))
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(LINE)
                .bg(SURFACE)
                .overflow_hidden()
                .child(img(path).size_full().object_fit(ObjectFit::Cover)),
        )
        .child(
            row()
                .absolute()
                .left(px(SPACE_2))
                .right(px(SPACE_2))
                .bottom(px(SPACE_1))
                .h(px(18.))
                .gap(px(SPACE_1))
                .type_style(CAPTION)
                .text_color(TEXT_SECONDARY)
                .child(icon(IconName::Image, 12., TEXT_MUTED))
                .child(truncate_lh(image.name.clone(), CAPTION.line))
                .child(div().flex_none().text_color(TEXT_MUTED).child(base::fmt::bytes(image.bytes))),
        )
        .into_any_element()
}

/// Status, priority, owner and "Runs with".
pub(super) fn props_section(
    this: &IssuePanel,
    issue: &Arc<Issue>,
    window: &mut Window,
    cx: &mut Context<IssuePanel>,
) -> AnyElement {
    let cw = this.content_width();
    let (status, priority, owner) = (issue.status, issue.priority, issue.owner.clone());
    let status_box = select_trigger(STATUS, issue.status.label(), TEXT, window, cx)
        .on_click(cx.listener(move |this, _, window, cx| this.open_status(status, window, cx)));
    let priority_box = select_trigger(PRIORITY, issue.priority.label(), priority_color(issue.priority), window, cx)
        .on_click(cx.listener(move |this, _, window, cx| this.open_priority(priority, window, cx)));
    let owner_row = owner_trigger(&issue.owner, window, cx)
        .on_click(cx.listener(move |this, _, window, cx| this.open_owner(owner.clone(), window, cx)));
    let mut sec = section()
        .child(kv("Status", status_box.popover_anchor(STATUS).probe(STATUS)))
        .child(kv("Priority", priority_box.popover_anchor(PRIORITY).probe(PRIORITY)).mt(px(SPACE_2)))
        .child(kv("Owner", owner_row.popover_anchor(OWNER).probe(OWNER)).mt(px(SPACE_2)))
        .child(div().flex_none().mt(px(SPACE_3_5)).type_style(LABEL).text_color(TEXT_MUTED).child("Runs with"));
    let open = ui::overlays(window, cx).is_open(RUNNER, cx);
    match issue.runner.clone() {
        Some(runner) => sec = sec.child(div().flex_none().mt(px(SPACE_1_5)).child(runner_card(this, issue, &runner, open, window, cx))),
        None => {
            sec = sec
                .child(
                    row().flex_none().mt(px(SPACE_1_5)).child(
                        Button::new(RUNNER, "Attach agent, orchestrator or pipeline")
                            .small()
                            .secondary()
                            .icon(IconName::Plus)
                            .selected(open)
                            .on_click(cx.listener(|this, _, window, cx| this.open_runner_picker(window, cx)))
                            .popover_anchor(RUNNER)
                            .probe(RUNNER),
                    ),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(cw))
                        .mt(px(SPACE_2))
                        .child(Note::new("Attach one and it starts on this issue. Nothing is attached yet.")),
                );
        }
    }
    sec.into_any_element()
}

/// One row of the properties: the muted key, then the value.
fn kv(key: &'static str, value: impl IntoElement) -> gpui::Div {
    row()
        .flex_none()
        .w_full()
        .gap(px(SPACE_3))
        .min_h(px(CONTROL_HEIGHT_SM))
        .child(div().flex_none().w(px(KEY_WIDTH)).type_style(LABEL).text_color(TEXT_MUTED).child(key))
        .child(row().flex_1().min_w_0().child(value))
}

fn priority_color(priority: Priority) -> Hsla {
    match priority {
        Priority::High => WARNING,
        Priority::Medium => TEXT_SECONDARY,
        Priority::Low => TEXT_MUTED,
    }
}

/// A small select (`.select.sm`): the value in its colour and a chevron, as wide as its value.
fn select_trigger(
    id: &'static str,
    value: &'static str,
    color: Hsla,
    window: &mut Window,
    cx: &mut Context<IssuePanel>,
) -> gpui::Stateful<gpui::Div> {
    let open = ui::overlays(window, cx).is_open(id, cx);
    let border = motion::fade_color(SharedString::from(format!("{id}-border")), if open { ACCENT } else { LINE_STRONG }, window, cx);
    let el = row()
        .id(id)
        .debug_selector(move || id.into())
        .flex_none()
        .gap(px(SPACE_2))
        .h(px(CONTROL_HEIGHT_SM))
        .pl(px(SPACE_2_5))
        .pr(px(SPACE_2))
        .border_1()
        .rounded(px(RADIUS_MD))
        .type_style(LABEL)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(div().flex_none().text_color(color).child(value))
        .child(icon(IconName::ChevronDown, 12., TEXT_MUTED));
    let rest = Look::bg(SURFACE_RAISED).border(border);
    let hover = Look::bg(SURFACE_HOVER).border(border);
    motion::hover_fade(el, id, rest, hover, window, cx)
}

/// The owner: avatar and name, a quiet row that opens the owner picker.
fn owner_trigger(owner: &str, window: &mut Window, cx: &mut Context<IssuePanel>) -> gpui::Stateful<gpui::Div> {
    let name: SharedString = if owner.is_empty() { "No owner".into() } else { owner.to_string().into() };
    let el = row()
        .id(OWNER)
        .debug_selector(|| OWNER.into())
        // As wide as the name, narrower (the name cut) when the row has no room.
        .flex_initial()
        .min_w_0()
        .gap(px(SPACE_2))
        .h(px(CONTROL_HEIGHT_SM))
        .px(px(SPACE_1_5))
        .ml(px(-SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .child(Avatar::new(pickers::initials(owner)))
        .child(truncate(name).text_color(if owner.is_empty() { TEXT_MUTED } else { TEXT }));
    motion::hover_fade(el, OWNER, Look::bg(TRANSPARENT), Look::bg(SURFACE_RAISED), window, cx)
}

/// The attached runner: what it is, its latest run in one line, "Start when" and its actions.
fn runner_card(
    this: &IssuePanel,
    issue: &Arc<Issue>,
    runner: &RunnableRef,
    open: bool,
    _window: &mut Window,
    cx: &mut Context<IssuePanel>,
) -> AnyElement {
    let cw = this.content_width();
    let catalog = this.catalog.read(cx);
    let runnable = catalog.runnable(runner);
    let kind = runnable.map(|r| r.kind).unwrap_or(match runner.scheme() {
        "pipeline" => RunnableKind::Pipeline,
        "orchestrator" => RunnableKind::Orchestrator,
        _ => RunnableKind::Agent,
    });
    let name: SharedString = runnable.map_or_else(|| runner.id().to_string(), |r| r.name.clone()).into();
    let state = catalog.activity(runner).map(|a| pickers::state_label(&a.state).0);
    let sub: SharedString = match state {
        Some(state) => format!("{} · {state}", pickers::kind_label(kind)).into(),
        None => pickers::kind_label(kind).into(),
    };
    let head = row()
        .flex_none()
        .gap(px(SPACE_2))
        .child(Tile::new(pickers::kind_icon(kind)))
        .child(
            col()
                .flex_1()
                .min_w_0()
                .child(truncate(name).type_style(BODY_STRONG))
                .child(truncate_lh(sub, LABEL.line).type_style(LABEL).text_color(TEXT_MUTED)),
        )
        .child(
            Button::new(RUNNER, "Change")
                .small()
                .ghost()
                .selected(open)
                .on_click(cx.listener(|this, _, window, cx| this.open_runner_picker(window, cx)))
                .popover_anchor(RUNNER)
                .probe(RUNNER),
        )
        .debug_selector(|| "issue-runner-head".into());
    let latest = this.latest_run(cx);
    let mut card = col()
        .flex_none()
        .w(px(cw))
        .px(px(SPACE_3))
        .py(px(SPACE_2_5))
        .rounded(px(RADIUS_XL))
        .border_1()
        .border_color(LINE)
        .bg(SURFACE)
        .child(head);
    if let Some(run) = latest.as_deref() {
        card = card.child(run::summary(run, run::wait_of(this, run, cx)));
    }
    card = card.child(triggers_row(issue.triggers, cw >= TRIGGERS_INLINE, cx));
    card.child(actions_row(runner, kind, latest.as_deref(), cx)).named("issue-runner-card").into_any_element()
}

/// "Start when": on assign, on comment, on @mention.
/// Content width from which "Start when" and its chips share one line, px.
const TRIGGERS_INLINE: f32 = 440.;

/// "Start when" and its chips: on one line when there is room, else the label above them.
fn triggers_row(triggers: StartTriggers, inline: bool, cx: &mut Context<IssuePanel>) -> impl IntoElement {
    type Flip = fn(&mut StartTriggers);
    let chips: [(&str, bool, Flip); 3] = [
        ("On assign", triggers.on_assign, |t| t.on_assign = !t.on_assign),
        ("On comment", triggers.on_comment, |t| t.on_comment = !t.on_comment),
        ("On @mention", triggers.on_mention, |t| t.on_mention = !t.on_mention),
    ];
    let label = || div().flex_none().mr(px(SPACE_1)).type_style(LABEL).text_color(TEXT_MUTED).child("Start when");
    let mut out = row().flex_none().gap(px(SPACE_1_5)).when(inline, |d| d.child(label()));
    for (ix, (label, on, flip)) in chips.into_iter().enumerate() {
        let name = SharedString::from(format!("issue-trigger-{ix}"));
        out = out.child(
            Chip::new(name.clone(), label)
                .pressed(on)
                .when(on, |c| c.icon(IconName::Check))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(issue) = this.current(cx) else { return };
                    let mut triggers = issue.triggers;
                    flip(&mut triggers);
                    this.patch(IssuePatch { triggers: Some(triggers), ..IssuePatch::default() }, cx);
                }))
                .probe(name),
        );
    }
    let top = col().flex_none().mt(px(SPACE_2_5)).debug_selector(|| "issue-runner-triggers".into());
    if inline { top.child(out) } else { top.gap(px(SPACE_1_5)).child(label()).child(out) }
}

/// "Open pipeline" (or the agent) and Detach.
fn actions_row(runner: &RunnableRef, kind: RunnableKind, latest: Option<&Run>, cx: &mut Context<IssuePanel>) -> impl IntoElement {
    let (label, route) = match kind {
        RunnableKind::Pipeline => (
            "Open pipeline",
            Route::Editor { pipeline: latest.map_or_else(|| PipelineId::new(runner.id()), |run| run.pipeline.clone()), node: None },
        ),
        RunnableKind::Orchestrator => ("Open orchestrator", Route::Agents { runnable: Some(runner.clone()) }),
        RunnableKind::Agent => ("Open agent", Route::Agents { runnable: Some(runner.clone()) }),
    };
    row()
        .flex_none()
        .gap(px(SPACE_1))
        .mt(px(SPACE_2_5))
        .debug_selector(|| "issue-runner-actions".into())
        .child(
            Button::new("issue-open-runner", label)
                .small()
                .ghost()
                .icon(IconName::External)
                .on_click(move |_, _, cx| hub::navigate(route.clone(), cx))
                .probe("issue-open-runner"),
        )
        .child(grow())
        .child(
            Button::new("issue-detach", "Detach")
                .small()
                .danger()
                .on_click(cx.listener(|this, _, _, cx| {
                    hub::commands(cx).attach_runner(&this.issue, None, cx).detach();
                }))
                .probe("issue-detach"),
        )
}

impl IssuePanel {
    /// Swaps the title for a field holding it, focused and selected.
    pub(super) fn edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(issue) = self.current(cx) else { return };
        self.title.update(cx, |input, cx| {
            input.set_text(&issue.title, cx);
            input.select_all(cx);
        });
        self.editing_title = true;
        self.title.read(cx).focus(window);
        cx.notify();
    }

    /// Ends editing; `save` sends the new title when it changed and is not empty.
    fn finish_title(&mut self, save: bool, cx: &mut Context<Self>) {
        if !self.editing_title {
            return;
        }
        self.editing_title = false;
        let text = self.title.read(cx).text().trim().to_string();
        let changed = self.current(cx).is_some_and(|issue| issue.title != text);
        if save && changed && !text.is_empty() {
            self.patch(IssuePatch { title: Some(text), ..IssuePatch::default() }, cx);
        }
        cx.notify();
    }

    pub(super) fn on_title_event(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Submit => {
                self.finish_title(true, cx);
                window.focus(&self.focus);
            }
            TextInputEvent::Cancel => {
                self.finish_title(false, cx);
                window.focus(&self.focus);
            }
            TextInputEvent::Blur => self.finish_title(true, cx),
            _ => {}
        }
    }
}
