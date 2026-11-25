//! Home: what needs you, what runs now, and how runs and spend went (analytics).
//! Design: design/screens/home.html. This is a starting point; flesh out the analytics
//! sections from real store data only.

use chrono::{Datelike as _, Local};

use super::prelude::*;
use harness_core::Task as CoreTask;

#[derive(Default)]
struct HomeData {
    waiting: Vec<CoreTask>,
    running: Vec<CoreTask>,
    runs_total: usize,
    runs_finished: usize,
    spend_month: f64,
    pipelines: usize,
}

impl HomeData {
    fn query(store: &Store) -> HomeData {
        let tasks = store.tasks();
        let runs = store.runs();
        let now = Local::now();
        let spend_month = tasks
            .iter()
            .filter(|t| {
                let d = t.created_at.with_timezone(&Local);
                d.year() == now.year() && d.month() == now.month()
            })
            .map(|t| t.cost_usd)
            .sum();
        let mut waiting: Vec<CoreTask> = tasks.iter().filter(|t| t.status == Status::Waiting).cloned().collect();
        waiting.sort_by_key(|t| t.created_at);
        let running = tasks.iter().filter(|t| t.status == Status::Running).cloned().collect();
        HomeData {
            waiting,
            running,
            runs_total: runs.len(),
            runs_finished: runs.iter().filter(|r| r.status.is_finished()).count(),
            spend_month,
            pipelines: store.pipelines().len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.runs_total == 0 && self.waiting.is_empty() && self.running.is_empty() && self.pipelines == 0
    }
}

pub struct HomeScreen {
    data: Option<HomeData>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl HomeScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| {
            if let AppEvent::StoreChanged { .. } = ev {
                this.reload(cx);
            }
        });
        let mut this = HomeScreen { data: None, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(cx, |s| HomeData::query(s), |this, d, _| this.data = Some(d)));
    }

    fn stat(label: &'static str, value: String, sub: Option<String>, warn: bool) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(theme::S6)
            .py(theme::S4)
            .border_r_1()
            .border_color(theme::LINE)
            .child(div().text_label().fg_muted().child(label))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme::S2)
                    .child(div().text_stat_sm().text_color(if warn { theme::WARNING } else { theme::TEXT }).child(value))
                    .when_some(sub, |d, s| d.child(div().text_label().fg_muted().child(s))),
            )
    }
}

impl Render for HomeScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = Local::now();
        let running = self.data.as_ref().map(|d| d.running.len()).unwrap_or(0);
        let header = PageHeader::new("Home")
            .subtitle(now.format("%A, %b %-d · %H:%M").to_string())
            .when(running > 0, |h| h.meta(Badge::new(format!("{running} running")).info().dot()))
            .action(
                Button::new("home-new-run", "New run")
                    .primary()
                    .icon(AppIcon::Play)
                    .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
            );

        let content = match &self.data {
            None => body().child(skeleton_rows("home-loading", 4)),
            Some(d) if d.is_empty() => body().child(
                EmptyState::new(
                    AppIcon::Home,
                    "Nothing has run yet",
                    "Create a pipeline or start a chat. Runs, spend and your decisions will show up here.",
                )
                .action(
                    Button::new("home-empty-create", "Create a pipeline")
                        .icon(AppIcon::Plus)
                        .on_click(|_, _, cx| state::navigate(Route::Pipelines { pipeline: None }, cx)),
                ),
            ),
            Some(d) => {
                let finished_pct = (d.runs_finished * 100).checked_div(d.runs_total).unwrap_or(0);
                let oldest = d.waiting.first().map(|t| format!("oldest {}", ui::fmt_ago(t.created_at)));
                body()
                    .p_0()
                    .gap_0()
                    .child(
                        div()
                            .flex()
                            .border_b_1()
                            .border_color(theme::LINE)
                            .child(Self::stat("Waiting for you", d.waiting.len().to_string(), oldest, !d.waiting.is_empty()))
                            .child(Self::stat("Spend this month", ui::fmt_usd(d.spend_month), None, false))
                            .child(Self::stat("Runs", d.runs_total.to_string(), Some(format!("{finished_pct}% finished")), false))
                            .child(Self::stat("Running", running.to_string(), None, false)),
                    )
                    .child(
                        div().p(theme::S6).child(
                            panel()
                                .child(
                                    div()
                                        .px(theme::S4)
                                        .py(theme::S3)
                                        .border_b_1()
                                        .border_color(theme::LINE)
                                        .child(SectionHeader::new("Needs you").icon(AppIcon::Inbox).meta(format!(
                                            "{} waiting",
                                            d.waiting.len()
                                        ))),
                                )
                                .when(d.waiting.is_empty(), |p| {
                                    p.child(
                                        EmptyState::new(AppIcon::CircleCheck, "All clear", "Nothing is waiting for you right now.")
                                            .compact(),
                                    )
                                })
                                .children(d.waiting.iter().map(|t| {
                                    let id = t.id.clone();
                                    div()
                                        .id(SharedString::from(format!("home-wait-{}", t.id)))
                                        .flex()
                                        .items_center()
                                        .gap(theme::S3)
                                        .h(px(56.))
                                        .px(theme::S4)
                                        .border_b_1()
                                        .border_color(theme::LINE)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme::SURFACE_RAISED))
                                        .child(div().flex_1().min_w_0().text_body_strong().truncate().child(t.title.clone()))
                                        .child(div().text_label().fg_muted().child(ui::fmt_ago(t.created_at)))
                                        .on_click(move |_, _, cx| {
                                            state::navigate(Route::Review { task: Some(id.clone()) }, cx)
                                        })
                                })),
                        ),
                    )
            }
        };

        screen().child(header).child(content)
    }
}
