//! Review: the queue of items waiting for you (gates with variants, questions, approvals)
//! and the selected item's detail with Ship / Send back. Design: design/screens/review.html.
//! Starting point: a real queue and a minimal detail pane.

use super::prelude::*;
use harness_core::Task as CoreTask;

pub struct ReviewScreen {
    waiting: Option<Vec<CoreTask>>,
    selected: Option<Id>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl ReviewScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::StoreChanged { .. } => this.reload(cx),
            AppEvent::Navigated(Route::Review { task: Some(task) }) => {
                this.selected = Some(task.clone());
                cx.notify();
            }
            _ => {}
        });
        let selected = match state::route(cx) {
            Route::Review { task } => task,
            _ => None,
        };
        let mut this = ReviewScreen { waiting: None, selected, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(
            cx,
            |s| {
                let mut v = s.waiting();
                v.sort_by_key(|t| t.created_at);
                v
            },
            |this, v, _| this.waiting = Some(v),
        ));
    }

    fn selected_task(&self) -> Option<&CoreTask> {
        let list = self.waiting.as_ref()?;
        match &self.selected {
            Some(id) => list.iter().find(|t| &t.id == id).or(list.first()),
            None => list.first(),
        }
    }
}

impl Render for ReviewScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.waiting.as_ref().map(|v| v.len()).unwrap_or(0);
        let header = PageHeader::new("Review").when(count > 0, |h| h.meta(Badge::new(format!("{count} waiting")).warning().dot()));

        let content = match &self.waiting {
            None => body().child(skeleton_rows("review-loading", 3)),
            Some(list) if list.is_empty() => body().child(EmptyState::new(
                AppIcon::Inbox,
                "Nothing waiting for you",
                "When a run reaches a gate, asks a question or needs an approval, it lands here.",
            )),
            Some(list) => {
                let sel = self.selected_task().map(|t| t.id.clone());
                let queue = div()
                    .id("review-queue")
                    .w(px(280.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(theme::LINE)
                    .p(theme::S3)
                    .flex()
                    .flex_col()
                    .gap(theme::S1)
                    .child(div().px(theme::S2).py(theme::S2).child(SectionHeader::new("Waiting for you").meta(count.to_string())))
                    .children(list.iter().map(|t| {
                        let active = sel.as_ref() == Some(&t.id);
                        let id = t.id.clone();
                        div()
                            .id(SharedString::from(format!("review-item-{}", t.id)))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .p(theme::S3)
                            .rounded(theme::RADIUS_LG)
                            .border_1()
                            .cursor_pointer()
                            .when(active, |d| d.bg(theme::SURFACE_RAISED).border_color(theme::LINE_STRONG))
                            .when(!active, |d| d.border_color(theme::TRANSPARENT).hover(|s| s.bg(theme::SURFACE)))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_mono_sm()
                                    .fg_muted()
                                    .child(t.kind.label())
                                    .child(div().text_color(theme::WARNING).child(ui::fmt_ago(t.created_at))),
                            )
                            .child(div().text_body_strong().fg().child(t.title.clone()))
                            .when(!t.actor.is_empty(), |d| d.child(div().text_label().fg_muted().child(t.actor.clone())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(id.clone());
                                state::replace_route(Route::Review { task: Some(id.clone()) }, cx);
                                cx.notify();
                            }))
                    }));
                let detail = match self.selected_task() {
                    None => div(),
                    Some(t) => {
                        let open_id = t.id.clone();
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme::S4)
                            .p(theme::S6)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme::S3)
                                    .child(div().text_title().child(t.title.clone()))
                                    .child(StatusPill::new(t.status).suffix(ui::fmt_ago(t.created_at)))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("review-open-thread", "Open thread")
                                            .ghost()
                                            .icon_right(AppIcon::ChevronRight)
                                            .on_click(move |_, _, cx| state::open_task(open_id.clone(), cx)),
                                    ),
                            )
                            .when(!t.summary.is_empty(), |d| d.child(card().child(div().text_body().fg_secondary().child(t.summary.clone()))))
                    }
                };
                body().p_0().gap_0().flex_row().child(queue).child(detail)
            }
        };
        screen().child(header).child(content)
    }
}
