//! The completion menu: a popover in the overlay host, built each frame from the composer's
//! state. The keyboard stays in the text area; the composer moves the highlight.

use gpui::{AnyElement, App, ElementId, IntoElement, WeakEntity, Window, div, prelude::*, px};
use ui::components::{KeyHints, MenuItem, PopLabel, Tile};
use ui::dev::Probe;
use ui::popover::Popover;
use ui::theme::MENU_ITEM_HEIGHT;

use super::Composer;
use super::types::TriggerKind;

/// Rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 7.;

/// The heading over the rows: the trigger's name while nothing is typed after it, else
/// `Matching “re” · 2 of 8`.
pub(crate) fn heading(kind: TriggerKind, query: &str, shown: usize, total: Option<usize>) -> String {
    if query.is_empty() {
        kind.title().to_string()
    } else {
        format!("Matching “{query}” · {shown} of {}", total.unwrap_or(shown).max(shown))
    }
}

/// Bytes of `label` that the typed `query` matches at its start, ignoring case: drawn in
/// accent (`Re` of `Reproducer`). 0 when the label does not start with the query.
pub(crate) fn matched_prefix(label: &str, query: &str) -> usize {
    let mut chars = label.char_indices();
    let mut end = 0;
    for q in query.chars() {
        match chars.next() {
            Some((i, c)) if c.to_lowercase().eq(q.to_lowercase()) => end = i + c.len_utf8(),
            _ => return 0,
        }
    }
    end
}

/// The popover's content for the composer behind `me`; empty once it is gone.
pub(crate) fn build(me: &WeakEntity<Composer>, _: &mut Window, cx: &mut App) -> AnyElement {
    let Some(entity) = me.upgrade() else {
        return div().into_any_element();
    };
    let c = entity.read(cx);
    let Some(trigger) = c.trigger.as_ref() else {
        return div().into_any_element();
    };
    let title = heading(trigger.kind, &trigger.query, c.completions.len(), c.total);
    let name = c.name.clone();
    let query = trigger.query.clone();
    let rows: Vec<AnyElement> = c
        .completions
        .iter()
        .enumerate()
        .map(|(ix, item)| {
            let id = ElementId::NamedInteger(format!("{name}-item").into(), ix as u64);
            let pick = entity.clone();
            let row = MenuItem::new(id, item.label.clone())
                .matched(matched_prefix(&item.label, &query))
                .selected(ix == c.selected)
                .when_some(item.detail.clone(), |r, d| r.subline(d))
                .when_some(item.icon, |r, i| r.tile(Tile::new(i).medium()))
                .when_some(item.hint.clone(), |r, h| r.hint(h))
                .on_click(move |_, window, cx| {
                    pick.update(cx, |c, cx| {
                        c.accept(ix, cx);
                        c.focus(window, cx);
                    });
                });
            let hover = entity.clone();
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .on_mouse_move(move |_, _, cx| hover.update(cx, |c, cx| c.hover_row(ix, cx)))
                .child(row)
                .probe(format!("{name}-menu-{ix}"))
                .into_any_element()
        })
        .collect();
    let list = div()
        .id(ElementId::Name(format!("{name}-menu-list").into()))
        .flex()
        .flex_col()
        .max_h(px(MENU_ITEM_HEIGHT * VISIBLE_ROWS))
        .overflow_y_scroll()
        .track_scroll(&c.menu_scroll)
        .children(rows);
    Popover::new()
        .width(c.menu_width)
        .child(PopLabel::new(title))
        .child(list)
        .footer(KeyHints::new().hint(["↑", "↓"], "move").hint(["↵"], "insert").hint(["Esc"], "close"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::matched_prefix;

    #[test]
    fn the_typed_query_marks_the_start_of_a_label_ignoring_case() {
        assert_eq!(matched_prefix("Reproducer", "re"), 2);
        assert_eq!(matched_prefix("Release captain", "REL"), 3);
        assert_eq!(matched_prefix("Écrire", "é"), "É".len());
        assert_eq!(matched_prefix("Fixer", "re"), 0);
        assert_eq!(matched_prefix("Re", "rerun"), 0);
        assert_eq!(matched_prefix("Reproducer", ""), 0);
    }
}
