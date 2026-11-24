//! Router nodes: the ordered rules, each a label and either a `when` condition or an `ask`
//! question for a model, and the implicit `else` at the end.

use std::rc::Rc;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};
use hub::model::{AskRule, GraphDef, NodeDef, NodeKind, PORT_ELSE, RouterNode, RouterRule};
use ui::dev::Probe;
use ui::prelude::*;

use super::fields::{Spec, count_value, on_kind, parse_count};
use super::form::{INNER, field, note, problem_line, titled};
use super::{InspectorView, ops};

/// Width of the fields inside a rule card.
const RULE_INNER: f32 = INNER - 2. * SPACE_3;

/// Changes rule `index` of the router on show.
fn on_rule(index: usize, f: impl Fn(&mut RouterRule, &str) + 'static) -> super::fields::Apply {
    on_kind(move |kind, text| {
        if let NodeKind::Router(router) = kind
            && let Some(rule) = router.rules.get_mut(index)
        {
            f(rule, text);
        }
    })
}

/// Changes the router on show, as one undo step.
fn router_edit(f: impl FnOnce(&mut RouterNode)) -> impl FnOnce(&mut NodeDef) {
    move |node| {
        if let NodeKind::Router(router) = &mut node.kind {
            f(router);
        }
    }
}

impl InspectorView {
    pub(super) fn router_sections(&mut self, graph: &GraphDef, node: &NodeDef, router: &RouterNode, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let problems: Vec<(Option<&'static str>, Option<String>, String)> = self
            .problems_of(Some(&node.id), cx)
            .into_iter()
            .map(|(_, message)| {
                let (field, rule) = ops::problem_field(&message);
                (field, rule, message)
            })
            .collect();
        let add = Button::new("insp-rule-add", "Add rule")
            .ghost()
            .small()
            .icon(IconName::Plus)
            .on_click(self.on_click(cx, |this, _, cx| this.edit_node("Add rule", router_edit(|r| { ops::add_rule(r); }), cx)))
            .probe("insp-rule-add");
        let count = router.rules.len();
        let mut section = titled("Rules", Some(add.into_any_element())).gap(px(SPACE_2));
        section = section.child(note("Tried top to bottom; the first match wins. Each rule is a port on the canvas.", INNER));
        for (index, rule) in router.rules.iter().enumerate() {
            let at = |field: &str| {
                problems
                    .iter()
                    .find(|(f, label, _)| *f == Some(field) && label.as_deref() == Some(rule.label.as_str()))
                    .map(|(_, _, message)| SharedString::from(message.clone()))
            };
            let card = self.rule_card(graph, node, index, count, rule, &at, window, cx);
            section = section.child(card);
        }
        let else_to: Vec<String> = graph
            .edges_from(&node.id)
            .filter(|e| e.port == PORT_ELSE)
            .filter_map(|e| graph.node(&e.to))
            .map(|n| n.name.clone())
            .collect();
        let else_text = if else_to.is_empty() { "skip: the run ends".to_string() } else { format!("goes to {}", else_to.join(", ")) };
        section = section.child(
            row()
                .gap(px(SPACE_2))
                .h(px(CONTROL_HEIGHT_MD))
                .px(px(SPACE_3))
                .rounded(px(RADIUS_XL))
                .border_1()
                .border_color(LINE_SUBTLE)
                .child(div().type_style(MONO).text_color(TEXT_SECONDARY).child("else"))
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("anything no rule matched"))
                .child(grow())
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(else_text))
                .probe("insp-rule-else"),
        );
        let general = problems
            .iter()
            .find(|(f, rule, _)| *f == Some("rules") && rule.is_none())
            .map(|(_, _, m)| SharedString::from(m.clone()));
        vec![section.children(problem_line(general, INNER)).into_any_element()]
    }

    #[expect(clippy::too_many_arguments, reason = "one rule card and where it sits")]
    fn rule_card(
        &mut self,
        graph: &GraphDef,
        node: &NodeDef,
        index: usize,
        count: usize,
        rule: &RouterRule,
        at: &dyn Fn(&str) -> Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = |part: &str| format!("insp-rule-{index}-{part}");
        let label = self.line(
            Spec::new(key("label"), rule.label.clone(), "Edit rule label", on_rule(index, |rule, text| rule.label = text.to_string())).placeholder("Bug ≥High"),
            at("label"),
            window,
            cx,
        );
        let icon_button = |this: &Self, part: &str, glyph: IconName, tip: &'static str, enabled: bool, cx: &mut Context<Self>, f: Rc<dyn Fn(&mut RouterNode)>| {
            IconButton::new(SharedString::from(key(part)), glyph)
                .small()
                .tooltip(tip)
                .disabled(!enabled)
                .on_click(this.on_click(cx, move |this, _, cx| {
                    let f = f.clone();
                    this.edit_node(tip_what(tip), router_edit(move |r| f(r)), cx)
                }))
                .probe(key(part))
        };
        let up = icon_button(self, "up", IconName::ArrowUp, "Move up", index > 0, cx, Rc::new(move |r| ops::move_rule(r, index, true)));
        let down = icon_button(self, "down", IconName::ChevronDown, "Move down", index + 1 < count, cx, Rc::new(move |r| ops::move_rule(r, index, false)));
        let remove = IconButton::new(SharedString::from(key("remove")), IconName::Trash)
            .small()
            .tooltip("Remove rule")
            .on_click(self.on_click(cx, move |this, _, cx| {
                let crate::Selection::Node(id) = this.shown.clone() else { return };
                this.edit("Remove rule", |g| ops::remove_rule(g, &id, index), cx)
            }))
            .probe(key("remove"));
        let asks = rule.ask.is_some();
        let mode = Segmented::new(SharedString::from(key("mode")))
            .small()
            .option("When")
            .option("Ask a model")
            .selected(usize::from(asks))
            .on_select(self.on_value(cx, move |this, ix: usize, cx| {
                this.edit_node("Change rule kind", router_edit(move |r| {
                    if let Some(rule) = r.rules.get_mut(index) {
                        if ix == 1 && rule.ask.is_none() {
                            rule.ask = Some(AskRule { max_steps: 6, ..AskRule::default() });
                            rule.when.clear();
                        } else if ix == 0 {
                            rule.ask = None;
                        }
                    }
                }), cx)
            }));
        let targets: Vec<String> = graph
            .edges_from(&node.id)
            .filter(|e| e.port == rule.id)
            .filter_map(|e| graph.node(&e.to))
            .map(|n| n.name.clone())
            .collect();
        let target = if targets.is_empty() { "not connected".to_string() } else { format!("→ {}", targets.join(", ")) };
        let head = row()
            .gap(px(SPACE_1))
            .child(div().flex_none().w(px(18.)).type_style(MONO_SM).text_color(TEXT_MUTED).child(format!("{}", index + 1)))
            .child(div().flex_1().min_w_0().child(label))
            .child(up)
            .child(down)
            .child(remove);
        let mut card = col()
            .gap(px(SPACE_2))
            .p(px(SPACE_3))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(LINE)
            .bg(SURFACE)
            .child(head)
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(div().child(mode).probe(key("mode")))
                    .child(grow())
                    .child(div().type_style(MONO_SM).text_color(TEXT_MUTED).child(rule.id.clone()))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(target)),
            );
        match &rule.ask {
            None => {
                let when = self.line(
                    Spec::new(key("when"), rule.when.clone(), "Edit condition", on_rule(index, |rule, text| rule.when = text.to_string()))
                        .mono()
                        .placeholder("type == \"bug\" && priority >= \"high\""),
                    at("when"),
                    window,
                    cx,
                );
                card = card.child(when);
            }
            Some(ask) => {
                let question = self.area(
                    Spec::new(key("ask"), ask.question.clone(), "Edit question", on_rule(index, |rule, text| {
                        if let Some(ask) = rule.ask.as_mut() {
                            ask.question = text.to_string();
                        }
                    }))
                    .area(2, 6)
                    .placeholder("Does the change touch the UI?"),
                    at("ask"),
                    window,
                    cx,
                );
                let set_model: super::fields::SetModel = Rc::new(move |node, model| {
                    if let NodeKind::Router(r) = &mut node.kind
                        && let Some(ask) = r.rules.get_mut(index).and_then(|rule| rule.ask.as_mut())
                    {
                        ask.model = model.unwrap_or_default();
                    }
                });
                let model = self.model_field(&key("model"), ask.model.clone(), None, set_model, at("ask-model"), window, cx);
                let workdir = self.workdir_picker(
                    &key("workdir"),
                    graph,
                    &ask.workdir,
                    Rc::new(move |node, id| {
                        if let NodeKind::Router(r) = &mut node.kind
                            && let Some(ask) = r.rules.get_mut(index).and_then(|rule| rule.ask.as_mut())
                        {
                            ask.workdir = id;
                        }
                    }),
                    window,
                    cx,
                );
                let steps = self.line(
                    Spec::new(key("steps"), if ask.max_steps == 0 { String::new() } else { ask.max_steps.to_string() }, "Set ask steps", on_rule(index, |rule, text| {
                        if let Some(ask) = rule.ask.as_mut() {
                            ask.max_steps = count_value(text).unwrap_or(0);
                        }
                    }))
                    .parse(parse_count)
                    .mono()
                    .placeholder("Default"),
                    None,
                    window,
                    cx,
                );
                card = card
                    .child(question)
                    .child(field("Model", model))
                    .child(field("Reads", workdir))
                    .children(problem_line(at("ask-workdir"), RULE_INNER))
                    .child(row().gap(px(SPACE_3)).child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Max steps")).child(div().w(px(96.)).child(steps)))
                    .child(super::form::note("A yes/no question the model answers with read-only tools.", RULE_INNER));
            }
        }
        card.probe(key("card")).into_any_element()
    }
}

/// The undo name of a rule button.
fn tip_what(tip: &'static str) -> &'static str {
    match tip {
        "Move up" => "Move rule up",
        _ => "Move rule down",
    }
}
