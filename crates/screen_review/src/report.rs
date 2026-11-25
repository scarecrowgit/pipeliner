//! Agent reports in the Inbox ("From agents"): list rows, the report view (pull request
//! cards with live status from `gh`, other links, the Markdown summary) and its actions.

use std::collections::HashMap;

use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, Hsla, IntoElement, SharedString, div,
    prelude::*, px,
};
use screen_chat::inbox::{self, InboxItem, Link, LinkKind, Outcome, PrInfo};
use ui::nav;
use ui::theme::*;
use ui::{Badge, Button, Status, Tone, icon, mono, status_dot};

use crate::ReviewScreen;

/// Live status of one pull request link.
pub(crate) enum PrState {
    Loading,
    Ready(PrInfo),
    Failed(String),
}

pub(crate) fn outcome_look(o: Outcome) -> (Status, Hsla, Tone) {
    match o {
        Outcome::Done => (Status::Live, SUCCESS, Tone::Success),
        Outcome::NeedsReview => (Status::Waiting, WARNING, Tone::Warning),
        Outcome::Blocked => (Status::Waiting, WARNING, Tone::Warning),
        Outcome::Failed => (Status::Error, DANGER, Tone::Danger),
    }
}

/// The entry after `id` in `order` that passes `keep`, else the nearest one before it.
fn neighbor(order: &[String], id: &str, keep: impl Fn(&String) -> bool) -> Option<String> {
    let i = order.iter().position(|r| r == id)?;
    order[i + 1..]
        .iter()
        .find(|o| keep(o))
        .or_else(|| order[..i].iter().rev().find(|o| keep(o)))
        .cloned()
}

/// `12m`, `3h`, `Sat`, `Nov 9`.
fn ago(t: u64) -> String {
    let now = ui::time::now_secs();
    ui::time::when_short(t, now, ui::time::utc_offset(now))
}

impl ReviewScreen {
    // ── State ───────────────────────────────────────────────────────────

    pub(crate) fn report(&self) -> Option<&InboxItem> {
        let id = self.report.as_ref()?;
        self.reports.iter().find(|r| &r.id == id)
    }

    /// Reports shown in the queue (not archived), newest first.
    pub(crate) fn visible_reports(&self) -> impl Iterator<Item = &InboxItem> {
        self.reports.iter().filter(|r| !r.archived)
    }

    pub(crate) fn reload_reports(&mut self, cx: &mut Context<Self>) {
        // Stamp first: a write landing during the load is then picked up by the next poll.
        self.inbox_stamp = crate::file_stamp(&inbox::path());
        self.reports = inbox::load();
        if self.report.as_ref().is_some() && self.report().is_none() {
            self.report = None;
        }
        self.publish_count(cx);
        self.fetch_prs(false, cx);
        self.archive_finished(cx);
        cx.notify();
    }

    /// Sidebar badge: unread reports plus pipeline reviews still waiting.
    pub(crate) fn publish_count(&self, cx: &mut Context<Self>) {
        let n = inbox::unread(&self.reports) + self.waiting_issues();
        cx.defer(move |cx| nav::set_inbox_count(n, cx));
    }

    pub(crate) fn open_report(&mut self, id: String, cx: &mut Context<Self>) {
        self.sending_back = false;
        let unread = self
            .reports
            .iter()
            .any(|r| r.id == id && !r.read);
        if unread {
            let _ = inbox::update(&id, |r| r.read = true);
            if let Some(r) = self.reports.iter_mut().find(|r| r.id == id) {
                r.read = true;
            }
            self.publish_count(cx);
        }
        self.report = Some(id);
        self.fetch_prs(false, cx);
        cx.notify();
    }

    /// Ask `gh` for the status of the pull requests of every report in the queue. Cached
    /// (loading or ready) ones are skipped; `force` re-fetches those of the shown report.
    pub(crate) fn fetch_prs(&mut self, force: bool, cx: &mut Context<Self>) {
        let shown: Vec<String> = self
            .report()
            .map(|r| r.prs().map(|l| l.url.clone()).collect())
            .unwrap_or_default();
        let mut urls: Vec<String> = Vec::new();
        for l in self.visible_reports().flat_map(|r| r.prs()) {
            if !urls.contains(&l.url) {
                urls.push(l.url.clone());
            }
        }
        for url in urls {
            let cached = matches!(
                self.prs.get(&url),
                Some(PrState::Loading | PrState::Ready(_))
            );
            if cached && !(force && shown.contains(&url)) {
                continue;
            }
            self.prs.insert(url.clone(), PrState::Loading);
            cx.spawn(async move |this, cx| {
                let u = url.clone();
                let res = cx
                    .background_spawn(async move { inbox::fetch_pr(&u) })
                    .await;
                this.update(cx, |this, cx| {
                    this.prs.insert(
                        url,
                        match res {
                            Ok(p) => PrState::Ready(p),
                            Err(e) => PrState::Failed(e),
                        },
                    );
                    this.archive_finished(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Archive the reports whose pull requests are all merged or closed.
    fn archive_finished(&mut self, cx: &mut Context<Self>) {
        let states: HashMap<String, String> = self
            .prs
            .iter()
            .filter_map(|(url, s)| match s {
                PrState::Ready(p) => Some((url.clone(), p.state.clone())),
                _ => None,
            })
            .collect();
        let order: Vec<String> = self.visible_reports().map(|r| r.id.clone()).collect();
        if !inbox::archive_finished_in(&mut self.reports, &states) {
            return;
        }
        let _ = inbox::archive_closed_prs(&states);
        self.publish_count(cx);
        if let Some(open) = self.report.clone()
            && self.report().is_some_and(|r| r.archived)
        {
            let next = neighbor(&order, &open, |id| {
                self.reports.iter().any(|r| r.id == *id && !r.archived)
            });
            self.show_after_archive(next, cx);
        }
        cx.notify();
    }

    /// Open `next`, or the pipeline reviews when there is none.
    fn show_after_archive(&mut self, next: Option<String>, cx: &mut Context<Self>) {
        match next {
            Some(n) => self.open_report(n, cx),
            None => self.report = None,
        }
    }

    fn set_flag(&mut self, f: impl Fn(&mut InboxItem) + Copy, cx: &mut Context<Self>) {
        let Some(id) = self.report.clone() else {
            return;
        };
        let _ = inbox::update(&id, f);
        if let Some(r) = self.reports.iter_mut().find(|r| r.id == id) {
            f(r);
        }
        self.publish_count(cx);
        cx.notify();
    }

    /// Archive report `id` (not necessarily the open one). When it is open, the next
    /// report (or the first pipeline review) gets selected.
    fn archive_report(&mut self, id: &str, cx: &mut Context<Self>) {
        let order: Vec<String> = self.visible_reports().map(|r| r.id.clone()).collect();
        let _ = inbox::update(id, |r| r.archived = true);
        if let Some(r) = self.reports.iter_mut().find(|r| r.id == id) {
            r.archived = true;
        }
        self.publish_count(cx);
        if self.report.as_deref() == Some(id) {
            let next = neighbor(&order, id, |o| o != id);
            self.show_after_archive(next, cx);
        }
        cx.notify();
    }

    fn archive_open_report(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.report.clone() {
            self.archive_report(&id, cx);
        }
    }

    // ── Queue rows ──────────────────────────────────────────────────────

    pub(crate) fn render_report_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.visible_reports()
            .enumerate()
            .map(|(i, r)| {
                let current = self.report.as_deref() == Some(r.id.as_str());
                let (status, color, _) = outcome_look(r.outcome);
                let tag = r
                    .prs()
                    .next()
                    .map(|l| l.label.clone())
                    .unwrap_or_else(|| r.wd_label.clone());
                let id = r.id.clone();
                let archive_id = r.id.clone();
                let key = gpui::ElementId::Name(format!("inbox-report-{}", r.id).into());
                let (rest, hover) = if current {
                    let l = ui::motion::Look::bg(SURFACE_HOVER).border(LINE_STRONG);
                    (l, l)
                } else {
                    let clear = gpui::transparent_black();
                    (
                        ui::motion::Look::bg(clear).border(clear),
                        ui::motion::Look::bg(SURFACE_RAISED).border(clear),
                    )
                };
                ui::motion::fade(div().id(("inbox-report", i)), key, rest, hover)
                    .flex()
                    .flex_none()
                    .items_start()
                    .gap(px(10.))
                    .w_full()
                    .p(px(10.))
                    .border_1()
                    .rounded(px(RADIUS_LG))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_report(id.clone(), cx)))
                    .child(div().mt(px(7.)).flex_none().child(status_dot(status, true)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap(px(2.))
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(SPACE_2))
                                    .min_w_0()
                                    // Absolute box: gpui measures nowrap text at full width.
                                    .child(
                                        div().relative().flex_1().min_w_0().h(px(20.)).child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .truncate()
                                                .child(mono(tag).text_color(TEXT_MUTED)),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(12.))
                                            .text_color(color)
                                            .child(r.outcome.label()),
                                    ),
                            )
                            .child(
                                div()
                                    .font_weight(if r.read {
                                        FontWeight::NORMAL
                                    } else {
                                        FontWeight::SEMIBOLD
                                    })
                                    .line_clamp(2)
                                    .child(r.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(TEXT_MUTED)
                                    .line_clamp(1)
                                    .text_ellipsis()
                                    .child(format!("{} · {}", r.agent, ago(r.created))),
                            ),
                    )
                    .when(!r.read, |d| {
                        d.child(
                            div()
                                .mt(px(7.))
                                .size(px(6.))
                                .flex_none()
                                .rounded_full()
                                .bg(ACCENT),
                        )
                    })
                    // Close without opening: the click must not reach the row.
                    .child(
                        Button::new(("inbox-report-archive", i), "")
                            .small()
                            .ghost()
                            .icon("check")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.archive_report(&archive_id, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }

    // ── Report view ─────────────────────────────────────────────────────

    pub(crate) fn render_report(&self, r: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let (_, _, tone) = outcome_look(r.outcome);
        let chat = r.chat_id.clone();
        let head = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_2))
                    .when(!r.wd_label.is_empty(), |d| {
                        d.child(mono(r.wd_label.clone()).text_color(TEXT_MUTED))
                    })
                    .child(Badge::new(r.outcome.label()).tone(tone))
                    .when(r.auto, |d| {
                        d.child(Badge::new("captured from the run").tone(Tone::Neutral))
                    }),
            )
            .child(
                div()
                    .text_size(px(TEXT_TITLE_LG))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(r.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO))
                    .text_color(TEXT_MUTED)
                    .child(r.agent.clone())
                    .child(div().text_color(TEXT_DISABLED).child("›"))
                    .child(
                        div()
                            .id("report-chat-link")
                            .text_color(ACCENT_TEXT)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(move |_, _, cx| {
                                nav::request_chat(nav::ChatRequest::Open(chat.clone().into()), cx)
                            })
                            .child(if r.chat_title.is_empty() {
                                "chat".to_string()
                            } else {
                                r.chat_title.clone()
                            }),
                    )
                    .child(div().text_color(TEXT_DISABLED).child("›"))
                    .child(div().text_color(WARNING).child("You"))
                    .child(div().text_color(TEXT_DISABLED).child("·"))
                    .child(ago(r.created)),
            );

        let prs: Vec<AnyElement> = r
            .prs()
            .enumerate()
            .map(|(i, l)| self.pr_card(i, l, cx))
            .collect();
        let others: Vec<&Link> = r
            .links
            .iter()
            .filter(|l| l.kind != LinkKind::PullRequest)
            .collect();
        let links = (!others.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .bg(SURFACE)
                .overflow_hidden()
                .child(
                    div()
                        .px(px(14.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(LINE)
                        .font_weight(FontWeight::MEDIUM)
                        .child("Links"),
                )
                .children(others.into_iter().enumerate().map(|(i, l)| {
                    let url = l.url.clone();
                    let icon_name = match l.kind {
                        LinkKind::Commit => "git-branch",
                        LinkKind::Issue => "check",
                        _ => "arrow-right",
                    };
                    ui::motion::fade_row(
                        div().id(("report-link", i)),
                        ("report-link-fade", i),
                        false,
                        SURFACE_RAISED,
                    )
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(LINE_SUBTLE)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(icon(icon_name, 14., TEXT_MUTED))
                    .child(mono(l.label.clone()).text_color(TEXT))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .line_clamp(1)
                            .text_ellipsis()
                            .child(l.url.clone()),
                    )
                }))
        });

        let summary = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .p(px(14.))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("Summary"),
            )
            .child(if r.summary.trim().is_empty() {
                div()
                    .text_color(TEXT_MUTED)
                    .child("The agent left no summary.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(screen_chat::render_markdown(&r.summary))
                    .into_any_element()
            });

        let first_pr = r.prs().next().map(|l| l.url.clone());
        let chat = r.chat_id.clone();
        let read = r.read;
        let footer = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_2))
            .pt(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .child(
                Button::new("report-open-chat", "Open chat")
                    .icon("message")
                    .on_click(move |_, _, cx| {
                        nav::request_chat(nav::ChatRequest::Open(chat.clone().into()), cx)
                    }),
            )
            .child(
                Button::new("report-unread", if read { "Mark unread" } else { "Mark read" })
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let f: fn(&mut InboxItem) = if read {
                            |r| r.read = false
                        } else {
                            |r| r.read = true
                        };
                        this.set_flag(f, cx)
                    })),
            )
            .child(
                Button::new("report-archive", "Archive")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.archive_open_report(cx))),
            )
            .child(div().flex_1())
            .when_some(first_pr, |d, url| {
                d.child(
                    Button::new("report-open-pr", "Open pull request")
                        .primary()
                        .icon("git-branch")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            });

        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_5))
            .child(head)
            .children(prs)
            .children(links)
            .child(summary)
            .child(footer)
            .into_any_element()
    }

    fn pr_card(&self, i: usize, l: &Link, cx: &mut Context<Self>) -> AnyElement {
        let state = self.prs.get(&l.url);
        let (badge, info): (Option<(String, Tone)>, Option<&PrInfo>) = match state {
            Some(PrState::Ready(p)) => {
                let tone = match p.state_label() {
                    "merged" => Tone::Success,
                    "closed" => Tone::Danger,
                    "draft" => Tone::Neutral,
                    _ => Tone::Info,
                };
                (Some((p.state_label().to_string(), tone)), Some(p))
            }
            _ => (None, None),
        };
        let title: SharedString = info
            .map(|p| p.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| l.url.clone())
            .into();
        let detail: AnyElement = match (state, info) {
            (_, Some(p)) => {
                let mut parts: Vec<(String, Hsla)> = Vec::new();
                let checks = p.checks_passed + p.checks_failed + p.checks_pending;
                if checks > 0 {
                    if p.checks_failed > 0 {
                        parts.push((format!("{} failed", p.checks_failed), DANGER));
                    }
                    if p.checks_pending > 0 {
                        parts.push((format!("{} running", p.checks_pending), INFO));
                    }
                    parts.push((format!("{} of {checks} checks passed", p.checks_passed), TEXT_SECONDARY));
                } else {
                    parts.push(("no checks".into(), TEXT_MUTED));
                }
                let open = !matches!(p.state.as_str(), "MERGED" | "CLOSED");
                match p.review.as_str() {
                    _ if !open => {}
                    "APPROVED" => parts.push(("approved".into(), SUCCESS)),
                    "CHANGES_REQUESTED" => parts.push(("changes requested".into(), DANGER)),
                    "REVIEW_REQUIRED" => parts.push(("review required".into(), WARNING)),
                    _ => {}
                }
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(10.))
                    .gap_y(px(4.))
                    .text_size(px(12.))
                    .children(parts.into_iter().map(|(t, c)| div().text_color(c).child(t)))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .flex()
                            .gap(px(6.))
                            .child(div().text_color(SUCCESS).child(format!("+{}", p.additions)))
                            .child(div().text_color(DANGER).child(format!("−{}", p.deletions))),
                    )
                    .when(!p.head.is_empty(), |d| {
                        d.child(
                            div()
                                .font_family(FONT_MONO)
                                .text_color(TEXT_MUTED)
                                .child(format!("{} → {}", p.head, p.base)),
                        )
                    })
                    .into_any_element()
            }
            (Some(PrState::Failed(e)), _) => div()
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(format!("Status unavailable: {e}"))
                .into_any_element(),
            _ => div()
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child("Checking status…")
                .into_any_element(),
        };
        let open_url = l.url.clone();
        let copy_url = l.url.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .border_1()
            .border_color(if i == 0 { ACCENT_RING } else { LINE })
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(24.))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_MD))
                            .bg(ACCENT_SOFT)
                            .child(icon("git-branch", 14., ACCENT_TEXT)),
                    )
                    .child(mono(l.label.clone()).text_color(TEXT_MUTED))
                    .when_some(badge, |d, (b, tone)| d.child(Badge::new(b).tone(tone)))
                    .child(div().flex_1())
                    .child(
                        Button::new(("pr-refresh", i), "")
                            .small()
                            .ghost()
                            .icon("replay")
                            .on_click(cx.listener(|this, _, _, cx| this.fetch_prs(true, cx))),
                    )
                    .child(
                        Button::new(("pr-copy", i), "Copy link")
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()))
                            }),
                    )
                    .child(
                        Button::new(("pr-open", i), "Open")
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(&open_url)),
                    ),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .line_clamp(2)
                    .child(title),
            )
            .child(detail)
            .into_any_element()
    }
}

/// Unused-import guard for `HashMap` in the screen struct's field type.
pub(crate) type PrCache = HashMap<String, PrState>;
