//! The activity feed, oldest first and newest at the bottom, in the wording of
//! `issue-comments.html`: comments by people as cards (with the wake-ups their mentions caused
//! inside), comments and notes by agents and pipelines as rows with a tile, status changes,
//! attaching and detaching, wake-ups and their failures, pull requests.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, HighlightStyle, Hsla, IntoElement, SharedString, StyledText, Window, div, prelude::*, px,
};
use hub::api::{Actor, Issue, IssueEvent, IssueEventKind, RunId, RunnableRef, Target};
use hub::{LoadState, Route};
use ui::dev::Probe;
use ui::motion;
use ui::prelude::*;
use ui_rich::markdown::Markdown;

use super::props::thumbnail;
use super::run::pr_number;
use super::{Named, IssuePanel, section, section_head};

/// Space before a comment's body, under its avatar, px.
const INDENT: f32 = 28.;
/// Inner width of a comment card, px: the content less the card's padding and border.
fn card_inner(content: f32) -> f32 {
    content - 2. * SPACE_3 - 2.
}

/// Width of the text of a row with a tile, px.
fn row_text(content: f32) -> f32 {
    content - 20. - SPACE_2_5
}
/// Room a box keeps for its button or link at the right, px.
const BOX_ACTION: f32 = 88.;

/// One entry of the feed: an event and the wake-ups drawn inside it.
struct Entry<'a> {
    ix: usize,
    event: &'a IssueEvent,
    inner: Vec<(usize, &'a IssueEvent)>,
}

/// Groups the wake-ups (and their failures) a comment's mentions caused under that comment.
fn entries(feed: &[IssueEvent]) -> Vec<Entry<'_>> {
    let mut out: Vec<Entry<'_>> = Vec::new();
    for (ix, event) in feed.iter().enumerate() {
        let target = match &event.kind {
            IssueEventKind::Woken { target, .. } | IssueEventKind::WakeFailed { target, .. } => Some(target),
            _ => None,
        };
        if let (Some(target), Some(last)) = (target, out.last_mut())
            && let IssueEventKind::Comment { mentions, .. } = &last.event.kind
            && mentions.contains(target)
        {
            last.inner.push((ix, event));
            continue;
        }
        out.push(Entry { ix, event, inner: Vec::new() });
    }
    out
}

fn is_person_comment(event: &IssueEvent) -> bool {
    matches!(event.kind, IssueEventKind::Comment { .. }) && event.actor == Actor::Me
}

/// The Activity section.
pub(super) fn feed(this: &mut IssuePanel, issue: &Arc<Issue>, window: &mut Window, cx: &mut Context<IssuePanel>) -> AnyElement {
    let cw = this.content_width();
    let loaded: Result<Vec<IssueEvent>, Option<SharedString>> = match this.issues.read(cx).activity(&this.issue) {
        None => Err(None),
        Some(feed) => match (feed.value(), feed.state()) {
            (Some(events), _) => Ok(events.as_ref().clone()),
            (None, LoadState::Failed(why)) => Err(Some(why.clone())),
            (None, _) => Err(None),
        },
    };
    let events = match loaded {
        Ok(events) => events,
        Err(Some(why)) => {
            return section()
                .child(section_head("Activity", None))
                .child(muted(format!("Could not load the activity · {why}"), cw))
                .into_any_element();
        }
        Err(None) => {
            return section().child(section_head("Activity", None)).child(muted("Loading activity…", cw)).into_any_element();
        }
    };
    let grouped = entries(&events);
    let comments = events.iter().filter(|e| is_person_comment(e)).count() as u64;
    let end = format!("{} · {}", grouped.len(), base::fmt::plural(comments, "comment", "comments"));
    let now = base::time::now_ms();
    let offset = base::time::utc_offset(now);
    let mut list = col().flex_none().w_full();
    for (n, entry) in grouped.iter().enumerate() {
        let woken_by = woken_by(&grouped[..n], entry.event);
        list = list.child(render_entry(this, issue, entry, woken_by, now, offset, window, cx));
    }
    section().child(section_head("Activity", Some(end.into()))).child(list).into_any_element()
}

fn muted(text: impl Into<SharedString>, width: f32) -> impl IntoElement {
    div().flex_none().w(px(width)).type_style(LABEL).text_color(TEXT_MUTED).child(text.into())
}

/// "woken by @mention from You" for an agent's comment that follows the wake-up of it.
fn woken_by(before: &[Entry<'_>], event: &IssueEvent) -> Option<String> {
    let Actor::Runnable { reference, .. } = &event.actor else { return None };
    if !matches!(event.kind, IssueEventKind::Comment { .. }) {
        return None;
    }
    before.iter().rev().find_map(|entry| {
        let woke = entry.inner.iter().any(|(_, e)| matches!(&e.kind, IssueEventKind::Woken { target, .. } if target == reference));
        woke.then(|| format!("woken by @mention from {}", who(&entry.event.actor).0))
    })
}

/// The name of an actor and its colour.
fn who(actor: &Actor) -> (SharedString, Hsla) {
    match actor {
        Actor::Me => ("You".into(), TEXT),
        Actor::Runnable { name, spawned, .. } => (name.clone().into(), if *spawned { SPAWN_TEXT } else { TEXT }),
        Actor::System => ("Harness".into(), TEXT),
    }
}

/// The 20px tile of an actor that is not a person.
fn small_tile(reference: Option<&RunnableRef>, spawned: bool) -> impl IntoElement {
    let glyph = match reference.map(RunnableRef::scheme) {
        Some("pipeline") => IconName::Pipelines,
        Some("orchestrator") => IconName::Orchestrator,
        Some("spawned") => IconName::Spawn,
        Some(_) => IconName::Agent,
        None => IconName::Zap,
    };
    let (bg, fg) = if spawned { (SPAWN_SOFT, SPAWN_TEXT) } else { (SURFACE_HOVER, TEXT_SECONDARY) };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(px(RADIUS_MD))
        .bg(bg)
        .child(icon(glyph, 12., fg))
}

/// "Fixer 5m · woken by …": the name strong, the rest muted.
fn byline(name: SharedString, color: Hsla, rest: String, width: f32) -> impl IntoElement {
    let text = format!("{name} {rest}");
    let strong = HighlightStyle { color: Some(color), font_weight: Some(gpui::FontWeight::MEDIUM), ..HighlightStyle::default() };
    let muted = HighlightStyle { color: Some(TEXT_MUTED), ..HighlightStyle::default() };
    let split = name.len();
    div()
        .flex_none()
        .w(px(width))
        .type_style(LABEL)
        .child(StyledText::new(text.clone()).with_highlights([(0..split, strong), (split..text.len(), muted)]))
}

/// What an event says, for rows that are not comments.
fn sentence(this: &IssuePanel, issue: &Issue, event: &IssueEvent, cx: &App) -> String {
    let name_of = |r: &RunnableRef| this.catalog.read(cx).runnable(r).map_or_else(|| r.id().to_string(), |r| r.name.clone());
    match &event.kind {
        IssueEventKind::Created => match &issue.filed_by {
            Some(agent) if event.actor != Actor::Me => format!("Filed this issue for {}.", name_of(agent)),
            _ => "Created this issue.".into(),
        },
        IssueEventKind::StatusChanged { from, to } => format!("Moved it from {} to {}.", from.label(), to.label()),
        IssueEventKind::Attached { runner } => format!("Attached {}.", name_of(runner)),
        IssueEventKind::Detached => "Detached the runner.".into(),
        IssueEventKind::Note { text } => text.clone(),
        IssueEventKind::Woken { name, number, cause, .. } => woken_text(name, *number, cause),
        IssueEventKind::WakeFailed { name, reason, .. } => format!("{name} not woken · {reason}"),
        IssueEventKind::PrOpened { url } => match pr_number(url) {
            Some(n) => format!("Opened pull request #{n}."),
            None => "Opened a pull request.".into(),
        },
        IssueEventKind::Comment { text, .. } => text.clone(),
    }
}

fn woken_text(name: &str, number: u32, cause: &str) -> String {
    let mut text = format!("{name} woken");
    if number > 0 {
        text.push_str(&format!(" · run {number} on this issue"));
    }
    if !cause.is_empty() {
        text.push_str(&format!(" · {cause}"));
    }
    text
}

#[expect(clippy::too_many_arguments, reason = "one entry needs the feed's clock and the panel's context")]
fn render_entry(
    this: &mut IssuePanel,
    issue: &Arc<Issue>,
    entry: &Entry<'_>,
    woken_by: Option<String>,
    now: i64,
    offset: i32,
    window: &mut Window,
    cx: &mut Context<IssuePanel>,
) -> AnyElement {
    let event = entry.event;
    let cw = this.content_width();
    let row_w = row_text(cw);
    let when = base::time::ago(now, event.at, offset);
    let probe = format!("issue-activity-{}", entry.ix);
    if is_person_comment(event) {
        return comment_card(this, entry, &when, window, cx);
    }
    let (name, color) = who(&event.actor);
    let rest = match woken_by {
        Some(by) => format!("{when} · {by}"),
        None => when,
    };
    let lead: AnyElement = match &event.actor {
        Actor::Me => Avatar::new("ME").into_any_element(),
        Actor::Runnable { reference, spawned, .. } => small_tile(Some(reference), *spawned).into_any_element(),
        Actor::System => small_tile(None, false).into_any_element(),
    };
    let mut text = col().flex_none().w(px(row_w)).child(byline(name, color, rest, row_w));
    // What a row that is not a comment says, selectable.
    let said = |this: &mut IssuePanel, cx: &mut Context<IssuePanel>| {
        let words = sentence(this, issue, event, cx);
        let view = this.texts.view(format!("event-{}", event.seq), words, BODY, TEXT_SECONDARY, cx);
        div().flex_none().w(px(row_w)).child(view).debug_selector({
            let name = format!("{probe}-text");
            move || name
        })
    };
    match &event.kind {
        IssueEventKind::Comment { images, .. } => {
            if let Some(body) = this.bodies.get(&event.seq) {
                text = text.child(div().flex_none().w(px(row_w)).mt(px(SPACE_0_5)).child(body.clone()));
            }
            if !images.is_empty() {
                let thumbs = images.iter().enumerate().map(|(i, image)| thumbnail((SharedString::from(format!("{probe}-image")), i), image, cx));
                text = text.child(row().flex_none().flex_wrap().gap(px(SPACE_2)).mt(px(SPACE_2)).children(thumbs));
            }
        }
        IssueEventKind::Woken { run, .. } => {
            text = text.child(said(this, cx));
            if let Some(run) = run.clone() {
                text = text.child(row().mt(px(SPACE_0_5)).child(open_run_link(format!("{probe}-open"), run)));
            }
        }
        IssueEventKind::WakeFailed { .. } => {
            return div()
                .flex_none()
                .py(px(SPACE_1_5))
                .child(fail_box(this, entry.ix, event, cw, cx))
                .named(probe)
                .into_any_element();
        }
        IssueEventKind::PrOpened { url } => {
            text = text.child(said(this, cx));
            let url = url.clone();
            text = text.child(
                row().mt(px(SPACE_0_5)).child(
                    Link::new(SharedString::from(format!("{probe}-pr")), "Open pull request").on_click(move |_, _, cx| cx.open_url(&url)),
                ),
            );
        }
        _ => {
            text = text.child(said(this, cx));
        }
    }
    row()
        .flex_none()
        .w_full()
        .items_start()
        .gap(px(SPACE_2_5))
        .py(px(SPACE_1_5))
        .child(lead)
        .child(text)
        .named(probe)
        .into_any_element()
}

/// "Open run": the run's pipeline when the panel knows it, else wherever the run leads.
fn open_run_link(name: String, run: RunId) -> impl IntoElement {
    let id = SharedString::from(name.clone());
    Link::new(id, "Open run")
        .on_click(move |_, _, cx| {
            let known = hub::runs(cx).read(cx).find(&run).map(|r| r.pipeline.clone());
            match known {
                Some(pipeline) => hub::navigate(Route::Editor { pipeline, node: None }, cx),
                None => hub::open(Target::Run(run.clone()), cx),
            }
        })
        .probe(name)
}

/// A comment by a person: a card with the avatar, the time, the body, its images and the
/// wake-ups it caused; one's own shows Delete while hovered.
fn comment_card(this: &mut IssuePanel, entry: &Entry<'_>, when: &str, window: &mut Window, cx: &mut Context<IssuePanel>) -> AnyElement {
    let event = entry.event;
    let ix = entry.ix;
    let key = SharedString::from(format!("issue-activity-{ix}"));
    let hovered = motion::hovered(key.clone(), window, cx);
    let shown = motion::fade_value(SharedString::from(format!("{key}-delete-fade")), if hovered { 1. } else { 0. }, window, cx);
    let seq = event.seq;
    let delete_name = SharedString::from(format!("issue-comment-delete-{ix}"));
    let delete = div().flex_none().opacity(shown).child(
        IconButton::new(delete_name.clone(), IconName::Trash)
            .small()
            .tooltip("Delete comment")
            .on_click(cx.listener(move |this, _, window, cx| this.delete_comment(seq, window, cx)))
            .probe(delete_name),
    );
    let head = row()
        .flex_none()
        .w_full()
        .h(px(CONTROL_HEIGHT_SM))
        .gap(px(SPACE_2))
        .type_style(LABEL)
        .child(Avatar::new("ME"))
        .child(div().flex_none().type_style(LABEL_STRONG).text_color(TEXT).child("You"))
        .child(div().flex_none().text_color(TEXT_MUTED).child(format!("{when} · comment")))
        .child(grow())
        .child(delete);
    let body_width = card_inner(this.content_width()) - INDENT;
    let mut card = col()
        .id(key.clone())
        .flex_none()
        .w_full()
        .my(px(SPACE_1_5))
        .px(px(SPACE_3))
        .pt(px(SPACE_1))
        .pb(px(SPACE_2_5))
        .rounded(px(RADIUS_XL))
        .border_1()
        .border_color(LINE)
        .child(head);
    if let Some(body) = this.bodies.get(&seq) {
        card = card.child(body_block(body, body_width));
    }
    if let IssueEventKind::Comment { images, .. } = &event.kind
        && !images.is_empty()
    {
        let thumbs = images.iter().enumerate().map(|(i, image)| thumbnail((SharedString::from(format!("{key}-image")), i), image, cx));
        card = card.child(row().flex_none().flex_wrap().gap(px(SPACE_2)).mt(px(SPACE_2)).ml(px(INDENT)).children(thumbs));
    }
    for (inner_ix, inner) in &entry.inner {
        let el = match &inner.kind {
            IssueEventKind::WakeFailed { .. } => fail_box(this, *inner_ix, inner, body_width, cx).into_any_element(),
            _ => woke_box(this, *inner_ix, inner, body_width, cx).into_any_element(),
        };
        card = card.child(div().flex_none().mt(px(SPACE_2)).ml(px(INDENT)).child(el));
    }
    let look = motion::Look::bg(SURFACE);
    motion::hover_fade(card, key.clone(), look, look, window, cx).named(key).into_any_element()
}

fn body_block(body: &Entity<Markdown>, width: f32) -> impl IntoElement {
    div().flex_none().w(px(width)).mt(px(SPACE_0_5)).ml(px(INDENT)).child(body.clone())
}

/// "Fixer woken · run 2 on this issue · comment passed as input  Open run".
fn woke_box(this: &mut IssuePanel, ix: usize, event: &IssueEvent, width: f32, cx: &mut Context<IssuePanel>) -> impl IntoElement {
    let IssueEventKind::Woken { name, number, cause, run, .. } = &event.kind else { return div().into_any_element() };
    let probe = format!("issue-activity-{ix}");
    let text_width = width - 2. * SPACE_2 - 12. - 2. * SPACE_2 - BOX_ACTION;
    let text = this.texts.view(format!("event-{}", event.seq), woken_text(name, *number, cause), LABEL, TEXT_SECONDARY, cx);
    row()
        .flex_none()
        .w(px(width))
        .items_start()
        .gap(px(SPACE_2))
        .px(px(SPACE_2))
        .py(px(SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .bg(SURFACE_RAISED)
        .type_style(LABEL)
        .child(div().flex_none().pt(px(3.)).child(icon(IconName::Zap, 12., TEXT_SECONDARY)))
        .child(div().flex_none().w(px(text_width)).child(text))
        .when_some(run.clone(), |d, run| d.child(grow()).child(open_run_link(format!("{probe}-open"), run)))
        .named(probe)
        .into_any_element()
}

/// "Codex not woken" in red, the reason under it (selectable), and Retry now.
fn fail_box(this: &mut IssuePanel, ix: usize, event: &IssueEvent, width: f32, cx: &mut Context<IssuePanel>) -> AnyElement {
    let IssueEventKind::WakeFailed { target, name, reason } = &event.kind else { return div().into_any_element() };
    let probe = format!("issue-activity-{ix}");
    let text_width = width - 2. * SPACE_2 - 12. - 2. * SPACE_2 - BOX_ACTION;
    let reason = this.texts.view(format!("event-{}", event.seq), reason.clone(), LABEL, TEXT_SECONDARY, cx);
    let target = target.clone();
    let key = this.issue.clone();
    let retry = SharedString::from(format!("issue-retry-wake-{ix}"));
    row()
        .flex_none()
        .w(px(width))
        .items_start()
        .gap(px(SPACE_2))
        .px(px(SPACE_2))
        .py(px(SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .bg(DANGER_SOFT)
        .type_style(LABEL)
        .child(div().flex_none().pt(px(3.)).child(icon(IconName::X, 12., DANGER)))
        .child(
            col()
                .flex_none()
                .w(px(text_width))
                .child(div().text_color(DANGER).child(format!("{name} not woken")))
                .child(div().w_full().child(reason).debug_selector({
                    let name = format!("{probe}-text");
                    move || name
                })),
        )
        .child(grow())
        .child(
            Button::new(retry.clone(), "Retry now")
                .small()
                .secondary()
                .on_click(cx.listener(move |_, _, _, cx| hub::commands(cx).retry_wake(&key, &target, cx)))
                .probe(retry),
        )
        .named(probe)
        .into_any_element()
}

impl IssuePanel {
    /// Deletes one's own comment (feed entry `seq`); the feed drops it when hub does.
    pub(super) fn delete_comment(&mut self, seq: u64, _window: &mut Window, cx: &mut Context<Self>) {
        hub::commands(cx).delete_comment(&self.issue, seq, cx);
    }
}
