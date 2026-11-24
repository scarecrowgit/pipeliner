//! Whether the sidebar is collapsed to the icon rail. The setting lives in the engine's
//! settings (`sidebar_collapsed`); a toggle shows at once and is saved through the hub.
//! Nothing but the person's own toggle changes it.

use gpui::{App, Entity, Global};
use hub::OpsModel;

/// A toggle that was saved but has not come back in the settings yet.
#[derive(Default)]
pub(crate) struct Pending(Option<bool>);

impl Global for Pending {}

fn pending(cx: &App) -> Option<bool> {
    cx.try_global::<Pending>().and_then(|p| p.0)
}

/// The sidebar is drawn as the rail: the toggle just made, else the saved setting.
pub fn is_collapsed(ops: &OpsModel, cx: &App) -> bool {
    let saved = ops.settings().value().is_some_and(|settings| settings.sidebar_collapsed);
    pending(cx).unwrap_or(saved)
}

/// Collapse or expand the sidebar and save it (before the settings have loaded it is shown
/// but not saved).
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
    let saved = ops.read(cx).settings().value().map(|settings| settings.sidebar_collapsed);
    let waiting = pending(cx);
    if waiting.is_some() && waiting == saved {
        cx.set_global(Pending(None));
    }
}
