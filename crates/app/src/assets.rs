//! App assets: our own lucide icon set (`assets/icons/*.svg`, embedded at build time)
//! layered over gpui-component's assets (which serve its `IconName` icons).
//!
//! Use icons through [`AppIcon`]: `Icon::new(AppIcon::Inbox)` or the `ui::icon()` helper.

// Shared API for screens: some tokens / helpers are not used by every build of the screens.
#![allow(dead_code)]

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use gpui_component::IconNamed;

macro_rules! app_icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// Icons shipped with the app (lucide, 24px grid, 2px stroke).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum AppIcon { $($variant),* }

        impl AppIcon {
            pub fn path(self) -> &'static str {
                match self { $(AppIcon::$variant => concat!("app-icons/", $file, ".svg")),* }
            }
        }

        fn app_icon_bytes(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("app-icons/", $file, ".svg") =>
                    Some(include_bytes!(concat!("../assets/icons/", $file, ".svg")).as_slice()),)*
                _ => None,
            }
        }

        const APP_ICON_PATHS: &[&str] = &[$(concat!("app-icons/", $file, ".svg")),*];
    };
}

app_icons! {
    Activity => "activity",
    Alert => "alert",
    ArrowLeft => "arrow-left",
    Board => "board",
    Book => "book",
    Bot => "bot",
    Chat => "chat",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    CircleCheck => "circle-check",
    CircleX => "circle-x",
    Clock => "clock",
    Copy => "copy",
    Download => "download",
    Edit => "edit",
    Ellipsis => "ellipsis",
    Enter => "enter",
    ExternalLink => "external-link",
    File => "file",
    Filter => "filter",
    Folder => "folder",
    GitBranch => "git-branch",
    GitFork => "git-fork",
    GitPullRequest => "git-pull-request",
    Home => "home",
    Image => "image",
    Inbox => "inbox",
    Info => "info",
    Loader => "loader",
    Lock => "lock",
    Pause => "pause",
    Play => "play",
    Plus => "plus",
    RotateCcw => "rotate-ccw",
    Scan => "scan",
    Search => "search",
    Send => "send",
    Shield => "shield",
    Sliders => "sliders",
    Sparkles => "sparkles",
    Terminal => "terminal",
    Timeline => "timeline",
    Trash => "trash",
    User => "user",
    Workflow => "workflow",
    X => "x",
    Zap => "zap",
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        AppIcon::path(self).into()
    }
}

/// Serves `app-icons/*` from the binary and everything else from gpui-component.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = app_icon_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = APP_ICON_PATHS
            .iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(*p))
            .collect();
        out.extend(gpui_component_assets::Assets.list(path)?);
        Ok(out)
    }
}
