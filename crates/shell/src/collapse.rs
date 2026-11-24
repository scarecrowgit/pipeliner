//! Whether the sidebar is collapsed to the icon rail. The setting lives in the engine's
//! settings (`sidebar_collapsed`); a toggle shows at once and is saved through the hub.

use gpui::{App, Entity, Global};
use hub::OpsModel;

/// A toggle that was saved but has not come back in the settings yet.
#[derive(Default)]
pub(crate) struct Pending(Option<bool>);

impl Global for Pending {}

/// The sidebar is drawn as the rail: the toggle just made, else the saved setting.
pub fn is_collapsed(ops: &OpsModel, cx: &App) -> bool {
    let saved = ops.settings().value().is_some_and(|settings| settings.sidebar_collapsed);
    cx.try_global::<Pending>().and_then(|pending| pending.0).unwrap_or(saved)
}

/// Collapse or expand the sidebar and save it. Before the settings have loaded the change is
/// shown but not saved.
pub fn set_collapsed(collapsed: bool, cx: &mut App) {
    let ops = hub::ops(cx);
    let settings = ops.read(cx).settings().value().cloned();
    cx.set_global(Pending(Some(collapsed)));
    if let Some(settings) = settings
        && settings.sidebar_collapsed != collapsed
    {
        let mut settings = (*settings).clone();
        settings.sidebar_collapsed = collapsed;
        hub::commands(cx).set_settings(settings, cx);
    }
}

/// Flip the sidebar between the full sidebar and the rail (the button and ⌘\).
pub fn toggle(cx: &mut App) {
    let ops = hub::ops(cx);
    let now = is_collapsed(ops.read(cx), cx);
    set_collapsed(!now, cx);
}

/// Forget the pending toggle once the saved settings agree with it.
pub(crate) fn settle(ops: &Entity<OpsModel>, cx: &mut App) {
    let pending = cx.try_global::<Pending>().and_then(|pending| pending.0);
    let saved = ops.read(cx).settings().value().map(|settings| settings.sidebar_collapsed);
    if pending.is_some() && pending == saved {
        cx.set_global(Pending(None));
    }
}
