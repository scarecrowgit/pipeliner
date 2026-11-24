//! Whether the sidebar is collapsed to the icon rail. The setting lives in the engine's
//! settings (`sidebar_collapsed`); a toggle shows at once and is saved through the hub.
//!
//! A visit to the pipeline editor in a narrow window starts collapsed without touching the
//! setting: toggles during that visit change only the visit, and leaving the editor brings
//! back the person's own setting.

use gpui::{App, Entity, Global};
use hub::OpsModel;

/// Collapse state that is not (yet) in the saved settings.
#[derive(Default)]
pub(crate) struct Pending {
    /// A toggle that was saved but has not come back in the settings yet.
    saved: Option<bool>,
    /// The state for the current editor visit, when one overrides the setting.
    visit: Option<bool>,
}

impl Global for Pending {}

fn pending(cx: &App) -> (Option<bool>, Option<bool>) {
    cx.try_global::<Pending>().map_or((None, None), |p| (p.saved, p.visit))
}

/// The sidebar as the person set it, ignoring an editor visit.
fn own_setting(ops: &OpsModel, cx: &App) -> bool {
    let saved = ops.settings().value().is_some_and(|settings| settings.sidebar_collapsed);
    pending(cx).0.unwrap_or(saved)
}

/// The sidebar is drawn as the rail: an editor visit's state, else the toggle just made,
/// else the saved setting.
pub fn is_collapsed(ops: &OpsModel, cx: &App) -> bool {
    pending(cx).1.unwrap_or_else(|| own_setting(ops, cx))
}

/// Collapse or expand the sidebar. During an editor visit this changes only the visit;
/// otherwise it is saved (before the settings have loaded it is shown but not saved).
pub fn set_collapsed(collapsed: bool, cx: &mut App) {
    let (saved, visit) = pending(cx);
    if visit.is_some() {
        cx.set_global(Pending { saved, visit: Some(collapsed) });
        return;
    }
    let ops = hub::ops(cx);
    let settings = ops.read(cx).settings().value().cloned();
    cx.set_global(Pending { saved: Some(collapsed), visit: None });
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

/// Start an editor visit drawn as the rail, unless the person's own setting already is.
pub(crate) fn begin_visit(ops: &Entity<OpsModel>, cx: &mut App) {
    if own_setting(ops.read(cx), cx) {
        return;
    }
    let (saved, _) = pending(cx);
    cx.set_global(Pending { saved, visit: Some(true) });
}

/// End the editor visit: the person's own setting is drawn again.
pub(crate) fn end_visit(cx: &mut App) {
    let (saved, visit) = pending(cx);
    if visit.is_some() {
        cx.set_global(Pending { saved, visit: None });
    }
}

/// Forget the pending toggle once the saved settings agree with it.
pub(crate) fn settle(ops: &Entity<OpsModel>, cx: &mut App) {
    let (saved_pending, visit) = pending(cx);
    let saved = ops.read(cx).settings().value().map(|settings| settings.sidebar_collapsed);
    if saved_pending.is_some() && saved_pending == saved {
        cx.set_global(Pending { saved: None, visit });
    }
}
