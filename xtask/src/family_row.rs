//! Shared row-scanning primitives the seven vim-family detectors
//! (`xtask/src/plus_prefixed_option.rs` and its six siblings) all read
//! raw help text through, so the "is this an option row" judgment is made
//! once rather than seven slightly different ways.

/// The leading whitespace-delimited token of `line`'s trimmed text, and
/// whatever follows it verbatim — `None` when `line` has no leading
/// indentation at all (a heading, not an option row).
pub(crate) fn leading_token(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return None;
    }
    let token = trimmed.split_whitespace().next()?;
    let rest = &trimmed[token.len()..];
    Some((token, rest))
}

/// True when `rest` (the text following a row's leading token) contains a
/// real description-column gap *somewhere* — a tab, or a run of at least
/// [`mandible_extract::help_text::MIN_COLUMN_GAP_SPACES`] spaces — the
/// same boundary the layout splitter itself uses. Anywhere, not only
/// immediately after the token, because a row may alias two spellings
/// with a single space (`nvim`'s `+<cmd>, -c <cmd>      Execute ...`)
/// before its real description column opens. Guards every detector in
/// this family against firing on a token that merely starts an ordinary
/// sentence, which is never followed by a wide gap at all.
pub(crate) fn opens_description_column(rest: &str) -> bool {
    if rest.contains('\t') {
        return true;
    }
    let gap = " ".repeat(mandible_extract::help_text::MIN_COLUMN_GAP_SPACES);
    rest.contains(&gap)
}
