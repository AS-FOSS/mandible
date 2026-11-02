//! Long-first flag heads, `--long -X value` (S-253): the block-level
//! decision, and a head wrapped onto two lines (`--delete-chain` /
//! `-X [chain]  Delete`). corpus/ip6tables-translate/1.8.10.

use super::flag_rows::FlagsBlockRow;
use super::{block_is_long_first, leading_whitespace, secondary_column_offsets};

/// The two lines joined into one head, when `line` is the second half of a
/// head `prev` opened: `prev` is one `--long` token and `line`, indented
/// deeper, starts with a short flag.
pub(super) fn join_wrapped_head(prev: &str, line: &str) -> Option<String> {
    let first = prev.trim();
    let first = crate::help_text::grammar::strip_bang_lead(first).unwrap_or(first);
    let lone_long = first.starts_with("--")
        && first.len() > 2
        && !first.contains(char::is_whitespace)
        && !first.contains(['=', ',', '|']);
    let rest = line.trim_start();
    let short_len = rest.find([' ', '\t']).unwrap_or(rest.len());
    let mut chars = rest[..short_len].chars();
    let short = chars.next() == Some('-')
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.next().is_none();
    (lone_long && short && leading_whitespace(line) > leading_whitespace(prev))
        .then(|| format!("{} {rest}", prev.trim_end()))
}

/// The raw lines of a block's entry rows.
pub(super) fn entry_row_lines<'a>(rows: &[FlagsBlockRow<'a>]) -> Vec<&'a str> {
    rows.iter()
        .filter_map(|r| match r {
            FlagsBlockRow::Entry(l) => Some(*l),
            _ => None,
        })
        .collect()
}

/// True when the block writes its heads long first (S-253), or has a head
/// wrapped onto two lines.
pub(super) fn block_reads_long_first(rows: &[FlagsBlockRow<'_>], entry_lines: &[&str]) -> bool {
    block_is_long_first(entry_lines) || rows.iter().any(|r| matches!(r, FlagsBlockRow::Joined(_)))
}

/// The block's secondary flag columns. A long-first block's second column
/// is the short alias, not a second flag.
pub(super) fn block_column_offsets(
    long_first: bool,
    entry_lines: &[&str],
    dash_cells: bool,
) -> Vec<usize> {
    if long_first {
        return Vec::new();
    }
    secondary_column_offsets(entry_lines, dash_cells)
}

/// Replaces the entry row just pushed with the joined head, when `line`
/// completes it; true when it did.
pub(super) fn join_into_previous<'a>(rows: &mut Vec<FlagsBlockRow<'a>>, line: &str) -> bool {
    let Some(FlagsBlockRow::Entry(prev)) = rows.last() else {
        return false;
    };
    let Some(joined) = join_wrapped_head(prev, line) else {
        return false;
    };
    rows.pop();
    rows.push(FlagsBlockRow::Joined(joined));
    true
}

/// The entry a joined head makes: its spec and description, no choices.
pub(super) fn joined_head_entry(text: &str) -> super::FlagRowEntry {
    let (spec, desc) = super::split_entry(text, true);
    (spec, desc, Vec::new())
}
