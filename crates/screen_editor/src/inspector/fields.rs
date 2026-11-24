//! Text fields bound to the graph: one `TextInput` or `TextArea` per field, kept between frames
//! under a key, written to the draft as the person types (one folded undo step per field) and
//! refreshed from the graph only when the graph changed underneath them.
//!
//! A field's slot remembers the graph's value as it last saw it (`value`). Each frame the view
//! passes the graph's value again: when it differs, something else changed the graph (an undo,
//! the canvas, another field) and the text is replaced; when it is the same, whatever the
//! person is typing stays, even when it does not round-trip (`0.60` for `0.6`).

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, SharedString, Subscription, Window, div, prelude::*, px,
};
use hub::model::{AgentNode, GraphDef, NodeDef, NodeKind};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{SubmitOn, TextArea, TextAreaEvent, TextInput, TextInputEvent, text_area_field, text_field};

use super::InspectorView;
use super::ops::{agent_mut, node_mut};
use crate::Selection;

/// Writes a field's text into the graph. Gets what is shown, so a node field finds its node.
pub(crate) type Apply = Rc<dyn Fn(&mut GraphDef, &Selection, &str)>;
/// Checks a field's text for shape and returns it normalised as the graph would print it.
pub(crate) type Parse = Rc<dyn Fn(&str) -> Result<String, SharedString>>;
/// Runs after a field's edit went into the draft (an id rename re-selects the node).
pub(crate) type After = Rc<dyn Fn(&mut InspectorView, &str, &mut Context<InspectorView>)>;

/// Writes a picked model into a node (`None`: inherit or the default).
pub(crate) type SetModel = Rc<dyn Fn(&mut NodeDef, Option<hub::model::ModelRef>)>;
/// Writes a picked workdir id into a node.
pub(crate) type SetWorkdir = Rc<dyn Fn(&mut NodeDef, String)>;

/// The editor behind a field.
#[derive(Clone)]
pub(crate) enum Editor {
    Line(Entity<TextInput>),
    Area(Entity<TextArea>),
}

impl Editor {
    fn text(&self, cx: &App) -> String {
        match self {
            Editor::Line(input) => input.read(cx).text().to_string(),
            Editor::Area(area) => area.read(cx).text().to_string(),
        }
    }

    fn set_text(&self, text: &str, cx: &mut App) {
        match self {
            Editor::Line(input) => input.update(cx, |input, cx| input.set_text(text, cx)),
            Editor::Area(area) => area.update(cx, |area, cx| area.set_text(text, cx)),
        }
    }

    fn set_placeholder(&self, text: SharedString, cx: &mut App) {
        match self {
            Editor::Line(input) => input.update(cx, |input, cx| input.set_placeholder(text, cx)),
            Editor::Area(area) => area.update(cx, |area, cx| area.set_placeholder(text, cx)),
        }
    }

    /// Gives the editor keyboard focus.
    pub(crate) fn focus(&self, window: &mut Window, cx: &App) {
        match self {
            Editor::Line(input) => input.read(cx).focus(window),
            Editor::Area(area) => area.read(cx).focus(window),
        }
    }
}

/// How a field's text reaches the graph.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Commit {
    /// On every change, folded into one undo step named `what`.
    Live,
    /// On Enter, then the field empties: an "add a file" field under a list.
    Submit,
}

/// Everything a field needs, rebuilt every frame from the graph.
pub(crate) struct Spec {
    key: SharedString,
    value: String,
    what: &'static str,
    parse: Parse,
    apply: Apply,
    after: Option<After>,
    commit: Commit,
    rows: Option<(usize, usize)>,
    mono: bool,
    placeholder: SharedString,
}

impl Spec {
    /// A field shown as `key` (also its probe name) holding `value`, written by `apply`.
    pub(crate) fn new(key: impl Into<SharedString>, value: impl Into<String>, what: &'static str, apply: Apply) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            what,
            parse: Rc::new(|text: &str| Ok(text.to_string())),
            apply,
            after: None,
            commit: Commit::Live,
            rows: None,
            mono: false,
            placeholder: SharedString::default(),
        }
    }

    /// Checks the text's shape before it is written.
    pub(crate) fn parse(mut self, parse: impl Fn(&str) -> Result<String, SharedString> + 'static) -> Self {
        self.parse = Rc::new(parse);
        self
    }

    /// Runs after the edit went in.
    pub(crate) fn after(mut self, after: After) -> Self {
        self.after = Some(after);
        self
    }

    /// Writes on Enter and empties the field.
    pub(crate) fn on_submit(mut self) -> Self {
        self.commit = Commit::Submit;
        self
    }

    /// A multi-line field growing between `min` and `max` lines.
    pub(crate) fn area(mut self, min: usize, max: usize) -> Self {
        self.rows = Some((min, max));
        self
    }

    /// Machine text: mono.
    pub(crate) fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Muted text while empty.
    pub(crate) fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }
}

/// One live field.
struct Slot {
    editor: Editor,
    /// The graph's value as last seen or written by this field.
    value: String,
    /// The text has the wrong shape; nothing was written.
    error: Option<SharedString>,
    placeholder: SharedString,
    what: &'static str,
    parse: Parse,
    apply: Apply,
    after: Option<After>,
    commit: Commit,
    _sub: Subscription,
}

/// The fields of what the inspector shows.
#[derive(Default)]
pub(crate) struct Fields {
    slots: HashMap<SharedString, Slot>,
    /// Keys asked for this frame, in order: Tab moves along it.
    order: Vec<SharedString>,
    /// Bumped when the inspector shows something else; events of older editors are dropped.
    generation: u64,
}

impl Fields {
    /// Drops every field: the inspector shows something else now.
    pub(crate) fn reset(&mut self) {
        self.slots.clear();
        self.order.clear();
        self.generation += 1;
    }

    pub(crate) fn begin_frame(&mut self) {
        self.order.clear();
    }

    /// Drops the fields this frame did not draw.
    pub(crate) fn end_frame(&mut self) {
        let order = &self.order;
        self.slots.retain(|key, _| order.contains(key));
    }

    /// The editor of field `key`, if it was drawn.
    pub(crate) fn editor(&self, key: &str) -> Option<Editor> {
        self.slots.get(key).map(|slot| slot.editor.clone())
    }

    /// Keys of the fields drawn last frame, in order.
    pub(crate) fn keys(&self) -> &[SharedString] {
        &self.order
    }
}

/// What a field reports to the view.
enum FieldEvent {
    Changed,
    Submit,
    Blur,
    Focus,
}

impl InspectorView {
    /// The editor of `spec`, created on first use and refreshed from the graph.
    fn slot(&mut self, spec: Spec, window: &mut Window, cx: &mut Context<Self>) -> (Editor, Option<SharedString>) {
        let key = spec.key.clone();
        self.fields.order.push(key.clone());
        if let Some(slot) = self.fields.slots.get_mut(&key) {
            if slot.value != spec.value && slot.commit == Commit::Live {
                slot.editor.set_text(&spec.value, cx);
                slot.value = spec.value;
                slot.error = None;
            }
            if slot.placeholder != spec.placeholder {
                slot.editor.set_placeholder(spec.placeholder.clone(), cx);
                slot.placeholder = spec.placeholder;
            }
            slot.parse = spec.parse;
            slot.apply = spec.apply;
            slot.after = spec.after;
            slot.what = spec.what;
            return (slot.editor.clone(), slot.error.clone());
        }
        let generation = self.fields.generation;
        let (editor, sub) = match spec.rows {
            None => {
                let input = cx.new(|cx| {
                    TextInput::new(cx).placeholder(spec.placeholder.clone()).with_text(&spec.value).tab_moves_focus(true)
                });
                let k = key.clone();
                let sub = cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| {
                    let event = match event {
                        TextInputEvent::Changed => FieldEvent::Changed,
                        TextInputEvent::Submit => FieldEvent::Submit,
                        TextInputEvent::Blur => FieldEvent::Blur,
                        TextInputEvent::Focus => FieldEvent::Focus,
                        _ => return,
                    };
                    this.field_event(&k, generation, event, window, cx);
                });
                (Editor::Line(input), sub)
            }
            Some((min, max)) => {
                let area = cx.new(|cx| {
                    TextArea::new(cx)
                        .placeholder(spec.placeholder.clone())
                        .submit_on(SubmitOn::Never)
                        .rows(min, max)
                        .with_text(&spec.value)
                });
                let k = key.clone();
                let sub = cx.subscribe_in(&area, window, move |this, _, event: &TextAreaEvent, window, cx| {
                    let event = match event {
                        TextAreaEvent::Changed => FieldEvent::Changed,
                        TextAreaEvent::Blur => FieldEvent::Blur,
                        TextAreaEvent::Focus => FieldEvent::Focus,
                        _ => return,
                    };
                    this.field_event(&k, generation, event, window, cx);
                });
                (Editor::Area(area), sub)
            }
        };
        let value = if spec.commit == Commit::Submit { String::new() } else { spec.value };
        self.fields.slots.insert(
            key,
            Slot {
                editor: editor.clone(),
                value,
                error: None,
                placeholder: spec.placeholder,
                what: spec.what,
                parse: spec.parse,
                apply: spec.apply,
                after: spec.after,
                commit: spec.commit,
                _sub: sub,
            },
        );
        (editor, None)
    }

    fn field_event(&mut self, key: &SharedString, generation: u64, event: FieldEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.fields.generation {
            return;
        }
        let Some(slot) = self.fields.slots.get_mut(key) else { return };
        match event {
            FieldEvent::Changed if slot.commit == Commit::Live => {
                let text = slot.editor.text(cx);
                match (slot.parse)(&text) {
                    Ok(normal) => {
                        slot.error = None;
                        slot.value = normal;
                        let (apply, what, after) = (slot.apply.clone(), slot.what, slot.after.clone());
                        let shown = self.shown.clone();
                        self.draft.update(cx, |draft, cx| draft.edit_folded(what, |graph| apply(graph, &shown, &text), cx));
                        if let Some(after) = after {
                            after(self, &text, cx);
                        }
                    }
                    Err(error) => slot.error = Some(error),
                }
                cx.notify();
            }
            FieldEvent::Submit if slot.commit == Commit::Submit => {
                let text = slot.editor.text(cx).trim().to_string();
                if text.is_empty() {
                    return;
                }
                match (slot.parse)(&text) {
                    Ok(_) => {
                        slot.error = None;
                        slot.editor.set_text("", cx);
                        let (apply, what) = (slot.apply.clone(), slot.what);
                        let shown = self.shown.clone();
                        self.draft.update(cx, |draft, cx| draft.edit(what, |graph| apply(graph, &shown, &text), cx));
                    }
                    Err(error) => slot.error = Some(error),
                }
                cx.notify();
            }
            FieldEvent::Blur => {
                self.draft.update(cx, |draft, _| draft.seal());
                cx.notify();
            }
            FieldEvent::Focus => cx.notify(),
            FieldEvent::Changed | FieldEvent::Submit => {}
        }
    }

    /// A single-line field in the `.input.sm` frame. `problem` marks it and shows below.
    pub(crate) fn line(&mut self, spec: Spec, problem: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = spec.key.clone();
        let mono = spec.mono;
        let (editor, error) = self.slot(spec, window, cx);
        let Editor::Line(input) = &editor else { return div().into_any_element() };
        let danger = error.is_some() || problem.is_some();
        let field = text_field(key.clone(), input).small().danger(danger).when(mono, |f| f.mono());
        let el = div().w_full().min_w_0().child(field).probe(key);
        self.with_messages(el.into_any_element(), error.or(problem), cx)
    }

    /// A multi-line field in the `.textarea` frame.
    pub(crate) fn area(&mut self, spec: Spec, problem: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = spec.key.clone();
        let mono = spec.mono;
        let (editor, error) = self.slot(spec, window, cx);
        let Editor::Area(area) = &editor else { return div().into_any_element() };
        let danger = error.is_some() || problem.is_some();
        let field = text_area_field(key.clone(), area).danger(danger).when(mono, |f| f.mono());
        let el = div().w_full().min_w_0().child(field).probe(key);
        self.with_messages(el.into_any_element(), error.or(problem), cx)
    }

    /// `control` with a danger line under it when there is a message; the line can be
    /// selected and copied.
    fn with_messages(&mut self, control: AnyElement, message: Option<SharedString>, cx: &mut Context<Self>) -> AnyElement {
        match message {
            None => control,
            Some(message) => {
                let text = self.texts.text("field-message", message, LABEL, DANGER, cx);
                col().w_full().min_w_0().gap(px(SPACE_1_5)).child(control).child(text).into_any_element()
            }
        }
    }
}

/// An [`Apply`] that changes the node on show.
pub(crate) fn on_node(f: impl Fn(&mut NodeDef, &str) + 'static) -> Apply {
    Rc::new(move |graph, shown, text| {
        if let Selection::Node(id) = shown
            && let Some(node) = node_mut(graph, id)
        {
            f(node, text);
        }
    })
}

/// An [`Apply`] that changes the agent settings of the agent or orchestrator on show.
pub(crate) fn on_agent(f: impl Fn(&mut AgentNode, &str) + 'static) -> Apply {
    on_node(move |node, text| {
        if let Some(agent) = agent_mut(node) {
            f(agent, text);
        }
    })
}

/// An [`Apply`] that changes the kind-specific part of the node on show.
pub(crate) fn on_kind(f: impl Fn(&mut NodeKind, &str) + 'static) -> Apply {
    on_node(move |node, text| f(&mut node.kind, text))
}

/// An [`Apply`] that changes the pipeline itself.
pub(crate) fn on_graph(f: impl Fn(&mut GraphDef, &str) + 'static) -> Apply {
    Rc::new(move |graph, _, text| f(graph, text))
}

/// Shape check of an optional amount of dollars: empty, or a number not below zero.
pub(crate) fn parse_usd(text: &str) -> Result<String, SharedString> {
    let text = text.trim().trim_start_matches('$');
    if text.is_empty() {
        return Ok(String::new());
    }
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(usd_text(Some(v))),
        _ => Err("A number of dollars, like 0.60".into()),
    }
}

/// The text of an optional amount of dollars.
pub(crate) fn usd_text(usd: Option<f64>) -> String {
    usd.map(|v| format!("{v:.2}")).unwrap_or_default()
}

/// The amount of an optional dollars field (shape already checked).
pub(crate) fn usd_value(text: &str) -> Option<f64> {
    text.trim().trim_start_matches('$').parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

/// Shape check of an optional whole number.
pub(crate) fn parse_count(text: &str) -> Result<String, SharedString> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    text.parse::<u32>().map(|n| n.to_string()).map_err(|_| "A whole number, like 40".into())
}

/// The number of an optional whole-number field (shape already checked).
pub(crate) fn count_value(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// Shape check of a name or id that cannot hold spaces.
pub(crate) fn parse_word(text: &str) -> Result<String, SharedString> {
    if text.chars().any(char::is_whitespace) { Err("No spaces".into()) } else { Ok(text.to_string()) }
}
