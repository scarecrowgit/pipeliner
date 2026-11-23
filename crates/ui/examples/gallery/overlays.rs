//! Gallery: overlays. Menus, popovers, the four dialog looks of `screens_c.py`, a toast with
//! Undo and tooltips, all opened through the overlay host.

use gpui::{AnyElement, App, Context, WeakEntity, Window, div};
use ui::dev::Probe;
use ui::prelude::*;

use super::{Gallery, caption, group, variants};

/// Builds a dialog's content each frame.
type BuildFn = dyn Fn(&mut Window, &mut App) -> AnyElement;

fn new_menu(_: &mut Window, _: &mut App) -> Menu {
    let it = |id: &'static str, icon: IconName, tone: TileTone, name: &'static str, desc: &'static str, key: &'static str| {
        MenuItem::new(id, name).tile(Tile::new(icon).tone(tone).medium()).subline(desc).kbd(key)
    };
    Menu::new("new-menu")
        .width(384.)
        .item(it("new-agent", IconName::Agent, TileTone::Neutral, "Agent", "One model with tools, skills and limits", "A"))
        .item(it("new-orch", IconName::Orchestrator, TileTone::Neutral, "Orchestrator", "Splits work across a team, can create agents", "O"))
        .item(it("new-pipe", IconName::Pipelines, TileTone::Neutral, "Pipeline", "Sources, routers and agents on a canvas", "P"))
        .item(it("new-auto", IconName::Clock, TileTone::Accent, "Autopilot", "Runs on a schedule or when something happens", "T"))
        .separator()
        .item(MenuItem::new("new-tpl", "From a template…").icon(IconName::File).hint("12 templates"))
        .item(MenuItem::new("new-import", "Import from file…").icon(IconName::Upload).coming_soon())
}

fn more_menu(gallery: WeakEntity<Gallery>) -> impl Fn(&mut Window, &mut App) -> Menu + 'static {
    move |_, _| {
        let g = gallery.clone();
        Menu::new("more-menu")
            .width(280.)
            .item(MenuItem::new("m-edit", "Edit prompt and tools").icon(IconName::Edit).hint("E"))
            .item(MenuItem::new("m-dup", "Duplicate").icon(IconName::Fork).hint("⌘D"))
            .item(MenuItem::new("m-hist", "Run history").icon(IconName::Clock).hint("14 today"))
            .item(MenuItem::new("m-export", "Export as YAML").icon(IconName::Download))
            .separator()
            .item(MenuItem::new("m-pause", "Pause").icon(IconName::Pause).subline("Finishes PAY-1436, takes no new work"))
            .item(MenuItem::new("m-archive", "Archive").icon(IconName::Archive).on_click(move |_, window, cx| {
                archive(g.clone(), window, cx)
            }))
            .item(
                MenuItem::new("m-delete", "Delete…")
                    .icon(IconName::Trash)
                    .subline("Used by PAY triage and 2 issues")
                    .danger()
                    .on_click(|_, window, cx| delete_dialog(window, cx)),
            )
    }
}

fn archive(gallery: WeakEntity<Gallery>, window: &mut Window, cx: &mut App) {
    gallery.update(cx, |g, cx| {
        g.archived = true;
        cx.notify();
    }).ok();
    let undo = gallery.clone();
    overlays(window, cx).toast(
        Toast::new("Release captain archived").detail("2 pipelines have no owner now").undo(move |_, cx| {
            undo.update(cx, |g, cx| {
                g.archived = false;
                cx.notify();
            })
            .ok();
        }),
        cx,
    );
}

fn project_switcher(_: &mut Window, _: &mut App) -> AnyElement {
    let proj = |id: &'static str, letter: IconName, name: &'static str, sub: &'static str, key: &'static str, cur: bool| {
        MenuItem::new(id, name).tile(Tile::new(letter).medium()).subline(sub).kbd(key).checked(cur)
    };
    let menu = Menu::new("projects")
        .label_with(PopLabel::new("Acme · team · 8 people").trailing("4 projects"))
        .item(proj("p-pay", IconName::Mark, "Payments", "payments-api, checkout-web, ledger · 3 repos", "⌘1", true))
        .item(proj("p-chk", IconName::Folder, "Checkout", "checkout-web, storefront · 2 repos", "⌘2", false))
        .item(proj("p-plat", IconName::Folder, "Platform", "infra, gateway · 2 repos", "⌘3", false))
        .label("Personal")
        .item(proj("p-side", IconName::Laptop, "Side projects", "3 folders on laptop · local only", "⌘5", false))
        .separator()
        .item(MenuItem::new("p-new", "New project…").icon(IconName::Plus))
        .item(MenuItem::new("p-invite", "Invite people to Acme").icon(IconName::Users).coming_soon())
        .item(MenuItem::new("p-settings", "Project settings").icon(IconName::Settings).hint("⌘,"));
    Popover::new()
        .width(360.)
        .search(InputFrame::new("p-search").small().sunken().icon(IconName::Search).placeholder("Find a project").kbd("⌘O"))
        .child(menu)
        .footer(KeyHints::new().hint(["↑↓"], "move").hint(["↵"], "open").hint(["⌘1–5"], "jump"))
        .into_any_element()
}

fn autopilots(_: &mut Window, _: &mut App) -> AnyElement {
    let ap = |id: &'static str, icon: IconName, name: &'static str, when: &'static str, on: bool| {
        row()
            .items_start()
            .gap(px(SPACE_2_5))
            .p(px(SPACE_2_5))
            .child(Tile::new(icon))
            .child(col().flex_1().child(div().type_style(BODY_STRONG).child(name)).child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child(when)))
            .child(Toggle::new(id, on))
    };
    Popover::new()
        .width(420.)
        .title("Autopilots")
        .trailing("2 on · 1 paused")
        .subtitle("Create or pick up issues without you: on a schedule or when something happens.")
        .child(ap("ap-1", IconName::Clock, "Nightly e2e triage", "Every day at 02:00 → PAY triage", true))
        .child(ap("ap-2", IconName::Zap, "New Sentry error", "On a new error in payments-api → Reproducer", true))
        .child(PopSeparator)
        .child(MenuItem::new("ap-new", "New autopilot…").icon(IconName::Plus))
        .into_any_element()
}

fn send_back_dialog(gallery: WeakEntity<Gallery>) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    move |_, cx| {
        let (opt, steps) = gallery.upgrade().map(|g| (g.read(cx).dlg_option, g.read(cx).dlg_steps)).unwrap_or((0, 2));
        let (g1, g2) = (gallery.clone(), gallery.clone());
        Dialog::new("send-back", "Send back to Fixer")
            .subtitle("PAY-1423 · Double charge on retry · 3 variants")
            .width(580.)
            .top(150.)
            .child(
                Field::new("What should change?")
                    .hint("Fixer gets your note, this thread and the diffs below it.")
                    .child(InputFrame::new("sb-note").multiline().focused(true).value("Keep A’s idempotency key, but don’t change the public retry() signature.")),
            )
            .child(
                Field::new("Start from").child(
                    OptionGroup::new("sb-start")
                        .option(OptionCard::new("sb-a", "Variant A").description("Keeps its branch fix/PAY-1423-a and builds on it").right(Badge::new("your pick").tone(BadgeTone::Accent)))
                        .option(OptionCard::new("sb-all", "All 3 variants").description("Each one gets the note and tries again"))
                        .option(OptionCard::new("sb-over", "Start over").description("New branches, the variants are archived"))
                        .selected(opt)
                        .on_select(move |ix, _, cx| {
                            g1.update(cx, |g, cx| {
                                g.dlg_option = ix;
                                cx.notify();
                            })
                            .ok();
                        }),
                ),
            )
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Variants to build"))
                    .child(Stepper::new("sb-steps", steps).on_change(move |v, _, cx| {
                        g2.update(cx, |g, cx| {
                            g.dlg_steps = v;
                            cx.notify();
                        })
                        .ok();
                    }))
                    .child(grow())
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child("about 6 min · $0.30")),
            )
            .note("The issue goes back to In progress")
            .cancel_button()
            .primary_action(Button::new("sb-go", "Send back").primary().icon(IconName::Replay).kbd("⌘↵"), |_, _| {})
            .into_any_element()
    }
}

fn new_issue_dialog(_: &mut Window, _: &mut App) -> AnyElement {
    Dialog::new("new-issue", "New issue")
        .subtitle("PAY-1442 · Payments")
        .width(640.)
        .top(110.)
        .child(InputFrame::new("ni-title").focused(true).value("Saved card shows the wrong last 4 digits"))
        .child(InputFrame::new("ni-body").multiline().placeholder("What should change, and why"))
        .child(
            row()
                .gap(px(SPACE_1_5))
                .child(SelectBox::new("ni-status", "Backlog"))
                .child(SelectBox::new("ni-prio", "Medium priority"))
                .child(SelectBox::new("ni-type", "Bug").icon(IconName::Issues))
                .child(SelectBox::new("ni-repo", "checkout-web").icon(IconName::Folder)),
        )
        .note(row().gap(px(SPACE_2)).child(Toggle::new("ni-more", false)).child("Create more"))
        .cancel_button()
        .primary_action(Button::new("ni-go", "Create issue").primary().kbd("⌘↵"), |_, _| {})
        .into_any_element()
}

fn deploy_dialog(gallery: WeakEntity<Gallery>) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    move |_, cx| {
        let rollout = gallery.upgrade().map(|g| g.read(cx).dlg_rollout).unwrap_or(0);
        let g = gallery.clone();
        Dialog::new("deploy", "Deploy v15 of PAY triage")
            .subtitle("Replaces v14 for new issues from Payments")
            .width(600.)
            .top(150.)
            .child(
                Banner::new()
                    .tone(BannerTone::Warning)
                    .child(icon(IconName::Alert, 16., WARNING))
                    .child("About 40% more per issue: ~$0.43 instead of $0.31."),
            )
            .child(
                Field::new("Roll out to").child(
                    OptionGroup::new("rollout")
                        .option(OptionCard::new("ro-all", "All new issues").description("37 queued issues start on v15 as soon as it’s live"))
                        .option(OptionCard::new("ro-ten", "1 in 10 new issues for 24 hours").description("The rest stay on v14. Promote or roll back from Versions"))
                        .selected(rollout)
                        .on_select(move |ix, _, cx| {
                            g.update(cx, |g, cx| {
                                g.dlg_rollout = ix;
                                cx.notify();
                            })
                            .ok();
                        }),
                ),
            )
            .child(Field::new("Note for the changelog").child(InputFrame::new("dn").placeholder("Severity split for P1, third Fixer variant")))
            .note("Started runs stay on v14")
            .cancel_button()
            .primary_action(Button::new("deploy-go", "Deploy v15").primary().kbd("⌘↵"), |_, _| {})
            .into_any_element()
    }
}

fn delete_dialog(window: &mut Window, cx: &mut App) {
    overlays(window, cx).open_dialog(
        "delete",
        |_, _| {
            Dialog::new("delete", "Delete Payments lead?")
                .subtitle("Orchestrator · used in 1 pipeline and 3 issues")
                .width(560.)
                .top(130.)
                .child(div().text_color(TEXT_SECONDARY).child(
                    "Its prompt, skills, limits and run history are removed for everyone in Acme. Chats and issues it worked on stay, with its name greyed out.",
                ))
                .child(
                    Banner::new()
                        .child(icon(IconName::Archive, 16., TEXT_MUTED))
                        .child(col().flex_1().child(div().type_style(BODY_STRONG).child("Archive keeps all of this")).child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child("It stops taking work, and you can restore it with its history.")))
                        .child(Button::new("del-archive", "Archive instead").small()),
                )
                .child(ConfirmInput::new("del-name", "Payments lead").value("Payments lead"))
                .note(row().gap(px(SPACE_1_5)).text_color(DANGER).child(icon(IconName::Alert, 14., DANGER)).child("This can’t be undone"))
                .cancel_button()
                .action(Button::new("del-go", "Delete Payments lead").danger().icon(IconName::Trash))
                .into_any_element()
        },
        window,
        cx,
    );
}

impl Gallery {
    pub fn overlays(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity().downgrade();
        let menus = variants(vec![
            (
                "menu below, end-aligned",
                Button::new("ov-new", "New")
                    .primary()
                    .icon(IconName::Plus)
                    .on_click(|_, window, cx| {
                        let anchor = Anchor::of("ov-new", window, cx);
                        overlays(window, cx).open_menu(anchor, Placement::below().end(), new_menu, window, cx);
                    })
                    .popover_anchor("ov-new")
                    .probe("ov-new")
                    .into_any_element(),
            ),
            (
                "more actions",
                {
                    let me = me.clone();
                    IconButton::new("ov-more", IconName::More)
                        .bordered()
                        .tooltip("More actions")
                        .on_click(move |_, window, cx| {
                            let anchor = Anchor::of("ov-more", window, cx);
                            overlays(window, cx).open_menu(anchor, Placement::below().end(), more_menu(me.clone()), window, cx);
                        })
                        .popover_anchor("ov-more")
                    .probe("ov-more")
                        .into_any_element()
                },
            ),
            (
                "search slot and key hints",
                SelectBox::new("ov-proj", "Payments")
                    .icon(IconName::Folder)
                    .on_click(|_, window, cx| {
                        let anchor = Anchor::of("ov-proj", window, cx);
                        overlays(window, cx).open_popover(anchor, Placement::below(), project_switcher, window, cx);
                    })
                    .popover_anchor("ov-proj")
                    .probe("ov-proj")
                    .into_any_element(),
            ),
            (
                "header popover, opens above",
                Button::new("ov-auto", "Autopilots · 2")
                    .ghost()
                    .icon(IconName::Clock)
                    .on_click(|_, window, cx| {
                        let anchor = Anchor::of("ov-auto", window, cx);
                        overlays(window, cx).open_popover(anchor, Placement::above(), autopilots, window, cx);
                    })
                    .popover_anchor("ov-auto")
                    .probe("ov-auto")
                    .into_any_element(),
            ),
        ]);
        let dialog_button = |id: &'static str, label: &'static str, build: Box<BuildFn>| {
            let build: std::rc::Rc<BuildFn> = build.into();
            Button::new(id, label)
                .on_click(move |_, window, cx| {
                    let build = build.clone();
                    overlays(window, cx).open_dialog(id, move |w, cx| build(w, cx), window, cx);
                })
                .probe(id)
        };
        let dialogs = row()
            .gap(px(SPACE_2))
            .child(dialog_button("ov-send", "Send back…", Box::new(send_back_dialog(me.clone()))))
            .child(dialog_button("ov-issue", "New issue…", Box::new(new_issue_dialog)))
            .child(dialog_button("ov-deploy", "Deploy v15…", Box::new(deploy_dialog(me.clone()))))
            .child(Button::new("ov-delete", "Delete…").danger().on_click(|_, window, cx| delete_dialog(window, cx)).probe("ov-delete"));
        let toast = {
            let me = me.clone();
            row()
                .gap(px(SPACE_3))
                .child(Button::new("ov-archive", "Archive Release captain").on_click(move |_, window, cx| archive(me.clone(), window, cx)).probe("ov-archive"))
                .child(Button::new("ov-toast", "Show a plain toast").ghost().on_click(|_, window, cx| {
                    overlays(window, cx).toast(Toast::new("Link copied").detail("harness.acme.dev/c/7f3a92e1"), cx)
                }))
                .child(
                    div()
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .child(if self.archived { "Release captain: archived" } else { "Release captain: active" }),
                )
        };
        let tooltips = row()
            .gap(px(SPACE_2))
            .child(IconButton::new("tt-panel", IconName::Panel).tooltip("Collapse sidebar"))
            .child(IconButton::new("tt-edit", IconName::Edit).bordered().tooltip("New chat"))
            .child(
                div()
                    .id("tt-kbd")
                    .child(Kbd::new("⌘K"))
                    .tooltip(Tooltip::with_kbd("Search chats", "⌘K")),
            );
        group(
            "Overlays",
            "Menu, Popover (header, search slot, key-hint footer), Dialog with OptionCard and ConfirmInput, Toast with Undo, Tooltip. Esc closes the top layer; Tab stays inside a dialog.",
            col()
                .gap(px(SPACE_4))
                .child(caption("Menus and popovers"))
                .child(menus)
                .child(caption("Dialogs"))
                .child(dialogs)
                .child(caption("Toast"))
                .child(toast)
                .child(caption("Tooltips"))
                .child(tooltips),
        )
    }
}
