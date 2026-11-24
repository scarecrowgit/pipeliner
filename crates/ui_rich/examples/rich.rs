//! Markdown and diff preview: a long agent message with every block type (and a code fence
//! still streaming) next to diff cards: the chat's edit card, a two-hunk review card with a
//! pager, an added file, a deleted file and a binary file. Under the message, verbatim text:
//! a failure banner (icon, small danger text, button) and a mono tool output that grows.
//!
//! `cargo run -p ui_rich --example rich`

use std::sync::Arc;

use gpui::{App, Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use ui::prelude::*;
use ui_rich::diff_view::{DiffCard, DiffCardState, DiffFile, DiffHunk, DiffLine, DiffLineKind};
use ui_rich::markdown::{self, Markdown};

const MESSAGE: &str = r#"# Fix double charge on retry

I checked how retries reach the gateway. **Every retry** sends a *new* request, so the gateway sees ~~one~~ two charges. The fix reuses one idempotency key: see `retryCharge` in `src/charges/retry.ts` and [the gateway docs](https://docs.example.com/idempotency). @Fixer wrote the failing test; @Codex reviewed it.

## Review

- **Triage (Decision):** yes · HAR-4 adds user-visible controls → the reviewer must sign off before the rollout reaches the payments team, and the change log says so in plain words.
- **Риск (оценка):** средний · ключ идемпотентности хранится в `charges.idempotency_key` → повторы не создают второй платёж, но [старые записи](https://docs.example.com/backfill) нужно заполнить заново.
- **Links:** see [the gateway docs on retries and idempotency keys](https://docs.example.com/idempotency) and `src/charges/retry.ts`, then compare with the webhook handler, which dedupes late.

A paragraph after the list stays regular: nothing here is bold, and the line is long enough to wrap twice in a narrow column.

> **Note (решение):** a quote with a bold label wraps onto a second line → the rest stays regular weight all the way through.

| Check | Result |
|:--|:--|
| **Triage:** yes · HAR-4 adds controls → sign-off needed before the rollout | **Ок:** проверено · two lines of plain text after the bold label |

## What changed

1. Derive the key once per charge
2. Pass it on every retry
   - the first attempt stores it on the charge
   - later attempts reuse it, even after a timeout
3. Add a test that fails on `main`

### Checklist

- [x] Find every caller of `retryCharge`
- [x] Add an idempotency key to retries
- [ ] Run the charges test suite

> Retries are safe only when the gateway can tell them apart from new charges.
> Without a key it cannot.

```ts
export async function retryCharge(charge: Charge) {
  const key = charge.idempotencyKey ?? deriveKey(charge);
  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });
  if (res.status === 'timeout') return retryCharge({ ...charge, idempotencyKey: key });
  return res;
}
```

```rust
fn derive_key(charge: &Charge) -> String {
    format!("{}:{}", charge.order_id, charge.attempt_group)
}
```

| Variant | Approach | Lines | Tests |
|:--|:--|--:|:-:|
| A | Idempotency key on retry | +18 −4 | 212 of 212 passed |
| B | Lock the row before charging; holds the lock for the whole gateway call | +31 −9 | 211 of 212 |
| C | Dedupe in the webhook handler | +12 −2 | 212 of 212 passed |

---

#### Next

Running the suite now:

```sh
bun test src/charges
bun test src/webhooks --filter retr"#;

fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no: old,
        new_no: new,
        text: text.into(),
        no_newline: false,
    }
}

fn ctx(o: u32, n: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Context, Some(o), Some(n), t)
}

fn add(n: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Add, None, Some(n), t)
}

fn del(o: u32, t: &str) -> DiffLine {
    line(DiffLineKind::Del, Some(o), None, t)
}

fn retry_diff() -> DiffFile {
    let first = DiffHunk {
        lines: vec![
            ctx(
                44,
                44,
                "export async function retryCharge(charge: Charge) {",
            ),
            del(
                45,
                "  const res = await gateway.charge(charge.amount, charge.card);",
            ),
            add(
                45,
                "  const key = charge.idempotencyKey ?? deriveKey(charge);",
            ),
            add(
                46,
                "  const res = await gateway.charge(charge.amount, charge.card, { idempotencyKey: key });",
            ),
            ctx(46, 47, "  if (res.status === 'timeout') {"),
            del(47, "    return retryCharge(charge);"),
            add(
                48,
                "    return retryCharge({ ...charge, idempotencyKey: key });",
            ),
            ctx(48, 49, "  }"),
            ctx(49, 50, "  return res;"),
            ctx(50, 51, "}"),
        ],
    };
    let mut last = add(92, "export { deriveKey };");
    last.no_newline = true;
    let second = DiffHunk {
        lines: vec![
            ctx(88, 89, "function deriveKey(charge: Charge): string {"),
            del(89, "  return `${charge.id}`;"),
            add(90, "  return `${charge.orderId}:${charge.attemptGroup}`;"),
            ctx(90, 91, "}"),
            last,
        ],
    };
    DiffFile {
        path: "src/charges/retry.ts".into(),
        added: 5,
        removed: 3,
        hunks: vec![first, second],
        ..Default::default()
    }
}

fn added_file() -> DiffFile {
    let lines: Vec<DiffLine> = [
        "import { retryCharge } from './retry';",
        "",
        "test('a retry after a timeout charges once', async () => {",
        "  const gateway = fakeGateway({ timeoutFirst: true });",
        "  await retryCharge(charge({ amount: 1200 }));",
        "  expect(gateway.charges).toHaveLength(1);",
        "});",
    ]
    .iter()
    .enumerate()
    .map(|(i, t)| add(i as u32 + 1, t))
    .collect();
    DiffFile {
        path: "src/charges/retry.test.ts".into(),
        added: lines.len() as u32,
        created: true,
        hunks: vec![DiffHunk { lines }],
        ..Default::default()
    }
}

fn long_edit() -> DiffFile {
    let lines: Vec<DiffLine> = (1..=60)
        .map(|n| {
            if n % 4 == 0 {
                add(n, &format!("  log.debug('step {n}');"))
            } else {
                ctx(n, n, "  step();")
            }
        })
        .collect();
    DiffFile {
        path: "src/charges/steps.ts".into(),
        added: 15,
        hunks: vec![DiffHunk { lines }],
        ..Default::default()
    }
}

const FAILURE: &str = "git worktree add failed: fatal: '/home/someone/.harness/worktrees/pipeliner/issue-1234-retry-budget' already exists (a3f9c2e17b5d4e0f8a6c1b2d3e4f5a6b7c8d9e0f)";

const OUTPUT: &str = "   Compiling charges v0.1.0 (/src/charges)\n# not a heading, *not* emphasis, `not` code\n\trunning 3 tests\n";

struct Demo {
    message: Entity<Markdown>,
    failure: Entity<Markdown>,
    output: Entity<Markdown>,
    output_lines: usize,
    files: Vec<Arc<DiffFile>>,
    page: usize,
    log: Option<SharedString>,
    /// The long edit's fold state, kept here as a virtualised list would keep it.
    long_edit: DiffCardState,
}

impl Demo {
    fn new(cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let message = cx.new(|cx| {
            let w = weak.clone();
            Markdown::new(MESSAGE, cx)
                .color(TEXT_SECONDARY)
                .mentions(["Fixer", "Codex"])
                .on_link(move |url, _, cx| {
                    let url = SharedString::from(format!("link: {url}"));
                    let _ = w.update(cx, |d, cx| {
                        d.log = Some(url);
                        cx.notify();
                    });
                })
        });
        let failure = cx.new(|cx| {
            Markdown::verbatim(FAILURE, cx)
                .type_style(LABEL)
                .color(DANGER)
        });
        let output = cx.new(|cx| {
            Markdown::verbatim(OUTPUT, cx)
                .type_style(MONO)
                .color(TEXT_SECONDARY)
        });
        let files = vec![
            Arc::new(retry_diff()),
            Arc::new(added_file()),
            Arc::new(DiffFile {
                path: "src/charges/legacy-retry.ts".into(),
                removed: 48,
                deleted: true,
                ..Default::default()
            }),
            Arc::new(DiffFile {
                path: "assets/receipt-logo.png".into(),
                binary: true,
                ..Default::default()
            }),
            Arc::new(long_edit()),
        ];
        Self {
            message,
            failure,
            output,
            output_lines: 0,
            files,
            page: 0,
            log: None,
            long_edit: DiffCardState::default(),
        }
    }
}

impl Demo {
    /// A streaming tool output: append one more line.
    fn append_line(&mut self, cx: &mut Context<Self>) {
        self.output_lines += 1;
        let n = self.output_lines;
        let more = format!("test charges::retry_{n} ... ok\n");
        self.output.update(cx, |o, cx| {
            let text = format!("{}{more}", o.text());
            o.set_text(text, cx);
        });
    }

    /// The failure banner (`icon | text | button`) and the tool output block.
    fn verbatim(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = row()
            .items_start()
            .gap(px(SPACE_2))
            .px(px(SPACE_3))
            .py(px(SPACE_2))
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(BANNER_DANGER_LINE)
            .bg(DANGER_SOFT)
            .child(
                div()
                    .flex_none()
                    .pt(px(3.))
                    .child(icon(IconName::Alert, 12., DANGER)),
            )
            .child(div().flex_1().min_w_0().child(self.failure.clone()))
            .child(Button::new("retry", "Retry").small());
        let output = col()
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(LINE)
            .overflow_hidden()
            .child(
                row()
                    .h(px(CONTROL_HEIGHT_MD))
                    .px(px(SPACE_3))
                    .child(
                        div()
                            .flex_1()
                            .type_style(MONO)
                            .child("$ cargo test -p charges"),
                    )
                    .child(
                        Button::new("append", "Append a line")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.append_line(cx))),
                    ),
            )
            .child(
                div()
                    .px(px(SPACE_3))
                    .py(px(SPACE_1_5))
                    .bg(BG_SIDEBAR)
                    .border_t_1()
                    .border_color(LINE)
                    .child(self.output.clone()),
            );
        col()
            .flex_none()
            .gap(px(SPACE_3))
            .child(div().type_style(HEADING).child("Verbatim text"))
            .child(banner)
            .child(output)
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lsp = row()
            .gap(px(SPACE_2))
            .child(StatusDot::new(Status::Live).small())
            .child("LSP · typescript · 0 errors");
        let page = self.page;
        let next = cx.listener(|this, _, _, cx| {
            this.page = (this.page + 1) % 3;
            cx.notify();
        });
        let reviewed = self.files[page].clone();
        row()
            .size_full()
            .items_start()
            .gap(px(SPACE_6))
            .p(px(SPACE_6))
            .child(
                col()
                    .id("message")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .gap(px(SPACE_3))
                    .child(div().type_style(HEADING).child("Agent message"))
                    .child(self.message.clone())
                    .child(self.verbatim(cx))
                    .children(
                        self.log
                            .clone()
                            .map(|l| div().type_style(MONO).text_color(TEXT_MUTED).child(l)),
                    ),
            )
            .child(
                col()
                    .id("diffs")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .gap(px(SPACE_4))
                    .child(div().type_style(HEADING).child("Edit card (chat)"))
                    .child(
                        DiffCard::new("edit", self.files[0].clone())
                            .label("edit")
                            .compact()
                            .collapsible()
                            .collapse_after(12)
                            .footer(lsp),
                    )
                    .child({
                        let me = cx.entity();
                        DiffCard::new("long-edit", self.files[4].clone())
                            .label("edit")
                            .compact()
                            .collapsible()
                            .collapse_after(8)
                            // The state lives in the demo, not the card.
                            .state(self.long_edit, move |next, _, cx| {
                                me.update(cx, |d, cx| {
                                    d.long_edit = next;
                                    cx.notify();
                                })
                            })
                    })
                    .child(div().type_style(HEADING).child("Review card (Inbox)"))
                    .child(
                        DiffCard::new("review", reviewed)
                            .lead(Tile::new(IconName::File).tone(TileTone::Accent))
                            .pager(page, 3, next),
                    )
                    .child(div().type_style(HEADING).child("Deleted and binary files"))
                    .child(DiffCard::new("deleted", self.files[2].clone()))
                    .child(DiffCard::new("binary", self.files[3].clone())),
            )
    }
}

fn main() {
    ui::run_app("Markdown and diffs", 1440., 960., |_, cx: &mut App| {
        markdown::bind_keys(cx);
        cx.new(Demo::new)
    });
}
