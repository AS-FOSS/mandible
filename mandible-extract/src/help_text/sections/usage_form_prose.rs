//! A usage form's own trailing prose (`vim [arguments] -t tag   edit file
//! where tag is defined`) read as the description of the one bare flag the
//! form introduces (`docs/shapes.md` S-152). Split out of `usage.rs` to keep
//! that file under its own size ceiling (AGENTS.md §2).

use super::*;

/// Give the flag `out[at]` the prose behind `line`'s description gap, when
/// it is the only flag the line added and the line has such prose.
pub(super) fn describe_form_flag(
    out: &mut [Entity],
    line: &str,
    line_start: usize,
    bare_flag_at: Option<usize>,
    columns: &[usize],
) {
    let Some(at) = bare_flag_at else { return };
    if out.len() != line_start + 1 {
        return;
    }
    if let Some(prose) = usage_form_trailing_prose(line, columns) {
        out[at].description = Some(Text::sanitize(&prose));
    }
}

/// The prose a usage form carries behind its description-column gap
/// (`vim [arguments] -t tag          edit file where tag is defined`): the
/// text after the first run of [`TAIL_OPERAND_GAP_SPACES`] spaces, or after
/// a shorter run of at least two spaces that ends exactly at a column a
/// sibling form's wide gap already established (`-q [errorfile]  edit file
/// with first error`, one space short of the column). Kept only when it
/// reads as plain words (at least three, none opening with a flag, bracket
/// or angle group). `columns` holds the char columns [`usage_prose_column`]
/// found. See docs/shapes.md S-152.
fn usage_form_trailing_prose(line: &str, columns: &[usize]) -> Option<String> {
    let start = usage_prose_start(line, columns)?;
    plain_prose(&line[start..])
}

fn plain_prose(rest: &str) -> Option<String> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    let plain = words.iter().all(|w| {
        !w.starts_with(['-', '[', '<', '{', '(', '|', '/'])
            && !w.chars().any(|c| matches!(c, '|' | '<' | '>'))
    });
    let first_alpha = words[0].chars().next().is_some_and(char::is_alphabetic);
    (plain && first_alpha).then(|| words.join(" "))
}

/// Byte offset where `line`'s trailing prose begins, if it has one.
fn usage_prose_start(line: &str, columns: &[usize]) -> Option<usize> {
    let head = cut_before_description_gap(line);
    if head.len() < line.len() {
        let rest = &line[head.len()..];
        return Some(head.len() + (rest.len() - rest.trim_start().len()));
    }
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    columns.iter().find_map(|&col| {
        let (byte, c) = *chars.get(col)?;
        let gap_before = col >= 2 && chars[col - 1].1 == ' ' && chars[col - 2].1 == ' ';
        (!c.is_whitespace() && gap_before).then_some(byte)
    })
}

/// The char columns at which two or more of `usage_lines` start their wide-
/// gap trailing text (of any length: `[file ...]  Edit file(s)` counts) at the very same place: the description column the
/// forms share.
pub(super) fn usage_prose_column(usage_lines: &[String]) -> Vec<usize> {
    let mut seen: Vec<usize> = Vec::new();
    for line in usage_lines {
        let head = cut_before_description_gap(line);
        if head.len() == line.len() {
            continue;
        }
        let rest = &line[head.len()..];
        let start = head.len() + (rest.len() - rest.trim_start().len());
        if line[start..]
            .chars()
            .next()
            .is_some_and(char::is_alphabetic)
        {
            seen.push(line[..start].chars().count());
        }
    }
    let mut cols: Vec<usize> = seen
        .iter()
        .copied()
        .filter(|c| seen.iter().filter(|d| *d == c).count() >= 2)
        .collect();
    cols.sort_unstable();
    cols.dedup();
    cols
}
