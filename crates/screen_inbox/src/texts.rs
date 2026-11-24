//! Selectable content text of the detail: one [`Markdown`] view for each piece of text that
//! came from an agent, a tool, an error or a person, kept while the same item or review is
//! open so a selection survives redraws and can be copied.

use std::collections::HashMap;

use gpui::{AnyElement, App, AppContext as _, Entity, Focusable as _, Hsla, SharedString, Window};
use ui::prelude::*;
use ui_rich::markdown::Markdown;

/// How a text is shown: Markdown written by a model, or exactly as given; its type style and
/// body colour.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Look {
    pub(crate) markdown: bool,
    pub(crate) style: TypeStyle,
    pub(crate) color: Hsla,
}

impl Look {
    /// Text shown exactly as given (an error, a path, a title).
    pub(crate) const fn plain(style: TypeStyle, color: Hsla) -> Self {
        Self {
            markdown: false,
            style,
            color,
        }
    }

    /// Markdown written by a model (a summary, a question).
    pub(crate) const fn markdown(style: TypeStyle, color: Hsla) -> Self {
        Self {
            markdown: true,
            style,
            color,
        }
    }
}

/// The text views of what is open, by key ("title", "pr-0-title"…).
#[derive(Default)]
pub(crate) struct Texts {
    scope: Option<SharedString>,
    views: HashMap<SharedString, (bool, Entity<Markdown>)>,
}

impl Texts {
    /// Shows what `scope` names (an item or a review): the views of another scope go.
    /// Returns whether one of the dropped views had keyboard focus, which the caller then
    /// takes back.
    pub(crate) fn scope(
        &mut self,
        scope: impl Into<SharedString>,
        window: &Window,
        cx: &App,
    ) -> bool {
        let scope = scope.into();
        if self.scope.as_ref() == Some(&scope) {
            return false;
        }
        let focused = self.focused(window, cx);
        self.views.clear();
        self.scope = Some(scope);
        focused
    }

    /// The view of `text` under `key`: made on first use, brought up to date after.
    pub(crate) fn get(
        &mut self,
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
        look: Look,
        cx: &mut App,
    ) -> Entity<Markdown> {
        let key = key.into();
        let text = text.into();
        if let Some((markdown, view)) = self.views.get(&key)
            && *markdown == look.markdown
        {
            let view = view.clone();
            view.update(cx, |md, cx| {
                md.set_text(text, cx);
                md.set_type_style(look.style, cx);
                md.set_color(look.color, cx);
            });
            return view;
        }
        let view = cx.new(|cx| {
            let md = if look.markdown {
                Markdown::new(text, cx).on_link(|url, _, cx| cx.open_url(url))
            } else {
                Markdown::verbatim(text, cx)
            };
            md.type_style(look.style).color(look.color)
        });
        self.views.insert(key, (look.markdown, view.clone()));
        view
    }

    /// One of the views has keyboard focus: a press on its text selects and Cmd/Ctrl+C copies.
    pub(crate) fn focused(&self, window: &Window, cx: &App) -> bool {
        self.views
            .values()
            .any(|(_, view)| view.focus_handle(cx).is_focused(window))
    }

    /// The view under `key`, for tests.
    #[cfg(test)]
    pub(crate) fn view(&self, key: &str) -> Option<Entity<Markdown>> {
        self.views.get(key).map(|(_, view)| view.clone())
    }
}

/// A text view in a box that gives it its width, named `selector` for tests: the view wraps
/// to the box. In a row the box is `flex_1().min_w_0()`; in a column it stretches.
pub(crate) fn cell(selector: &'static str, view: Entity<Markdown>) -> gpui::Div {
    gpui::div()
        .flex_1()
        .min_w_0()
        .debug_selector(move || selector.into())
        .child(view)
}

/// [`cell`] as an element of a column (stretched, not grown).
pub(crate) fn block(selector: &'static str, view: Entity<Markdown>) -> AnyElement {
    gpui::div()
        .w_full()
        .min_w_0()
        .debug_selector(move || selector.into())
        .child(view)
        .into_any_element()
}
