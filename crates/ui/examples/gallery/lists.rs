//! Gallery: lists and the table.

use gpui::{AnyElement, Context};
use ui::prelude::*;

use super::{Gallery, caption, group};

impl Gallery {
    pub fn lists(&self) -> AnyElement {
        let nav = col()
            .w(px(216.))
            .gap(px(1.))
            .child(NavItem::new("n-home", IconName::Home, "Home"))
            .child(NavItem::new("n-inbox", IconName::Inbox, "Inbox").count("4").current(true))
            .child(NavItem::new("n-chats", IconName::Chat, "Chats").hint("1 waiting"))
            .child(NavItem::new("n-issues", IconName::Issues, "Issues"))
            .child(NavItem::new("n-agents", IconName::Orchestrator, "Agents & pipelines"));
        let list = col()
            .w(px(216.))
            .gap(px(1.))
            .child(ListItem::new("l-1", "PAY triage").dot(Status::Live).meta("37 queued"))
            .child(ListItem::new("l-2", "Nightly audit").dot(Status::Idle).meta("02:00"))
            .child(ListItem::new("l-3", "Release notes").dot(Status::Draft).meta("draft"))
            .child(ListItem::new("l-4", "Fix double charge on retry with a very long title").dot(Status::Waiting).meta("12m").current(true))
            .child(ListItem::new("l-5", "@explore · callers").spawned().meta("done"))
            .child(ListItem::new("l-6", "@general · gateway docs").spawned().meta("done"))
            .child(ListItem::new("l-7", "Refund webhook timeout").dot(Status::Running).meta("20m"))
            .child(
                ListItem::new("l-8", "Hover me: pin and archive")
                    .dot(Status::Idle)
                    .meta("3h")
                    .hover_actions(
                        row()
                            .child(IconButton::new("l-8-pin", IconName::Pin).small().tooltip("Pin"))
                            .child(IconButton::new("l-8-archive", IconName::Archive).small().tooltip("Archive")),
                    ),
            );
        let menu = col()
            .w(px(300.))
            .p(px(SPACE_1))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .child(PopLabel::new("Project"))
            .child(MenuItem::new("m-1", "Acme · Payments").checked(true).tile(Tile::new(IconName::Folder)).subline("12 chats"))
            .child(MenuItem::new("m-2", "Acme · Platform").check_column().tile(Tile::new(IconName::Folder)).hint("Settings"))
            .child(PopSeparator)
            .child(MenuItem::new("m-3", "New project…").icon(IconName::Plus))
            .child(MenuItem::new("m-4", "Project settings").icon(IconName::Settings).kbd("⌘,"))
            .child(MenuItem::new("m-5", "Selected row").icon(IconName::Search).selected(true))
            .child(MenuItem::new("m-6", "Delete project").icon(IconName::Trash).danger())
            .child(PopLabel::new("Matching “re” · 2 of 8"))
            .child(MenuItem::new("m-7", "Reproducer").tile(Tile::new(IconName::Agent).medium()).matched(2).subline("Agent · on this issue now").hint("running").hint_tone(HintTone::Info))
            .child(MenuItem::new("m-7b", "Fixer").tile(Tile::new(IconName::Agent).medium()).label_suffix("opus-5.5").subline("Agent · fixes failing checks and opens a PR").hint("idle"))
            .child(MenuItem::new("m-8", "Release captain").tile(Tile::new(IconName::Orchestrator).medium()).matched(2).subline("Orchestrator").hint("idle"))
            .child(MenuItem::new("m-9", "Import from file…").icon(IconName::Upload).disabled(true).tooltip("No file picker yet").probe_name("gallery-import"));
        group(
            "Lists",
            "NavItem, ListItem (with dot, meta, current, spawned child, hover actions), MenuItem (check, tile, sub-line, label suffix, hint and its tone, matched prefix, kbd, danger, selected, disabled with a tooltip), PopLabel, PopSeparator.",
            row().gap(px(SPACE_6)).items_start().child(nav).child(list).child(menu),
        )
    }

    pub fn table(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let table = Table::new("table")
            .column(Column::new("Issue").fixed(120.).sortable())
            .column(Column::new("Title").flex(2.).sortable())
            .column(Column::new("Agent").flex(1.))
            .column(Column::new("State").fixed(140.))
            .column(Column::new("Cost").fixed(80.).right().sortable())
            .sorted(self.sort.0, self.sort.1)
            .on_sort(move |column, direction, _, cx| {
                this.update(cx, |g, cx| {
                    g.sort = (column, direction);
                    cx.notify();
                })
            })
            .group("Today", Some("3".into()))
            .row(
                TableRow::new("t1")
                    .mono("PAY-1423")
                    .strong("Fix double charge on retry")
                    .text("Fixer ×3")
                    .cell(row().gap(px(SPACE_1_5)).child(StatusDot::new(Status::Waiting)).child("waiting for you"))
                    .mono("$0.31")
                    .selected(true),
            )
            .row(
                TableRow::new("t2")
                    .mono("PAY-1424")
                    .strong("Refund webhook timeout")
                    .text("Analyst")
                    .cell(row().gap(px(SPACE_1_5)).child(StatusDot::new(Status::Running)).child("running"))
                    .mono("$0.09"),
            )
            .group("Yesterday", Some("1".into()))
            .row(
                TableRow::new("t3")
                    .mono("PAY-1401")
                    .strong("Bump gateway SDK to v9")
                    .text("Reproducer")
                    .cell(row().gap(px(SPACE_1_5)).child(StatusDot::new(Status::Live)).child("done"))
                    .mono("$0.54"),
            );
        group(
            "Table",
            "Header 36px, rows 48px, hover fade, group header rows, selected row with the inset accent bar, fixed and flexible columns, right alignment, sortable headers (the caller sorts).",
            col().gap(px(SPACE_3)).child(caption("Table")).child(table),
        )
    }
}
