//! Drawing the palette: the scrim and panel, the search row, the chips, the list and the
//! footer. The preview of the selected chat is in `preview`, a result row in `row`.

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, Render, SharedString, Window, div, prelude::*, px,
};
use hub::LoadState;
use ui::dev::Probe as _;
use ui::prelude::*;

use crate::facets::{Facet, filtered};
use crate::groups::{Entry, local_offset};
use crate::labels::{count_line, index_line};
use crate::palette::Palette;
use crate::{PageDown, PageUp, PinSelected};

/// The panel's width and height in the mockup; smaller windows shrink it.
const PANEL_WIDTH: f32 = 1040.;
const PANEL_HEIGHT: f32 = 820.;
/// The panel's distance from the top of the window.
const PANEL_TOP: f32 = 56.;
/// The result list's width when the preview sits beside it.
const LIST_WIDTH: f32 = 540.;
/// Below this panel width the preview is left out and the list takes the room.
const PREVIEW_MIN_PANEL: f32 = 860.;

/// What the list area shows.
pub(crate) enum Body {
    /// Nothing has arrived yet.
    Loading,
    Failed(SharedString),
    /// The query is answered and nothing matches.
    Empty,
    Results,
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The layer took focus when it opened, after the palette was built: take it back once.
        if !self.took_focus {
            self.took_focus = true;
            self.focus(window, cx);
        }

        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 2. * SPACE_6).clamp(320., PANEL_WIDTH);
        let height = (f32::from(viewport.height) - PANEL_TOP - SPACE_8).clamp(320., PANEL_HEIGHT);
        let wide = width >= PREVIEW_MIN_PANEL;

        // The overlay layer draws the scrim, centres this panel and fades it in and out.
        col()
            .id("palette")
            .w(px(width))
            .h(px(height))
            .flex_none()
            .mt(px(PANEL_TOP))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_DIALOG))
            .bg(SURFACE)
            .overflow_hidden()
            .occlude()
            .key_context(crate::CONTEXT)
            .on_action(cx.listener(|this, _: &keys::Cancel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.page(-1, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.page(1, cx)))
            .on_action(cx.listener(|this, _: &PinSelected, _, cx| this.pin_selected(cx)))
            // Cmd-Backspace deletes to the line start in the field. With nothing typed there is
            // nothing to delete, and it is the mockup's key for archiving the selected chat.
            .capture_action(cx.listener(|this, _: &ui_rich::text::DeleteToLineStart, _, cx| {
                if this.input.read(cx).is_empty() && this.selected_hit().is_some() && !this.closing {
                    this.archive_selected(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // A press on text or a button must not take the keyboard from the field.
                    this.focus(window, cx);
                }),
            )
            .child(self.search_row(cx))
            .child(self.chips(cx))
            .child(self.body(wide, cx))
            .child(self.footer())
    }
}


impl Palette {
    /// The search field with its scope tag, the count of matches and the Esc key cap.
    fn search_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, matching, indexed) = match &self.result {
            Some(result) => (result.hits.len(), result.total, result.index.sessions),
            None => (0, 0, 0),
        };
        let loading = matches!(self.search.read(cx).state(), LoadState::Loading);
        let counts = if self.result.is_none() {
            String::new()
        } else if loading {
            "Searching…".to_string()
        } else {
            count_line(shown, matching, indexed)
        };
        row()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px(px(SPACE_4))
            .gap(px(SPACE_2_5))
            .border_b_1()
            .border_color(LINE)
            .child(icon(IconName::Search, 16., TEXT_MUTED))
            .children(self.scope.as_ref().map(|repo| {
                div()
                    .id("palette-scope")
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.clear_scope(cx)))
                    .child(Tag::new(format!("in:{repo}")).mono().tone(TagTone::Accent))
                    .probe("palette-scope")
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .type_style(TITLE_SM)
                    .text_color(TEXT)
                    .child(self.input.clone())
                    .probe("palette-field"),
            )
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(counts))
            .child(
                div()
                    .id("palette-close")
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                    .child(Kbd::new("Esc"))
                    .probe("palette-close"),
            )
    }

    /// The filter chips, with the counts of the current scope.
    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let facets = self.result.as_ref().map(|result| result.facets).unwrap_or_default();
        let query = self.query(cx).clone();
        let chips = Facet::ALL.into_iter().enumerate().map(|(at, facet)| {
            let mut chip = Chip::new(("palette-chip", at), facet.label())
                .pressed(facet.pressed(&query))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_facet(facet, cx)));
            if facet == Facet::StartedByAgents {
                chip = chip.spawn();
            }
            if let Some(count) = facet.count(&facets) {
                chip = chip.count(base::fmt::count(count));
            }
            chip.probe(format!("palette-chip-{}", facet.slug())).into_any_element()
        });
        row()
            .flex_none()
            .px(px(SPACE_4))
            .py(px(SPACE_2_5))
            .gap(px(SPACE_1_5))
            .border_b_1()
            .border_color(LINE)
            .overflow_hidden()
            .children(chips)
    }

    /// The list, and the preview of the selected chat beside it when the panel is wide enough.
    fn body(&self, wide: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div()
            .flex()
            .flex_col()
            .h_full()
            .min_w_0()
            .when(wide, |d| d.w(px(LIST_WIDTH)).flex_none())
            .when(!wide, |d| d.flex_1())
            .child(self.list(cx));
        let preview = wide.then(|| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_l_1()
                .border_color(LINE)
                .child(self.preview(cx))
        });
        div().flex().flex_row().flex_1().min_h_0().child(list).children(preview)
    }

    pub(crate) fn body_state(&self, cx: &gpui::App) -> Body {
        match (self.search.read(cx).state(), &self.result) {
            (LoadState::Failed(message), _) => Body::Failed(message.clone()),
            (_, None) => Body::Loading,
            (LoadState::Loading, Some(result)) if result.hits.is_empty() => Body::Loading,
            (_, Some(result)) if result.hits.is_empty() => Body::Empty,
            _ => Body::Results,
        }
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.body_state(cx) {
            Body::Loading => notice(None, "Searching…", None, "palette-loading"),
            Body::Failed(message) => self.failed(message, cx),
            Body::Empty => {
                let query = self.query(cx);
                let (text, detail) = if query.text.trim().is_empty() && self.scope.is_none() && !filtered(query) {
                    ("No chats yet".to_string(), "Start one with New session.".to_string())
                } else {
                    let what = match (query.text.trim(), &self.scope) {
                        ("", Some(repo)) => format!("in {repo}"),
                        (text, _) => format!("“{text}”"),
                    };
                    (format!("No chats match {what}"), "Try fewer words, or turn a filter off.".to_string())
                };
                notice(Some(IconName::Search), text, Some(detail.into()), "palette-empty")
            }
            Body::Results => {
                // The previous results stay up while a newer query runs, dimmed.
                let stale = matches!(self.search.read(cx).state(), LoadState::Loading);
                let entries: Vec<AnyElement> = self
                    .listing
                    .entries
                    .iter()
                    .map(|entry| match *entry {
                        Entry::Header(at) => self.header(at, cx),
                        Entry::Row { hit, row } => match self.result.as_ref().and_then(|result| result.hits.get(hit)) {
                            Some(hit) => self.result_row(row, hit, cx),
                            None => div().into_any_element(),
                        },
                    })
                    .collect();
                div()
                    .id("palette-list")
                    .flex()
                    .flex_col()
                    .size_full()
                    .p(px(SPACE_1_5))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when(stale, |d| d.opacity(0.6))
                    .children(entries)
                    .into_any_element()
            }
        }
    }

    /// A section heading: chevron, name, and the count and cost at the right.
    fn header(&self, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(group) = self.listing.groups.get(at) else { return div().into_any_element() };
        let open = !self.collapsed.contains(&group.label);
        let label = group.label.clone();
        div()
            .id(("palette-group", at))
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(px(SPACE_1_5))
            .h(px(TABLE_GROUP_HEIGHT))
            .px(px(SPACE_3))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&label, cx)))
            .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., TEXT_MUTED))
            .child(div().type_style(LABEL_STRONG).text_color(TEXT_SECONDARY).child(group.label.clone()))
            .child(grow())
            .child(div().tabular().child(group.meta()))
            .probe(format!("palette-group-{at}"))
            .into_any_element()
    }

    /// The search failed: what the engine said, and a way to try again.
    fn failed(&self, message: SharedString, cx: &mut Context<Self>) -> AnyElement {
        col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(SPACE_3))
            .p(px(SPACE_8))
            .child(notice(Some(IconName::Alert), "Could not search chats", Some(message), "palette-error"))
            .child(
                Button::new("palette-retry", "Try again")
                    .secondary()
                    .small()
                    .icon(IconName::Replay)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    .probe("palette-retry"),
            )
            .into_any_element()
    }

    /// The key hints and the index note.
    fn footer(&self) -> impl IntoElement {
        let now = self.clock();
        let note = self.result.as_ref().map(|result| index_line(&result.index, now, local_offset(now)));
        row()
            .h(px(CARD_FOOT_HEIGHT))
            .flex_none()
            .px(px(SPACE_4))
            .gap(px(SPACE_3))
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                KeyHints::new()
                    .hint(["↑", "↓"], "move")
                    .hint(["↵"], "open")
                    .hint(["⌘P"], "pin")
                    .hint(["⌘⌫"], "archive"),
            )
            .child(grow())
            .children(note.map(|note| div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(note)))
    }
}

/// A centred message for the list area: an optional icon, a title, an optional detail line.
fn notice(
    glyph: Option<IconName>,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    probe: &'static str,
) -> AnyElement {
    let color = if glyph == Some(IconName::Alert) { DANGER } else { TEXT_MUTED };
    col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(SPACE_1_5))
        .p(px(SPACE_8))
        .children(glyph.map(|glyph| icon(glyph, 16., color)))
        .child(div().type_style(BODY_STRONG).text_color(TEXT_SECONDARY).child(title.into()))
        .children(detail.map(|detail| div().max_w(px(360.)).type_style(LABEL).text_color(TEXT_MUTED).child(detail)))
        .probe(probe)
        .into_any_element()
}
