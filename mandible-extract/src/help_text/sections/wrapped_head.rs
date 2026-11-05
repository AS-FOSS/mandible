//! Long-first flag heads, `--long -X value` (S-254): the block-level
//! decision, and a head wrapped onto two lines (`--delete-chain` /
//! `-X [chain]  Delete`). corpus/ip6tables-translate/1.8.10.

use super::flag_rows::FlagsBlockRow;
use super::{
    block_is_long_first, entry_row_carries_own_description, find_multi_space_gap,
    leading_whitespace, looks_like_flag_start, secondary_column_offsets,
};

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

/// The value words of the continuation `text` and its description, when
/// it finishes the value list of the dash-led head `prev`: a few value
/// words, then ` : ` and the description (`--change-counters -C chain`
/// over `[rulenum] pcnt bcnt : change counters`). S-258.
pub(super) fn value_tail_before_colon<'a>(prev: &str, text: &'a str) -> Option<(&'a str, &'a str)> {
    let (values, desc) = text.split_once(" : ")?;
    let plain = values
        .split_whitespace()
        .all(|w| w.len() <= 12 && !w.starts_with('-'));
    let fits = (1..=4).contains(&values.split_whitespace().count()) && plain;
    (prev.trim_start().starts_with("--") && !entry_row_carries_own_description(prev) && fits)
        .then_some((values, desc))
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

/// True when the block writes its heads long first (S-254), or has a head
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

/// True when `line` holds spellings and values only: no column gap and no
/// description on it.
fn spelling_only(line: &str) -> bool {
    find_multi_space_gap(line.trim_start()).is_none() && !entry_row_carries_own_description(line)
}

/// True when the spelling-only line `lines[at]` and any spelling-only
/// lines directly after it at the same indent are followed by a deeper
/// line: the description they share.
fn stack_has_description(lines: &[&str], at: usize) -> bool {
    let indent = leading_whitespace(lines[at]);
    if !spelling_only(lines[at]) {
        return false;
    }
    for line in &lines[at + 1..] {
        match leading_whitespace(line).cmp(&indent) {
            _ if line.trim().is_empty() => return false,
            std::cmp::Ordering::Greater => return !looks_like_flag_start(line.trim_start()),
            std::cmp::Ordering::Equal
                if looks_like_flag_start(line.trim_start()) && spelling_only(line) => {}
            _ => return false,
        }
    }
    false
}

/// The name of the first spelling on `line`, without its dashes.
fn first_name(line: &str) -> String {
    line.trim_start()
        .trim_start_matches('-')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .to_lowercase()
}

/// True when the letters of the shorter name appear in order in the longer
/// one (`cp` in `classpath`, `ea` in `enableassertions`): two lines that
/// stack above one description are spellings of one flag only then.
fn abbreviates(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut rest = long.chars();
    !short.is_empty() && short.chars().all(|c| rest.any(|l| l == c))
}

/// Joins `lines[at]` into the entry row just pushed when both are
/// spelling-only lines at one indent stacked above one shared description
/// (`-cp <path>` / `-classpath <path>` / `--class-path <path>` over its
/// text): the lines are one alias run. See docs/shapes.md S-258.
pub(super) fn stack_into_previous<'a>(
    rows: &mut Vec<FlagsBlockRow<'a>>,
    lines: &[&str],
    at: usize,
) -> bool {
    let line = lines[at];
    let prev = match rows.last() {
        Some(FlagsBlockRow::Entry(p)) => p.trim_end().to_string(),
        Some(FlagsBlockRow::Joined(p)) => p.clone(),
        _ => return false,
    };
    let same_indent = leading_whitespace(line) == leading_whitespace(&prev);
    if !spelling_only(&prev)
        || !same_indent
        || !abbreviates(&first_name(&prev), &first_name(line))
        || !stack_has_description(lines, at)
    {
        return false;
    }
    rows.pop();
    rows.push(FlagsBlockRow::Joined(format!("{prev}, {}", line.trim())));
    true
}

/// An `@` row of one or two lower-case words with no description on its
/// own line and the description on the deeper lines below it
/// (`@argument files` / `one or more argument files containing
/// options`): the entry and the index past its description. docs/shapes.md
/// S-258.
pub(super) fn hanging_argfile_row(
    lines: &[&str],
    at: usize,
) -> Option<(super::FlagRowEntry, usize)> {
    let line = lines[at];
    let words: Vec<&str> = line.split_whitespace().collect();
    let head = words.first()?.strip_prefix('@')?;
    let plain = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase());
    if words.len() > 2 || !plain(head) || !words.iter().skip(1).all(|w| plain(w)) {
        return None;
    }
    let indent = leading_whitespace(line);
    let end = (at + 1..lines.len())
        .find(|&j| lines[j].trim().is_empty() || leading_whitespace(lines[j]) <= indent)
        .unwrap_or(lines.len());
    if end == at + 1 {
        return None;
    }
    let desc = lines[at + 1..end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let value_name = line.trim().trim_start_matches('@').to_string();
    Some(((value_name, desc, Vec::new()), end))
}

/// Folds a continuation into the entry's head when it finishes an unclosed
/// placeholder, or the value list of a description-less head
/// (`[rulenum] pcnt bcnt : text`): true when it did. S-048, S-258.
pub(super) fn extend_open_head(last: &mut super::FlagRowEntry, text: &str) -> bool {
    if super::flag_rows::placeholder_left_open(&last.0) {
        last.0.push_str(text);
        return true;
    }
    let Some((values, desc)) = last
        .1
        .is_empty()
        .then(|| value_tail_before_colon(&last.0, text))
        .flatten()
    else {
        return false;
    };
    last.0.push(' ');
    last.0.push_str(values);
    last.1.push_str(desc);
    true
}

/// Joins `lines[at]` into the entry row before it as a wrapped head or as
/// stacked alias lines; true when it did.
pub(super) fn join_head_lines<'a>(
    rows: &mut Vec<FlagsBlockRow<'a>>,
    lines: &[&str],
    at: usize,
) -> bool {
    join_into_previous(rows, lines[at]) || stack_into_previous(rows, lines, at)
}

/// Reads the hanging argfile row at `*at` into `entry` and moves `*at` past
/// it; true when there was one.
pub(super) fn take_hanging_argfile(
    lines: &[&str],
    at: &mut usize,
    entry: &mut Option<super::FlagRowEntry>,
) -> bool {
    let Some((found, end)) = hanging_argfile_row(lines, *at) else {
        return false;
    };
    *entry = Some(found);
    *at = end;
    true
}
