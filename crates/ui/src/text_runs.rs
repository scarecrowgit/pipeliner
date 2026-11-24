//! Text runs for `shape_line`, made safe from a GPUI 0.2.2 merge bug.
//!
//! `TextSystem::layout_line` (behind `shape_line`) folds a run into the previous font run
//! whenever its colour, underline and strikethrough equal the previous run's, without
//! comparing the fonts. A bold label followed by regular text of the same colour is then
//! shaped bold to the end of the line (or the regular text bold, or mono text sans).
//! `shape_text` (wrapped text, the editors) compares the fonts and is not affected.
//!
//! [`separate_font_runs`] nudges the colour of such a run by an invisible amount so the
//! decorations differ and GPUI keeps the font change. Call it on every run list passed to
//! `shape_line` that can change font without changing colour.

use gpui::{Hsla, TextRun};

/// How much the alpha of a run's colour moves to keep its font apart: below what a screen
/// shows (1/255 is one step of an 8-bit channel).
const NUDGE: f32 = 0.002;

/// Make every font change in `runs` survive `shape_line`: a run whose font differs from the
/// run before it, with the same colour, underline and strikethrough, gets its colour's alpha
/// moved by an invisible amount.
pub fn separate_font_runs(runs: &mut [TextRun]) {
    for i in 1..runs.len() {
        let (before, rest) = runs.split_at_mut(i);
        let (prev, run) = (&before[i - 1], &mut rest[0]);
        if run.font != prev.font && same_decoration(prev, run) {
            run.color = nudged(prev.color);
        }
    }
}

fn same_decoration(a: &TextRun, b: &TextRun) -> bool {
    a.color == b.color && a.underline == b.underline && a.strikethrough == b.strikethrough
}

fn nudged(c: Hsla) -> Hsla {
    let a = if c.a > 0.5 { c.a - NUDGE } else { c.a + NUDGE };
    Hsla { a, ..c }
}

/// GPUI 0.2.2's merge in `TextSystem::layout_line`, for tests (GPUI's test platform gives
/// every font the same id, so a window cannot show the bug): the fonts and lengths of the
/// font runs it shapes `runs` with.
pub fn font_runs_as_shaped(runs: &[TextRun]) -> Vec<(gpui::Font, usize)> {
    let mut out: Vec<(gpui::Font, usize)> = Vec::new();
    let mut last: Option<&TextRun> = None;
    for run in runs {
        let changed = match last {
            Some(l) if same_decoration(l, run) => false,
            _ => {
                last = Some(run);
                true
            }
        };
        match out.last_mut() {
            Some((_, len)) if !changed => *len += run.len,
            _ => out.push((run.font.clone(), run.len)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use gpui::{FontWeight, font, hsla};

    use super::*;

    fn run(len: usize, weight: FontWeight) -> TextRun {
        TextRun {
            len,
            font: gpui::Font { weight, ..font("Inter") },
            color: hsla(0., 0., 1., 1.),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn gpui_folds_a_font_change_of_the_same_colour_and_the_nudge_keeps_it() {
        let runs = vec![run(21, FontWeight::SEMIBOLD), run(40, FontWeight::NORMAL), run(5, FontWeight::SEMIBOLD)];
        let shaped = font_runs_as_shaped(&runs);
        assert_eq!(shaped.len(), 1, "the bug: one bold run for the whole line");
        let mut fixed = runs.clone();
        separate_font_runs(&mut fixed);
        let shaped = font_runs_as_shaped(&fixed);
        let got: Vec<(FontWeight, usize)> = shaped.iter().map(|(f, n)| (f.weight, *n)).collect();
        assert_eq!(got, [(FontWeight::SEMIBOLD, 21), (FontWeight::NORMAL, 40), (FontWeight::SEMIBOLD, 5)]);
        assert!(fixed.iter().all(|r| (r.color.a - 1.).abs() < 0.01), "the nudge is invisible");
    }

    #[test]
    fn runs_that_differ_already_are_left_alone() {
        let mut runs = vec![run(3, FontWeight::NORMAL), run(3, FontWeight::NORMAL)];
        runs[1].color = hsla(0.6, 1., 0.5, 1.);
        let before = runs.clone();
        separate_font_runs(&mut runs);
        assert_eq!(runs, before);
    }
}
