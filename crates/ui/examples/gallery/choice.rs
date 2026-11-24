//! Gallery: choice controls and fields.

use gpui::{AnyElement, Context};
use ui::prelude::*;

use super::{Gallery, caption, group, variants};

impl Gallery {
    pub fn choice(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let set = move |f: fn(&mut Gallery, usize)| {
            let this = this.clone();
            move |ix: usize, _: &mut gpui::Window, cx: &mut gpui::App| {
                this.update(cx, |g, cx| {
                    f(g, ix);
                    cx.notify();
                })
            }
        };

        let tabs = Tabs::new("tabs")
            .tab("Flow")
            .tab_n("Runs", "12")
            .tab("Spawned agents")
            .tab("Settings")
            .selected(self.tab)
            .on_select(set(|g, ix| g.tab = ix));
        let icon_tabs = Tabs::new("tabs-icons")
            .tab("Board")
            .tab_icon(IconName::Issues)
            .tab_probe("tabs-board")
            .tab("List")
            .tab_icon(IconName::Filter)
            .tab("Problems")
            .tab_trailing(CountBadge::new("2").tone(CountTone::Danger))
            .selected(self.tab.min(2))
            .on_select(set(|g, ix| g.tab = ix));

        let seg = |id: &'static str, selected: usize, f: fn(&mut Gallery, usize)| {
            Segmented::new(id)
                .option("Day")
                .option_probe(format!("{id}-day"))
                .option("Week")
                .option("Month")
                .selected(selected)
                .on_select(set(f))
        };
        let segs = variants(vec![
            ("default", seg("seg", self.seg, |g, ix| g.seg = ix).into_any_element()),
            ("small", seg("seg-sm", self.seg_small, |g, ix| g.seg_small = ix).small().into_any_element()),
            (
                "full width",
                row()
                    .w(px(360.))
                    .child(seg("seg-full", self.seg_full, |g, ix| g.seg_full = ix).full_width())
                    .into_any_element(),
            ),
        ]);

        let chip = |ix: usize, label: &'static str, count: &'static str| {
            let this = cx.entity();
            Chip::new(("chip", ix), label).count(count).pressed(self.chips[ix]).on_click(move |_, _, cx| {
                this.update(cx, |g, cx| {
                    g.chips[ix] = !g.chips[ix];
                    cx.notify();
                })
            })
        };
        let chips = variants(vec![
            ("pressed", chip(0, "Needs you", "4").into_any_element()),
            ("rest", chip(1, "Running", "12").into_any_element()),
            ("rest, no count", chip(2, "Done", "").into_any_element()),
            ("spawn", Chip::new("chip-spawn", "Agent-made").count("3").spawn().into_any_element()),
            ("icon", Chip::new("chip-icon", "Filter").icon(IconName::Filter).into_any_element()),
        ]);

        let t = cx.entity();
        let toggle = Toggle::new("toggle", self.toggle).on_change(move |v, _, cx| {
            t.update(cx, |g, cx| {
                g.toggle = v;
                cx.notify();
            })
        });
        let c = cx.entity();
        let check = Checkbox::new("check", self.check).label("Run tests before deploy").on_change(move |v, _, cx| {
            c.update(cx, |g, cx| {
                g.check = v;
                cx.notify();
            })
        });
        let radio = |ix: usize, label: &'static str| {
            let this = cx.entity();
            Radio::new(("radio", ix), self.radio == ix).label(label).on_select(move |_, _, cx| {
                this.update(cx, |g, cx| {
                    g.radio = ix;
                    cx.notify();
                })
            })
        };
        let s = cx.entity();
        let stepper = Stepper::new("stepper", self.steps).range(1, 5).on_change(move |v, _, cx| {
            s.update(cx, |g, cx| {
                g.steps = v;
                cx.notify();
            })
        });
        let controls = variants(vec![
            ("toggle on", toggle.into_any_element()),
            ("toggle off", Toggle::new("toggle-off", false).into_any_element()),
            ("toggle disabled", Toggle::new("toggle-dis", true).disabled(true).into_any_element()),
            ("checkbox", check.into_any_element()),
            ("unchecked", Checkbox::new("check-off", false).into_any_element()),
            ("radio", col().gap(px(SPACE_2)).child(radio(0, "Manual")).child(radio(1, "Auto")).into_any_element()),
            ("stepper 1 to 5", stepper.into_any_element()),
            ("stepper at min", Stepper::new("stepper-min", 1).into_any_element()),
        ]);
        let selects = variants(vec![
            ("select", col().w(px(220.)).child(SelectBox::new("sel", "Claude Sonnet")).into_any_element()),
            ("with icon", col().w(px(220.)).child(SelectBox::new("sel-icon", "fix/PAY-1423-idempotency-key-retry").icon(IconName::Branch)).into_any_element()),
            ("small", col().w(px(180.)).child(SelectBox::new("sel-sm", "Any priority").small()).into_any_element()),
            ("placeholder", col().w(px(220.)).child(SelectBox::new("sel-ph", "Choose a repo").placeholder(true)).into_any_element()),
            ("open", col().w(px(220.)).child(SelectBox::new("sel-open", "Claude Sonnet").open(true)).into_any_element()),
            ("mono", col().w(px(180.)).child(SelectBox::new("sel-mono", "opus-5.5").mono().small()).into_any_element()),
            ("danger", col().w(px(220.)).child(SelectBox::new("sel-danger", "main-old").icon(IconName::Branch).mono().danger(true)).into_any_element()),
        ]);
        group(
            "Choice",
            "Tabs (with counts, icons, trailing badges, per-tab probes), Chip, Segmented (per-option probes), Toggle, Checkbox, Radio, Stepper, SelectBox (mono, danger).",
            col()
                .gap(px(SPACE_4))
                .child(caption("Tabs"))
                .child(tabs)
                .child(icon_tabs)
                .child(caption("Segmented"))
                .child(segs)
                .child(caption("Chips"))
                .child(chips)
                .child(caption("Controls"))
                .child(controls)
                .child(caption("Select triggers"))
                .child(selects),
        )
    }

    pub fn fields(&self) -> AnyElement {
        let w = |el: AnyElement| row().w(px(320.)).child(div_fill(el)).into_any_element();
        let inputs = variants(vec![
            ("placeholder", w(InputFrame::new("in-1").placeholder("Search or open a chat").icon(IconName::Search).kbd("⌘K").into_any_element())),
            ("value", w(InputFrame::new("in-2").value("Fix double charge on retry").into_any_element())),
            ("focused", w(InputFrame::new("in-3").value("PAY-1423").focused(true).into_any_element())),
            ("small", w(InputFrame::new("in-4").placeholder("Filter").icon(IconName::Filter).small().into_any_element())),
            ("compact", w(ui::dev::Probe::probe(InputFrame::new("in-6").placeholder("Filter by name, model or runtime").icon(IconName::Search).kbd("F").compact(), "gallery-compact").into_any_element())),
            ("shrinking button", w(ui::dev::Probe::probe(row().w(px(200.)).child(Button::new("b-long", "Archive Payments lead and everything it made").small().icon(IconName::Archive).shrinks()), "gallery-button").into_any_element())),
            ("textarea", w(InputFrame::new("in-5").placeholder("Describe what to change").multiline().into_any_element())),
        ]);
        let fields = row()
            .gap(px(SPACE_4))
            .items_start()
            .child(row().w(px(320.)).child(div_fill(
                Field::new("Title")
                    .child(InputFrame::new("f-1").value("Retry budget"))
                    .hint("Shown in the inbox and in the sidebar.")
                    .into_any_element(),
            )))
            .child(row().w(px(320.)).child(div_fill(
                Field::new("Model").child(SelectBox::new("f-2", "Claude Sonnet")).into_any_element(),
            )));
        let mark = gpui::HighlightStyle { color: Some(ACCENT_TEXT), background_color: Some(ACCENT_SOFT), ..Default::default() };
        let strong = gpui::HighlightStyle { font_weight: Some(gpui::FontWeight::MEDIUM), color: Some(TEXT), ..Default::default() };
        let cut = |el: AnyElement| row().w(px(220.)).px(px(SPACE_2)).border_1().border_color(LINE).rounded(px(RADIUS_MD)).child(el).into_any_element();
        let truncation = variants(vec![
            ("truncate, mono, cut", cut(truncate("~/code/acme/services/payments-api/fix-PAY-1423").font_family(FONT_MONO).text_color(TEXT_SECONDARY).into_any_element())),
            ("truncate_middle", cut(truncate_middle("~/code/acme/services/payments-api/fix-PAY-1423", BODY.line).font_family(FONT_MONO).text_color(TEXT_SECONDARY).into_any_element())),
            ("middle, no slash", cut(truncate_middle("run-2026-11-24-0f3a9c1e-replay-of-nightly-audit", BODY.line).into_any_element())),
            ("truncate_styled", cut(truncate_styled("Retry the charge once the gateway is back", [(0..5, mark), (10..16, strong)], BODY.line).text_color(TEXT_SECONDARY).into_any_element())),
        ]);
        group(
            "Fields",
            "Field (label above a control) and InputFrame (the static look of `.input`; the editable input goes inside it). One-line text: middle-cut paths and styled text that ends in an ellipsis.",
            col().gap(px(SPACE_4)).child(inputs).child(fields).child(truncation),
        )
    }
}

fn div_fill(el: AnyElement) -> gpui::Div {
    gpui::div().flex_1().min_w_0().child(el)
}
