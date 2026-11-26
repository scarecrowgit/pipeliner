//! Unified diff -> colored rows with old/new line numbers (Review screen, task threads).
//!
//! Parse once and cache (`ParsedDiff::parse(&variant.diff)` when the data changes), then
//! render `DiffView::new(parsed.clone())` each frame.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, App, Hsla, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window,
};

use super::{icon_sized, AppIcon};
use crate::theme::{self, TextStyles as _};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Hunk,
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`, binary notices.
    Meta,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: SharedString,
}

#[derive(Clone, Debug, Default)]
pub struct DiffFile {
    pub path: String,
    /// "modified", "new file", "deleted", "renamed".
    pub status: &'static str,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedDiff {
    pub files: Vec<DiffFile>,
}

impl ParsedDiff {
    pub fn parse(text: &str) -> Arc<ParsedDiff> {
        let mut files: Vec<DiffFile> = vec![];
        let (mut old_no, mut new_no) = (0u32, 0u32);
        // Lines left in the current hunk (from its header), so a hunk ends exactly.
        let (mut old_left, mut new_left) = (0u32, 0u32);
        let mut in_hunk = false;
        let strip = |p: &str| -> String {
            let p = p.split('\t').next().unwrap_or(p).trim();
            p.strip_prefix("a/")
                .or_else(|| p.strip_prefix("b/"))
                .unwrap_or(p)
                .to_string()
        };
        for raw in text.lines() {
            if let Some(rest) = raw.strip_prefix("diff --git ") {
                let path = rest.rsplit(" b/").next().unwrap_or(rest).to_string();
                files.push(DiffFile {
                    path,
                    status: "modified",
                    ..Default::default()
                });
                in_hunk = false;
                continue;
            }
            if !in_hunk {
                if raw.starts_with("new file mode") {
                    if let Some(f) = files.last_mut() {
                        f.status = "new file";
                    }
                    continue;
                }
                if raw.starts_with("deleted file mode") {
                    if let Some(f) = files.last_mut() {
                        f.status = "deleted";
                    }
                    continue;
                }
                if let Some(to) = raw.strip_prefix("rename to ") {
                    if let Some(f) = files.last_mut() {
                        f.status = "renamed";
                        f.path = to.to_string();
                    }
                    continue;
                }
                if let Some(p) = raw.strip_prefix("--- ") {
                    // A diff without `diff --git` headers starts a file here.
                    if files.last().is_none_or(|f| !f.lines.is_empty()) {
                        files.push(DiffFile {
                            path: strip(p),
                            status: "modified",
                            ..Default::default()
                        });
                    }
                    if p.trim() == "/dev/null" {
                        if let Some(f) = files.last_mut() {
                            f.status = "new file";
                        }
                    }
                    continue;
                }
                if let Some(p) = raw.strip_prefix("+++ ") {
                    if let Some(f) = files.last_mut() {
                        if p.trim() == "/dev/null" {
                            f.status = "deleted";
                        } else {
                            f.path = strip(p);
                        }
                    }
                    continue;
                }
            }
            if raw.starts_with("@@") {
                in_hunk = true;
                let (o, oc, n, nc) = parse_hunk(raw);
                old_no = o;
                new_no = n;
                old_left = oc;
                new_left = nc;
                if files.is_empty() {
                    files.push(DiffFile {
                        path: String::new(),
                        status: "modified",
                        ..Default::default()
                    });
                }
                if let Some(f) = files.last_mut() {
                    f.lines.push(DiffLine {
                        kind: DiffLineKind::Hunk,
                        old: None,
                        new: None,
                        text: raw.to_string().into(),
                    });
                }
                continue;
            }
            let Some(f) = files.last_mut() else { continue };
            if !in_hunk {
                if raw.starts_with("Binary files") {
                    f.lines.push(DiffLine {
                        kind: DiffLineKind::Meta,
                        old: None,
                        new: None,
                        text: raw.to_string().into(),
                    });
                }
                continue;
            }
            let (kind, body) = match raw.chars().next() {
                Some('+') => (DiffLineKind::Add, &raw[1..]),
                Some('-') => (DiffLineKind::Del, &raw[1..]),
                Some(' ') => (DiffLineKind::Context, &raw[1..]),
                Some('\\') => (DiffLineKind::Meta, raw),
                None => (DiffLineKind::Context, ""),
                _ => {
                    // Something outside the hunk grammar: the hunk ended.
                    in_hunk = false;
                    continue;
                }
            };
            let text: SharedString = clip_line(&body.replace('\t', "    ")).into();
            let line = match kind {
                DiffLineKind::Add => {
                    f.added += 1;
                    new_no += 1;
                    DiffLine {
                        kind,
                        old: None,
                        new: Some(new_no - 1),
                        text,
                    }
                }
                DiffLineKind::Del => {
                    f.removed += 1;
                    old_no += 1;
                    DiffLine {
                        kind,
                        old: Some(old_no - 1),
                        new: None,
                        text,
                    }
                }
                DiffLineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    DiffLine {
                        kind,
                        old: Some(old_no - 1),
                        new: Some(new_no - 1),
                        text,
                    }
                }
                _ => DiffLine {
                    kind,
                    old: None,
                    new: None,
                    text,
                },
            };
            match kind {
                DiffLineKind::Add => new_left = new_left.saturating_sub(1),
                DiffLineKind::Del => old_left = old_left.saturating_sub(1),
                DiffLineKind::Context => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
                _ => {}
            }
            f.lines.push(line);
            if old_left == 0 && new_left == 0 {
                in_hunk = false;
            }
        }
        Arc::new(ParsedDiff { files })
    }

    pub fn added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }
    pub fn removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|f| f.lines.is_empty())
    }
}

/// `@@ -38,12 +38,17 @@` -> (38, 12, 38, 17). A missing count means 1.
fn parse_hunk(h: &str) -> (u32, u32, u32, u32) {
    let range = |r: &str| -> (u32, u32) {
        let mut it = r.split(',');
        let start = it.next().and_then(|n| n.parse().ok()).unwrap_or(1);
        let count = it.next().and_then(|n| n.parse().ok()).unwrap_or(1);
        (start, count)
    };
    let (mut o, mut oc, mut n, mut nc) = (1, 1, 1, 1);
    for part in h.split_whitespace().skip(1).take(2) {
        if let Some(r) = part.strip_prefix('-') {
            (o, oc) = range(r);
        } else if let Some(r) = part.strip_prefix('+') {
            (n, nc) = range(r);
        }
    }
    (o, oc, n, nc)
}

/// Renders a [`ParsedDiff`]: per file a header (path, status, +/-), then numbered rows.
/// Very long diffs are cut at `max_lines` with a note.
#[derive(IntoElement)]
pub struct DiffView {
    diff: Arc<ParsedDiff>,
    max_lines: usize,
}

impl DiffView {
    pub fn new(diff: Arc<ParsedDiff>) -> Self {
        DiffView {
            diff,
            max_lines: 4000,
        }
    }
    pub fn max_lines(mut self, n: usize) -> Self {
        self.max_lines = n;
        self
    }
}

fn line_no(n: Option<u32>) -> gpui::Div {
    div()
        .w(px(40.))
        .flex_none()
        .pr(theme::S2)
        .text_right()
        .text_color(theme::TEXT_DISABLED)
        .child(n.map(|n| n.to_string()).unwrap_or_default())
}

impl RenderOnce for DiffView {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut budget = self.max_lines;
        let total: usize = self.diff.files.iter().map(|f| f.lines.len()).sum();
        let mut out = div().flex().flex_col().w_full().text_mono();
        for (fi, f) in self.diff.files.iter().enumerate() {
            let header = div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .h(px(40.))
                .px(theme::S4)
                .bg(theme::SURFACE)
                .when(fi > 0, |d| d.border_t_1().border_color(theme::LINE))
                .border_b_1()
                .border_color(theme::LINE)
                .child(icon_sized(AppIcon::File, theme::ICON_SM).text_color(theme::TEXT_MUTED))
                .child(div().fg().ellipsis().child(f.path.clone()))
                .child(
                    div()
                        .font_family(theme::sans())
                        .text_label()
                        .fg_muted()
                        .child(f.status),
                )
                .child(
                    div()
                        .text_color(theme::DIFF_ADD_TEXT)
                        .child(format!("+{}", f.added)),
                )
                .child(
                    div()
                        .text_color(theme::DIFF_DEL_TEXT)
                        .child(format!("-{}", f.removed)),
                );
            out = out.child(header);
            let take = f.lines.len().min(budget);
            budget -= take;
            out = out.children(f.lines[..take].iter().map(|l| {
                let (bg, fg, sign): (Hsla, Hsla, &str) = match l.kind {
                    DiffLineKind::Add => (theme::DIFF_ADD_BG, theme::DIFF_ADD_TEXT, "+"),
                    DiffLineKind::Del => (theme::DIFF_DEL_BG, theme::DIFF_DEL_TEXT, "-"),
                    DiffLineKind::Hunk => (theme::TRANSPARENT, theme::ACCENT_TEXT, ""),
                    DiffLineKind::Meta => (theme::TRANSPARENT, theme::TEXT_MUTED, ""),
                    DiffLineKind::Context => (theme::TRANSPARENT, theme::TEXT_SECONDARY, ""),
                };
                div()
                    .flex()
                    .items_start()
                    .min_h(px(20.))
                    .bg(bg)
                    .child(line_no(l.old))
                    .child(line_no(l.new))
                    .child(div().w(px(20.)).flex_none().text_color(fg).child(sign))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(theme::S4)
                            .text_color(fg)
                            .child(l.text.clone()),
                    )
            }));
            if budget == 0 {
                break;
            }
        }
        if total > self.max_lines {
            out = out.child(
                div()
                    .px(theme::S4)
                    .py(theme::S2)
                    .font_family(theme::sans())
                    .text_label()
                    .fg_muted()
                    .child(format!("{} more lines not shown", total - self.max_lines)),
            );
        }
        out
    }
}

/// Longest diff line shown in full. Minified or generated files can have lines of
/// hundreds of kilobytes; laying those out wraps into thousands of rows and blows up
/// the page, so the rest is replaced with a count.
const MAX_LINE_CHARS: usize = 400;

fn clip_line(s: &str) -> String {
    match s.char_indices().nth(MAX_LINE_CHARS) {
        None => s.to_string(),
        Some((cut, _)) => {
            let rest = s[cut..].chars().count();
            format!("{} … (+{rest} chars on this line)", &s[..cut])
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn very_long_lines_are_clipped() {
        let long = "x".repeat(5000);
        let d = ParsedDiff::parse(&format!("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-{long}\n+y\n"));
        let del = &d.files[0].lines[1];
        assert!(del.text.len() < 500, "{}", del.text.len());
        assert!(
            del.text.ends_with("(+4600 chars on this line)"),
            "{}",
            del.text
        );
        assert_eq!(d.files[0].lines[2].text.as_ref(), "y");
    }

    use super::*;

    #[test]
    fn parses_git_diff() {
        let d = ParsedDiff::parse(
            "diff --git a/p/retry.py b/p/retry.py\nindex 1..2 100644\n--- a/p/retry.py\n+++ b/p/retry.py\n@@ -38,3 +38,4 @@ def f():\n ctx\n-old\n+new\n+new2\n",
        );
        assert_eq!(d.files.len(), 1);
        let f = &d.files[0];
        assert_eq!(f.path, "p/retry.py");
        assert_eq!((f.added, f.removed), (2, 1));
        assert_eq!(f.lines[1].old, Some(38));
        assert_eq!(f.lines[3].new, Some(39));
    }

    #[test]
    fn plain_multi_file() {
        let d = ParsedDiff::parse(
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n--- a/y\n+++ b/y\n@@ -1,2 +1 @@\n ctx\n-z\n",
        );
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[1].path, "y");
        assert_eq!(d.removed(), 2);
    }

    #[test]
    fn new_file() {
        let d = ParsedDiff::parse("--- /dev/null\n+++ b/t.py\n@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(d.files[0].status, "new file");
        assert_eq!(d.files[0].path, "t.py");
        assert_eq!(d.added(), 2);
    }
}
