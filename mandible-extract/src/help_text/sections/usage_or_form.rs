//! Usage lines that open a form of their own without an `or:` marker: a bare
//! `or` word followed by the tool's name (`   or  java [options] -jar ...`),
//! and a line wholly in parentheses that notes the form above it
//! (`(to execute a jar file)`), only when the line above it is itself a form.
//! `docs/shapes.md` S-264.

use super::*;

/// A line that opens a usage form of its own: a `usage:` or `or:` label, an
/// `or  <tool>` line, or a parenthesised note directly under a form line
/// (the nearest non-blank line above `lines[i]`).
pub(super) fn is_form_marker(lines: &[&str], i: usize, tool_name: Option<&str>) -> bool {
    let t = lines[i].trim_start();
    let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty());
    is_form_line(t, tool_name)
        || (is_form_note(t) && prev.is_some_and(|p| is_form_line(p.trim(), tool_name)))
}

/// A line that opens a form: a label, an `or` form, or the tool's own name.
fn is_form_line(t: &str, tool_name: Option<&str>) -> bool {
    starts_with_usage_prefix(t)
        || starts_with_or_marker(t)
        || is_or_word_form(t, tool_name)
        || tool_name.is_some_and(|n| {
            starts_with_tool_name(t, n) || starts_with_tool_name_spelled_differently(t, n)
        })
}

/// `or  <tool> ...`: the lowercase word `or`, then the tool's own name.
pub(super) fn is_or_word_form(t: &str, tool_name: Option<&str>) -> bool {
    let Some(name) = tool_name else { return false };
    let mut parts = t.splitn(2, char::is_whitespace);
    parts.next().is_some_and(|w| w == "or")
        && parts
            .next()
            .is_some_and(|rest| starts_with_tool_name(rest.trim_start(), name))
}

/// A line that is one parenthesised note and nothing else.
pub(super) fn is_form_note(t: &str) -> bool {
    t.starts_with('(')
        && t.ends_with(')')
        && t.matches('(').count() == 1
        && t.matches(')').count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn or_word_then_tool_name_opens_a_form() {
        assert!(is_or_word_form("or  java [options] -jar x", Some("java")));
        assert!(!is_or_word_form("or the other", Some("java")));
        assert!(!is_or_word_form("OR     java [options]", Some("java")));
        assert!(!is_or_word_form("or  java", None));
    }

    #[test]
    fn a_whole_parenthesised_line_is_a_note() {
        assert!(is_form_note("(to execute a class)"));
        assert!(!is_form_note("(a) b (c)"));
        assert!(!is_form_note("(to execute) a class"));
    }

    #[test]
    fn a_note_needs_a_form_line_above_it() {
        let t = Some("java");
        assert!(is_form_marker(
            &["  java [options] x", "  (to execute a class)"],
            1,
            t
        ));
        assert!(!is_form_marker(
            &["bs must match sector size", "  (def=512)"],
            1,
            t
        ));
        assert!(!is_form_marker(&["  (def=512)"], 0, t));
    }
}
