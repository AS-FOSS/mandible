//! A row of a flag's sub-option table whose name is `=` plus an all-caps
//! placeholder word (`=FILE  list to FILE`, `=<file>  ...`) is the flag's
//! value form, not one of its choices. Its text stays with the flag's
//! description. See docs/shapes.md S-204 and corpus/as/2.42-listing.

use super::entry::find_description_gap;
use super::flag_rows::FlagsBlockRow;

/// Per row: `true` for the one continuation row that is the sole
/// `=`-prefixed row of its run of continuation rows (a flag's sub-option
/// table whose other rows are not `=`-prefixed). A table where every row
/// is `=`-prefixed (an llvm cl::opt enum) names literal choices, so none
/// of its rows qualify.
pub(super) fn mark_sole_equals_rows(rows: &[FlagsBlockRow<'_>]) -> Vec<bool> {
    let mut marks = vec![false; rows.len()];
    let mut i = 0;
    while i < rows.len() {
        if !matches!(rows[i], FlagsBlockRow::Continuation(..)) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut equals = Vec::new();
        while let Some(FlagsBlockRow::Continuation(text, _)) = rows.get(j) {
            if text.starts_with('=') {
                equals.push(j);
            }
            j += 1;
        }
        if let [only] = equals[..] {
            marks[only] = j - i > 1;
        }
        i = j;
    }
    marks
}

/// `=NAME: description` for a sub-row `=NAME  description` where NAME is an
/// all-caps placeholder word (or `<word>`), else `None`.
pub(super) fn value_form_sub_row(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix('=')?;
    let gap = find_description_gap(trimmed)?;
    let name = trimmed.get(..gap)?.trim_end();
    let word = name.strip_prefix('=')?;
    if !is_placeholder_word(word) {
        return None;
    }
    let desc = trimmed.get(gap..)?.trim();
    if desc.is_empty() || rest.is_empty() {
        return None;
    }
    Some(format!("{name}: {desc}"))
}

fn is_placeholder_word(word: &str) -> bool {
    if let Some(inner) = word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
        return !inner.is_empty()
            && inner
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    }
    word.chars().any(|c| c.is_ascii_uppercase())
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_caps_value_form_becomes_description_text() {
        assert_eq!(
            value_form_sub_row("=FILE  list to FILE (must be last sub-option)").as_deref(),
            Some("=FILE: list to FILE (must be last sub-option)")
        );
        assert_eq!(
            value_form_sub_row("=<file>  list to file").as_deref(),
            Some("=<file>: list to file")
        );
    }

    #[test]
    fn lowercase_equals_value_stays_a_choice() {
        assert!(value_form_sub_row("=default            -   default").is_none());
        assert!(value_form_sub_row("=FILE").is_none());
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    #[test]
    fn only_a_sole_equals_row_among_other_rows_qualifies() {
        let c = |t| FlagsBlockRow::Continuation(t, t);
        let a = [c("c  omit"), c("s  symbols"), c("=FILE  list to FILE")];
        assert_eq!(mark_sole_equals_rows(&a), [false, false, true]);
        let b = [c("=DPP  x"), c("=Iterative  y"), c("=None  z")];
        assert_eq!(mark_sole_equals_rows(&b), [false, false, false]);
        let lone = [c("=FILE  list")];
        assert_eq!(mark_sole_equals_rows(&lone), [false]);
    }
}
