//! Every Graphite component in every variant and state, on the app background, for checking
//! against the mockups. Run: `cargo run -p ui --example gallery`.

mod choice;
mod containers;
mod lists;
mod overlays;
mod status;

use gpui::{AnyElement, Context, Render, Window, div};
use ui::prelude::*;

/// Demo state so the interactive components can be tried.
struct Gallery {
    toggle: bool,
    check: bool,
    radio: usize,
    steps: i32,
    tab: usize,
    seg: usize,
    seg_small: usize,
    seg_full: usize,
    chips: [bool; 3],
    clicks: usize,
    archived: bool,
    dlg_option: usize,
    dlg_steps: i32,
    dlg_rollout: usize,
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        root().child(
            div()
                .id("page")
                .size_full()
                .overflow_y_scroll()
                .child(
                    col()
                        .p(px(SPACE_6))
                        .gap(px(SPACE_8))
                        .child(ui::dev::Probe::probe(div().type_style(TITLE_LG).child("Graphite components"), "title"))
                        .child(self.actions(cx))
                        .child(self.overlays(cx))
                        .child(self.status())
                        .child(self.choice(cx))
                        .child(self.containers())
                        .child(self.identity())
                        .child(self.lists())
                        .child(self.table())
                        .child(self.fields()),
                ),
        )
    }
}

/// A named group with a caption line and its content.
pub fn group(title: &'static str, caption: &'static str, content: impl IntoElement) -> AnyElement {
    col()
        .flex_none()
        .gap(px(SPACE_3))
        .child(
            col()
                .child(div().type_style(HEADING).child(title))
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(caption)),
        )
        .child(
            col()
                .gap(px(SPACE_4))
                .p(px(SPACE_4))
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .bg(BG_SIDEBAR)
                .child(content),
        )
        .into_any_element()
}

/// A row of variants, each with a small caption underneath.
pub fn variants(items: Vec<(&'static str, AnyElement)>) -> AnyElement {
    row()
        .flex_wrap()
        .items_start()
        .gap(px(SPACE_4))
        .children(items.into_iter().map(|(label, el)| {
            col()
                .flex_none()
                .items_start()
                .gap(px(SPACE_1_5))
                .child(el)
                .child(div().type_style(CAPTION).text_color(TEXT_DISABLED).child(label))
        }))
        .into_any_element()
}

/// A bare label above content, for rows inside a group.
pub fn caption(text: &'static str) -> AnyElement {
    div().type_style(LABEL).text_color(TEXT_MUTED).child(text).into_any_element()
}

impl Gallery {
    fn actions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let click = cx.listener(|this, _, _, cx| {
            this.clicks += 1;
            cx.notify();
        });
        let click = std::rc::Rc::new(click);
        let btn = |id: &'static str, label: &'static str| {
            let click = click.clone();
            Button::new(id, label).on_click(move |e, w, cx| click(e, w, cx))
        };
        let buttons = variants(vec![
            ("primary", btn("b-primary", "Deploy v15").primary().into_any_element()),
            ("secondary", btn("b-secondary", "Test run").into_any_element()),
            ("ghost", btn("b-ghost", "Replay last 100").ghost().into_any_element()),
            ("danger", btn("b-danger", "Cancel run").danger().into_any_element()),
            ("icon", btn("b-icon", "Test run").icon(IconName::Play).into_any_element()),
            ("kbd", btn("b-kbd", "Ship A").primary().kbd("⌘↵").into_any_element()),
            ("disabled", btn("b-disabled", "Deploy v15").primary().disabled(true).into_any_element()),
            ("selected", btn("b-selected", "Pinned").selected(true).into_any_element()),
        ]);
        let small = variants(vec![
            ("sm primary", btn("s-primary", "Add rule").primary().small().icon(IconName::Plus).into_any_element()),
            ("sm secondary", btn("s-secondary", "Filter").small().into_any_element()),
            ("sm ghost", btn("s-ghost", "Discard").ghost().small().into_any_element()),
            ("sm danger", btn("s-danger", "Reject all").danger().small().into_any_element()),
            ("sm kbd", btn("s-kbd", "Open").small().kbd("↵").into_any_element()),
            ("sm disabled", btn("s-disabled", "Open").small().disabled(true).into_any_element()),
        ]);
        let icons = variants(vec![
            ("plain", IconButton::new("i-plain", IconName::Panel).tooltip("Collapse sidebar").into_any_element()),
            ("bordered", IconButton::new("i-bd", IconName::Edit).bordered().tooltip("New chat").into_any_element()),
            ("small", IconButton::new("i-sm", IconName::X).small().into_any_element()),
            ("small bordered", IconButton::new("i-sm-bd", IconName::Plus).small().bordered().into_any_element()),
            ("rail", IconButton::new("i-rail", IconName::Home).rail().into_any_element()),
            ("rail current", IconButton::new("i-rail-cur", IconName::Inbox).rail().current(true).count("4").into_any_element()),
            ("rail pip", IconButton::new("i-rail-pip", IconName::Chat).rail().pip(WARNING).into_any_element()),
            ("disabled", IconButton::new("i-dis", IconName::Trash).disabled(true).into_any_element()),
        ]);
        let links = row()
            .gap(px(SPACE_4))
            .child(Link::new("link", "Change"))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(format!("{} clicks on buttons", self.clicks)));
        group(
            "Actions",
            "Button, IconButton, Link: primary once per view; ghost, secondary, then danger last.",
            col().gap(px(SPACE_4)).child(buttons).child(small).child(icons).child(links),
        )
    }
}

fn main() {
    ui::run_app("Graphite gallery", 1440., 960., |_, cx| {
        cx.new(|_| Gallery {
            toggle: true,
            check: true,
            radio: 1,
            steps: 3,
            tab: 0,
            seg: 0,
            seg_small: 1,
            seg_full: 0,
            chips: [true, false, false],
            clicks: 0,
            archived: false,
            dlg_option: 0,
            dlg_steps: 2,
            dlg_rollout: 0,
        })
    });
}
