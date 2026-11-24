//! Text the loop itself puts into the conversation, and pull request links it spots there.

/// Opens a user message written by the loop, not by a person: a reminder to finish, the
/// summary of a compacted conversation. A transcript can show these differently.
pub const NOTE_OPEN: &str = "<harness-note>";
/// Closes what [`NOTE_OPEN`] opens.
pub const NOTE_CLOSE: &str = "</harness-note>";

/// `text` as a message from the loop.
pub(crate) fn note(text: &str) -> String {
    format!("{NOTE_OPEN}\n{}\n{NOTE_CLOSE}", text.trim())
}

/// Whether a user message's text was written by the loop.
pub fn is_note(text: &str) -> bool {
    text.trim_start().starts_with(NOTE_OPEN)
}

/// Pull and merge request links in `text`, each once, without trailing punctuation or
/// sub-pages: GitHub `/pull/N`, GitLab `/-/merge_requests/N`, Bitbucket `/pull-requests/N`.
pub(crate) fn pr_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("https://") {
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | '`' | '<' | ','))
            .unwrap_or(tail.len());
        let url = &tail[..end];
        if let Some(link) = pr_link(url)
            && !out.contains(&link)
        {
            out.push(link);
        }
        rest = &tail[end..];
    }
    out
}

/// `url` cut after the request number when it points at a pull or merge request.
fn pr_link(url: &str) -> Option<String> {
    for marker in ["/pull/", "/-/merge_requests/", "/pull-requests/"] {
        let Some(at) = url.find(marker) else { continue };
        let number_start = at + marker.len();
        let digits = url[number_start..].chars().take_while(char::is_ascii_digit).count();
        // A host and a project path must come before the marker.
        if digits > 0 && url["https://".len()..at].contains('/') {
            return Some(url[..number_start + digits].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_recognised() {
        let text = note("  Continue.  ");
        assert_eq!(text, "<harness-note>\nContinue.\n</harness-note>");
        assert!(is_note(&text));
        assert!(!is_note("Continue."));
    }

    #[test]
    fn pr_links_are_found_once_and_trimmed() {
        let text = "Opened https://github.com/acme/pay/pull/42. See (https://github.com/acme/pay/pull/42/files) \
                    and https://gitlab.com/g/sub/p/-/merge_requests/7, not https://github.com/acme/pay/issues/3 \
                    or https://github.com/pull/9.";
        assert_eq!(
            pr_links(text),
            ["https://github.com/acme/pay/pull/42", "https://gitlab.com/g/sub/p/-/merge_requests/7"]
        );
        assert!(pr_links("no links, and https://example.com/pull/ has no number").is_empty());
    }
}
