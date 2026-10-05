//! A command row whose description starts one space after its operand
//! placeholders (`set-property UNIT PROPERTY=VALUE... Sets one or more
//! ...`), at the very column the other rows of its block start theirs —
//! `docs/shapes.md` S-193 on a bare command row. With no column gap of its
//! own the row read as one spec and the command never reached the tree.

use super::*;

/// Fewest rows that must agree on one description column.
const MIN_AGREEING_ROWS: usize = 3;

/// The byte column where at least [`MIN_AGREEING_ROWS`] entry rows, and at
/// least half of the block's gapped entry rows, start their description.
pub(super) fn entry_description_column(block_lines: &[&str], baseline: usize) -> Option<usize> {
    if block_lines.iter().any(|l| l.contains('\t')) {
        return None;
    }
    let mut cols: Vec<usize> = Vec::new();
    for line in block_lines {
        if line.trim().is_empty() || leading_whitespace(line) > baseline + 1 {
            continue;
        }
        if let Some(gap) = find_multi_space_gap(line) {
            let rest = &line[gap..];
            cols.push(gap + (rest.len() - rest.trim_start().len()));
        }
    }
    let best = cols
        .iter()
        .copied()
        .max_by_key(|c| cols.iter().filter(|d| *d == c).count())?;
    let agree = cols.iter().filter(|d| **d == best).count();
    (agree >= MIN_AGREEING_ROWS && agree * 2 >= cols.len()).then_some(best)
}

/// `line` split at `col` into the command spelling and its description,
/// when the line has no gap of its own, the text before `col` is a command
/// name plus placeholders only, and the text after reads as a sentence.
pub(super) fn split_command_row_at_column(line: &str, col: usize) -> Option<(&str, String)> {
    if find_description_gap(line).is_some() || col < 2 || !line.is_char_boundary(col) {
        return None;
    }
    let (left, right) = line.split_at(col);
    if !left.ends_with(' ') || right.starts_with(' ') {
        return None;
    }
    let sentence = right.split_whitespace().count() >= 3
        && right.chars().next().is_some_and(char::is_alphabetic);
    let spec = left.trim();
    (sentence && command_name_with_operand_placeholders(spec).is_some())
        .then(|| (spec, right.trim().to_string()))
}
