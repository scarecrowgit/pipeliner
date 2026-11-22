//! Static data for the app sidebar (ported from `reference/app_sidebar_v4.html`).

use ui::nav::Screen;

pub struct NavEntry {
    pub screen: Screen,
    pub label: &'static str,
    /// Rail tooltip.
    pub aria: &'static str,
    pub icon: &'static str,
    pub count: Option<u32>,
}

pub const NAV: &[NavEntry] = &[
    NavEntry {
        screen: Screen::Home,
        label: "Home",
        aria: "Home",
        icon: "runs",
        count: None,
    },
    NavEntry {
        screen: Screen::Inbox,
        label: "Inbox",
        aria: "Inbox · 4 need you",
        icon: "inbox",
        count: Some(4),
    },
    NavEntry {
        screen: Screen::Issues,
        label: "Issues",
        aria: "Issues",
        icon: "check",
        count: None,
    },
    NavEntry {
        screen: Screen::Agents,
        label: "Agents & pipelines",
        aria: "Agents & pipelines",
        icon: "agent",
        count: None,
    },
];

pub const WORKSPACE: &str = "Acme · Payments";
