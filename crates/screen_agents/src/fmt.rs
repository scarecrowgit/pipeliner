//! Text formatting shared by the cards and the detail panel.

use std::path::Path;

/// `950`, `12.3k`, `1.2M`.
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1_000.),
        _ => format!("{:.1}M", n as f64 / 1_000_000.),
    }
}

/// `$0.42`, `<$0.01`.
pub fn cost(usd: f64) -> String {
    if usd > 0. && usd < 0.01 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

/// `12m`, `3h`, `Sat 18:20`, `Nov 9` (local time).
pub fn when(t: u64) -> String {
    let now = ui::time::now_secs();
    ui::time::when_label(t, now, ui::time::utc_offset(now))
}

/// `nr/cc/claude-opus-5-5` → `claude-opus-5-5`.
pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// A path with the home folder as `~`.
pub fn path_label(p: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = p.display().to_string();
    match s.strip_prefix(&home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => s,
    }
}

/// First non-empty line of a Markdown text, without list markers and emphasis.
pub fn plain_line(md: &str) -> String {
    let line = md.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line
        .trim_start_matches(['-', '*', '#', '>'])
        .trim_start();
    line.replace("**", "").replace('`', "")
}

/// Markdown as plain text for the panel: emphasis and code marks dropped, list markers as
/// bullets, headings as plain lines (gpui's word-level Markdown layout mis-measures inside a
/// scrolling column; same approach as the issue panel).
pub fn plain_markdown(md: &str) -> String {
    md.trim()
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = &l[..l.len() - t.len()];
            let t = t.trim_start_matches('#').trim_start();
            let t = match t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Some(rest) => format!("{indent}• {rest}"),
                None => format!("{indent}{t}"),
            };
            t.replace("**", "").replace("__", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(tokens(812), "812");
        assert_eq!(tokens(48_200), "48.2k");
        assert_eq!(cost(0.004), "<$0.01");
        assert_eq!(cost(1.5), "$1.50");
        assert_eq!(short_model("nr/cc/claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(plain_line("\n- **Home** now has `90 days`.\nmore"), "Home now has 90 days.");
        assert_eq!(plain_markdown("## Done\n- **a:** `x`\n  - b"), "Done\n• a: x\n  • b");
    }
}
