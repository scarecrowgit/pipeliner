//! Hub notices as toasts.

use hub::HubNotice;
use hub::api::NoticeLevel;
use ui::prelude::*;

/// Longest detail a toast shows; the rest of an error chain is cut.
const DETAIL_CHARS: usize = 96;

/// The toast for a notice: the sentence, its cause after a dot, an icon by level.
pub fn toast_for(notice: &HubNotice) -> Toast {
    let toast = Toast::new(notice.text.clone());
    let toast = match notice.level {
        NoticeLevel::Success => toast,
        NoticeLevel::Info => toast.no_icon(),
        NoticeLevel::Warning => toast.icon(IconName::Alert, WARNING),
        NoticeLevel::Error => toast.error(),
    };
    match &notice.detail {
        Some(detail) => {
            let line = base::fmt::first_line(detail);
            toast.detail(base::fmt::truncate_chars(line, DETAIL_CHARS))
        }
        None => toast,
    }
}
