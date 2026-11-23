//! Gallery: lists and the table.

use gpui::AnyElement;
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
            .child(ListItem::new("l-7", "Refund webhook timeout").dot(Status::Running).meta("20m"));
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
            .child(MenuItem::new("m-6", "Delete project").icon(IconName::Trash).danger());
        group(
            "Lists",
            "NavItem, ListItem (with dot, meta, current, spawned child), MenuItem (check, tile, sub-line, hint, kbd, danger, selected), PopLabel, PopSeparator.",
            row().gap(px(SPACE_6)).items_start().child(nav).child(list).child(menu),
        )
    }

    pub fn table(&self) -> AnyElement {
        let table = Table::new("table")
            .column(Column::new("Issue").fixed(120.))
            .column(Column::new("Title").flex(2.))
            .column(Column::new("Agent").flex(1.))
            .column(Column::new("State").fixed(140.))
            .column(Column::new("Cost").fixed(80.).right())
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
            "Header 36px, rows 48px, hover fade, group header rows, selected row with the inset accent bar, fixed and flexible columns, right alignment.",
            col().gap(px(SPACE_3)).child(caption("Table")).child(table),
        )
    }
}
