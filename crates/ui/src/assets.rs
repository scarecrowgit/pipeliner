//! Icons embedded from `assets/icons` and the [`icon`] element that draws them.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, SharedString, Styled, Svg, px, svg};
use rust_embed::RustEmbed;

/// The embedded icon files, addressed as `icons/<name>.svg`. Pass it to
/// `Application::with_assets`; [`crate::run_app`] already does.
#[derive(RustEmbed)]
#[folder = "../../assets/icons"]
#[prefix = "icons/"]
#[include = "*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Self::iter().filter(|p| p.starts_with(path)).map(|p| p.to_string().into()).collect())
    }
}

macro_rules! icon_names {
    ($($variant:ident => $file:literal,)*) => {
        /// Every icon in `assets/icons`. A name that is not here does not compile, and a
        /// test keeps this list and the folder in step.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $(
                #[doc = concat!("`", $file, "`")]
                $variant,
            )*
        }

        impl IconName {
            /// All icons, in file name order.
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)*];

            /// File stem of the icon, e.g. `chevron-down`.
            pub const fn name(self) -> &'static str {
                match self { $(IconName::$variant => $file,)* }
            }

            /// The icon called `name`, for names that come from data.
            pub fn from_name(name: &str) -> Option<IconName> {
                match name { $($file => Some(IconName::$variant),)* _ => None }
            }
        }
    };
}

icon_names! {
    Agent => "agent",
    Alert => "alert",
    Archive => "archive",
    ArrowDown => "arrow-down",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    Branch => "branch",
    Chat => "chat",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    Clock => "clock",
    Cloud => "cloud",
    Copy => "copy",
    Download => "download",
    Edit => "edit",
    External => "external",
    File => "file",
    Filter => "filter",
    Folder => "folder",
    Fork => "fork",
    Home => "home",
    Image => "image",
    Inbox => "inbox",
    Issues => "issues",
    Laptop => "laptop",
    Lock => "lock",
    Mark => "mark",
    Minus => "minus",
    More => "more",
    Orchestrator => "orchestrator",
    Panel => "panel",
    Paperclip => "paperclip",
    Pause => "pause",
    Pin => "pin",
    Pipelines => "pipelines",
    Play => "play",
    Plus => "plus",
    Policy => "policy",
    Redo => "redo",
    Replay => "replay",
    Router => "router",
    Runs => "runs",
    Search => "search",
    Server => "server",
    Settings => "settings",
    Share => "share",
    Source => "source",
    Sparkles => "sparkles",
    Spawn => "spawn",
    Stop => "stop",
    Terminal => "terminal",
    Trash => "trash",
    Undo => "undo",
    Upload => "upload",
    Users => "users",
    X => "x",
    Zap => "zap",
}

impl IconName {
    /// Asset path of the icon file.
    pub fn path(self) -> SharedString {
        SharedString::from(format!("icons/{}.svg", self.name()))
    }
}

/// A stroke icon tinted with `color`, `size` px square. Icons are 24-grid outlines drawn as an
/// alpha mask: 16 in nav, 14 in buttons and card heads, 12 in chips.
pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg().path(name.path()).size(px(size)).flex_none().text_color(color)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_listed_icon_has_a_file_and_every_file_is_listed() {
        let listed: BTreeSet<String> = IconName::ALL.iter().map(|n| n.path().to_string()).collect();
        assert_eq!(listed.len(), IconName::ALL.len(), "duplicate icon names");
        let files: BTreeSet<String> = Assets::iter().map(|p| p.to_string()).collect();
        for path in &listed {
            assert!(files.contains(path), "{path} is in IconName but has no file");
        }
        for path in &files {
            assert!(listed.contains(path), "{path} is a file but not in IconName");
        }
    }

    #[test]
    fn names_round_trip() {
        for n in IconName::ALL {
            assert_eq!(IconName::from_name(n.name()), Some(*n));
        }
        assert_eq!(IconName::from_name("no-such-icon"), None);
    }

    #[test]
    fn the_asset_source_serves_icons() {
        let data = Assets.load("icons/check.svg").unwrap().expect("check.svg is embedded");
        assert!(data.starts_with(b"<svg"));
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
