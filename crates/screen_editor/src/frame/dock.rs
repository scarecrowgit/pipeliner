//! The dock under the canvas: three tabs ("Test run", "Log", "Problems") at a fixed height,
//! and a button that folds it down to its tab bar.
//!
//! Probes: `dock-tab-test-run`, `dock-tab-log`, `dock-tab-problems`, `dock-collapse`.

use std::f32::consts::PI;

use gpui::{
    AnyElement, Context, ElementId, IntoElement, SharedString, Transformation, Window, div, prelude::*, px, radians,
};
use hub::model::Severity;
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui::tooltip::Tooltip;

use super::EditorView;

/// Height of the open dock, tab bar included.
const DOCK_HEIGHT: f32 = 344.0;

/// A tab of the dock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockTab {
    #[default]
    TestRun,
    Log,
    Problems,
}

impl DockTab {
    /// Every tab, left to right.
    pub const ALL: [DockTab; 3] = [DockTab::TestRun, DockTab::Log, DockTab::Problems];

    /// The tab's label.
    pub fn label(self) -> &'static str {
        match self {
            DockTab::TestRun => "Test run",
            DockTab::Log => "Log",
            DockTab::Problems => "Problems",
        }
    }

    /// The tab's name in probes: `dock-tab-test-run`.
    pub fn key(self) -> &'static str {
        match self {
            DockTab::TestRun => "test-run",
            DockTab::Log => "log",
            DockTab::Problems => "problems",
        }
    }
}

/// Which tab is open and whether the dock is folded.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Dock {
    pub(super) tab: DockTab,
    pub(super) collapsed: bool,
}

impl EditorView {
    /// The tab on show.
    pub fn dock_tab(&self) -> DockTab {
        self.dock.tab
    }

    /// Shows `tab`, unfolding the dock.
    pub fn open_tab(&mut self, tab: DockTab, cx: &mut Context<Self>) {
        self.dock = Dock { tab, collapsed: false };
        cx.notify();
    }

    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        self.dock.collapsed = !self.dock.collapsed;
        cx.notify();
    }

    pub(super) fn render_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tabs: Vec<AnyElement> = DockTab::ALL.iter().map(|&tab| self.dock_tab_button(tab, window, cx)).collect();
        let collapsed = self.dock.collapsed;
        let chevron = icon(IconName::ChevronDown, 14., TEXT_MUTED)
            .with_transformation(Transformation::rotate(radians(if collapsed { PI } else { 0.0 })));
        let fold = motion::hover_fade_state(
            "dock-collapse",
            Look::bg(TRANSPARENT),
            Look::bg(SURFACE_RAISED),
            window,
            cx,
        )
        .apply(div().id("dock-collapse").debug_selector(|| "dock-collapse".into()))
        .ml_auto()
        .size(px(CONTROL_HEIGHT_SM))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .tooltip(Tooltip::text(if collapsed { "Expand panel" } else { "Collapse panel" }))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_dock(cx)))
        .child(chevron);
        let bar = row()
            .relative()
            .flex_none()
            .h(px(TAB_HEIGHT))
            .pl(px(SPACE_5))
            .pr(px(SPACE_3))
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(LINE))
            .children(tabs)
            .child(fold.probe("dock-collapse"));
        let body = (!collapsed).then(|| match self.dock.tab {
            DockTab::TestRun => self.render_test_run(window, cx),
            DockTab::Log => self.render_log(window, cx),
            DockTab::Problems => self.render_problems(window, cx),
        });
        col()
            .flex_none()
            .h(px(if collapsed { TAB_HEIGHT + 1. } else { DOCK_HEIGHT }))
            .border_t_1()
            .border_color(LINE)
            .bg(BG)
            .child(bar)
            .children(body.map(|body| div().flex().flex_col().flex_1().min_h_0().child(body)))
            .into_any_element()
    }

    /// One tab: its label, a count after "Log" and a badge after "Problems".
    fn dock_tab_button(&self, tab: DockTab, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected = tab == self.dock.tab && !self.dock.collapsed;
        let name: SharedString = format!("dock-tab-{}", tab.key()).into();
        let fg = if selected { TEXT } else { TEXT_MUTED };
        let faded = motion::hover_fade_state(
            ElementId::Name(name.clone()),
            Look::bg(TRANSPARENT).fg(fg),
            Look::bg(TRANSPARENT).fg(TEXT),
            window,
            cx,
        );
        let selector = name.clone();
        let el = div().id(ElementId::Name(name.clone())).debug_selector(move || selector.to_string());
        let trailing = match tab {
            DockTab::TestRun => None,
            DockTab::Log => Some(
                div()
                    .text_color(TEXT_MUTED)
                    .font_weight(gpui::FontWeight::NORMAL)
                    .tabular()
                    .child(truncate(base::fmt::count(self.log_count(cx) as u64)))
                    .into_any_element(),
            ),
            DockTab::Problems => {
                let draft = self.parts.draft.read(cx);
                let problems = draft.problems();
                let errors = problems.iter().filter(|p| p.severity == Severity::Error).count();
                (!problems.is_empty()).then(|| {
                    let badge = CountBadge::new(base::fmt::count(problems.len() as u64));
                    let badge = if errors > 0 { badge.tone(CountTone::Danger) } else { badge.quiet() };
                    badge.into_any_element()
                })
            }
        };
        faded
            .apply(el)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_1_5))
            .h(px(TAB_HEIGHT))
            .mr(px(SPACE_5))
            .border_b_2()
            .border_color(if selected { ACCENT } else { TRANSPARENT })
            .type_style(if selected { BODY_STRONG } else { BODY })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open_tab(tab, cx)))
            .child(truncate(tab.label()))
            .children(trailing)
            .probe(name)
            .into_any_element()
    }
}
