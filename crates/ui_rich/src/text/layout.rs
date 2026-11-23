//! Shaped, soft-wrapped text and its geometry: which visual line holds an offset, where the
//! caret goes, which offset is under the mouse, the selection rectangles.

use std::ops::Range;

use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, SharedString, TextAlign, TextRun, UnderlineStyle,
    Window, WrappedLine, point, px, size,
};

use super::buffer::VisualLines;
use super::segment as seg;

/// One logical line (between `\n`s) after shaping.
struct Logical {
    line: WrappedLine,
    /// Byte offset of the line in the whole text.
    start: usize,
    /// Index of its first visual line.
    first_visual: usize,
}

/// One visual line: a logical line, or the part of it between two soft wraps.
#[derive(Clone, Debug)]
struct Visual {
    logical: usize,
    /// Byte range in the whole text, without the line break.
    range: Range<usize>,
    /// x of `range.start` in the unwrapped logical line.
    x0: f32,
    /// The logical line continues on the next visual line (a soft wrap ends this one).
    soft: bool,
}

/// What the text looks like: font, size, line height, colours.
#[derive(Clone)]
pub(crate) struct TextLook {
    pub font: Font,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub color: Hsla,
}

/// Shaped text, ready to paint and to answer geometry questions.
pub(crate) struct Shaped {
    text: SharedString,
    logical: Vec<Logical>,
    visual: Vec<Visual>,
    line_height: f32,
}

impl Shaped {
    /// Shape `text` with `look`, wrapping at `wrap_width` when given, underlining `marked`.
    pub fn new(
        text: SharedString,
        look: &TextLook,
        marked: Option<Range<usize>>,
        wrap_width: Option<Pixels>,
        window: &Window,
    ) -> Self {
        let runs = runs_for(&text, look, marked);
        let lines = window
            .text_system()
            .shape_text(text.clone(), look.font_size, &runs, wrap_width, None)
            .map(|v| v.into_vec())
            .unwrap_or_default();
        Self::from_lines(text, lines, f32::from(look.line_height))
    }

    fn from_lines(text: SharedString, lines: Vec<WrappedLine>, line_height: f32) -> Self {
        let mut logical = Vec::with_capacity(lines.len());
        let mut visual = Vec::new();
        let mut start = 0;
        for (ix, line) in lines.into_iter().enumerate() {
            let len = line.len();
            let layout = &line.unwrapped_layout;
            let mut starts = vec![0];
            for b in line.wrap_boundaries.iter() {
                let glyph = layout
                    .runs
                    .get(b.run_ix)
                    .and_then(|r| r.glyphs.get(b.glyph_ix));
                if let Some(g) = glyph
                    && g.index > *starts.last().unwrap_or(&0)
                    && g.index <= len
                {
                    starts.push(g.index);
                }
            }
            let first_visual = visual.len();
            for (k, &s) in starts.iter().enumerate() {
                let e = starts.get(k + 1).copied().unwrap_or(len);
                visual.push(Visual {
                    logical: ix,
                    range: start + s..start + e,
                    x0: f32::from(layout.x_for_index(s)),
                    soft: k + 1 < starts.len(),
                });
            }
            logical.push(Logical {
                line,
                start,
                first_visual,
            });
            start += len + 1;
        }
        if visual.is_empty() {
            visual.push(Visual {
                logical: 0,
                range: 0..0,
                x0: 0.,
                soft: false,
            });
        }
        Self {
            text,
            logical,
            visual,
            line_height,
        }
    }

    /// Line height in pixels.
    pub fn line_height(&self) -> Pixels {
        px(self.line_height)
    }

    /// Number of visual lines.
    pub fn rows(&self) -> usize {
        self.visual.len()
    }

    /// Height of all visual lines.
    pub fn height(&self) -> Pixels {
        px(self.line_height * self.visual.len() as f32)
    }

    /// Width of the widest visual line.
    pub fn width(&self) -> Pixels {
        let w = (0..self.visual.len())
            .map(|i| self.line_width(i))
            .fold(0., f32::max);
        px(w)
    }

    fn line_width(&self, line: usize) -> f32 {
        let Some(v) = self.visual.get(line) else {
            return 0.;
        };
        self.x_in_logical(v.logical, v.range.end) - v.x0
    }

    fn x_in_logical(&self, logical: usize, offset: usize) -> f32 {
        let Some(l) = self.logical.get(logical) else {
            return 0.;
        };
        let rel = offset.saturating_sub(l.start).min(l.line.len());
        f32::from(l.line.unwrapped_layout.x_for_index(rel))
    }

    /// Top-left of the caret at `offset`, relative to the text's origin.
    pub fn caret_position(&self, offset: usize) -> Point<Pixels> {
        let line = self.line_for_offset(offset);
        point(
            px(self.x_for_offset(offset)),
            px(line as f32 * self.line_height),
        )
    }

    /// The caret offset closest to `local` (relative to the text's origin).
    pub fn offset_at(&self, local: Point<Pixels>) -> usize {
        let y = f32::from(local.y) / self.line_height;
        let line = if y < 0. {
            0
        } else {
            (y as usize).min(self.visual.len() - 1)
        };
        self.offset_for_x(line, f32::from(local.x))
    }

    /// The offset of the grapheme under `local`, rather than the nearest caret position: for
    /// a double click on the last letter of a word.
    pub fn offset_under(&self, local: Point<Pixels>) -> usize {
        let o = self.offset_at(local);
        let line = self.line_for_offset(o);
        let start = self.visual[line].range.start;
        if o > start && self.x_for_offset(o) > f32::from(local.x) {
            seg::prev_grapheme(&self.text, o)
        } else {
            o
        }
    }

    /// Rectangles covering `range`, relative to the text's origin: one per visual line, a
    /// little wider than the text where the selection runs on past a line break.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        let newline_w = self.line_height * 0.3;
        for (i, v) in self.visual.iter().enumerate() {
            let before = range.end < v.range.start
                || (range.end == v.range.start && range.start < v.range.start);
            let after = range.start > v.range.end || (range.start == v.range.end && v.soft);
            if before || after {
                continue;
            }
            let starts_here = range.start >= v.range.start;
            let x0 = if starts_here {
                self.x_in_logical(v.logical, range.start) - v.x0
            } else {
                0.
            };
            let runs_on = range.end > v.range.end;
            let x1 = if runs_on {
                self.line_width(i) + if v.soft { 0. } else { newline_w }
            } else {
                self.x_in_logical(v.logical, range.end) - v.x0
            };
            if x1 > x0 {
                let y = i as f32 * self.line_height;
                out.push(Bounds::new(
                    point(px(x0), px(y)),
                    size(px(x1 - x0), px(self.line_height)),
                ));
            }
        }
        out
    }

    /// Paint the text with its top-left at `origin`, only the lines inside `clip`.
    pub fn paint(
        &self,
        origin: Point<Pixels>,
        clip: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let lh = self.line_height;
        for (ix, l) in self.logical.iter().enumerate() {
            let rows = self
                .logical
                .get(ix + 1)
                .map_or(self.visual.len(), |n| n.first_visual)
                - l.first_visual;
            let top = origin.y + px(l.first_visual as f32 * lh);
            let bottom = top + px(rows as f32 * lh);
            if bottom < clip.top() || top > clip.bottom() {
                continue;
            }
            // A paint error means a glyph could not be rasterised; skip the line.
            let _ = l.line.paint(
                point(origin.x, top),
                px(lh),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

impl VisualLines for Shaped {
    fn line_count(&self) -> usize {
        self.visual.len()
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.visual
            .partition_point(|v| v.range.start <= offset)
            .saturating_sub(1)
    }

    fn x_for_offset(&self, offset: usize) -> f32 {
        let v = &self.visual[self.line_for_offset(offset)];
        self.x_in_logical(v.logical, offset.min(v.range.end)) - v.x0
    }

    fn offset_for_x(&self, line: usize, x: f32) -> usize {
        let Some(v) = self.visual.get(line) else {
            return self.text.len();
        };
        let Some(l) = self.logical.get(v.logical) else {
            return v.range.start;
        };
        let rel = l
            .line
            .unwrapped_layout
            .closest_index_for_x(px(x.max(0.) + v.x0));
        let mut at = (l.start + rel).clamp(v.range.start, v.range.end);
        if v.soft && at >= v.range.end {
            // The end of a soft-wrapped line is the start of the next; stay on this one.
            at = seg::prev_grapheme(&self.text, v.range.end).max(v.range.start);
        }
        seg::floor_grapheme(&self.text, at)
    }
}

/// One run for the whole text, split to underline the IME composition.
fn runs_for(text: &str, look: &TextLook, marked: Option<Range<usize>>) -> Vec<TextRun> {
    let run = |len: usize, underline: bool| TextRun {
        len,
        font: look.font.clone(),
        color: look.color,
        background_color: None,
        underline: underline.then_some(UnderlineStyle {
            color: Some(look.color),
            thickness: px(1.),
            wavy: false,
        }),
        strikethrough: None,
    };
    let len = text.len();
    match marked {
        Some(m) if m.start < m.end && m.end <= len => [
            run(m.start, false),
            run(m.end - m.start, true),
            run(len - m.end, false),
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        _ => vec![run(len, false)],
    }
}
