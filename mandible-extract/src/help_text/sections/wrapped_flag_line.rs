//! A wrapped description line that begins with a flag spelling
//! (`-U/--multiline flag.`) at its description's indent continues that
//! description; it opens no row. corpus/rg/14.1.0-wrapped-flag-line, S-011.

use super::flag_rows::FlagsBlockRow;
use super::leading_whitespace;

/// `line` repeats the previous description line's indent, deeper than its row.
pub(super) fn continues_wrapped_description(
    rows: &[FlagsBlockRow<'_>],
    current_entry_line: Option<&str>,
    line: &str,
) -> bool {
    let (Some(entry), Some(FlagsBlockRow::Continuation(_, prev))) =
        (current_entry_line, rows.last())
    else {
        return false;
    };
    let indent = leading_whitespace(line);
    indent > leading_whitespace(entry) && indent == leading_whitespace(prev)
}

/// True when `line` is an example command line inside the open row's
/// description: it sits deeper than the description line just before it,
/// that line ends in a colon (`one can use:`), and it has no description
/// column of its own. Such a line stays description text. See
/// docs/shapes.md S-011.
pub(super) fn is_example_under_colon(
    rows: &[FlagsBlockRow<'_>],
    current_entry_line: Option<&str>,
    line: &str,
) -> bool {
    let (Some(_), Some(FlagsBlockRow::Continuation(text, prev))) =
        (current_entry_line, rows.last())
    else {
        return false;
    };
    text.ends_with(':')
        && leading_whitespace(line) > leading_whitespace(prev)
        && !line.trim().contains("  ")
}
