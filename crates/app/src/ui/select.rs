//! Dropdown select (gpui-component `Select`) over a list of `(value, label)` options.
//!
//! ```ignore
//! // new():
//! let model = ui::select::select_state(
//!     engine.models().into_iter().map(|m| SelectOption::new(m.clone(), Config::short_model(&m))).collect(),
//!     Some(&chat.model), true, window, cx);
//! let sub = cx.subscribe_in(&model, window, |this, _, ev: &ui::select::SelectEvent<ui::select::Options>, window, cx| {
//!     if let ui::select::SelectEvent::Confirm(Some(value)) = ev { /* value: String */ }
//! });
//! // render():
//! .child(ui::select::select(&self.model).w(px(220.)))
//! ```

use gpui::{px, App, AppContext as _, Entity, SharedString, Styled as _, Window};
use gpui_component::select::{SearchableVec, Select, SelectItem, SelectState};
use gpui_component::IndexPath;

use crate::theme;

pub use gpui_component::select::SelectEvent;

/// One option: the value you get back and the label shown.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: SharedString,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<SharedString>) -> Self {
        SelectOption {
            value: value.into(),
            label: label.into(),
        }
    }
    /// Value and label are the same text.
    pub fn plain(v: impl Into<String>) -> Self {
        let v = v.into();
        SelectOption {
            label: v.clone().into(),
            value: v,
        }
    }
}

impl SelectItem for SelectOption {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &String {
        &self.value
    }
}

/// The delegate type used by these helpers (for `SelectEvent<Options>`).
pub type Options = SearchableVec<SelectOption>;

/// Creates the select state. `selected` is matched against option values.
pub fn select_state(
    options: Vec<SelectOption>,
    selected: Option<&str>,
    searchable: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SelectState<Options>> {
    let ix = selected
        .and_then(|v| options.iter().position(|o| o.value == v))
        .map(IndexPath::new);
    cx.new(|cx| {
        SelectState::new(SearchableVec::new(options), ix, window, cx).searchable(searchable)
    })
}

/// Replaces the options (e.g. after the store changed), keeping `selected` if present.
pub fn set_options(
    state: &Entity<SelectState<Options>>,
    options: Vec<SelectOption>,
    selected: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    let ix = selected
        .and_then(|v| options.iter().position(|o| o.value == v))
        .map(IndexPath::new);
    state.update(cx, |s, cx| {
        s.set_items(SearchableVec::new(options), window, cx);
        s.set_selected_index(ix, window, cx);
    });
}

/// Renders a 40px select styled like [`super::input::field`].
pub fn select(state: &Entity<SelectState<Options>>) -> Select<Options> {
    Select::new(state)
        .h(theme::CONTROL_H)
        .text_size(px(13.))
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
}

/// 28px variant for toolbars and filters.
pub fn select_sm(state: &Entity<SelectState<Options>>) -> Select<Options> {
    Select::new(state)
        .h(theme::CONTROL_H_SM)
        .text_size(px(12.))
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_MD)
}
