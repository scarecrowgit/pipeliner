//! The dock under the canvas: three tabs ("Test run", "Log", "Problems") at a fixed height,
//! and a button that folds it down to its tab bar.
//!
//! Probes: `dock-tab-test-run`, `dock-tab-log`, `dock-tab-problems`, `dock-collapse`.

use std::f32::consts::PI;

use gpui::{AnyElement, Context, IntoElement, Transformation, Window, div, prelude::*, px, radians};
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
        let tabs = self.dock_tabs(cx);
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
            .gap(px(SPACE_2))
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(LINE))
            .child(div().flex_1().min_w_0().child(tabs))
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

    /// The tabs: a count after "Log" and a badge after "Problems".
    fn dock_tabs(&self, cx: &mut Context<Self>) -> Tabs {
        let selected = DockTab::ALL.iter().position(|t| *t == self.dock.tab).unwrap_or(0);
        let (problems, errors) = {
            let draft = self.parts.draft.read(cx);
            let problems = draft.problems();
            (problems.len(), problems.iter().filter(|p| p.severity == Severity::Error).count())
        };
        let mut tabs = Tabs::new("dock-tabs").flush().selected(selected);
        for tab in DockTab::ALL {
            tabs = match tab {
                DockTab::Log => tabs.tab_n(tab.label(), base::fmt::count(self.log_count(cx) as u64)),
                _ => tabs.tab(tab.label()),
            };
            if tab == DockTab::Problems && problems > 0 {
                let badge = CountBadge::new(base::fmt::count(problems as u64));
                tabs = tabs.tab_trailing(if errors > 0 { badge.tone(CountTone::Danger) } else { badge.quiet() });
            }
            tabs = tabs.tab_probe(format!("dock-tab-{}", tab.key()));
        }
        let view = cx.entity().downgrade();
        tabs.on_select(move |ix, _, cx| {
            let tab = DockTab::ALL[ix.min(DockTab::ALL.len() - 1)];
            view.update(cx, |this, cx| this.open_tab(tab, cx)).ok();
        })
    }
}
