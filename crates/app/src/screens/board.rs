//! Board: every task as a card in Queued / Running / Waiting for you / Done columns.
//! Design: design/screens/board.html. Starting point: real columns with simple cards.

use super::prelude::*;
use harness_core::Task as CoreTask;

const COLUMNS: [(Status, &str); 4] = [
    (Status::Queued, "Queued"),
    (Status::Running, "Running"),
    (Status::Waiting, "Waiting for you"),
    (Status::Done, "Done"),
];

pub struct BoardScreen {
    tasks: Option<Vec<CoreTask>>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl BoardScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes.iter().any(|c| matches!(c, Change::Task { .. } | Change::External)) {
                    this.reload(cx);
                }
            }
        });
        let mut this = BoardScreen { tasks: None, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(
            cx,
            |s| {
                let mut v = s.tasks();
                v.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.created_at.cmp(&a.created_at)));
                v
            },
            |this, v, _| this.tasks = Some(v),
        ));
    }

    fn card(t: &CoreTask, ix: usize) -> impl IntoElement {
        let id = t.id.clone();
        card()
            .id(SharedString::from(format!("board-card-{}", t.id)))
            .gap(theme::S2)
            .cursor_pointer()
            .hover(|s| s.border_color(theme::LINE_STRONG))
            .when(t.status == Status::Waiting, |d| d.bg(theme::WARNING_SOFT).border_color(theme::alpha(theme::WARNING, 0.35)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_mono_sm()
                    .fg_muted()
                    .child(t.kind.label())
                    .child(ui::fmt_ago(t.created_at)),
            )
            .child(div().text_body_strong().fg().child(t.title.clone()))
            .when(!t.summary.is_empty(), |d| d.child(div().text_label().fg_muted().line_clamp(2).child(t.summary.clone())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .text_label()
                    .fg_muted()
                    .child(StatusDot::new(t.status).id(("board-dot", ix)).small())
                    .child(t.status.label())
                    .child(div().flex_1())
                    .child(div().text_mono_sm().child(ui::fmt_usd(t.cost_usd))),
            )
            .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
    }
}

impl Render for BoardScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Board").subtitle("all runs");
        let content = match &self.tasks {
            None => body().child(skeleton_rows("board-loading", 4)),
            Some(list) if list.is_empty() => body().child(
                EmptyState::new(AppIcon::Board, "No tasks yet", "Steps of pipeline runs and chat tasks show up here as cards.")
                    .action(
                        Button::new("board-empty-pipelines", "Go to Pipelines")
                            .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
                    ),
            ),
            Some(list) => body().flex_row().items_start().gap(theme::S4).children(COLUMNS.iter().map(|(status, title)| {
                let col: Vec<&CoreTask> = list
                    .iter()
                    .filter(|t| if *status == Status::Done { t.status.is_finished() } else { t.status == *status })
                    .take(50)
                    .collect();
                panel()
                    .flex_1()
                    .min_w(px(220.))
                    .p(theme::S3)
                    .gap(theme::S2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .px(theme::S1)
                            .h(px(28.))
                            .child(StatusDot::new(*status).id(SharedString::from(format!("board-col-{}", status.as_str()))))
                            .child(div().text_body_strong().child(*title))
                            .child(div().text_label().fg_muted().child(col.len().to_string())),
                    )
                    .children(col.into_iter().enumerate().map(|(i, t)| Self::card(t, i)))
            })),
        };
        screen().child(header).child(content)
    }
}
