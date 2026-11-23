//! A [`RichText`] shaped into visual lines: every run in its own font and size (prose at 13,
//! inline code at 12 mono), padded boxes behind code and mentions, our own line breaking,
//! and the geometry selection and links need (offset under the mouse, selection rects).

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, Point, ShapedLine, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, point, px, size,
};
use ui::theme::{
    ACCENT_SOFT, ACCENT_TEXT, FONT_MONO, FONT_SANS, MONO, RADIUS_SM, SURFACE_HOVER, TEXT,
    TypeStyle, sans_features,
};

use super::rich::{RichText, SpanStyle};
use super::wrap::wrap;
use crate::text::segment as seg;

/// Side padding of an inline code box (`.code`: `padding: 1px 5px`).
const CODE_PAD: f32 = 5.;
/// Side padding of a mention box (`.mention`: `padding: 0 3px`).
const MENTION_PAD: f32 = 3.;
/// Height of an inline box: the 12px content area plus its 1px padding.
const BOX_HEIGHT: f32 = 18.;

/// How a unit's text looks and lays out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub size: f32,
    pub line_height: f32,
    pub weight: FontWeight,
    pub color: Hsla,
    /// Everything in the mono font (code blocks).
    pub mono: bool,
    /// Break lines at the available width.
    pub wrap: bool,
    /// Take the parent's full width rather than the text's.
    pub fill: bool,
    /// Never wider than this, even with more room.
    pub max_width: Option<f32>,
}

impl Look {
    /// Wrapping, full-width text in the type style `t`.
    pub fn text(t: TypeStyle, color: Hsla) -> Self {
        Self {
            size: t.size,
            line_height: t.line,
            weight: t.weight,
            color,
            mono: t.mono,
            wrap: true,
            fill: true,
            max_width: None,
        }
    }
}

/// One run of a logical line shaped as a whole: a padded box (code, mention) or the text
/// between boxes.
struct Atom {
    range: Range<usize>,
    /// x of the atom's left edge (its box, when padded) in the unwrapped line.
    x: f32,
    pad_l: f32,
    pad_r: f32,
    shaped: ShapedLine,
    /// Glyph start offsets (relative to the atom) and their x.
    glyphs: Vec<(usize, f32)>,
    size: f32,
    boxed: Option<Hsla>,
}

impl Atom {
    fn inner_x(&self, rel: usize) -> f32 {
        let i = self.glyphs.partition_point(|(g, _)| *g < rel);
        self.glyphs
            .get(i)
            .map_or(f32::from(self.shaped.width), |g| g.1)
    }

    fn right(&self) -> f32 {
        self.x + self.pad_l + f32::from(self.shaped.width) + self.pad_r
    }
}

/// Part of a visual line shaped on its own.
struct Piece {
    range: Range<usize>,
    /// x where the text starts, relative to the line.
    x: f32,
    /// Vertical shift that puts this piece on the line's baseline.
    dy: f32,
    shaped: ShapedLine,
    glyphs: Vec<(usize, f32)>,
    /// The box behind the piece: left, right, colour.
    bg: Option<(f32, f32, Hsla)>,
}

impl Piece {
    fn width(&self) -> f32 {
        f32::from(self.shaped.width)
    }

    fn x_of(&self, rel: usize) -> f32 {
        let i = self.glyphs.partition_point(|(g, _)| *g < rel);
        self.x + self.glyphs.get(i).map_or(self.width(), |g| g.1)
    }

    /// The glyph boundary nearest to `x` (relative to the line).
    fn closest(&self, x: f32) -> usize {
        let x = x - self.x;
        let mut prev: Option<(usize, f32)> = None;
        for &(i, gx) in &self.glyphs {
            if let Some((pi, px_)) = prev
                && x < (px_ + gx) / 2.
            {
                return self.range.start + pi;
            }
            prev = Some((i, gx));
        }
        match prev {
            Some((pi, px_)) if x < (px_ + self.width()) / 2. => self.range.start + pi,
            _ => self.range.end,
        }
    }

    /// The glyph under `x` (relative to the line).
    fn under(&self, x: f32) -> usize {
        let x = x - self.x;
        let i = self.glyphs.partition_point(|(_, gx)| *gx <= x);
        self.range.start + self.glyphs.get(i.saturating_sub(1)).map_or(0, |g| g.0)
    }
}

struct Line {
    range: Range<usize>,
    /// The logical line goes on (a wrap, not a `\n`, ends this one).
    soft: bool,
    pieces: Vec<Piece>,
    width: f32,
}

/// Shaped, wrapped rich text with its geometry.
pub(crate) struct InlineLayout {
    pub rich: Rc<RichText>,
    lines: Vec<Line>,
    line_height: f32,
    width: f32,
}

impl InlineLayout {
    /// Shape `rich` with `look`, breaking lines at `wrap_width` when given and underlining
    /// the `hover` range (a hovered link).
    pub fn build(
        rich: Rc<RichText>,
        look: &Look,
        hover: Option<Range<usize>>,
        wrap_width: Option<f32>,
        window: &Window,
    ) -> Self {
        let mut lines = Vec::new();
        let mut ls = 0;
        let text = rich.text.clone();
        for logical in text.split('\n') {
            let le = ls + logical.len();
            let atoms = atoms(&rich, ls..le, look, hover.as_ref(), window);
            let left = |o: usize| x_left(&atoms, ls + o);
            let right = |o: usize| x_right(&atoms, ls + o);
            let ranges = match wrap_width {
                Some(w) if !logical.is_empty() => wrap(logical, w, left, right),
                _ => std::iter::once(0..logical.len()).collect(),
            };
            let n = ranges.len();
            for (k, r) in ranges.into_iter().enumerate() {
                let g = ls + r.start..ls + r.end;
                lines.push(line(
                    &rich,
                    &atoms,
                    g,
                    k + 1 < n,
                    look,
                    hover.as_ref(),
                    window,
                ));
            }
            ls = le + 1;
        }
        let width = lines.iter().map(|l| l.width).fold(0., f32::max);
        Self {
            rich,
            lines,
            line_height: look.line_height,
            width,
        }
    }

    /// Width of the widest line (without hanging spaces).
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Height of all lines.
    pub fn height(&self) -> f32 {
        self.line_height * self.lines.len().max(1) as f32
    }

    fn line_at(&self, y: f32) -> Option<(usize, &Line)> {
        let ix = if y <= 0. {
            0
        } else {
            (y / self.line_height) as usize
        };
        let ix = ix.min(self.lines.len().checked_sub(1)?);
        Some((ix, &self.lines[ix]))
    }

    /// The caret offset nearest to `local` (relative to the text's top-left).
    pub fn offset_at(&self, local: Point<Pixels>) -> usize {
        let Some((_, l)) = self.line_at(f32::from(local.y)) else {
            return 0;
        };
        let x = f32::from(local.x);
        let mut at = l.range.end;
        for p in &l.pieces {
            if x < p.x {
                at = p.range.start;
                break;
            }
            if x < p.x + p.width() {
                at = p.closest(x);
                break;
            }
        }
        seg::floor_grapheme(&self.rich.text, at.clamp(l.range.start, l.range.end))
    }

    /// The offset of the character under `local`, if the point is on text.
    pub fn offset_under(&self, local: Point<Pixels>) -> Option<usize> {
        let y = f32::from(local.y);
        if y < 0. || y >= self.height() {
            return None;
        }
        let (_, l) = self.line_at(y)?;
        let x = f32::from(local.x);
        let p = l.pieces.iter().find(|p| x >= p.x && x < p.x + p.width())?;
        Some(p.under(x))
    }

    /// The link (index into the text's links) under `local`.
    pub fn link_under(&self, local: Point<Pixels>) -> Option<usize> {
        self.rich.link_at(self.offset_under(local)?)
    }

    fn x_in_line(&self, l: &Line, o: usize) -> f32 {
        for p in &l.pieces {
            if o < p.range.start {
                return p.bg.map_or(p.x, |b| b.0);
            }
            if o < p.range.end {
                return p.x_of(o - p.range.start);
            }
        }
        l.pieces.last().map_or(0., |p| p.x + p.width())
    }

    /// Rectangles covering `range`, one per visual line, relative to the text's top-left.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        let newline_w = self.line_height * 0.3;
        let last = self.lines.len().saturating_sub(1);
        for (i, l) in self.lines.iter().enumerate() {
            let r = &l.range;
            let before = range.end < r.start || (range.end == r.start && range.start < r.start);
            let after = range.start > r.end || (range.start == r.end && l.soft);
            if before || after {
                continue;
            }
            let x0 = if range.start > r.start {
                self.x_in_line(l, range.start)
            } else {
                0.
            };
            let runs_on = range.end > r.end;
            let x1 = if runs_on {
                l.width.max(self.x_in_line(l, r.end))
                    + if l.soft || i == last { 0. } else { newline_w }
            } else {
                self.x_in_line(l, range.end)
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

    /// Paint the boxes and text with the top-left at `origin`, only lines inside `clip`.
    pub fn paint(
        &self,
        origin: Point<Pixels>,
        clip: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let lh = self.line_height;
        for (i, l) in self.lines.iter().enumerate() {
            let top = origin.y + px(i as f32 * lh);
            if top + px(lh) < clip.top() || top > clip.bottom() {
                continue;
            }
            for p in &l.pieces {
                if let Some((x0, x1, color)) = p.bg {
                    let h = BOX_HEIGHT.min(lh);
                    let b = Bounds::new(
                        point(origin.x + px(x0), top + px((lh - h) / 2.)),
                        size(px(x1 - x0), px(h)),
                    );
                    window.paint_quad(fill(b, color).corner_radii(px(RADIUS_SM)));
                }
            }
            for p in &l.pieces {
                let at = point(origin.x + px(p.x), top + px(p.dy));
                // A paint error means a glyph could not be rasterised; skip the piece.
                let _ = p.shaped.paint(at, px(lh), window, cx);
            }
        }
    }
}

fn glyphs_of(shaped: &ShapedLine) -> Vec<(usize, f32)> {
    let mut g: Vec<(usize, f32)> = shaped
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| (g.index, f32::from(g.position.x))))
        .collect();
    g.sort_by_key(|(i, _)| *i);
    g.dedup_by_key(|(i, _)| *i);
    g
}

fn font_for(style: SpanStyle, look: &Look) -> Font {
    let mono = look.mono || style.code;
    let weight = if style.bold {
        FontWeight(look.weight.0.max(FontWeight::SEMIBOLD.0))
    } else {
        look.weight
    };
    Font {
        family: SharedString::new_static(if mono { FONT_MONO } else { FONT_SANS }),
        features: if mono {
            Default::default()
        } else {
            sans_features(false)
        },
        fallbacks: None,
        weight,
        style: if style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

fn color_for(style: SpanStyle, look: &Look) -> Hsla {
    if style.link || style.mention {
        ACCENT_TEXT
    } else if style.code {
        TEXT
    } else {
        look.color
    }
}

fn size_for(style: SpanStyle, look: &Look) -> f32 {
    if style.code && !look.mono {
        MONO.size
    } else {
        look.size
    }
}

/// Text runs for `range` of `rich`, underlining the part inside `hover`.
fn runs(
    rich: &RichText,
    range: Range<usize>,
    look: &Look,
    hover: Option<&Range<usize>>,
) -> Vec<TextRun> {
    let mut out = Vec::new();
    for s in &rich.spans {
        let a = s.range.start.max(range.start);
        let b = s.range.end.min(range.end);
        if a >= b {
            continue;
        }
        let color = color_for(s.style, look);
        let mut cuts = vec![a, b];
        if let Some(h) = hover {
            cuts.extend([h.start, h.end].into_iter().filter(|c| *c > a && *c < b));
        }
        cuts.sort_unstable();
        for w in cuts.windows(2) {
            if w[0] >= w[1] {
                continue;
            }
            let under = hover.is_some_and(|h| h.start <= w[0] && w[1] <= h.end);
            out.push(TextRun {
                len: w[1] - w[0],
                font: font_for(s.style, look),
                color,
                background_color: None,
                underline: under.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: s.style.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color),
                }),
            });
        }
    }
    out
}

fn shape(
    rich: &RichText,
    range: Range<usize>,
    size: f32,
    look: &Look,
    hover: Option<&Range<usize>>,
    window: &Window,
) -> ShapedLine {
    let text = SharedString::from(rich.text[range.clone()].to_string());
    let runs = runs(rich, range, look, hover);
    window.text_system().shape_line(text, px(size), &runs, None)
}

/// Split the logical line `range` into atoms at every boundary of a padded span.
fn atoms(
    rich: &RichText,
    range: Range<usize>,
    look: &Look,
    hover: Option<&Range<usize>>,
    window: &Window,
) -> Vec<Atom> {
    let mut groups: Vec<(Range<usize>, SpanStyle)> = Vec::new();
    for s in &rich.spans {
        let a = s.range.start.max(range.start);
        let b = s.range.end.min(range.end);
        if a >= b {
            continue;
        }
        let key = |st: SpanStyle| (st.code, st.mention);
        match groups.last_mut() {
            Some((r, st)) if key(*st) == key(s.style) && r.end == a => r.end = b,
            _ => groups.push((a..b, s.style)),
        }
    }
    let mut x = 0.;
    let mut out = Vec::with_capacity(groups.len());
    for (r, st) in groups {
        let (pad, boxed) = match (st.code && !look.mono, st.mention) {
            (true, _) => (CODE_PAD, Some(SURFACE_HOVER)),
            (false, true) => (MENTION_PAD, Some(ACCENT_SOFT)),
            _ => (0., None),
        };
        let size = size_for(st, look);
        let shaped = shape(rich, r.clone(), size, look, hover, window);
        let glyphs = glyphs_of(&shaped);
        let atom = Atom {
            range: r,
            x,
            pad_l: pad,
            pad_r: pad,
            shaped,
            glyphs,
            size,
            boxed,
        };
        x = atom.right();
        out.push(atom);
    }
    out
}

/// x where a line starting at `o` begins.
fn x_left(atoms: &[Atom], o: usize) -> f32 {
    for a in atoms {
        if o == a.range.start {
            return a.x;
        }
        if o < a.range.end {
            return a.x + a.pad_l + a.inner_x(o - a.range.start);
        }
    }
    atoms.last().map_or(0., Atom::right)
}

/// x where a line ending at `o` ends.
fn x_right(atoms: &[Atom], o: usize) -> f32 {
    for a in atoms {
        if o <= a.range.start {
            return a.x;
        }
        if o < a.range.end {
            return a.x + a.pad_l + a.inner_x(o - a.range.start);
        }
        if o == a.range.end {
            return a.right();
        }
    }
    atoms.last().map_or(0., Atom::right)
}

fn line(
    rich: &RichText,
    atoms: &[Atom],
    g: Range<usize>,
    soft: bool,
    look: &Look,
    hover: Option<&Range<usize>>,
    window: &Window,
) -> Line {
    let x0 = x_left(atoms, g.start);
    let lh = look.line_height;
    let baseline = |s: &ShapedLine| {
        let (a, d) = (f32::from(s.ascent), f32::from(s.descent));
        (lh - a - d) / 2. + a
    };
    let mut pieces = Vec::new();
    for a in atoms {
        let s = a.range.start.max(g.start);
        let e = a.range.end.min(g.end);
        if s >= e {
            continue;
        }
        let shaped = if s == a.range.start && e == a.range.end {
            a.shaped.clone()
        } else {
            shape(rich, s..e, a.size, look, hover, window)
        };
        let starts = s == a.range.start;
        let x = if starts {
            a.x + a.pad_l
        } else {
            a.x + a.pad_l + a.inner_x(s - a.range.start)
        } - x0;
        let bg = a.boxed.map(|c| {
            let l = if starts { x - a.pad_l } else { x };
            let r = x + f32::from(shaped.width) + if e == a.range.end { a.pad_r } else { 0. };
            (l, r, c)
        });
        let glyphs = glyphs_of(&shaped);
        pieces.push(Piece {
            range: s..e,
            x,
            dy: 0.,
            shaped,
            glyphs,
            bg,
        });
    }
    // Align every piece's baseline with the prose's (inline code is a smaller size).
    let reference = pieces
        .iter()
        .find(|p| p.bg.is_none())
        .or(pieces.first())
        .map(|p| baseline(&p.shaped));
    if let Some(r) = reference {
        for p in &mut pieces {
            p.dy = r - baseline(&p.shaped);
        }
    }
    let trimmed = g.start + rich.text[g.clone()].trim_end_matches(' ').len();
    let width = (x_right(atoms, trimmed) - x0).max(0.);
    Line {
        range: g,
        soft,
        pieces,
        width,
    }
}
