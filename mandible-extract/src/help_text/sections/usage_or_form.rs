//! Usage lines that open a form of their own without an `or:` marker: a bare
//! `or` word followed by the tool's name (`   or  java [options] -jar ...`),
//! and a line wholly in parentheses that notes the form above it
//! (`(to execute a jar file)`). `docs/shapes.md` S-264.

use super::*;

/// A line that opens a usage form of its own: a `usage:` or `or:` label, an
/// `or  <tool>` line, or a parenthesised note.
pub(super) fn is_form_marker(t: &str, tool_name: Option<&str>) -> bool {
    starts_with_usage_prefix(t)
        || starts_with_or_marker(t)
        || is_or_word_form(t, tool_name)
        || is_form_note(t)
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
}
