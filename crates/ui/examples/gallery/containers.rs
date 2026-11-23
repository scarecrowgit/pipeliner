//! Gallery: containers.

use gpui::AnyElement;
use ui::prelude::*;

use super::{Gallery, caption, group};

impl Gallery {
    pub fn containers(&self) -> AnyElement {
        let strip = StatStrip::new()
            .stat(Stat::new("Spend today", "$3.81").suffix("of $10.00"))
            .stat(Stat::new("In review", "7").warning())
            .stat(Stat::new("Agents", "14").suffix("3 spawned").spawn())
            .stat(Stat::new("Queued", "37"));
        let card = |id: &'static str, selected: bool| {
            let _ = id;
            Card::new()
                .selected(selected)
                .header(
                    CardHeader::new("Needs you")
                        .lead(Tile::new(IconName::Inbox).tone(TileTone::Accent))
                        .count("4")
                        .meta("2 of 3 online"),
                )
                .child(div_text("Refund webhook timeout"))
                .child(KeyValue::new().row("Agent", "Fixer").mono_row("branch", "fix/refund-timeout"))
                .footer(CardFooter::new().child("Open inbox").child(icon(IconName::ArrowRight, 12., TEXT_MUTED)))
        };
        let cards = row()
            .gap(px(SPACE_4))
            .items_start()
            .child(row().w(px(340.)).child(card("c1", false)))
            .child(row().w(px(340.)).child(card("c2", true)))
            .child(
                col()
                    .w(px(340.))
                    .gap(px(SPACE_3))
                    .child(Card::new().header(CardHeader::new("Runtimes").dot(Status::Live)).flush().child(div_text("flush body")))
                    .child(Note::new("Notes are 12px muted text under a control.")),
            );
        let banners = col()
            .gap(px(SPACE_2))
            .child(Banner::new().child(icon(IconName::Clock, 14., TEXT_MUTED)).child("Neutral: the run resumes after the review."))
            .child(Banner::new().tone(BannerTone::Warning).child(icon(IconName::Alert, 14., WARNING)).child("Warning: 6 are waiting for you."))
            .child(Banner::new().tone(BannerTone::Danger).child(icon(IconName::Alert, 14., DANGER)).child("Danger: depth 3 exceeds policy 2 · asked human."))
            .child(Banner::new().tone(BannerTone::Spawn).child(icon(IconName::Spawn, 14., SPAWN_TEXT)).child("Spawn: Fixer asks to create Test writer ×2."));
        let panel = col()
            .w(px(360.))
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .child(Section::new("Workdir").meta(Link::new("sec-change", "Change")).child(KeyValue::new().row("git", "2 uncommitted · 1 ahead of main").row("GitHub", "connected")))
            .child(Section::new("Context").meta("48.2k of 200k").child(Progress::new(0.24)))
            .child(Section::new("Todo").meta("3 of 4").last().child(Note::new("Last section: no hairline below.")));
        group(
            "Containers",
            "Card (header, footer, selected, flush), StatStrip and Stat, Banner, Section, KeyValue, Note, Divider.",
            col()
                .gap(px(SPACE_4))
                .child(caption("Stat strip"))
                .child(strip)
                .child(caption("Cards: default, selected, flush"))
                .child(cards)
                .child(caption("Banners"))
                .child(banners)
                .child(caption("Panel sections and dividers"))
                .child(
                    row()
                        .gap(px(SPACE_4))
                        .items_start()
                        .child(panel)
                        .child(row().h(px(80.)).gap(px(SPACE_4)).child(Divider::new().vertical()).child(Divider::new().vertical().strong()).child(Divider::new().vertical().subtle()))
                        .child(col().w(px(200.)).gap(px(SPACE_3)).child(Divider::new()).child(Divider::new().strong()).child(Divider::new().subtle())),
                ),
        )
    }
}

fn div_text(text: &'static str) -> gpui::Div {
    gpui::div().type_style(BODY_STRONG).child(text)
}
