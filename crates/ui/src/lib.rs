//! Graphite tokens, typography, components, the single-line input, overlay hosts and motion.
//!
//! This crate is the UI kit every screen builds from. It knows nothing about the domain.
//!
//! - [`theme`]: every token of `tokens.css` as a constant, and the type scale (`BODY`,
//!   `LABEL`…) applied with `.type_style(BODY)`.
//! - [`assets`]: the embedded icons and [`icon`](assets::icon).
//! - [`layout`]: `row`, `col`, `hr`, `vr`, `grow`, `truncate`: the layout traps of this GPUI
//!   version fixed once; [`truncate`](mod@truncate): styled and middle-cut one-line text.
//! - [`motion`]: hover fades, popup presence, reduced motion.
//! - [`components`]: the Graphite components as `RenderOnce` builders.
//! - [`overlay`]: the overlay host at the root of each window and the [`overlays`] handle
//!   that opens [`popover`]s, [`menu`]s, [`dialog`]s and [`toast`]s.
//! - [`keys`]: the kit's key contexts and actions (`Cancel`, `Confirm`, `MoveUp`…), bound
//!   once for every screen; [`focus`]: the keyboard focus ring and tab order.
//! - [`tooltip`]: the one tooltip view.
//!
//! `use ui::prelude::*;` brings the common names in.

pub mod assets;
pub mod components;
pub mod dev;
pub mod dialog;
pub mod focus;
pub mod keys;
pub mod layout;
pub mod menu;
pub mod motion;
pub mod overlay;
pub mod popover;
pub mod text_runs;
pub mod theme;
pub mod toast;
pub mod tooltip;
pub mod truncate;

#[cfg(test)]
mod tests;

use gpui::{
    AnyView, App, Application, Bounds, Context, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};

pub use keys::Quit;
pub use overlay::{OverlayHost, Overlays, overlays};

/// The names most views need, in one import.
pub mod prelude {
    pub use gpui::prelude::*;
    pub use gpui::px;
    pub use crate::assets::{IconName, icon};
    pub use crate::components::*;
    pub use crate::dialog::{ConfirmInput, Dialog, OptionCard, OptionGroup};
    pub use crate::keys;
    pub use crate::layout::{
        col, grow, hr, row, truncate, truncate_block, truncate_lh, truncate_middle, truncate_styled,
        truncate_styled_block, vr,
    };
    pub use crate::menu::Menu;
    pub use crate::motion;
    pub use crate::overlay::{Anchor, Placement, PopoverAnchorExt, overlays};
    pub use crate::popover::Popover;
    pub use crate::toast::Toast;
    pub use crate::theme::*;
    pub use crate::{Preview, root};
}

/// Global setup every window needs: reads the reduced-motion preference once and binds the
/// kit's keys ([`keys::init`]). The text input adds its own bindings here as it lands.
pub fn init(cx: &mut App) {
    motion::init();
    keys::init(cx);
    // Inert unless HARNESS_DEV_SOCKET is set: scripted input for checks in a real window.
    dev::install(cx);
}

/// Base style for a window's root element: bg ground, primary text, Inter 13/20 with the
/// stylistic sets the mockups use (`cv11`, `ss03`).
pub fn root() -> gpui::Div {
    use crate::theme::{BG, BODY, TEXT, TextStyleExt};
    let mut el = div().size_full().bg(BG).text_color(TEXT).type_style(BODY);
    let style = gpui::Styled::text_style(&mut el).get_or_insert_with(Default::default);
    style.font_features = Some(crate::theme::sans_features(false));
    el
}

/// Open a window of `width` × `height` px and run the app until it closes. `build` creates
/// the content view; the window's root is an [`OverlayHost`] around it, so overlays work.
/// For standalone previews of one screen or of the gallery; the window's app id is
/// `harness-demo`.
pub fn run_app<V: Render + 'static>(
    title: &'static str,
    width: f32,
    height: f32,
    build: impl FnOnce(&mut Window, &mut App) -> gpui::Entity<V> + 'static,
) {
    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        init(cx);
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                app_id: Some("harness-demo".into()),
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            },
            move |window, cx| {
                let content = build(window, cx);
                OverlayHost::wrap(content, window, cx)
            },
        );
        if let Err(err) = opened {
            eprintln!("could not open the window: {err:#}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// Standalone preview wrapper for a screen crate's `examples/<crate>.rs`: renders the view
/// full-window on the base root style.
pub struct Preview(pub AnyView);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        root().flex().child(div().flex_1().min_w_0().h_full().child(self.0.clone()))
    }
}
