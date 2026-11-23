//! Gallery: status and identity.

use gpui::AnyElement;
use ui::prelude::*;

use super::{Gallery, group, variants};

impl Gallery {
    pub fn status(&self) -> AnyElement {
        let dots = variants(
            [
                ("live", Status::Live),
                ("running", Status::Running),
                ("waiting", Status::Waiting),
                ("error", Status::Error),
                ("spawn", Status::Spawn),
                ("draft", Status::Draft),
                ("idle", Status::Idle),
            ]
            .into_iter()
            .map(|(l, s)| (l, row().gap(px(SPACE_2)).child(StatusDot::new(s)).child(StatusDot::new(s).small()).into_any_element()))
            .collect(),
        );
        let badges = variants(vec![
            ("neutral", Badge::new("v14").into_any_element()),
            ("accent", Badge::new("Draft v15").tone(BadgeTone::Accent).into_any_element()),
            ("spawn", Badge::new("2 sub-agents").tone(BadgeTone::Spawn).into_any_element()),
            ("success", Badge::new("v14 live").tone(BadgeTone::Success).dot(Status::Live).into_any_element()),
            ("warning", Badge::new("waiting for you").tone(BadgeTone::Warning).dot(Status::Waiting).into_any_element()),
            ("danger", Badge::new("failed").tone(BadgeTone::Danger).dot(Status::Error).into_any_element()),
            ("square", Badge::new("GitHub").square().into_any_element()),
            ("square warning", Badge::new("High").square().tone(BadgeTone::Warning).into_any_element()),
            ("icon", Badge::new("Build").icon(IconName::Agent).into_any_element()),
            ("accent icon", Badge::new("Forced").tone(BadgeTone::Accent).icon(IconName::Lock).into_any_element()),
        ]);
        let counts = variants(vec![
            ("accent", CountBadge::new("4").into_any_element()),
            ("quiet", CountBadge::new("128").quiet().into_any_element()),
            ("danger", CountBadge::new("2").tone(CountTone::Danger).into_any_element()),
            ("tag", Tag::new("bug").into_any_element()),
            ("tag mono", Tag::new("fix/retry-budget").mono().into_any_element()),
            ("tag spawn", Tag::new("agent-made").tone(TagTone::Spawn).into_any_element()),
            ("tag accent", Tag::new("edited").tone(TagTone::Accent).into_any_element()),
            ("kbd", Kbd::new("⌘K").into_any_element()),
            ("key hints", KeyHints::new().hint(["↑", "↓"], "move").hint(["↵"], "open").hint(["⌘↵"], "open beside").into_any_element()),
        ]);
        let bar = |t: ProgressTone, f: f32| row().w(px(160.)).child(Progress::new(f).tone(t)).into_any_element();
        let progress = variants(vec![
            ("accent", bar(ProgressTone::Accent, 0.38)),
            ("info", bar(ProgressTone::Info, 0.6)),
            ("warning", bar(ProgressTone::Warning, 0.85)),
            ("danger", bar(ProgressTone::Danger, 1.0)),
            ("success", bar(ProgressTone::Success, 0.5)),
            ("32px", Progress::new(0.38).width(32.).into_any_element()),
        ]);
        let chips = variants(vec![
            ("spawn chip", SpawnChip::new("Test writer ×2 spawned by Fixer · depth 1 of 2").into_any_element()),
            ("spawn chip, no icon", SpawnChip::new("Spawns Reproducer · 2 of 5").no_icon().into_any_element()),
            ("rule chip", RuleChip::new("type", "=", "Bug").into_any_element()),
            ("rule chip edited", RuleChip::new("priority", "≥", "High").edited(true).into_any_element()),
        ]);
        group(
            "Status",
            "StatusDot (8 and 6 px), Badge, CountBadge, Tag, Kbd, KeyHints, Progress, SpawnChip, RuleChip.",
            col().gap(px(SPACE_4)).child(dots).child(badges).child(counts).child(progress).child(chips),
        )
    }

    pub fn identity(&self) -> AnyElement {
        let tiles = variants(
            [
                ("neutral", TileTone::Neutral),
                ("accent", TileTone::Accent),
                ("info", TileTone::Info),
                ("spawn", TileTone::Spawn),
                ("warn", TileTone::Warn),
            ]
            .into_iter()
            .map(|(l, t)| {
                (
                    l,
                    row()
                        .gap(px(SPACE_2))
                        .child(Tile::new(IconName::Agent).tone(t))
                        .child(Tile::new(IconName::Policy).tone(t).large())
                        .into_any_element(),
                )
            })
            .collect(),
        );
        let marks = variants(vec![
            ("avatar 20", Avatar::new("AR").into_any_element()),
            ("avatar 24", Avatar::new("ME").large().into_any_element()),
            ("code", Code::new("git bisect run").into_any_element()),
        ]);
        group("Identity", "Avatar, Tile (24 and 32), Code.", col().gap(px(SPACE_4)).child(tiles).child(marks))
    }
}
