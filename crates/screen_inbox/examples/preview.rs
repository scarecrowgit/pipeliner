//! The Inbox over the fixture world, in a window, under a stand-in for the shell's header
//! (crumbs, the view's badges and actions).
//!
//! `cargo run -p screen_inbox --example preview [-- --fixture inbox-review]`
//!
//! Any scenario of `hub::fixtures::SCENARIOS` works; `empty` shows an empty inbox.
//! `--renders <dir>` gives the first review's variants the PNGs of `dir` as renders
//! (`<screen>-before.png` / `<screen>-after.png`: A gets them all, B only the "after" ones),
//! copied into the fixture's blob folder. `--stress` adds long titles, branch names and
//! addresses, an agent waiting for an answer, a failed run and links to local images (the
//! PNGs of `--renders`).

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::{Hub, Route};
use screen_inbox::InboxView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "inbox-review".to_string();
    let mut long_report = false;
    let mut renders: Option<std::path::PathBuf> = None;
    let mut stress = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => {
                if let Some(name) = args.next() {
                    fixture = name;
                }
            }
            "--long-report" => long_report = true,
            "--renders" => renders = args.next().map(Into::into),
            "--stress" => stress = true,
            _ => {}
        }
    }
    ui::run_app("Inbox", 1440., 900., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        if long_report {
            api.with_world(|world| {
                if let Some(item) = world
                    .inbox
                    .iter_mut()
                    .find(|i| i.id.as_str() == "inbox-1419")
                {
                    item.summary = LONG_REPORT.to_string();
                }
            });
        }
        if let Some(dir) = &renders {
            attach_renders(&api, dir);
        }
        if stress {
            stress_world(&api, renders.as_deref());
        }
        Hub::new_fixture(api, cx);
        let focus = match fixtures::start_route(&fixture) {
            _ if long_report => Some(hub::InboxFocus::Item(hub::api::InboxId::new("inbox-1419"))),
            Some((Route::Inbox { item }, _)) => item,
            _ => None,
        };
        hub::navigate(
            Route::Inbox {
                item: focus.clone(),
            },
            cx,
        );
        cx.new(|cx| Stage::new(focus, window, cx))
    });
}

/// Gives the first review's variants the PNGs of `dir` as renders.
fn attach_renders(api: &FixtureApi, dir: &std::path::Path) {
    let blobs = std::env::temp_dir().join("harness-fixture-blobs");
    let _ = std::fs::create_dir_all(&blobs);
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "png"));
    files.sort();
    // The size from the PNG header: width and height at bytes 16..24.
    let size = |path: &std::path::Path| {
        let bytes = std::fs::read(path).unwrap_or_default();
        let at = |i: usize| {
            bytes
                .get(i..i + 4)
                .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        (at(16), at(20))
    };
    let blob = |path: &std::path::Path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = std::fs::copy(path, blobs.join(&name));
        hub::api::BlobId::new(name)
    };
    let mut shots = Vec::new();
    for after in files
        .iter()
        .filter(|p| p.to_string_lossy().ends_with("-after.png"))
    {
        let stem = after
            .file_name()
            .map(|n| n.to_string_lossy().replace("-after.png", ""))
            .unwrap_or_default();
        let before = after.with_file_name(format!("{stem}-before.png"));
        let (width, height) = size(after);
        shots.push(hub::api::Render {
            path: format!("mockups/screens-v2/{stem}.html"),
            before: before.exists().then(|| blob(&before)),
            after: blob(after),
            width,
            height,
        });
    }
    api.with_world(|world| {
        if let Some(review) = world.reviews.first_mut() {
            if let Some(a) = review.variants.get_mut(0) {
                a.renders = shots.clone();
            }
            if let Some(b) = review.variants.get_mut(1) {
                b.renders = shots
                    .iter()
                    .map(|s| hub::api::Render {
                        before: None,
                        ..s.clone()
                    })
                    .collect();
            }
        }
    });
}

/// Long strings everywhere, a question that waits for an answer, a failed run, image links.
fn stress_world(api: &FixtureApi, shots: Option<&std::path::Path>) {
    use hub::api::{InboxId, InboxKind, Link, Outcome, RunId, RunState};
    api.with_world(|world| {
        let long_branch = "fix/PAY-1423-idempotency-key-reused-across-every-retry-of-the-gateway-charge-call-a";
        if let Some(review) = world.reviews.first_mut() {
            review.title = "Double charge on retry when the gateway times out after the card was already authorised and the webhook arrives late".to_string();
            if let Some(v) = review.variants.first_mut() {
                v.branch = Some(long_branch.to_string());
                v.name = "Idempotency key on retry, derived from the charge id and the attempt counter".to_string();
            }
        }
        if let Some(run) = world.runs.iter_mut().find(|r| r.id.as_str() == "run-1427") {
            run.state = RunState::WaitingAnswer;
            if let Some(node) = run.nodes.iter_mut().find(|n| n.node.as_str() == "repro") {
                node.question = Some("Which log sinks are in scope?".to_string());
            }
        }
        let image = |name: &str| {
            shots.map(|dir| dir.join(name).display().to_string()).unwrap_or_default()
        };
        let base = world.inbox[0].clone();
        for item in world.inbox.iter_mut() {
            match item.id.as_str() {
                "inbox-1435" => {
                    item.run = Some(RunId::new("run-1427"));
                    item.summary = format!(
                        "Which log sinks are in scope? I found **stdout**, **Datadog** and **S3 archives** in `config/observability/production/sinks/archive-to-s3-with-lifecycle-rules.yaml`.\n\nThe current page: ![checkout]({}) and the receipt: ![receipt]({}).",
                        image("checkout-after.png"),
                        image("receipt-after.png")
                    );
                }
                "inbox-1419" => {
                    item.title = "Webhook signature check shipped to production after the canary held for a full day without a single 401".to_string();
                    item.links = vec![
                        Link { label: "PR #2280".to_string(), url: "https://github.com/acme/payments-api-with-a-very-long-repository-name/pull/2280".to_string() },
                        Link { label: "checkout-after.png".to_string(), url: image("checkout-after.png") },
                        Link { label: "mobile-after.png".to_string(), url: image("mobile-after.png") },
                        Link { label: "Runbook".to_string(), url: "https://wiki.acme.dev/payments/runbooks/webhooks/signature-rotation-and-the-401-rate-dashboard".to_string() },
                    ];
                }
                _ => {}
            }
        }
        world.inbox.push(hub::api::InboxItem {
            id: InboxId::new("inbox-failed"),
            kind: InboxKind::Failure,
            outcome: Some(Outcome::Failed),
            title: "Refund webhook times out: the reproducer crashed".to_string(),
            summary: "error: command `pnpm test --filter @acme/payments-api -- src/webhooks/refund.spec.ts` exited with 1\n/home/ci/.harness/worktrees/payments-api/fix-PAY-1427-refund-webhook-times-out/node_modules/.pnpm/vitest@1.6.0/node_modules/vitest/dist/vendor/execute.fc9ef7a2.js:2: TypeError: Cannot read properties of undefined (reading 'signature')".to_string(),
            run: Some(RunId::new("run-1427")),
            read: false,
            created: base.created,
            ..base
        });
    });
}

/// A report body like real agents write: lists whose items start with a bold label.
const LONG_REPORT: &str = "Checked the webhook signature path end to end.\n\n\
- **Triage (Decision):** yes · the bad signature reached the handler before the check, so the fix belongs in the middleware\n\
- **Scope:** webhooks only · API calls were already signed\n\
- **Tests:** 6 new · all 218 pass\n\
- **Risk (Low):** a merchant sending an old signature format now gets a 401 instead of a silent accept, which the changelog mentions\n\n\
1. **Next step:** watch the 401 rate for a day\n\
2. **Owner:** payments team\n";

/// The shell's part around the view: a header bar over the content, in an inset frame.
struct Stage {
    view: Entity<InboxView>,
}

impl Stage {
    fn new(focus: Option<hub::InboxFocus>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| InboxView::new(focus, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        let router = hub::router(cx);
        let follow = view.clone();
        cx.observe_in(&router, window, move |_, router, window, cx| {
            if let Route::Inbox { item } = router.read(cx).current().clone() {
                follow.update(cx, |view, cx| view.show(item, window, cx));
            }
        })
        .detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, trail, badges, actions) = self.view.update(cx, |v, cx| {
            (
                v.title(cx),
                v.trail(cx),
                v.header_badges(window, cx),
                v.header_actions(window, cx),
            )
        });
        let mut crumbs = row()
            .gap(px(SPACE_1_5))
            .type_style(HEADING)
            .child(div().text_color(TEXT_MUTED).child("Inbox"));
        for crumb in trail {
            crumbs = crumbs
                .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
                .child(
                    div()
                        .text_color(TEXT_MUTED)
                        .font_weight(gpui::FontWeight::NORMAL)
                        .child(crumb),
                );
        }
        if title.as_ref() != "Inbox" {
            crumbs = crumbs
                .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
                .child(title);
        }
        ui::root().bg(BG_SIDEBAR).flex().p(px(SPACE_2)).child(
            col()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(BG)
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .overflow_hidden()
                .child(
                    row()
                        .flex_none()
                        .h(px(HEADER_HEIGHT))
                        .gap(px(SPACE_2))
                        .pl(px(SPACE_5))
                        .pr(px(SPACE_2))
                        .border_b_1()
                        .border_color(LINE)
                        .child(crumbs)
                        .children(badges)
                        .child(grow())
                        .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
                )
                .child(div().flex_1().min_h_0().flex().child(self.view.clone())),
        )
    }
}
