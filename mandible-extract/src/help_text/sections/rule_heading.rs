//! A section heading drawn between two rules of box-drawing glyphs
//! (clipanion's `━━━ Options ━━━━━━━━`), read as the plain heading its
//! label names. The label is the heading; the glyph runs are layout.
//! Rewritten to `Label:` once, on the raw text before layout analysis, so
//! the unmodified engine sees an ordinary `Options:` heading (generic, so
//! no group) rather than the whole rule kept verbatim as a flag's group.
//! See docs/shapes.md S-213.

/// True for a horizontal box-drawing glyph (U+2500..=U+257F).
fn is_rule_glyph(c: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&c)
}

/// The label of a `━━━ Label ━━━━` line, or `None` for any other line.
/// Both runs are at least two glyphs; the label is plain words (the class
/// `is_section_heading_line` accepts) and the line carries nothing else.
fn rule_bracketed_label(line: &str) -> Option<&str> {
    let t = line.trim();
    let after_head = t.trim_start_matches(is_rule_glyph);
    let inner = after_head.trim_end_matches(is_rule_glyph);
    let head = t.chars().count() - after_head.chars().count();
    let tail = after_head.chars().count() - inner.chars().count();
    if head < 2 || tail < 2 || !inner.starts_with(char::is_whitespace) {
        return None;
    }
    if !inner.ends_with(char::is_whitespace) {
        return None;
    }
    let label = inner.trim();
    let plain = !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_');
    plain.then_some(label)
}

/// `raw` with each rule-bracketed heading line replaced by `Label:`.
pub(super) fn rewrite_rule_bracketed_headings(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        match rule_bracketed_label(line) {
            Some(label) => {
                out.push_str(label);
                out.push(':');
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_between_two_glyph_runs_is_the_heading() {
        assert_eq!(
            rule_bracketed_label("━━━ Options ━━━━━━━━━━"),
            Some("Options")
        );
        assert_eq!(
            rule_bracketed_label("── Global flags ──"),
            Some("Global flags")
        );
    }

    #[test]
    fn other_lines_are_left_alone() {
        for l in [
            "━━━━━━━━━━",
            "━ Options ━",
            "Options",
            "━━━ a: b ━━━",
            "━━━ Options",
            "--- Options ---",
        ] {
            assert_eq!(rule_bracketed_label(l), None, "{l}");
        }
    }
}
