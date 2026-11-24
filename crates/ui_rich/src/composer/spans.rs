//! The GPUI-free bookkeeping of a composer: keeping mention spans in step with edits,
//! attachment limits and their wording.

use std::ops::Range;

use gpui::ImageFormat;

use super::types::{Attachment, Mention};

/// How many attachments a message takes and how large each may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachmentLimits {
    /// Attachments per message.
    pub max_count: usize,
    /// Bytes per attachment.
    pub max_size: u64,
}

impl Default for AttachmentLimits {
    /// The mockups' limits: 10 per message, 20 MB each.
    fn default() -> Self {
        Self { max_count: 10, max_size: 20_000_000 }
    }
}

/// Byte length of the longest common prefix of `a` and `b`, on a char boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or_else(|| a.len().min(b.len()), |((i, _), _)| i)
}

/// Byte length of the longest common suffix of `a` and `b`, on a char boundary, at most
/// `limit` bytes.
fn common_suffix(a: &str, b: &str, limit: usize) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y || n + x.len_utf8() > limit {
            break;
        }
        n += x.len_utf8();
    }
    n
}

/// The span of `old` that an edit replaced and how many bytes replaced it: one contiguous
/// change found from the common prefix and suffix.
pub(crate) fn changed_span(old: &str, new: &str) -> (Range<usize>, usize) {
    let p = common_prefix(old, new);
    let s = common_suffix(&old[p..], &new[p..], old.len().min(new.len()) - p);
    (p..old.len() - s, new.len() - s - p)
}

/// Nothing but whitespace (or the end) follows `range` in `text`: a word typed straight
/// after a mention makes it a different name.
fn ends_alone(text: &str, range: &Range<usize>) -> bool {
    text[range.end..].chars().next().is_none_or(char::is_whitespace)
}

/// Move `mentions` along an edit from `old` to `new`: spans before the change stay, spans
/// after it shift, spans the change touched are dropped (their text stays as plain text).
/// A span that no longer reads its text, or now runs into the word after it, is dropped too.
pub(crate) fn reconcile(old: &str, new: &str, mentions: &mut Vec<Mention>) {
    if old == new {
        return;
    }
    let (span, added) = changed_span(old, new);
    mentions.retain_mut(|m| {
        if m.range.end <= span.start {
            // Before the change.
        } else if m.range.start >= span.end {
            let shift = |o: usize| o - span.len() + added;
            m.range = shift(m.range.start)..shift(m.range.end);
        } else {
            return false;
        }
        new.get(m.range.clone()) == Some(m.text.as_ref()) && ends_alone(new, &m.range)
    });
}

/// The mention a deletion from `caret` toward `left` (Backspace) or right (Delete) would cut
/// into: the caret is inside it or at the edge being deleted from.
pub(crate) fn mention_at(mentions: &[Mention], caret: usize, left: bool) -> Option<Range<usize>> {
    mentions
        .iter()
        .find(|m| {
            if left {
                m.range.start < caret && caret <= m.range.end
            } else {
                m.range.start <= caret && caret < m.range.end
            }
        })
        .map(|m| m.range.clone())
}

/// Why `attachment` cannot join `count` others under `limits`, if it cannot.
pub(crate) fn refusal(attachment: &Attachment, count: usize, limits: AttachmentLimits) -> Option<String> {
    let noun = if attachment.is_image() { "images" } else { "attachments" };
    if count >= limits.max_count {
        return Some(format!(
            "{} not added · {count} of {} {noun} already · remove one first",
            attachment.name, limits.max_count
        ));
    }
    if attachment.size > limits.max_size {
        return Some(format!(
            "{} is {} · {} max each",
            attachment.name,
            base::fmt::bytes(attachment.size),
            base::fmt::bytes(limits.max_size)
        ));
    }
    None
}

/// The note beside the chips: `2 of 10 images`, `1 uploading`, `20 MB max each`.
pub(crate) fn summary(attachments: &[Attachment], limits: AttachmentLimits) -> Vec<String> {
    let noun = if attachments.iter().all(Attachment::is_image) { "images" } else { "attachments" };
    let mut parts = vec![format!("{} of {} {noun}", attachments.len(), limits.max_count)];
    let uploading = attachments.iter().filter(|a| a.progress.is_some()).count();
    if uploading > 0 {
        parts.push(format!("{uploading} uploading"));
    }
    parts.push(format!("{} max each", base::fmt::bytes(limits.max_size)));
    parts
}

/// File extension for an image format.
fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    }
}

/// Name of the `n`th image pasted into a composer (`pasted-image-1.png`).
pub(crate) fn pasted_name(n: u64, format: ImageFormat) -> String {
    format!("pasted-image-{n}.{}", extension(format))
}
