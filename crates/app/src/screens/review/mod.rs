//! Review: the queue of items waiting for you (gates with variants, PR approvals, plan
//! approvals) and the selected item's detail with Ship / Send back.
//! Design: design/screens/review.html; behaviour: design/explanation/run-step-by-step.html.
//!
//! * `data.rs`   — what the screen shows, derived from the Store on a background thread.
//! * `queue.rs`  — the left column (waiting list, also needs you, decided today).
//! * `detail.rs` — the main panel (breadcrumb, steps, variants, images, diff, send back).

mod data;
mod detail;
mod queue;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    canvas, img, Animation, AnimationExt as _, DispatchPhase, Focusable as _, KeyDownEvent,
    ObjectFit, StyledImage as _,
};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;
use harness_core::DecisionKind;

use self::data::{Item, ItemKind, ReviewData};
use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self, Options, SelectOption};

/// How long a decided row takes to slide out of the list.
const LEAVE: Duration = Duration::from_millis(260);

/// The full-size image overlay.
struct Zoom {
    path: String,
    /// 0 fit · 1 actual size · 2 fill width · 3 twice the width.
    level: usize,
}

pub struct ReviewScreen {
    data: Option<ReviewData>,
    selected: Option<Id>,
    /// Gate task id -> the variant index you picked (what "Ship" sends).
    picked: HashMap<Id, u32>,
    /// Gate task id -> the variant whose summary/diff is open.
    viewing: HashMap<Id, u32>,
    /// 0 = Summary, 1 = Diff.
    detail_tab: usize,
    image_tab: usize,
    /// (variant id, file path) of diff files marked "Viewed".
    viewed: HashSet<(Id, String)>,
    /// (variant id, file path) of folded diff files you unfolded.
    opened: HashSet<(Id, String)>,
    /// Decided rows still sliding out (or waiting for the engine to pick the decision up).
    leaving: HashSet<Id>,
    zoom: Option<Zoom>,
    /// Item the inputs below were last prepared for.
    synced: Option<Id>,
    /// Item drawn in the last frame. Decisions, picks and keys act on this one, so what
    /// you ship is always what you were looking at (the list can change under you).
    shown: Option<Id>,
    note: Entity<InputState>,
    target: Entity<SelectState<Options>>,
    load: Option<Task<()>>,
    /// A read is running / another one is due when it ends.
    loading: bool,
    reload_again: bool,
    /// Re-renders every 30s so ages ("6m") keep ticking.
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
}

impl ReviewScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let note = input::composer("", window, cx);
        let target = select::select_state(vec![], None, false, window, cx);
        let subs = vec![
            cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
                // Streaming messages and log lines don't change anything shown here.
                AppEvent::StoreChanged { changes, .. } => {
                    if changes.iter().any(|c| {
                        !matches!(
                            c,
                            Change::Message { .. }
                                | Change::Event
                                | Change::Artifact
                                | Change::Config
                        )
                    }) {
                        this.reload(cx);
                    }
                }
                AppEvent::Navigated(Route::Review { task: Some(task) }) => {
                    this.selected = Some(task.clone());
                    cx.notify();
                }
                AppEvent::Navigated(Route::Review { task: None }) => {}
                // Leaving the screen closes the image overlay.
                AppEvent::Navigated(_) => this.close_zoom(cx),
                AppEvent::CloseOverlay if this.zoom.is_some() => this.close_zoom(cx),
                _ => {}
            }),
            cx.subscribe_in(&note, window, |this, _, ev: &InputEvent, window, cx| {
                // Ctrl/Cmd+Enter in the note sends it.
                if let InputEvent::PressEnter { secondary: true } = ev {
                    this.submit_note(window, cx);
                }
            }),
        ];
        let selected = match state::route(cx) {
            Route::Review { task } => task,
            _ => None,
        };
        let ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });
        let mut this = ReviewScreen {
            data: None,
            selected,
            picked: HashMap::new(),
            viewing: HashMap::new(),
            detail_tab: 0,
            image_tab: 0,
            viewed: HashSet::new(),
            opened: HashSet::new(),
            leaving: HashSet::new(),
            zoom: None,
            synced: None,
            shown: None,
            note,
            target,
            load: None,
            loading: false,
            reload_again: false,
            _ticker: ticker,
            _subs: subs,
        };
        this.reload(cx);
        this
    }

    /// Re-reads the queue. While a read is running, changes only mark it stale and one
    /// more read follows: a busy run changes tasks every few hundred ms, and restarting
    /// the read each time kept the screen from ever loading.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.reload_again = true;
            return;
        }
        self.loading = true;
        self.load = Some(load(cx, ReviewData::query, |this, data, cx| {
            // Rows the engine has consumed are gone: stop animating them.
            this.leaving
                .retain(|id| data.items.iter().any(|i| &i.task.id == id));
            this.data = Some(data);
            this.loading = false;
            if std::mem::take(&mut this.reload_again) {
                this.reload(cx);
            }
        }));
    }

    // ---- selection ---------------------------------------------------------------

    fn items(&self) -> &[Item] {
        self.data
            .as_ref()
            .map(|d| d.items.as_slice())
            .unwrap_or(&[])
    }

    /// Items still in the list (decided ones are animating out).
    fn live(&self) -> Vec<&Item> {
        self.items()
            .iter()
            .filter(|i| !self.leaving.contains(&i.task.id))
            .collect()
    }

    /// The selected item; falls back to the first one when the selection is gone.
    fn current(&self) -> Option<&Item> {
        let live = self.live();
        self.selected
            .as_ref()
            .and_then(|id| live.iter().copied().find(|i| &i.task.id == id))
            .or_else(|| live.first().copied())
    }

    /// The item on screen right now (see `shown`); None when it has left the list.
    fn acted(&self) -> Option<&Item> {
        let id = self.shown.as_ref()?;
        self.live().into_iter().find(|i| &i.task.id == id)
    }

    fn select(&mut self, id: Id, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        state::replace_route(Route::Review { task: Some(id) }, cx);
        cx.notify();
    }

    /// Moves the selection by `delta` rows (wraps never; stops at the ends).
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let live = self.live();
        let Some(cur) = self.current().map(|i| i.task.id.clone()) else {
            return;
        };
        let pos = live.iter().position(|i| i.task.id == cur).unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, live.len() as isize - 1) as usize;
        if let Some(id) = live.get(next).map(|i| i.task.id.clone()) {
            self.select(id, cx);
        }
    }

    /// The id the selection should move to when `id` leaves the list: the next row,
    /// else the previous one.
    fn neighbour_of(&self, id: &str) -> Option<Id> {
        let live = self.live();
        let pos = live.iter().position(|i| i.task.id == id)?;
        live.get(pos + 1)
            .or_else(|| pos.checked_sub(1).and_then(|p| live.get(p)))
            .map(|i| i.task.id.clone())
    }

    // ---- variants ----------------------------------------------------------------

    /// The variant Ship would send: your pick, or the only variant.
    fn pick_of(&self, item: &Item) -> Option<u32> {
        self.picked
            .get(&item.task.id)
            .copied()
            .or_else(|| (item.variants.len() == 1).then(|| item.variants[0].v.index))
    }

    /// The variant whose summary and diff are open.
    fn viewing_of(&self, item: &Item) -> Option<usize> {
        let idx = self
            .viewing
            .get(&item.task.id)
            .copied()
            .or_else(|| self.pick_of(item));
        idx.and_then(|i| item.variants.iter().position(|v| v.v.index == i))
            .or(if item.variants.is_empty() {
                None
            } else {
                Some(0)
            })
    }

    fn pick(&mut self, item_id: Id, index: u32, cx: &mut Context<Self>) {
        self.picked.insert(item_id.clone(), index);
        self.viewing.insert(item_id, index);
        cx.notify();
    }

    // ---- decisions ---------------------------------------------------------------

    /// Sends a decision for the current item, animates it out and moves on.
    fn decide(
        &mut self,
        kind: DecisionKind,
        toast: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.acted().map(|i| i.task.id.clone()) else {
            return;
        };
        let next = self.neighbour_of(&id);
        self.leaving.insert(id.clone());
        if let Some(next) = next {
            self.select(next, cx);
        }
        self.note.update(cx, |s, cx| s.set_value("", window, cx));
        let job = {
            let id = id.clone();
            state::run_engine(cx, move |e| e.decide(&id, kind))
        };
        cx.spawn(async move |this, cx| {
            let ok = job.await.is_some();
            let _ = this.update(cx, |this, cx| {
                if ok {
                    state::toast(ToastKind::Success, toast, cx);
                } else {
                    // The engine refused: bring the row back.
                    this.leaving.remove(&id);
                }
                cx.notify();
            });
        })
        .detach();
        // Drop the row from the animation set once it has left, even if the store is slow.
        cx.notify();
    }

    /// The primary action: ship the pick, approve the PR or the plan.
    fn ship(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.acted() else { return };
        let title = item.task.title.clone();
        let (kind, toast) = match item.kind {
            ItemKind::Pick => {
                let Some(index) = self.pick_of(item) else {
                    state::toast(ToastKind::Warning, "Pick a variant first", cx);
                    return;
                };
                let letter = harness_core::variant_letter(index);
                let toast = if item.variants.len() > 1 {
                    format!("Shipped {letter} · {title}")
                } else {
                    format!("Approved · {title}")
                };
                (DecisionKind::Pick { index }, toast)
            }
            ItemKind::ApprovePr => (
                DecisionKind::Approve,
                format!("Approved · merging the pull request · {title}"),
            ),
            ItemKind::Plan => (DecisionKind::Approve, format!("Plan approved · {title}")),
            ItemKind::Other => return,
        };
        self.decide(kind, toast, window, cx);
    }

    fn note_text(&self, cx: &App) -> String {
        self.note.read(cx).value().trim().to_string()
    }

    /// Sends the note: a send-back for gates, feedback (reply) for plans and questions.
    fn submit_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.acted() else { return };
        let text = self.note_text(cx);
        if text.is_empty() {
            state::toast(ToastKind::Warning, "Write a note first", cx);
            return;
        }
        let (kind, toast) = match item.kind {
            ItemKind::Pick | ItemKind::ApprovePr => {
                let target = self.target.read(cx).selected_value().cloned();
                let name = target
                    .as_ref()
                    .and_then(|t| {
                        item.run
                            .as_ref()?
                            .back_options
                            .iter()
                            .find(|(id, _)| id == t)
                    })
                    .map(|(_, n)| n.clone());
                let toast = match name {
                    Some(n) => format!("Sent back to {n}"),
                    None => "Sent back".to_string(),
                };
                (DecisionKind::SendBack { note: text, target }, toast)
            }
            ItemKind::Plan | ItemKind::Other => {
                (DecisionKind::Reply { text }, "Feedback sent".to_string())
            }
        };
        self.decide(kind, toast, window, cx);
    }

    fn reject(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reason = self.note_text(cx);
        let toast = match self.acted().map(|i| i.kind) {
            Some(ItemKind::ApprovePr) => "Rejected · the pull request stays open",
            _ => "Rejected",
        };
        self.decide(
            DecisionKind::Reject { reason },
            toast.to_string(),
            window,
            cx,
        );
    }

    /// Moves to the next item without deciding.
    fn skip(&mut self, cx: &mut Context<Self>) {
        let live = self.live();
        let Some(cur) = self.current().map(|i| i.task.id.clone()) else {
            return;
        };
        let pos = live.iter().position(|i| i.task.id == cur).unwrap_or(0);
        if live.len() < 2 {
            return;
        }
        let next = live[(pos + 1) % live.len()].task.id.clone();
        self.select(next, cx);
    }

    fn focus_note(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.note.update(cx, |s, cx| s.focus(window, cx));
    }

    fn close_zoom(&mut self, cx: &mut Context<Self>) {
        if self.zoom.take().is_some() {
            AppState::global(cx).update(cx, |s, _| s.overlay_open = false);
            cx.notify();
        }
    }

    fn open_zoom(&mut self, path: String, cx: &mut Context<Self>) {
        self.zoom = Some(Zoom { path, level: 0 });
        AppState::global(cx).update(cx, |s, _| s.overlay_open = true);
        cx.notify();
    }

    // ---- keyboard ----------------------------------------------------------------

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.note.focus_handle(cx).is_focused(window)
            || self.target.focus_handle(cx).is_focused(window)
    }

    /// J/K move, 1-9 pick, R focuses the note, Ctrl/Cmd+Enter ships.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.typing(window, cx)
            || AppState::global(cx).read(cx).overlay_open && self.zoom.is_none()
        {
            return;
        }
        let k = &ev.keystroke;
        let m = &k.modifiers;
        let handled = if self.zoom.is_some() {
            false
        } else if (m.control || m.platform) && k.key == "enter" {
            self.ship(window, cx);
            true
        } else if m.control || m.platform || m.alt {
            false
        } else {
            match k.key.as_str() {
                "j" | "down" => {
                    self.move_selection(1, cx);
                    true
                }
                "k" | "up" => {
                    self.move_selection(-1, cx);
                    true
                }
                "r" => {
                    self.focus_note(window, cx);
                    true
                }
                key if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && key != "0" => {
                    let n = (key.as_bytes()[0] - b'1') as usize;
                    if let Some(item) = self.acted().filter(|i| i.kind == ItemKind::Pick) {
                        if let Some(v) = item.variants.get(n) {
                            let (id, index) = (item.task.id.clone(), v.v.index);
                            self.pick(id, index, cx);
                        }
                    }
                    true
                }
                _ => false,
            }
        };
        if handled {
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    /// Prepares the shared inputs when the selected item changes.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cur = self.current().map(|i| i.task.id.clone());
        if cur == self.synced {
            return;
        }
        let (options, default, has_diff) = match self.current() {
            Some(item) => {
                let opts: Vec<SelectOption> = item
                    .run
                    .as_ref()
                    .map(|r| {
                        r.back_options
                            .iter()
                            .map(|(id, n)| SelectOption::new(id.clone(), n.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let default = item
                    .back_to
                    .clone()
                    .filter(|b| opts.iter().any(|o| &o.value == b))
                    .or_else(|| opts.first().map(|o| o.value.clone()));
                let has_diff = self
                    .viewing_of(item)
                    .is_some_and(|ix| !item.variants[ix].diff.is_empty());
                (opts, default, has_diff)
            }
            None => (vec![], None, false),
        };
        select::set_options(&self.target, options, default.as_deref(), window, cx);
        let placeholder = match self.current().map(|i| i.kind) {
            Some(ItemKind::Plan) => "What should change in the plan?",
            Some(ItemKind::Other) => "Your answer",
            _ => "e.g. Keep the approach, but log at debug level, not warning.",
        };
        self.note.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_placeholder(placeholder, window, cx);
        });
        self.detail_tab = usize::from(has_diff);
        self.image_tab = 0;
        self.synced = cur;
    }
}

impl Render for ReviewScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_inputs(window, cx);
        let me = cx.entity();
        // Key handling works wherever focus is (the listener is registered with the screen's
        // paint, so it only exists while the screen is visible).
        let keys = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_key_event(move |ev: &KeyDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        me.update(cx, |this, cx| this.on_key(ev, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();

        let Some(data) = &self.data else {
            return screen()
                .child(PageHeader::new("Review"))
                .child(body().child(skeleton_rows("review-loading", 4)));
        };

        self.shown = self.current().map(|i| i.task.id.clone());
        let live = self.live();
        let count = live.len();
        let mut header = PageHeader::new("Review").when(count > 0, |h| {
            h.meta(Badge::new(format!("{count} waiting")).warning().dot())
                .when_some(data.oldest_secs(), |h, s| {
                    h.meta(
                        div()
                            .text_label()
                            .fg_muted()
                            .child(format!("oldest {}", ui::fmt_duration(s))),
                    )
                })
        });
        if let Some(item) = self.current() {
            header = self.header_actions(header, item, count, cx);
        }

        let main = match self.current() {
            Some(item) => self.render_detail(item, window, cx).into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(EmptyState::new(
                    AppIcon::Inbox,
                    "Nothing waiting for you",
                    "When a run reaches a gate, asks for a plan approval or needs a pull request merged, it lands here.",
                ))
                .into_any_element(),
        };

        screen()
            .relative()
            .child(keys)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_queue(data, cx))
                    .child(main),
            )
            .when_some(self.zoom.as_ref(), |d, z| {
                d.child(self.render_zoom(z, window, cx))
            })
    }
}

impl ReviewScreen {
    /// Hints, Skip, Send back and the primary action.
    fn header_actions(
        &self,
        header: PageHeader,
        item: &Item,
        count: usize,
        cx: &mut Context<Self>,
    ) -> PageHeader {
        let ship_key = if cfg!(target_os = "macos") {
            "⌘↵"
        } else {
            "Ctrl+↵"
        };
        let hint = |keys: Vec<&'static str>, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys.into_iter().map(kbd))
                .child(div().child(what))
        };
        let hints = div()
            .flex()
            .items_center()
            .gap(theme::S3)
            .text_label()
            .fg_muted()
            .child(hint(vec!["J", "K"], "move"))
            .when(
                item.kind == ItemKind::Pick && item.variants.len() > 1,
                |d| d.child(hint(vec!["1–9"], "pick")),
            )
            .child(hint(vec!["R"], "send back"))
            .when(item.kind != ItemKind::Other, |d| {
                d.child(hint(vec![ship_key], "ship"))
            });

        let (primary, primary_disabled) = match item.kind {
            ItemKind::Pick => match self
                .pick_of(item)
                .and_then(|i| item.variants.iter().find(|v| v.v.index == i))
            {
                Some(_) if item.variants.len() == 1 => ("Approve".to_string(), false),
                Some(v) => (
                    format!("Ship {}", harness_core::variant_letter(v.v.index)),
                    false,
                ),
                None => ("Ship".to_string(), true),
            },
            ItemKind::ApprovePr => ("Approve & merge".to_string(), false),
            ItemKind::Plan => ("Approve plan".to_string(), false),
            ItemKind::Other => (String::new(), true),
        };
        let back_label = match item.kind {
            ItemKind::Pick | ItemKind::ApprovePr => "Send back…",
            ItemKind::Plan => "Give feedback…",
            ItemKind::Other => "Reply…",
        };
        header
            .action(hints)
            .action(
                Button::new("review-skip", "Skip")
                    .ghost()
                    .disabled(count < 2)
                    .on_click(cx.listener(|this, _, _, cx| this.skip(cx))),
            )
            .action(
                Button::new("review-back-top", back_label)
                    .secondary()
                    .icon(AppIcon::RotateCcw)
                    .on_click(cx.listener(|this, _, window, cx| this.focus_note(window, cx))),
            )
            .when(item.kind != ItemKind::Other, |h| {
                h.action(
                    Button::new("review-ship", primary)
                        .primary()
                        .icon(AppIcon::Check)
                        .disabled(primary_disabled)
                        .on_click(cx.listener(|this, _, window, cx| this.ship(window, cx))),
                )
            })
    }

    /// The image overlay: scrim, zoom controls and the scrollable image.
    fn render_zoom(&self, z: &Zoom, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vw = f32::from(window.viewport_size().width);
        let fill = (vw - 96.).max(200.);
        let image = img(std::path::PathBuf::from(&z.path)).object_fit(ObjectFit::Contain);
        let image = match z.level {
            0 => image.w_full().h_full(),
            1 => image,
            2 => image.w(px(fill)),
            _ => image.w(px(fill * 2.)),
        };
        let level = z.level;
        div()
            .id("review-zoom")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .gap(theme::S3)
            .p(theme::S6)
            .bg(theme::SCRIM)
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.close_zoom(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_label()
                            .fg_secondary()
                            .child(data::image_name(&z.path)),
                    )
                    .child(ui::segmented(
                        "review-zoom-level",
                        &["Fit", "100%", "Fill", "2×"],
                        level,
                        {
                            let me = cx.entity();
                            move |ix, _, cx| {
                                me.update(cx, |this, cx| {
                                    if let Some(z) = this.zoom.as_mut() {
                                        z.level = ix;
                                        cx.notify();
                                    }
                                })
                            }
                        },
                    ))
                    .child(
                        Button::new("review-zoom-close", "Close")
                            .secondary()
                            .kbd("Esc")
                            .on_click(cx.listener(|this, _, _, cx| this.close_zoom(cx))),
                    ),
            )
            .child(
                div()
                    .id("review-zoom-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .bg(theme::SURFACE)
                    // Clicks on the image itself don't close the overlay.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(image),
            )
            .with_animation(
                "review-zoom-in",
                Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                |d, t| d.opacity(t),
            )
    }
}
