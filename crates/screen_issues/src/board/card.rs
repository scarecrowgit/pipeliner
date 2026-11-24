//! One issue card of the board (`icard()` of the mockups) and the words it says about the
//! latest run.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, HighlightStyle, Hsla, IntoElement, Render, SharedString, StyledText, Window, div,
    prelude::*, px,
};
use hub::api::{Issue, IssueKey, IssueStatus, NodeStatus, Priority, Run, RunState, RunnableRef, SpanKind};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::IssuesView;

/// The card width of a board column.
pub(crate) const CARD_WIDTH: f32 = 256.;
/// Most lines the run line of a card takes: a long error is cut, the panel has all of it.
const STRIP_LINES: usize = 3;

/// How the run line of a card is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripTone {
    /// Queued, cancelled: muted.
    Muted,
    /// Working right now: blue dot, secondary text.
    Running,
    /// Waiting for a person: amber.
    Waiting,
    /// Finished well: green.
    Done,
    /// Failed: red.
    Failed,
}

impl StripTone {
    /// The dot of the tone.
    pub(crate) fn status(self) -> Status {
        match self {
            StripTone::Muted => Status::Idle,
            StripTone::Running => Status::Running,
            StripTone::Waiting => Status::Waiting,
            StripTone::Done => Status::Live,
            StripTone::Failed => Status::Error,
        }
    }

    fn color(self) -> Hsla {
        match self {
            StripTone::Muted => TEXT_MUTED,
            StripTone::Running => TEXT_SECONDARY,
            StripTone::Waiting => WARNING,
            StripTone::Done => SUCCESS,
            StripTone::Failed => DANGER,
        }
    }
}

/// The run line of a card: "Reproducer running · Log digger spawned" and a progress bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Strip {
    pub tone: StripTone,
    pub text: String,
    /// Agents created by other agents, said after the text in violet.
    pub spawned: Option<String>,
    /// Running runs show how far they are.
    pub progress: Option<f32>,
}

impl Strip {
    fn new(tone: StripTone, text: impl Into<String>) -> Self {
        Self { tone, text: text.into(), spawned: None, progress: None }
    }
}

/// What a card says about `run`, the latest run of `issue`; without a run, a merged pull
/// request of a done issue is still worth a line.
pub(crate) fn strip(issue: &Issue, run: Option<&Run>) -> Option<Strip> {
    let Some(run) = run else {
        let pr = issue.pr_url.as_deref().filter(|_| issue.status == IssueStatus::Done)?;
        return Some(Strip::new(StripTone::Done, pr_label(pr)));
    };
    Some(match run.state {
        RunState::Queued => Strip::new(StripTone::Muted, "Queued"),
        RunState::Running => {
            let names = |spawned: bool| {
                let mut names: Vec<&str> = Vec::new();
                for node in &run.nodes {
                    if node.status == NodeStatus::Running
                        && (node.kind == SpanKind::Spawned) == spawned
                        && !names.contains(&node.name.as_str())
                    {
                        names.push(&node.name);
                    }
                }
                names.join(", ")
            };
            let (working, spawned) = (names(false), names(true));
            let text = if working.is_empty() { "Running".to_string() } else { format!("{working} running") };
            Strip {
                tone: StripTone::Running,
                text,
                spawned: (!spawned.is_empty()).then(|| format!("{spawned} spawned")),
                progress: Some(run.progress()),
            }
        }
        RunState::WaitingReview => {
            let summary = waiting_node(run).map(|node| node.summary.as_str()).filter(|s| !s.is_empty());
            Strip::new(StripTone::Waiting, summary.unwrap_or("Waiting for review"))
        }
        RunState::WaitingAnswer => {
            let question = run.nodes.iter().rev().find_map(|node| node.question.as_deref());
            match question {
                Some(question) => Strip::new(StripTone::Waiting, format!("asks: {question}")),
                None => Strip::new(StripTone::Waiting, "Waiting for an answer"),
            }
        }
        RunState::Done => {
            let pr = run.pr_url.as_deref().or(issue.pr_url.as_deref());
            Strip::new(StripTone::Done, pr.map_or_else(|| "Done".to_string(), pr_label))
        }
        RunState::Failed => {
            let reason = run.error.as_deref().and_then(|e| e.lines().next()).filter(|e| !e.is_empty());
            Strip::new(StripTone::Failed, reason.map_or_else(|| "Failed".to_string(), |e| format!("Failed: {e}")))
        }
        RunState::Cancelled => Strip::new(StripTone::Muted, "Cancelled"),
        RunState::Interrupted => Strip::new(StripTone::Waiting, "Interrupted · resume it from the run"),
    })
}

/// The strip's words wrapped beside the dot, at most [`STRIP_LINES`] lines. Plain words end
/// in an ellipsis when cut; words with the spawned agents in violet are only cut (GPUI's
/// ellipsis drops the runs after the first).
fn strip_block(strip: &Strip) -> gpui::Div {
    let block = div().flex_1().min_w_0().text_color(strip.tone.color()).line_clamp(STRIP_LINES);
    let block = if strip.spawned.is_none() { block.text_ellipsis() } else { block };
    block.child(strip_text(strip))
}

/// The strip's words, the spawned agents in violet after a middle dot, wrapping as one text.
fn strip_text(strip: &Strip) -> StyledText {
    match &strip.spawned {
        None => StyledText::new(strip.text.clone()),
        Some(spawned) => {
            let text = format!("{} · {spawned}", strip.text);
            let start = text.len() - spawned.len();
            let violet = HighlightStyle { color: Some(SPAWN_TEXT), ..HighlightStyle::default() };
            StyledText::new(text.clone()).with_highlights(vec![(start..text.len(), violet)])
        }
    }
}

fn waiting_node(run: &Run) -> Option<&hub::api::NodeInstance> {
    run.nodes.iter().rev().find(|node| node.status == NodeStatus::Waiting)
}

/// "PR #2280" from a pull request URL; the URL itself when it has no number at the end.
fn pr_label(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url);
    if !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) { format!("PR #{last}") } else { url.to_string() }
}

/// Two uppercase letters for an owner's avatar; "You" is "ME".
pub(crate) fn initials(owner: &str) -> SharedString {
    if owner.eq_ignore_ascii_case("you") || owner.eq_ignore_ascii_case("me") {
        return "ME".into();
    }
    let mut words = owner.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let second = words.next_back().and_then(|w| w.chars().next());
    let letters: String = match (first, second) {
        (Some(a), Some(b)) => [a, b].iter().collect(),
        (Some(_), None) => owner.chars().filter(|c| c.is_alphanumeric()).take(2).collect(),
        _ => String::new(),
    };
    letters.to_uppercase().into()
}

/// The colour of a priority's word.
pub(crate) fn priority_color(priority: Priority) -> Hsla {
    match priority {
        Priority::High => WARNING,
        Priority::Medium => TEXT_SECONDARY,
        Priority::Low => TEXT_MUTED,
    }
}

/// Icon and kind word of what is attached.
pub(crate) fn runner_kind(runner: &RunnableRef) -> (IconName, &'static str) {
    match runner.scheme() {
        "orchestrator" => (IconName::Orchestrator, "Orchestrator"),
        "pipeline" => (IconName::Pipelines, "Pipeline"),
        _ => (IconName::Agent, "Agent"),
    }
}

/// The value a dragged card carries.
#[derive(Clone, Debug)]
pub(crate) struct DraggedIssue {
    pub key: IssueKey,
    pub status: IssueStatus,
    pub title: SharedString,
}

/// What follows the pointer while a card is dragged.
pub(crate) struct DragGhost {
    key: IssueKey,
    title: SharedString,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        col()
            .w(px(CARD_WIDTH - 16.))
            .p(px(SPACE_3))
            .gap(px(SPACE_1))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(ACCENT_RING)
            .rounded(px(RADIUS_XL))
            .opacity(0.92)
            .child(div().type_style(MONO_SM).text_color(TEXT_MUTED).child(self.key.to_string()))
            .child(div().type_style(BODY_STRONG).line_clamp(2).child(self.title.clone()))
    }
}

impl IssuesView {
    /// The card of `issue` as `icard()` draws it.
    pub(crate) fn render_card(&self, issue: &Arc<Issue>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = issue.key.clone();
        let selected = self.cursor.as_ref() == Some(&key);
        let run = self.runs.read(cx).latest_for(&key).cloned();
        let strip = strip(issue, run.as_deref());
        let id = SharedString::from(format!("issue-card-{key}"));

        let head = row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .child(div().flex_none().type_style(MONO).text_color(TEXT_MUTED).child(key.to_string()))
            .child(div().flex_none().text_color(priority_color(issue.priority)).child(issue.priority.label()))
            .child(grow())
            .when(!issue.owner.is_empty(), |d| d.child(Avatar::new(initials(&issue.owner))));
        let title = div()
            .mt(px(SPACE_1))
            .w_full()
            .type_style(BODY_STRONG)
            .line_clamp(3)
            .child(issue.title.clone());
        let filed_by = issue
            .filed_by
            .as_ref()
            .map(|by| format!("by {}", self.runner_name(by, cx)));
        let tags = row()
            .mt(px(SPACE_2))
            .gap(px(SPACE_1_5))
            .child(Tag::new(issue.kind.label()))
            .when_some(filed_by, |d, by| d.child(SpawnChip::new(by).shrinks()))
            .when(!issue.images.is_empty(), |d| {
                d.child(
                    row()
                        .flex_none()
                        .gap(px(SPACE_1))
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .child(icon(IconName::Paperclip, 12., TEXT_MUTED))
                        .child(issue.images.len().to_string()),
                )
            });
        let runner = issue.runner.as_ref().map(|runner| {
            let (glyph, kind) = runner_kind(runner);
            row()
                .mt(px(SPACE_2_5))
                .pt(px(SPACE_2_5))
                .gap(px(SPACE_2))
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .type_style(LABEL)
                .child(icon(glyph, 12., TEXT_MUTED))
                .child(div().flex_none().text_color(TEXT_MUTED).child(kind))
                .child(truncate_lh(self.runner_name(runner, cx), LABEL.line).font_weight(gpui::FontWeight::MEDIUM))
        });
        let strip = strip.map(|strip| {
            let progress = strip.progress;
            col()
                .child(
                    row()
                        .mt(px(SPACE_1_5))
                        .items_start()
                        .gap(px(SPACE_2))
                        .type_style(LABEL)
                        .child(div().flex_none().mt(px(6.)).child(StatusDot::new(strip.tone.status()).small()))
                        .child(strip_block(&strip)),
                )
                .when_some(progress, |d, p| d.child(div().mt(px(SPACE_2)).child(Progress::new(p).tone(ProgressTone::Info))))
        });

        let (rest, hover) = if selected {
            (Look::bg(SURFACE).border(ACCENT_RING), Look::bg(SURFACE).border(ACCENT_RING))
        } else {
            (Look::bg(SURFACE).border(LINE), Look::bg(SURFACE).border(LINE_STRONG))
        };
        let faded = motion::hover_fade_state(id.clone(), rest, hover, window, cx);
        let selector = id.to_string();
        let dragged = DraggedIssue { key: key.clone(), status: issue.status, title: issue.title.clone().into() };
        let el = div()
            .id(id.clone())
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .p(px(SPACE_3))
            .border_1()
            .rounded(px(RADIUS_XL))
            .cursor_pointer();
        faded
            .apply(el)
            .when(selected, |d| d.shadow(selected_glow()))
            .child(head)
            .child(title)
            .child(tags)
            .children(runner)
            .children(strip)
            .on_click(cx.listener(move |this, _, window, cx| this.open_issue(&key, window, cx)))
            .on_drag(dragged, |dragged, _, _, cx| {
                let (key, title) = (dragged.key.clone(), dragged.title.clone());
                cx.new(|_| DragGhost { key, title })
            })
            .probe(id)
            .into_any_element()
    }

    /// The display name of a runnable: from the catalog, else its id.
    pub(crate) fn runner_name(&self, runner: &RunnableRef, cx: &App) -> SharedString {
        match self.catalog.read(cx).runnable(runner) {
            Some(runnable) => runnable.name.clone().into(),
            None => runner.id().to_string().into(),
        }
    }
}
