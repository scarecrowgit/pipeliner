use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, relative, Animation, AnimationExt as _, App, ElementId, InteractiveElement as _,
    IntoElement, ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window,
};
use gpui_component::StyledExt as _;

use crate::theme::{self, TextStyles as _};

type SelectHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// Underline tabs (`.gr-tabs`): "Graph  Runs 12  Versions". Bottom border included.
///
/// ```ignore
/// Tabs::new("pipeline-tabs")
///     .tab("Graph").tab_count("Runs", runs.len()).tab("Versions")
///     .selected(self.tab)
///     .on_select(cx.listener(|this, ix: &usize, _, cx| { this.tab = *ix; cx.notify(); }))
///     .px(theme::S6)
/// ```
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<(SharedString, Option<usize>)>,
    selected: usize,
    on_select: Option<SelectHandler>,
    style: StyleRefinement,
}

impl Tabs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Tabs { id: id.into(), tabs: vec![], selected: 0, on_select: None, style: StyleRefinement::default() }
    }
    pub fn tab(mut self, label: impl Into<SharedString>) -> Self {
        self.tabs.push((label.into(), None));
        self
    }
    /// A tab with a muted count after the label.
    pub fn tab_count(mut self, label: impl Into<SharedString>, count: usize) -> Self {
        self.tabs.push((label.into(), Some(count)));
        self
    }
    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }
    /// Works with `cx.listener(|this, ix: &usize, window, cx| ...)`.
    pub fn on_select(mut self, f: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Tabs {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let selected = self.selected;
        let id = self.id.clone();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_end()
            .gap(theme::S5)
            .h(theme::TAB_H)
            .border_b_1()
            .border_color(theme::LINE)
            .refine_style(&self.style)
            .children(self.tabs.into_iter().enumerate().map(|(ix, (label, count))| {
                let active = ix == selected;
                let on_select = self.on_select.clone();
                div()
                    .id(ix)
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h_full()
                    .text_body()
                    .cursor_pointer()
                    .when(active, |d| d.fg().font_weight(gpui::FontWeight::MEDIUM))
                    .when(!active, |d| d.fg_muted().hover(|s| s.text_color(theme::TEXT)))
                    .child(label)
                    .when_some(count, |d, n| {
                        d.child(div().fg_muted().font_weight(gpui::FontWeight::NORMAL).child(n.to_string()))
                    })
                    .when(active, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-1.))
                                .left_0()
                                .h(px(2.))
                                .w_full()
                                .bg(theme::ACCENT)
                                .with_animation(
                                    ElementId::Name(format!("{id}-underline-{ix}").into()),
                                    Animation::new(theme::DUR_STATE).with_easing(theme::ease_out),
                                    |d, t| d.w(relative(t)).left(relative((1.0 - t) / 2.0)),
                                ),
                        )
                    })
                    .when_some(on_select, |d, f| d.on_click(move |_, window, cx| f(&ix, window, cx)))
            }))
    }
}
