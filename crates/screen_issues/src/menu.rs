//! The pipeline select: in the "New issue" dialog and in the issue panel ("Change
//! pipeline"). The menu pops below its box; a transparent backdrop closes it on any outside
//! press.

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, MouseButton, SharedString, deferred, div,
    prelude::*, px,
};
use ui::theme::*;
use ui::{icon, motion};

use crate::IssuesScreen;
use crate::widgets::Ellipsis;

/// Which select is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    /// The dialog's "Pipeline" field.
    NewIssue,
    /// "Change pipeline" of the selected issue.
    Issue,
}

impl Menu {
    fn id(self) -> &'static str {
        match self {
            Menu::NewIssue => "ni-pipeline",
            Menu::Issue => "issue-pipeline",
        }
    }
}

/// One pipeline as the menu lists it.
struct Item {
    id: String,
    name: String,
    about: String,
}

impl IssuesScreen {
    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) { None } else { Some(menu) };
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn pick(&mut self, menu: Menu, id: String, cx: &mut Context<Self>) {
        self.menu = None;
        match menu {
            Menu::NewIssue => {
                self.new.pipeline = Some(id);
                cx.notify();
            }
            Menu::Issue => self.set_issue_pipeline(id, cx),
        }
    }

    /// Select box showing the pipeline `current` (an id; `None` = the default pipeline).
    pub(crate) fn pipeline_select(
        &self,
        menu: Menu,
        current: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.pipes.read(cx);
        let current = current
            .and_then(|id| p.pipeline(id))
            .or(p.default_pipeline())
            .map(|d| d.id.clone());
        let items: Vec<Item> = p
            .pipelines
            .iter()
            .map(|d| {
                let n = d.steps.len();
                let steps = format!("{n} step{}", if n == 1 { "" } else { "s" });
                Item {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    about: if d.description.trim().is_empty() {
                        steps
                    } else {
                        format!("{steps} · {}", d.description.trim())
                    },
                }
            })
            .collect();
        let label = current
            .as_ref()
            .and_then(|id| items.iter().find(|i| &i.id == id))
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "No pipeline".into());
        let open = self.menu == Some(menu);

        div()
            .relative()
            .child(
                div()
                    .id(menu.id())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(CONTROL_HEIGHT))
                    .px(px(12.))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(if open { ACCENT_STRONG } else { LINE_STRONG })
                    .bg(SURFACE_RAISED)
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .child(icon("pipelines", 14., TEXT_MUTED))
                    .child(
                        div()
                            .flex_1()
                            .ellipsis()
                            .text_color(TEXT)
                            .child(SharedString::from(label)),
                    )
                    .child(icon("chevron-down", 14., TEXT_MUTED))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(menu, cx))),
            )
            .when(open, |d| d.child(self.menu_popover(menu, items, current, cx)))
    }

    fn menu_popover(
        &self,
        menu: Menu,
        items: Vec<Item>,
        current: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = items.into_iter().enumerate().map(|(i, item)| {
            let on = current.as_deref() == Some(item.id.as_str());
            let id = SharedString::from(format!("{}-item-{i}", menu.id()));
            let pick = item.id.clone();
            let row = div()
                .id(id.clone())
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .ellipsis()
                                .child(SharedString::from(item.name)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .ellipsis()
                                .child(SharedString::from(item.about)),
                        ),
                )
                .when(on, |d| d.child(icon("check", 12., ACCENT_TEXT)))
                .on_click(cx.listener(move |this, _, _, cx| this.pick(menu, pick.clone(), cx)));
            motion::fade_row(row, id, false, SURFACE_HOVER)
        });
        deferred(motion::pop_in(
            (menu.id(), 1usize),
            motion::Pop::Below,
            div()
                .id((menu.id(), 2usize))
                .absolute()
                .top(px(CONTROL_HEIGHT + 4.))
                .left_0()
                .right_0()
                .max_h(px(320.))
                .overflow_y_scroll()
                .p(px(4.))
                .flex()
                .flex_col()
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(LINE_STRONG)
                .bg(SURFACE_RAISED)
                .shadow(popover_shadow())
                .occlude()
                .children(rows),
        ))
        .with_priority(3)
    }

    /// Closes the open select on a press anywhere else (above the dialog, below the menu).
    pub(crate) fn render_menu_backdrop(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.menu?;
        Some(
            deferred(
                div()
                    .id("pipeline-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close_menu(cx)),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
