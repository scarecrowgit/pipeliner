//! The project switcher: the popover under the sidebar's project button.

use gpui::{App, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::OpsModel;
use model::{Project, ProjectId};
use ui::prelude::*;

use super::logo;

/// The anchor key (and probe) of the project button.
pub(crate) const ANCHOR: &str = "sidebar-project";

/// Open the switcher under the project button.
pub(crate) fn open(ops: Entity<OpsModel>, window: &mut Window, cx: &mut App) {
    let anchor = Anchor::of(ANCHOR, window, cx);
    ui::overlays(window, cx).open_popover(
        anchor,
        Placement::below(),
        move |_, cx| {
            let ops = ops.read(cx);
            let projects: Vec<Project> = ops.projects().value().map(|p| p.to_vec()).unwrap_or_default();
            let current = ops.current_project().map(|project| project.id.clone());
            let count = base::fmt::plural(projects.len() as u64, "project", "projects");
            let mut menu = Menu::new("project-menu")
                .label_with(PopLabel::new("Projects").trailing(SharedString::from(count)));
            for (ix, project) in projects.iter().enumerate() {
                menu = menu.item(project_row(ix, project, current.as_ref() == Some(&project.id)));
            }
            let menu = menu
                .separator()
                .item(MenuItem::new("project-new", "New project…").icon(IconName::Plus).coming_soon())
                .item(MenuItem::new("project-invite", "Invite people").icon(IconName::Users).coming_soon())
                .item(MenuItem::new("project-settings", "Project settings").icon(IconName::Settings).coming_soon());
            Popover::new()
                .width(360.)
                .child(menu)
                .footer(KeyHints::new().hint(["↑↓"], "move").hint(["↵"], "open"))
                .into_any_element()
        },
        window,
        cx,
    );
}

/// A project: its letter tile (the app mark for the current one), name, repos.
fn project_row(ix: usize, project: &Project, current: bool) -> MenuItem {
    let repos: Vec<String> =
        project.repos.iter().filter_map(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()).collect();
    let sub = if repos.is_empty() {
        "no repos yet".to_string()
    } else {
        format!("{} · {}", repos.join(", "), base::fmt::plural(repos.len() as u64, "repo", "repos"))
    };
    let tile = if current { logo(28.).into_any_element() } else { letter_tile(&project.name).into_any_element() };
    let id: ProjectId = project.id.clone();
    MenuItem::new(("project", ix), project.name.clone())
        .tile(tile)
        .subline(sub)
        .checked(current)
        .on_click(move |_, _, cx| switch_to(&id, cx))
}

/// The first letter of a project's name on a neutral tile.
fn letter_tile(name: &str) -> impl IntoElement {
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(RADIUS_MD))
        .bg(SURFACE_HOVER)
        .text_color(TEXT_SECONDARY)
        .type_style(CAPTION_STRONG)
        .child(letter)
}

/// Make `id` the current project (saved in the settings).
fn switch_to(id: &ProjectId, cx: &mut App) {
    let ops = hub::ops(cx);
    let Some(settings) = ops.read(cx).settings().value().cloned() else { return };
    if settings.current_project.as_ref() == Some(id) {
        return;
    }
    let mut settings = (*settings).clone();
    settings.current_project = Some(id.clone());
    hub::commands(cx).set_settings(settings, cx);
}
