//! Parser for unified diffs as `git diff` prints them.

use model::{DiffHunk, DiffLine, DiffLineKind, FileDiff};

/// Parses the output of `git diff` (or any unified diff) into one [`FileDiff`] per file.
///
/// Understands new, deleted, renamed (the new path is kept) and mode-only files, binary
/// files (no hunks), paths with spaces or quoted escapes, and `\ No newline at end of file`
/// (the marker is not a line). Hunk bodies are read by the counts in their headers, so a
/// removed line that looks like `--- a/x` is not mistaken for a file header.
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut parser = Parser::default();
    for line in text.lines() {
        parser.feed(line);
    }
    parser.finish()
}

/// The file being read.
#[derive(Default)]
struct File {
    diff: FileDiff,
    /// Path from the `---` line, `None` for `/dev/null`.
    old_path: Option<String>,
    /// Path from the `+++` line, `None` for `/dev/null`.
    new_path: Option<String>,
    rename_to: Option<String>,
    header_path: Option<String>,
    seen_old: bool,
    seen_new: bool,
    deleted: bool,
    started_hunks: bool,
}

/// The hunk being read.
struct Hunk {
    lines: Vec<DiffLine>,
    old_no: u32,
    new_no: u32,
    old_left: u32,
    new_left: u32,
}

#[derive(Default)]
struct Parser {
    done: Vec<FileDiff>,
    file: Option<File>,
    hunk: Option<Hunk>,
}

impl Parser {
    fn feed(&mut self, line: &str) {
        if let Some(hunk) = &mut self.hunk {
            if line.starts_with('\\') {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
                return;
            }
            // A line that is not part of a hunk after all means the counts were wrong.
            if (hunk.old_left > 0 || hunk.new_left > 0) && hunk.take(line) {
                return;
            }
            self.close_hunk();
        }
        self.header(line);
    }

    fn header(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.close_file();
            self.file = Some(File { header_path: header_path(rest), ..File::default() });
        } else if let Some(rest) = line.strip_prefix("--- ") {
            let starts_new = match &self.file {
                None => true,
                Some(f) => f.seen_old || f.started_hunks,
            };
            if starts_new {
                self.close_file();
                self.file = Some(File::default());
            }
            if let Some(file) = &mut self.file {
                file.seen_old = true;
                file.old_path = diff_path(rest, "a/");
                if file.old_path.is_none() {
                    file.diff.created = true;
                }
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let file = self.file.get_or_insert_with(File::default);
            file.seen_new = true;
            file.new_path = diff_path(rest, "b/");
            file.deleted = file.new_path.is_none();
        } else if line.starts_with("new file mode") {
            if let Some(file) = &mut self.file {
                file.diff.created = true;
            }
        } else if line.starts_with("deleted file mode") {
            if let Some(file) = &mut self.file {
                file.deleted = true;
            }
        } else if let Some(rest) = line.strip_prefix("rename from ") {
            if let Some(file) = &mut self.file {
                file.diff.old_path = Some(unquote_or_plain(rest));
            }
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            if let Some(file) = &mut self.file {
                file.diff.binary = true;
            }
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            if let Some(file) = &mut self.file {
                file.rename_to = Some(unquote_or_plain(rest));
            }
        } else if let Some(rest) = line.strip_prefix("@@ ")
            && let Some(hunk) = parse_hunk_header(rest)
        {
            let file = self.file.get_or_insert_with(File::default);
            file.started_hunks = true;
            self.hunk = Some(hunk);
        }
        // Everything else (index, mode, similarity, rename from, Binary files, GIT binary
        // patch) needs no action: a file without hunks stays a file without hunks.
    }

    fn close_hunk(&mut self) {
        if let (Some(hunk), Some(file)) = (self.hunk.take(), &mut self.file)
            && !hunk.lines.is_empty()
        {
            file.diff.hunks.push(DiffHunk { lines: hunk.lines });
        }
    }

    fn close_file(&mut self) {
        self.close_hunk();
        let Some(mut file) = self.file.take() else { return };
        let path = if file.deleted {
            file.old_path.take().or_else(|| file.header_path.take())
        } else {
            file.new_path.take().or_else(|| file.rename_to.take()).or_else(|| file.header_path.take())
        };
        let Some(path) = path.or(file.old_path.take()) else { return };
        file.diff.path = path;
        file.diff.deleted = file.deleted;
        for line in file.diff.hunks.iter().flat_map(|h| &h.lines) {
            match line.kind {
                DiffLineKind::Add => file.diff.added += 1,
                DiffLineKind::Del => file.diff.removed += 1,
                DiffLineKind::Context => {}
            }
        }
        self.done.push(file.diff);
    }

    fn finish(mut self) -> Vec<FileDiff> {
        self.close_file();
        self.done
    }
}

impl Hunk {
    /// Reads one body line; `false` when the line cannot belong to a hunk.
    fn take(&mut self, line: &str) -> bool {
        let (kind, text) = match line.chars().next() {
            Some('+') => (DiffLineKind::Add, &line[1..]),
            Some('-') => (DiffLineKind::Del, &line[1..]),
            Some(' ') => (DiffLineKind::Context, &line[1..]),
            // An empty context line whose single space was stripped on the way.
            None => (DiffLineKind::Context, ""),
            Some(_) => return false,
        };
        let (old_no, new_no) = match kind {
            DiffLineKind::Context => {
                let numbers = (Some(self.old_no), Some(self.new_no));
                self.old_no += 1;
                self.new_no += 1;
                self.old_left = self.old_left.saturating_sub(1);
                self.new_left = self.new_left.saturating_sub(1);
                numbers
            }
            DiffLineKind::Add => {
                let number = Some(self.new_no);
                self.new_no += 1;
                self.new_left = self.new_left.saturating_sub(1);
                (None, number)
            }
            DiffLineKind::Del => {
                let number = Some(self.old_no);
                self.old_no += 1;
                self.old_left = self.old_left.saturating_sub(1);
                (number, None)
            }
        };
        self.lines.push(DiffLine { kind, old_no, new_no, text: text.to_string(), no_newline: false });
        true
    }
}

/// `-a[,b] +c[,d] @@ section` after the leading `@@ `.
fn parse_hunk_header(rest: &str) -> Option<Hunk> {
    let mut parts = rest.split_whitespace();
    let (old_start, old_len) = range(parts.next()?.strip_prefix('-')?)?;
    let (new_start, new_len) = range(parts.next()?.strip_prefix('+')?)?;
    Some(Hunk { lines: Vec::new(), old_no: old_start, new_no: new_start, old_left: old_len, new_left: new_len })
}

fn range(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

/// The path of a `---` or `+++` line: quoted or plain, minus a tab and timestamp, minus the
/// `a/` or `b/` prefix. `None` for `/dev/null`.
fn diff_path(rest: &str, prefix: &str) -> Option<String> {
    let path = match unquote(rest) {
        Some((path, _)) => path,
        None => rest.split('\t').next().unwrap_or(rest).trim_end().to_string(),
    };
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).map_or(path.clone(), str::to_string))
}

/// The path out of `a/x b/x` (the part of a `diff --git` line after the command).
fn header_path(rest: &str) -> Option<String> {
    if rest.starts_with('"') {
        let (_, used) = unquote(rest)?;
        let second = rest[used..].trim_start();
        let path = match unquote(second) {
            Some((path, _)) => path,
            None => second.to_string(),
        };
        return Some(path.strip_prefix("b/").map_or(path.clone(), str::to_string));
    }
    let bytes = rest.as_bytes();
    // `a/P b/P`: the halves around the middle space are equal after their prefixes.
    if bytes.len() >= 7 && bytes.len() % 2 == 1 {
        let mid = bytes.len() / 2;
        if bytes[mid] == b' ' && rest.is_char_boundary(mid) {
            let (left, right) = (&rest[..mid], &rest[mid + 1..]);
            if let (Some(l), Some(r)) = (left.strip_prefix("a/"), right.strip_prefix("b/"))
                && l == r
            {
                return Some(r.to_string());
            }
        }
    }
    // Different old and new paths (a rename names them on its own lines): take the last.
    rest.rfind(" b/").map(|i| rest[i + 3..].to_string())
}

fn unquote_or_plain(text: &str) -> String {
    unquote(text).map_or_else(|| text.trim_end().to_string(), |(path, _)| path)
}

/// Reads a C-style quoted string at the start of `text`: returns it and the bytes used.
fn unquote(text: &str) -> Option<(String, usize)> {
    let inner = text.strip_prefix('"')?;
    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), i + 2)),
            b'\\' => {
                i += 1;
                let escaped = *bytes.get(i)?;
                match escaped {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'v' => out.push(11),
                    b'0'..=b'7' => {
                        let mut value = 0u32;
                        let mut digits = 0;
                        while digits < 3 && bytes.get(i).is_some_and(|b| (b'0'..=b'7').contains(b)) {
                            value = value * 8 + u32::from(bytes[i] - b'0');
                            i += 1;
                            digits += 1;
                        }
                        out.push(u8::try_from(value & 0xff).unwrap_or(0));
                        continue;
                    }
                    other => out.push(other),
                }
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    None
}
