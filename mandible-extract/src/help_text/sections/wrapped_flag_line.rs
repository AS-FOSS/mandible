//! A wrapped description line that begins with a flag spelling.
//!
//! A description that wraps can land a flag reference at the start of a
//! physical line (`-U/--multiline flag.`). Row detection reads that as a
//! new row, fabricating a flag and ending the real row's description. Such
//! a line sits at the indent the description's previous line already used,
//! deeper than its row, so it continues that description. See docs/shapes.md
//! S-011.

use super::flag_rows::FlagsBlockRow;
use super::leading_whitespace;

/// True when `line` (about to be classified) repeats the indent of the
/// description line just before it and sits deeper than the open row.
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
