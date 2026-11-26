use gpui::{
    div, px, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::text::TextView;

use crate::theme::{self, TextStyles as _};

/// Message text: Markdown (GFM: lists, tables, code blocks, links) with app type.
/// Needs a stable, unique id per message (`("msg", &message.id)` style).
///
/// ```ignore
/// .child(ui::markdown(SharedString::from(format!("msg-{}", m.id)), m.content.clone(), window, cx))
/// ```
pub fn markdown(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> TextView {
    TextView::markdown(id, text, window, cx)
        .selectable(true)
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(theme::TEXT_SECONDARY)
}

/// Monospace block for logs, commands, tool output. Scrolls horizontally, never wraps.
#[derive(IntoElement)]
pub struct CodeBlock {
    id: ElementId,
    text: SharedString,
    max_height: Option<gpui::Pixels>,
}

impl CodeBlock {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        CodeBlock {
            id: id.into(),
            text: text.into(),
            max_height: None,
        }
    }
    /// Scroll vertically beyond this height.
    pub fn max_height(mut self, h: gpui::Pixels) -> Self {
        self.max_height = Some(h);
        self
    }
}

impl RenderOnce for CodeBlock {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut d = div()
            .id(self.id)
            .w_full()
            .p(theme::S3)
            .rounded(theme::RADIUS_LG)
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::LINE)
            .text_mono()
            .text_color(theme::TEXT_SECONDARY)
            .overflow_x_scroll();
        if let Some(h) = self.max_height {
            d = d.max_h(h).overflow_y_scroll();
        }
        d.child(
            div().whitespace_nowrap().flex().flex_col().children(
                self.text
                    .lines()
                    .map(|l| {
                        div()
                            .min_h(px(20.))
                            .child(SharedString::from(l.replace('\t', "    ")))
                    })
                    .collect::<Vec<_>>(),
            ),
        )
    }
}

/// Text cut to at most `lines` lines, the last one ending in "…" when it doesn't fit.
///
/// gpui 0.2.2's `line_clamp` drops the extra lines without an ellipsis (it places the
/// "…" by unwrapped width, before wrapping). This measures the text at the width the
/// layout gives it (one frame late, then cached per width) and cuts it at a character
/// boundary so the ellipsis lands on the last visible line. Text styles come from the
/// parent: `div().text_label().fg_muted().child(Clamped::new(id, text, 2))`.
#[derive(IntoElement)]
pub struct Clamped {
    id: ElementId,
    text: SharedString,
    lines: usize,
}

impl Clamped {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>, lines: usize) -> Self {
        Clamped {
            id: id.into(),
            text: text.into(),
            lines: lines.max(1),
        }
    }
}

#[derive(Default)]
struct ClampState {
    /// Text and width the fit below was computed for.
    src: SharedString,
    width: Option<gpui::Pixels>,
    /// `None`: the whole text fits.
    fit: Option<SharedString>,
}

impl RenderOnce for Clamped {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |_, _| ClampState::default());
        let shown = {
            let s = state.read(cx);
            match (&s.fit, s.src == self.text) {
                (Some(fit), true) => fit.clone(),
                _ => self.text.clone(),
            }
        };
        let (src, n) = (self.text, self.lines);
        div()
            .relative()
            .w_full()
            .min_w_0()
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let w = bounds.size.width;
                        let s = state.read(cx);
                        if s.src == src && s.width == Some(w) {
                            return;
                        }
                        let style = window.text_style();
                        let size = style.font_size.to_pixels(window.rem_size());
                        // Fit with a little slack: the "…" glyph can come from a fallback
                        // font and must not wrap onto a line the clamp then hides.
                        let fit_w = (w - gpui::px(12.)).max(gpui::px(12.));
                        let mut lines_of = |t: &str| -> usize {
                            let run = style.to_run(t.len());
                            window
                                .text_system()
                                .shape_text(SharedString::from(t.to_string()), size, &[run], Some(fit_w), None)
                                .map(|ls| ls.iter().map(|l| l.wrap_boundaries.len() + 1).sum())
                                .unwrap_or(1)
                        };
                        let fit = fit_lines(&src, n, &mut lines_of).map(SharedString::from);
                        state.update(cx, |s, cx| {
                            let changed = s.fit != fit || s.src != src;
                            s.src = src.clone();
                            s.width = Some(w);
                            s.fit = fit;
                            if changed {
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // No ellipsis here: gpui would cut the fitted text again by unwrapped width and
            // its "…" line would then fall past the clamp. `line_clamp` is a safety net.
            .child(div().overflow_hidden().line_clamp(n).child(shown))
    }
}

/// The longest prefix of `text` (cut at a char, trailing spaces dropped) that with "…"
/// fits in `max` lines, or `None` when the whole text fits. `lines_of` measures.
pub fn fit_lines(text: &str, max: usize, lines_of: &mut impl FnMut(&str) -> usize) -> Option<String> {
    if lines_of(text) <= max {
        return None;
    }
    let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let cand = |k: usize| format!("{}…", text[..cuts[k]].trim_end());
    // Largest k (chars kept) whose candidate fits; k = 0 ("…") always counts as fitting.
    let (mut lo, mut hi) = (0usize, cuts.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if lines_of(&cand(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    // Prefer ending at a word when one ends close by ("long sentence…", not "…wi…").
    let kept = &text[..cuts[lo]];
    let tail = kept.chars().rev().take(WORD_BACKTRACK).collect::<String>();
    match tail.find(char::is_whitespace) {
        Some(back) if back > 0 && lo > 0 => {
            let at = kept.len() - back;
            Some(format!("{}…", kept[..at].trim_end()))
        }
        _ => Some(cand(lo)),
    }
}

/// How many characters a clamped text gives up to end on a whole word.
const WORD_BACKTRACK: usize = 12;

#[cfg(test)]
mod clamp_tests {
    use super::fit_lines;

    /// 10 chars per line, newlines start a new line.
    fn lines(t: &str) -> usize {
        t.split('\n').map(|l| l.chars().count().max(1).div_ceil(10)).sum()
    }

    #[test]
    fn short_text_is_left_alone() {
        assert_eq!(fit_lines("hello", 2, &mut lines), None);
    }

    #[test]
    fn long_text_ends_in_an_ellipsis_within_the_lines() {
        let t = "abcdefghij klmnopqrst uvwxyz";
        let fit = fit_lines(t, 2, &mut lines).unwrap();
        assert!(fit.ends_with('…'));
        assert_eq!(lines(&fit), 2);
        assert!(t.starts_with(fit.trim_end_matches('…')));
        // Ends on the whole word before the cut rather than mid-word.
        assert_eq!(fit, "abcdefghij…");
    }

    #[test]
    fn long_words_are_cut_mid_word() {
        let fit = fit_lines("abcdefghijklmnopqrstuvwxyz0123456789", 2, &mut lines).unwrap();
        assert_eq!(fit, "abcdefghijklmnopqrs…");
    }

    #[test]
    fn newlines_count_as_lines() {
        let fit = fit_lines("one\ntwo\nthree", 2, &mut lines).unwrap();
        assert_eq!(fit, "one\ntwo…");
    }
}
