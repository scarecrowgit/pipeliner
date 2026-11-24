//! The queue on the left: tabs with counts, the rows, the key hints.

use std::sync::Arc;

use gpui::{AnyElement, Context, ElementId, IntoElement, Window, div, prelude::*, px};
use hub::LoadState;
use hub::api::{InboxItem, InboxKind};
use ui::dev::Probe;
use ui::prelude::*;

use crate::view::{InboxView, QUEUE_WIDTH};

/// The tabs of the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    /// Items a person has to act on.
    Waiting,
    /// Every item that is not archived: the waiting ones first.
    All,
    /// Archived items, newest first.
    Archived,
}

impl Tab {
    pub(crate) const ALL: [Tab; 3] = [Tab::Waiting, Tab::All, Tab::Archived];

    /// The tab's name in probes: `inbox-tab-waiting`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tab::Waiting => "waiting",
            Tab::All => "all",
            Tab::Archived => "archived",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Waiting => "Waiting",
            Tab::All => "All",
            Tab::Archived => "Archived",
        }
    }
}

/// The rows of a tab of live items: the waiting ones (`waiting`, in the model's order), then
/// the rest in the model's order (newest first). An item whose run moved on (`moved_on`)
/// no longer waits.
pub(crate) fn ordered(
    waiting: Vec<Arc<InboxItem>>,
    items: &[Arc<InboxItem>],
    tab: Tab,
    moved_on: impl Fn(&InboxItem) -> bool,
) -> Vec<Arc<InboxItem>> {
    let first: Vec<Arc<InboxItem>> = waiting.into_iter().filter(|item| !moved_on(item)).collect();
    match tab {
        Tab::Waiting => first,
        Tab::All => {
            let rest: Vec<Arc<InboxItem>> = items
                .iter()
                .filter(|item| !first.iter().any(|known| known.id == item.id))
                .cloned()
                .collect();
            first.into_iter().chain(rest).collect()
        }
        Tab::Archived => Vec::new(),
    }
}

/// A link to a pull or merge request, whose live status the detail shows.
pub(crate) fn is_pull_request(url: &str) -> bool {
    url.contains("/pull/") || url.contains("/merge_requests/") || url.contains("/pull-requests/")
}

/// What kind of item this is, in words.
pub(crate) fn kind_label(item: &InboxItem) -> &'static str {
    match item.kind {
        InboxKind::Review { .. } => "Review",
        InboxKind::Question { .. } => "Question",
        InboxKind::Failure => "Failure",
        InboxKind::Report if item.links.iter().any(|link| is_pull_request(&link.url)) => {
            "Pull request"
        }
        InboxKind::Report => "Report",
    }
}

/// The row's dot: waiting for a person, failed, or unread.
fn row_status(item: &InboxItem) -> Option<Status> {
    match item.kind {
        InboxKind::Failure if !item.archived => Some(Status::Error),
        _ if item.needs_person() => Some(Status::Waiting),
        _ if !item.read && !item.archived => Some(Status::Running),
        _ => None,
    }
}

impl InboxView {
    /// The queue: tabs, rows, key hints.
    pub(crate) fn render_queue(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self.visible_items(cx);
        let state = match self.tab {
            Tab::Archived => self.archived.state.clone(),
            _ => self.inbox.read(cx).state().clone(),
        };
        let now = self.now();
        let offset = base::time::utc_offset(now);
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(ix, item)| self.render_row(ix, item, now, offset, window, cx))
            .collect();
        let body = if !rows.is_empty() {
            div()
                .id("inbox-rows")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(SPACE_2))
                .gap(px(SPACE_1))
                .children(rows)
                .into_any_element()
        } else {
            let text = match (&state, self.tab) {
                (LoadState::Loading, _) => "Loading…".to_string(),
                (LoadState::Failed(error), _) => format!("Could not load the inbox: {error}"),
                (LoadState::Ready, Tab::Waiting) => "Nothing waits for you".to_string(),
                (LoadState::Ready, Tab::All) => "The inbox is empty".to_string(),
                (LoadState::Ready, Tab::Archived) => "Nothing is archived".to_string(),
            };
            let failed = matches!(state, LoadState::Failed(_));
            col()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .px(px(SPACE_5))
                .child(
                    div()
                        .w(px(QUEUE_WIDTH - 2. * SPACE_5))
                        .text_center()
                        .type_style(LABEL)
                        .text_color(if failed { DANGER } else { TEXT_MUTED })
                        .child(text),
                )
                .debug_selector(|| "inbox-empty".into())
                .into_any_element()
        };
        let picks = self
            .open_review(cx)
            .map(|review| review.variants.len())
            .filter(|n| *n > 0)
            .unwrap_or(3);

        col()
            .w(px(QUEUE_WIDTH))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(LINE)
            .child(self.render_tabs(cx))
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px(px(SPACE_4))
                    .py(px(SPACE_3))
                    .border_t_1()
                    .border_color(LINE)
                    .child(key_hints(picks)),
            )
            .into_any_element()
    }

    /// Waiting / All / Archived, with counts.
    fn render_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let waiting = self.live_items(Tab::Waiting, cx).len();
        let all = self.inbox.read(cx).items().len();
        let archived = self.archived.started.then_some(self.archived.items.len());
        let selected = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let mut tabs = Tabs::new("inbox-tabs").flush().selected(selected);
        for tab in Tab::ALL {
            let count = match tab {
                Tab::Waiting => Some(waiting),
                Tab::All => Some(all),
                Tab::Archived => archived,
            };
            tabs = match count {
                Some(n) => tabs.tab_n(tab.label(), base::fmt::count(n as u64)),
                None => tabs.tab(tab.label()),
            }
            .tab_probe(format!("inbox-tab-{}", tab.name()));
        }
        let view = cx.entity().downgrade();
        let tabs = tabs.on_select(move |ix, _, cx| {
            let tab = Tab::ALL[ix.min(Tab::ALL.len() - 1)];
            view.update(cx, |this, cx| this.set_tab(tab, cx)).ok();
        });
        // The hairline spans the whole queue, under the padded bar.
        div()
            .relative()
            .flex_none()
            .px(px(SPACE_4))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(LINE),
            )
            .child(tabs)
            .into_any_element()
    }

    /// One row: issue key, dot, age; the title; who it is from and through which pipeline.
    fn render_row(
        &mut self,
        ix: usize,
        item: &Arc<InboxItem>,
        now: i64,
        offset: i32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.is_open(item);
        let id = ElementId::NamedInteger("inbox-row".into(), ix as u64);
        let selector = format!("inbox-row-{ix}");
        let probe = selector.clone();
        let key = item.issue.as_ref().map(|key| key.to_string());
        let pipeline = item
            .run
            .as_ref()
            .and_then(|run| self.run_pipeline_name(run, cx));
        let kind = match item.kind {
            InboxKind::Review { .. } => None,
            _ => Some(kind_label(item)),
        };
        let unread = !item.read && !item.archived;
        let line = row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .when_some(key, |d, key| {
                d.child(div().flex_none().type_style(MONO).child(key))
            })
            .when_some(row_status(item), |d, status| {
                d.child(StatusDot::new(status).small())
            })
            .when_some(kind, |d, kind| d.child(div().flex_none().child(kind)))
            .child(grow())
            .child(div().flex_none().type_style(MONO_SM).child(base::time::ago(
                now,
                item.created,
                offset,
            )));
        let title = div()
            .type_style(if unread { BODY_STRONG } else { BODY })
            .text_color(if unread || selected {
                TEXT
            } else {
                TEXT_SECONDARY
            })
            .child(truncate_block(item.title.clone(), BODY.line));
        let meta_text = match pipeline {
            // A run's failure is filed by the pipeline itself: say the name once.
            Some(name) if !item.source.is_empty() && item.source.as_str() != name.as_ref() => format!("{} · {name}", item.source),
            Some(name) => name.to_string(),
            None => item.source.clone(),
        };
        let meta = div()
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(truncate_block(meta_text, LABEL.line));
        let el = div()
            .id(id.clone())
            .debug_selector(move || selector)
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(SPACE_0_5))
            .py(px(SPACE_2_5))
            .px(px(SPACE_3))
            .rounded(px(RADIUS_LG))
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(ACCENT),
                )
            })
            .child(line)
            .child(title)
            .child(meta)
            .on_click(cx.listener(move |this, _, window, cx| this.open_index(ix, window, cx)));
        motion::fade_row(el, id, selected, SURFACE_RAISED)
            .probe(probe)
            .into_any_element()
    }
}

/// The queue's key hints: "J K move  1 – 3 pick  R send back", with the range of picks drawn
/// as two key caps around a dash, as the mockup does.
fn key_hints(picks: usize) -> AnyElement {
    let hint = |keys: Vec<AnyElement>, label: &'static str| {
        row()
            .flex_none()
            .gap(px(SPACE_1))
            .children(keys)
            .child(label)
            .into_any_element()
    };
    let picks_keys: Vec<AnyElement> = if picks > 1 {
        vec![
            Kbd::new("1").into_any_element(),
            div().child("–").into_any_element(),
            Kbd::new(picks.to_string()).into_any_element(),
        ]
    } else {
        vec![Kbd::new("1").into_any_element()]
    };
    row()
        .gap_x(px(SPACE_3_5))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(hint(
            vec![
                Kbd::new("J").into_any_element(),
                Kbd::new("K").into_any_element(),
            ],
            "move",
        ))
        .child(hint(picks_keys, "pick"))
        .child(hint(vec![Kbd::new("R").into_any_element()], "send back"))
        .into_any_element()
}
