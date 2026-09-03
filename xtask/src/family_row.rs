//! Shared row-scanning primitives the seven vim-family detectors read raw
//! help text through, one judgment made once rather than seven ways.

/// The leading token of `line`'s trimmed text and whatever follows it —
/// `None` when `line` has no leading indentation (a heading, not a row).
pub(crate) fn leading_token(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return None;
    }
    let token = trimmed.split_whitespace().next()?;
    let rest = &trimmed[token.len()..];
    Some((token, rest))
}

/// True when `rest` contains a real description-column gap somewhere — a
/// tab, or [`mandible_extract::help_text::MIN_COLUMN_GAP_SPACES`] spaces —
/// not only right after the token, since a row may alias two spellings
/// with a single space first (nvim's `+<cmd>, -c <cmd>      Execute...`).
pub(crate) fn opens_description_column(rest: &str) -> bool {
    if rest.contains('\t') {
        return true;
    }
    let gap = " ".repeat(mandible_extract::help_text::MIN_COLUMN_GAP_SPACES);
    rest.contains(&gap)
}
