//! The Home view: its state, the header parts, the keyboard and the page that holds the blocks.

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, KeyDownEvent, MouseButton,
    Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use hub::api::Period;
use hub::{CatalogModel, OpsModel, Route};
use ui::motion::{self, Look};
use ui::prelude::*;

use crate::blocks::{Block, block, probed};
use crate::period::{self, PERIODS};

/// Width of the right column (`grid-template-columns: minmax(0,1fr) 340px`).
pub(crate) const SIDE_WIDTH: f32 = 340.;

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
    fn period_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let options: Vec<AnyElement> = PERIODS
            .into_iter()
            .enumerate()
            .map(|(ix, option)| {
                let chosen = option == self.period;
                let id = gpui::ElementId::from(("home-period", ix));
                let bg = if chosen { SURFACE_HOVER } else { TRANSPARENT };
                let rest = Look::bg(bg).fg(if chosen { TEXT } else { TEXT_MUTED });
                let hover = Look::bg(bg).fg(TEXT);
                let style = if chosen { TypeStyle { weight: FontWeight::MEDIUM, ..BODY } } else { BODY };
                let el = motion::hover_fade(div().id(id.clone()), id, rest, hover, window, cx)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .px(px(SPACE_3))
                    .rounded(px(RADIUS_MD))
                    .type_style(style)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(period::label(option))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_period(option, cx)));
                probed(el, format!("home-period-{}", period::name(option))).into_any_element()
            })
            .collect();
        row()
            .flex_none()
            .h(px(CONTROL_HEIGHT))
            .p(px(3.))
            .gap(px(SPACE_0_5))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_LG))
            .children(options)
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
        let strip = self.render_stats(cx);
        let needs = self.render_needs(window, cx);
        let working = self.render_working(window, cx);
        let spend = self.render_spend(cx);
        let agents = self.render_agents(cx);
        let runtimes = self.render_runtimes(cx);
        let focus = self.focus_handle.clone();
        let left = col().flex_1().min_w_0().gap(px(SPACE_4)).child(needs).child(working);
        let right =
            col().flex_none().w(px(SIDE_WIDTH)).gap(px(SPACE_4)).child(spend).child(agents).child(runtimes);
        div()
            .id("home")
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
                    .flex_none()
                    .w_full()
                    .p(px(SPACE_6))
                    .gap(px(SPACE_4))
                    .child(strip)
                    .child(row().flex_none().w_full().items_start().gap(px(SPACE_4)).child(left).child(right)),
            )
    }
}
