//! The old app's inline encodings in user text, turned into readable text.
//!
//! The old chat kept one flat list of messages for the model, so a file attached with `@path`,
//! a `!command` the person ran and a "Moved to ..." divider were each stored as a user text
//! block wrapped in a tag with escaped attributes. The new store keeps plain text, so the
//! decoders below take those wrappers apart and [`readable`] writes what they held as Markdown.

const FILE_OPEN: &str = "<file path=\"";
const NOTE_OPEN: &str = "<workdir_note text=\"";
const SHELL_OPEN: &str = "<shell_output command=\"";
const SHELL_PREFACE: &str =
    "The user ran this command in the workspace shell; the output is context for the next request.\n";

fn unescape_attr(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&amp;", "&")
}

/// `(path, content)` of an attached file block.
pub(super) fn decode_file(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix(FILE_OPEN)?;
    let (path, body) = rest.split_once("\">\n")?;
    let body = body.strip_suffix("\n</file>")?;
    Some((unescape_attr(path), body.to_string()))
}

/// `(command, output, failed)` of a `!command` run block.
pub(super) fn decode_shell(s: &str) -> Option<(String, String, bool)> {
    let rest = s.strip_prefix(SHELL_OPEN)?;
    let (command, rest) = rest.split_once("\" failed=\"")?;
    let (failed, body) = rest.split_once("\">\n")?;
    let body = body.strip_suffix("\n</shell_output>")?;
    let body = body.strip_prefix(SHELL_PREFACE).unwrap_or(body);
    Some((unescape_attr(command), body.to_string(), failed == "true"))
}

/// `(text, note)` of a divider: the line shown in the transcript and the context sent along.
pub(super) fn decode_note(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix(NOTE_OPEN)?;
    let (text, body) = rest.split_once("\">\n")?;
    let note = body.strip_suffix("\n</workdir_note>")?;
    Some((unescape_attr(text), note.to_string()))
}

/// Whether a user text block is one of the encodings rather than something the person typed.
pub(super) fn is_encoded(s: &str) -> bool {
    decode_note(s).is_some() || decode_shell(s).is_some() || decode_file(s).is_some()
}

/// A fence of backticks longer than any run inside `body`, so the body cannot close it.
fn fence(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

/// The text of a user block as a person reads it: encodings become Markdown, anything else
/// is returned as it is.
pub(super) fn readable(s: &str) -> String {
    if let Some((text, note)) = decode_note(s) {
        let line = format!("\u{2014} {text} \u{2014}");
        return if note.trim().is_empty() {
            line
        } else {
            format!("{line}\n{note}")
        };
    }
    if let Some((command, output, failed)) = decode_shell(s) {
        let mut body = format!("$ {command}");
        if !output.is_empty() {
            body.push('\n');
            body.push_str(&output);
        }
        let fence = fence(&body);
        let head = if failed {
            "Shell command (failed):"
        } else {
            "Shell command:"
        };
        return format!("{head}\n{fence}\n{body}\n{fence}");
    }
    if let Some((path, content)) = decode_file(s) {
        let fence = fence(&content);
        return format!("Attached file `{path}`:\n{fence}\n{content}\n{fence}");
    }
    s.to_string()
}

/// `text` cut to `max_chars` characters with an ellipsis, as the old app cut titles.
pub(super) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}\u{2026}", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The encoders of the old app, kept here so the decoders are tested against what it wrote.
    fn escape_attr(s: &str) -> String {
        s.replace('&', "&amp;").replace('"', "&quot;")
    }

    fn encode_file(path: &str, content: &str) -> String {
        format!("{FILE_OPEN}{}\">\n{content}\n</file>", escape_attr(path))
    }

    fn encode_shell(command: &str, output: &str, is_error: bool) -> String {
        format!(
            "{SHELL_OPEN}{}\" failed=\"{is_error}\">\n{SHELL_PREFACE}{output}\n</shell_output>",
            escape_attr(command)
        )
    }

    fn encode_note(text: &str, note: &str) -> String {
        format!("{NOTE_OPEN}{}\">\n{note}\n</workdir_note>", escape_attr(text))
    }

    #[test]
    fn a_file_block_round_trips_with_quotes_and_ampersands() {
        let block = encode_file("a\"b&c.rs", "line 1\n</file>\nline 3");
        assert_eq!(
            decode_file(&block),
            Some(("a\"b&c.rs".into(), "line 1\n</file>\nline 3".into()))
        );
        assert_eq!(
            decode_file(&encode_file("src/a.rs", "1: fn a() {}")),
            Some(("src/a.rs".into(), "1: fn a() {}".into()))
        );
    }

    #[test]
    fn a_shell_block_round_trips_with_its_failure_flag() {
        assert_eq!(
            decode_shell(&encode_shell("ls", "a.rs", false)),
            Some(("ls".into(), "a.rs".into(), false))
        );
        assert_eq!(
            decode_shell(&encode_shell("echo \"x\" && ls", "x\n", true)),
            Some(("echo \"x\" && ls".into(), "x\n".into(), true))
        );
        // A block written without the preface still decodes.
        let bare = "<shell_output command=\"ls\" failed=\"false\">\nout\n</shell_output>";
        assert_eq!(decode_shell(bare), Some(("ls".into(), "out".into(), false)));
    }

    #[test]
    fn a_divider_round_trips() {
        let block = encode_note("Moved to \"x\" \u{b7} clean on main", "The working folder is now /x.");
        assert_eq!(
            decode_note(&block),
            Some((
                "Moved to \"x\" \u{b7} clean on main".into(),
                "The working folder is now /x.".into()
            ))
        );
    }

    #[test]
    fn plain_text_and_broken_blocks_are_not_decoded() {
        for text in [
            "fix it",
            "",
            "<file path=\"a\">no newline</file>",
            "<file path=\"a\">\nbody without end",
        ] {
            assert!(!is_encoded(text), "{text:?}");
            assert_eq!(readable(text), text);
        }
        assert!(is_encoded(&encode_file("a", "b")));
    }

    #[test]
    fn an_attached_file_reads_as_a_fenced_block() {
        let text = readable(&encode_file("src/a.rs", "fn a() {}"));
        assert_eq!(text, "Attached file `src/a.rs`:\n```\nfn a() {}\n```");
        // Content with its own fence gets a longer one.
        let text = readable(&encode_file("README.md", "```rust\nx\n```"));
        assert!(text.contains("````\n```rust"), "{text}");
    }

    #[test]
    fn a_shell_run_reads_as_a_command_and_its_output() {
        assert_eq!(
            readable(&encode_shell("ls", "a.rs", false)),
            "Shell command:\n```\n$ ls\na.rs\n```"
        );
        assert_eq!(
            readable(&encode_shell("false", "", true)),
            "Shell command (failed):\n```\n$ false\n```"
        );
    }

    #[test]
    fn a_divider_reads_as_its_line_and_note() {
        assert_eq!(
            readable(&encode_note("Moved to x", "Folder is /x.")),
            "\u{2014} Moved to x \u{2014}\nFolder is /x."
        );
        assert_eq!(readable(&encode_note("Moved to x", "")), "\u{2014} Moved to x \u{2014}");
    }

    #[test]
    fn titles_are_cut_like_the_old_app_cut_them() {
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
