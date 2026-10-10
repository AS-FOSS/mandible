//! A flag row whose description starts one space after its value, at the
//! very column every other row of the same block starts its description
//! (`--include FILE add an #include file ...` among rows padded to column
//! 19) — `docs/shapes.md` S-193. The row has no column gap of its own, so
//! the ordinary splitter reads the whole line as the spec and the
//! description never reaches the tree.

use super::*;

/// Fewest rows that must agree on one description column before a row with
/// no gap of its own is split there.
const MIN_AGREEING_ROWS: usize = 3;

/// The byte column where at least [`MIN_AGREEING_ROWS`] flag rows, and at
/// least half of the block's gapped flag rows, start their description.
pub(super) fn block_description_column(entry_lines: &[&str]) -> Option<usize> {
    if entry_lines.iter().any(|l| l.contains('\t')) {
        return None;
    }
    let mut cols: Vec<usize> = Vec::new();
    for line in entry_lines {
        if !line.trim_start().starts_with('-') {
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

/// `line` split at `col`, when it has no gap of its own (a lone long spelling
/// that overruns the column included), the text before
/// `col` is a short flag spec and the text after reads as a sentence.
pub(super) fn split_at_block_column(line: &str, col: usize) -> Option<(String, String)> {
    if find_description_gap(line).is_some() || !line.is_char_boundary(col) || col < 2 {
        return None;
    }
    let (left, right) = line.split_at(col);
    if !left.ends_with(' ') || left.trim_end().is_empty() || right.starts_with(' ') {
        return None;
    }
    let spec = left.trim();
    let tokens = spec.split_whitespace().count();
    let sentence = right.split_whitespace().count() >= 3
        && right.chars().next().is_some_and(char::is_alphabetic);
    // A lone spelling is split only when the text after it opens with a
    // word, not with the spelling's own value (`--skip-files FILES, --skip
    // FILES`): that first word carries a lowercase letter and no alias
    // follows.
    let lone_ok = tokens > 1
        || (right
            .split_whitespace()
            .next()
            .is_some_and(|w| w.chars().any(char::is_lowercase))
            && !right.contains(", -"));
    (spec.starts_with('-') && (1..=3).contains(&tokens) && sentence && lone_ok)
        .then(|| (spec.to_string(), right.trim().to_string()))
}
