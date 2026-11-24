//! Texts of the panel that are not Markdown but must be selectable and copyable: a run's
//! error, an agent's question, a step's summary, the sentences of the activity feed. Each is a
//! verbatim [`Markdown`] view made once per key and updated when its text changes; views no
//! frame asked for are dropped after it.

use std::collections::{HashMap, HashSet};

use gpui::{App, AppContext as _, Entity, Hsla, SharedString};
use ui::theme::TypeStyle;
use ui_rich::markdown::Markdown;

/// One view and the colour it was last given.
struct Kept {
    view: Entity<Markdown>,
    color: Hsla,
}

/// The selectable texts of one panel, by key.
#[derive(Default)]
pub(super) struct Texts {
    kept: HashMap<SharedString, Kept>,
    used: HashSet<SharedString>,
}

impl Texts {
    /// Starts a frame: every view not asked for until [`end`](Self::end) is dropped then.
    pub(super) fn begin(&mut self) {
        self.used.clear();
    }

    /// Ends a frame: drops the views it did not ask for.
    pub(super) fn end(&mut self) {
        let used = &self.used;
        self.kept.retain(|key, _| used.contains(key));
    }

    /// The view of `key` showing `text` in `style` and `color`. The style is fixed when the
    /// view is made: a key always names the same kind of text.
    pub(super) fn view(
        &mut self,
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
        style: TypeStyle,
        color: Hsla,
        cx: &mut App,
    ) -> Entity<Markdown> {
        let key = key.into();
        let text = text.into();
        self.used.insert(key.clone());
        if let Some(kept) = self.kept.get_mut(&key) {
            let recolor = kept.color != color;
            kept.color = color;
            kept.view.update(cx, |md, cx| {
                md.set_text(text, cx);
                if recolor {
                    md.set_color(color, cx);
                }
            });
            return kept.view.clone();
        }
        let view = cx.new(|cx| Markdown::verbatim(text, cx).type_style(style).color(color));
        self.kept.insert(key, Kept { view: view.clone(), color });
        view
    }

    /// The view of `key` if the last frame asked for it, for tests.
    #[cfg(test)]
    pub(super) fn get(&self, key: &str) -> Option<Entity<Markdown>> {
        self.kept.get(key).map(|kept| kept.view.clone())
    }
}
