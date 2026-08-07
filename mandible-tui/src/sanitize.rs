//! Belt-and-braces defenses at the widget layer.
//!
//! Spec §4.1 makes `Text::sanitize` the IR boundary where untrusted text is
//! cleaned, and §9 says widgets may assume a `Text` is clean. But
//! [`mandible_core::CommandNode::name`] and a few other identity-ish fields
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

/// Truncate `s` to at most `max_width` display columns, breaking at the
/// last word boundary before the limit and appending an ellipsis (`…`,
/// itself 1 column wide) rather than cutting mid-word (spec §9.1: "the
/// ellipsis is a real signal that the detail pane has more; a mid-word
/// cut just looks broken"). Falls back to a hard, still-ellipsis-suffixed
/// truncation when there's no usable word boundary (a single token wider
/// than the whole budget) — some indication of "more" beats none. Returns
/// `s` unchanged if it already fits; returns an empty string if
/// `max_width` is 0.
pub fn truncate_to_width_ellipsis(s: &str, max_width: usize) -> String {
    truncate_to_width_marker(s, max_width, "…")
}

/// [`truncate_to_width_ellipsis`] with an explicit truncation marker, so a
/// terminal that cannot draw `…` gets `...` instead of tofu (see
/// [`crate::glyphs`]). The marker's own display width is reserved, which
/// matters because the ASCII fallback is three columns wide, not one —
/// assuming one would overflow the pane by two cells on every truncated
/// row, and overflowing a pane is how border corruption starts.
pub fn truncate_to_width_marker(s: &str, max_width: usize, marker: &str) -> String {
    if display_width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let marker_width = display_width(marker);
    if max_width <= marker_width {
        return truncate_to_width(marker, max_width);
    }
    let hard = truncate_to_width(s, max_width - marker_width);
    let base = match hard.rfind(char::is_whitespace) {
        // A word boundary strictly inside the truncated text: cut there.
        Some(idx) if idx > 0 => hard[..idx].trim_end(),
        _ => hard.as_str(),
    };
    if base.is_empty() {
        format!("{hard}{marker}")
    } else {
        format!("{base}{marker}")
    }
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

    #[test]
    fn ellipsis_truncation_breaks_at_word_boundary() {
        // "Add file contents to the index" truncated to 15 columns: a
        // hard cut would land mid-word ("Add file conte…"); the
        // word-boundary version must back up to the last full word
        // instead.
        let s = "Add file contents to the index";
        let truncated = truncate_to_width_ellipsis(s, 15);
        assert_eq!(truncated, "Add file…");
        assert!(display_width(&truncated) <= 15, "{truncated:?}");
    }

    #[test]
    fn ellipsis_truncation_falls_back_to_hard_cut_for_one_giant_word() {
        let s = "supercalifragilisticexpialidocious";
        let truncated = truncate_to_width_ellipsis(s, 10);
        assert!(display_width(&truncated) <= 10);
        assert!(truncated.ends_with('…'));
        assert!(truncated.len() > 1, "must not be just the ellipsis");
    }

    #[test]
    fn ellipsis_truncation_leaves_short_strings_untouched() {
        assert_eq!(truncate_to_width_ellipsis("short", 40), "short");
    }

    #[test]
    fn ellipsis_truncation_respects_the_exact_budget() {
        for width in 1..30 {
            let truncated = truncate_to_width_ellipsis("Use binary search to find a bug", width);
            assert!(
                display_width(&truncated) <= width,
                "width={width} truncated={truncated:?} used={}",
                display_width(&truncated)
            );
        }
    }
}
