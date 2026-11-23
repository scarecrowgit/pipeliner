//! Graphite components: one builder per component of the mockups' CSS, grouped by job.
//!
//! Every component is a `RenderOnce` builder styled only from [`crate::theme`] tokens. They
//! hold no state of their own: the view owns the value (`checked`, `selected`, `value`) and
//! passes it in with a callback for changes. Interactive components take an id
//! (`impl Into<ElementId>`) that must be unique among siblings, and callbacks of the form
//! `Fn(&ClickEvent, &mut Window, &mut App)` or `Fn(value, &mut Window, &mut App)`.
//!
//! ```ignore
//! Button::new("deploy", "Deploy v15").primary().kbd("⌘↵").on_click(cx.listener(|this, _, _, cx| { .. }))
//! Tabs::new("views").tab("Flow").tab_n("Runs", "12").selected(0).on_select(|ix, _, _| { .. })
//! ```

mod actions;
mod choice;
mod containers;
mod controls;
mod fields;
mod identity;
mod lists;
mod status;
mod table;

pub use actions::{Button, IconButton, Link};
pub use choice::{Chip, Segmented, Tabs};
pub use containers::{
    Banner, BannerTone, Card, CardFooter, CardHeader, Divider, KeyValue, Note, Section, Stat,
    StatStrip,
};
pub use controls::{Checkbox, Radio, RadioGroup, SelectBox, Stepper, Toggle};
pub use fields::{Field, InputFrame};
pub use identity::{Avatar, Code, Tile, TileTone};
pub use lists::{ListItem, MenuItem, NavItem, PopLabel, PopSeparator};
pub use status::{
    Badge, BadgeTone, CountBadge, CountTone, KeyHints, Kbd, Progress, ProgressTone, RuleChip,
    SpawnChip, Status, StatusDot, Tag, TagTone,
};
pub use table::{Align, Column, Table, TableRow};
pub use crate::tooltip::Tooltip;
pub(crate) use controls::radio_dot;

use std::rc::Rc;

use gpui::{App, ClickEvent, ElementId, Window};

/// A click callback, shared between the element and its children.
pub(crate) type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A callback with a value (the new state, the chosen index).
pub(crate) type ValueHandler<T> = Rc<dyn Fn(T, &mut Window, &mut App) + 'static>;

/// `"<parent>-<suffix>"`: the id of a part of a component, unique among its siblings.
pub(crate) fn child_id(parent: &ElementId, suffix: impl std::fmt::Display) -> ElementId {
    ElementId::Name(format!("{parent}-{suffix}").into())
}
