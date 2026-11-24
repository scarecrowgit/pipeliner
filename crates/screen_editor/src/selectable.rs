//! Content text a person can select with the mouse and copy (log lines, problems, errors),
//! shown through `ui_rich`'s verbatim [`Markdown`] view, and the measuring the editor's
//! narrow layouts need.
//!
//! A verbatim view is an entity: it has to live in the view that shows it, not be made anew
//! each frame. [`Texts`] keeps one per text a view draws and drops those it stopped drawing.

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, Font, FontStyle, Hsla, SharedString, TextRun, Window, px};
use ui::theme::{FONT_MONO, FONT_SANS, TypeStyle, sans_features};
use ui_rich::markdown::Markdown;

/// The selectable texts one view draws, kept from frame to frame by what they say.
///
/// Call [`begin_frame`](Self::begin_frame) before drawing and [`end_frame`](Self::end_frame)
/// after; [`text`](Self::text) in between hands out the view for a text, the same one as last
/// frame when the text was drawn then too (so a selection survives a redraw).
#[derive(Default)]
pub(crate) struct Texts {
    slots: HashMap<(SharedString, usize), Entity<Markdown>>,
    /// How often each key was asked for this frame: the same text twice gets two views.
    seen: HashMap<SharedString, usize>,
}

impl Texts {
    /// A new frame: nothing asked for yet.
    pub(crate) fn begin_frame(&mut self) {
        self.seen.clear();
    }

    /// Drops the views this frame did not ask for.
    pub(crate) fn end_frame(&mut self) {
        let seen = &self.seen;
        self.slots.retain(|(key, nth), _| seen.get(key).is_some_and(|count| nth < count));
    }

    /// The selectable view of `text` in `style` and `color`. `scope` tells apart the same
    /// words in two places (a problem in the list and in a banner).
    pub(crate) fn text<V: 'static>(
        &mut self,
        scope: &str,
        text: impl Into<SharedString>,
        style: TypeStyle,
        color: Hsla,
        cx: &mut Context<V>,
    ) -> Entity<Markdown> {
        let text: SharedString = text.into();
        let key: SharedString = format!("{scope}\u{0}{text}").into();
        let nth = {
            let count = self.seen.entry(key.clone()).or_default();
            *count += 1;
            *count - 1
        };
        if let Some(view) = self.slots.get(&(key.clone(), nth)) {
            let view = view.clone();
            view.update(cx, |view, cx| {
                view.set_type_style(style, cx);
                view.set_color(color, cx);
            });
            return view;
        }
        let view = cx.new(|cx| Markdown::verbatim(text, cx).type_style(style).color(color));
        self.slots.insert((key, nth), view.clone());
        view
    }
}

/// The views of `views` hold a selection: the press that ends now was a drag over text.
pub(crate) fn selected(views: &[&Entity<Markdown>], cx: &App) -> bool {
    views.iter().any(|view| view.read(cx).selected_text().is_some())
}

/// Width of the widest line of `text` in `style`, in px, as the verbatim view shapes it.
pub(crate) fn text_width(text: &str, style: TypeStyle, window: &Window) -> f32 {
    text.split('\n').map(|line| line_width(line, style, window)).fold(0., f32::max)
}

fn line_width(text: &str, style: TypeStyle, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = Font {
        family: SharedString::new_static(if style.mono { FONT_MONO } else { FONT_SANS }),
        features: if style.mono { Default::default() } else { sans_features(false) },
        fallbacks: None,
        weight: style.weight,
        style: FontStyle::Normal,
    };
    let run = TextRun { len: text.len(), font, color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(SharedString::from(text.to_string()), px(style.size), &[run], None);
    f32::from(line.width).ceil()
}
