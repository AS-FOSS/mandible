//! Belt-and-braces defenses at the widget layer.
//!
//! Spec §4.1 makes `Text::sanitize` the IR boundary where untrusted text is
//! cleaned, and §9 says widgets may assume a `Text` is clean. But
//! [`mantui_core::CommandNode::name`] and a few other identity-ish fields
//! (aliases, `group`) are typed as plain `String`, not `Text` — they're
//! meant to be tool/subcommand identifiers, not prose, so the IR doesn't
//! force them through sanitization. A pathological or adversarial catalog
//! entry could still put a newline in a name. This module is the
//! extra defense the spec calls "belt-and-braces": display-width-safe
//! truncation plus stripping any stray control characters from those
//! specific fields, applied right before they reach a `Span`.

use unicode_width::UnicodeWidthChar;

/// Strip C0 control characters (including newlines and tabs) from a plain
/// `String` field that isn't guaranteed to have gone through
/// `Text::sanitize`. Cheap no-op on the well-formed input this will see in
/// practice.
pub fn defensive_single_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Truncate `s` to at most `max_width` display columns (per
/// `unicode-width`), never by byte or `char` count — required so CJK/emoji
/// (double-width) text can't overflow a pane border by one cell (spec §9).
/// Returns the truncated string; does not pad.
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > max_width {
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

/// The display width of `s` per `unicode-width`.
pub fn display_width(s: &str) -> usize {
    s.chars().filter_map(UnicodeWidthChar::width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_newlines_and_tabs() {
        assert_eq!(defensive_single_line("a\nb\tc"), "abc");
    }

    #[test]
    fn truncates_ascii_by_count() {
        assert_eq!(truncate_to_width("hello world", 5), "hello");
    }

    #[test]
    fn truncates_wide_chars_without_overflow() {
        // Each CJK char is width 2; budget of 5 must not include a 6th
        // column, so only 2 chars (width 4) fit, not 3 (width 6).
        let s = "日本語";
        let truncated = truncate_to_width(s, 5);
        assert!(display_width(&truncated) <= 5);
        assert_eq!(truncated, "日本");
    }

    #[test]
    fn handles_emoji_width() {
        let s = "🎉🎉🎉🎉";
        let truncated = truncate_to_width(s, 5);
        assert!(display_width(&truncated) <= 5);
    }

    #[test]
    fn zero_width_budget_yields_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }
}
