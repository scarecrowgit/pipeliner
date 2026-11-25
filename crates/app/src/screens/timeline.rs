//! Timeline: tasks as bars over time (today by default), grouped by run.
//! Design: design/screens/timeline.html. Starting point: today's tasks in start order.

use chrono::Local;

use super::prelude::*;
use harness_core::Task as CoreTask;

pub struct TimelineScreen {
    today: Option<Vec<CoreTask>>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl TimelineScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes.iter().any(|c| matches!(c, Change::Task { .. } | Change::External)) {
                    this.reload(cx);
                }
            }
        });
        let mut this = TimelineScreen { today: None, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(
            cx,
            |s| {
                let today = Local::now().date_naive();
                let mut v: Vec<CoreTask> = s
                    .tasks()
                    .into_iter()
                    .filter(|t| t.started_at.unwrap_or(t.created_at).with_timezone(&Local).date_naive() == today)
                    .collect();
                v.sort_by_key(|t| t.started_at.unwrap_or(t.created_at));
                v
            },
            |this, v, _| this.today = Some(v),
        ));
    }
}

impl Render for TimelineScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Timeline").subtitle(Local::now().format("Today · %A, %b %-d").to_string());
        let content = match &self.today {
            None => body().child(skeleton_rows("timeline-loading", 4)),
            Some(v) if v.is_empty() => body().child(EmptyState::new(
                AppIcon::Timeline,
                "Nothing ran today",
                "Every step, chat task and sub-agent that runs today appears here as a bar.",
            )),
            Some(v) => body().gap(theme::S1).children(v.iter().enumerate().map(|(i, t)| {
                let id = t.id.clone();
                let start = t.started_at.unwrap_or(t.created_at).with_timezone(&Local);
                div()
                    .id(("timeline-row", i))
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .h(theme::ROW_H)
                    .px(theme::S3)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::SURFACE_RAISED))
                    .child(div().w(px(48.)).text_mono().fg_muted().child(start.format("%H:%M").to_string()))
                    .child(StatusDot::new(t.status).id(("timeline-dot", i)))
                    .child(div().flex_1().min_w_0().text_body().truncate().child(t.title.clone()))
                    .child(div().text_label().fg_muted().child(t.actor.clone()))
                    .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
            })),
        };
        screen().child(header).child(content)
    }
}
