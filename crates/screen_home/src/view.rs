//! The Home view: its state, the header parts, the keyboard and the page that holds the blocks.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton,
    Render, SharedString, Subscription, Window, canvas, div, prelude::*, px,
};
use hub::api::Period;
use hub::{CatalogModel, OpsModel, Route};
use ui::prelude::*;

use crate::blocks::{Block, block, probed};
use crate::period::{self, PERIODS};

/// Width of the right column (`grid-template-columns: minmax(0,1fr) 340px`).
pub(crate) const SIDE_WIDTH: f32 = 340.;
/// Narrowest the left column may get beside the right one: the "Working now" table's fixed
/// columns and room for a name and a task. Below it the right column goes under the left.
pub(crate) const MIN_LEFT_WIDTH: f32 = 620.;
/// The page's width until its first frame is measured: a wide window.
const FIRST_WIDTH: f32 = 1200.;

/// The whole content of `Route::Home`.
pub struct HomeView {
    focus_handle: FocusHandle,
    /// The period every block of the page shows.
    pub(crate) period: Period,
    pub(crate) ops: Entity<OpsModel>,
    pub(crate) catalog: Entity<CatalogModel>,
    /// The "Needs you" row the keyboard is on.
    pub(crate) selected: Option<usize>,
    /// A pinned clock for tests; the wall clock otherwise.
    pub(crate) now: Option<i64>,
    /// The page's width in the last frame, px.
    page_width: Rc<Cell<f32>>,
    /// Width of the right column's cards this frame: [`SIDE_WIDTH`] beside the left column,
    /// the page's width when it is under it.
    pub(crate) side_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    /// Home for the current project. Loading starts at once.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ops = hub::ops(cx);
        let catalog = hub::catalog(cx);
        let period = period::remembered(cx);
        ops.update(cx, |ops, cx| ops.ensure_period(period, cx));
        let subscriptions = vec![cx.observe(&ops, |_, _, cx| cx.notify()), cx.observe(&catalog, |_, _, cx| cx.notify())];
        let view = Self {
            focus_handle: cx.focus_handle(),
            period,
            ops,
            catalog,
            selected: None,
            now: None,
            page_width: Rc::new(Cell::new(FIRST_WIDTH)),
            side_width: SIDE_WIDTH,
            _subscriptions: subscriptions,
        };
        window.focus(&view.focus_handle);
        view
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &App) -> SharedString {
        "Home".into()
    }

    /// What follows the title in the shell's header: today's date.
    pub fn header_badges(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        let now = self.now();
        let date = base::time::long_date(now, base::time::utc_offset(now));
        vec![div().ml(px(SPACE_2)).type_style(BODY).text_color(TEXT_MUTED).child(date).into_any_element()]
    }

    /// The controls on the right of the shell's header, left to right: the period switch and New
    /// chat.
    pub fn header_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.period_switch(window, cx),
            probed(
                div().child(
                    Button::new("home-new-chat", "New chat")
                        .primary()
                        .icon(IconName::Plus)
                        .kbd("⌘N")
                        .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx)),
                ),
                "home-new-chat",
            )
            .into_any_element(),
        ]
    }

    /// Shows `period` in every block, asking hub for it.
    pub fn set_period(&mut self, period: Period, cx: &mut Context<Self>) {
        self.period = period;
        period::remember(period, cx);
        self.ops.update(cx, |ops, cx| ops.ensure_period(period, cx));
        cx.notify();
    }

    /// The period the page shows.
    pub fn period(&self) -> Period {
        self.period
    }

    /// The right column goes under the left one: the page is too narrow for both side by side.
    pub fn stacked(&self) -> bool {
        self.inner_width() < MIN_LEFT_WIDTH + SPACE_4 + SIDE_WIDTH
    }

    /// The page's width inside its padding, as measured in the last frame.
    fn inner_width(&self) -> f32 {
        self.page_width.get() - 2. * SPACE_6
    }

    /// The clock the ages are read against.
    pub(crate) fn now(&self) -> i64 {
        self.now.unwrap_or_else(base::time::now_ms)
    }

    /// Where a block's data stands: the model's slot (`None`: not asked for yet).
    pub(crate) fn state<T>(&self, slot: Option<&hub::Loadable<T>>) -> Block<T> {
        block(slot)
    }

    /// Asks hub again for what the page shows: the retry of a block that failed.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        let period = self.period;
        hub::ops(cx).update(cx, |ops, cx| ops.reload(period, cx));
        cx.notify();
    }

    /// `.seg`: a raised track with a thumb under the chosen period.
    fn period_switch(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected = PERIODS.iter().position(|option| *option == self.period).unwrap_or(0);
        let seg = PERIODS.into_iter().fold(Segmented::new("home-period"), |seg, option| {
            seg.option(period::label(option)).option_probe(format!("home-period-{}", period::name(option)))
        });
        let view = cx.entity();
        seg.selected(selected)
            .on_select(move |ix, _, cx| {
                if let Some(option) = PERIODS.get(ix).copied() {
                    view.update(cx, |this, cx| this.set_period(option, cx));
                }
            })
            .into_any_element()
    }

    // ------------------------------------------------------------ keyboard

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return;
        }
        match keystroke.key.as_str() {
            "j" | "down" => self.move_selection(1, cx),
            "k" | "up" => self.move_selection(-1, cx),
            "enter" => self.open_selected(cx),
            "escape" if self.selected.is_some() => {
                self.selected = None;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// J and K: the next or previous "Needs you" row, the first one when none was chosen.
    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.ops.read(cx).needs().value().map_or(0, |needs| needs.len());
        if count == 0 {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(ix) => (ix as isize + step).clamp(0, count as isize - 1) as usize,
        };
        self.selected = Some(next);
        cx.notify();
    }

    /// Enter: goes where the chosen "Needs you" row leads.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let target = self
            .selected
            .and_then(|ix| self.ops.read(cx).needs().value().and_then(|needs| needs.get(ix).map(|need| need.target.clone())));
        if let Some(target) = target {
            hub::open(target, cx);
        }
    }
}

impl Focusable for HomeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stacked = self.stacked();
        self.side_width = if stacked { self.inner_width().max(0.) } else { SIDE_WIDTH };
        let strip = self.render_stats(cx);
        let needs = self.render_needs(window, cx);
        let working = self.render_working(window, cx);
        let spend = self.render_spend(cx);
        let agents = self.render_agents(cx);
        let runtimes = self.render_runtimes(cx);
        let focus = self.focus_handle.clone();
        let left = col().flex_1().min_w_0().gap(px(SPACE_4)).child(needs).child(working);
        let right = col()
            .flex_none()
            .w(px(self.side_width))
            .gap(px(SPACE_4))
            .child(spend)
            .child(agents)
            .child(runtimes);
        let columns = if stacked {
            col().flex_none().w_full().gap(px(SPACE_4)).child(left.flex_none().w_full()).child(right)
        } else {
            row().flex_none().w_full().items_start().gap(px(SPACE_4)).child(left).child(right)
        };
        let width = self.page_width.clone();
        div()
            .id("home")
            .debug_selector(|| "home".into())
            .key_context("Home")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, move |_, window, _| window.focus(&focus))
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                col()
                    .id("home-page")
                    .debug_selector(|| "home-page".into())
                    .relative()
                    .flex_none()
                    .w_full()
                    .p(px(SPACE_6))
                    .gap(px(SPACE_4))
                    .child(measure_width(move |w| width.replace(w) != w))
                    .child(strip)
                    .child(columns),
            )
    }
}

/// An invisible layer over its (relative) parent that hands the parent's width to `store`;
/// when `store` says it changed, the window is drawn again with it.
fn measure_width(store: impl Fn(f32) -> bool + 'static) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            if store(f32::from(bounds.size.width)) {
                // A refresh asked for while drawing is dropped: ask once this frame is done.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
