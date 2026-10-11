//! The bare `(` and `)` rows of an option table (`git grep`'s grouping):
//! flag rows with no dash, like the argfile `@` sigil. Before, the `(` ended
//! the block and the `)` opened a group. See `docs/shapes.md` S-222.

use super::*;

/// The plus-or-minus pair (S-086) or a bare paren row.
pub(super) fn sigil_row(trimmed: &str, indent: usize, min_entry_indent: Option<usize>) -> bool {
    starts_with_plus_minus_pair(trimmed) || is_paren_row(trimmed, indent, min_entry_indent)
}

/// A lone `(` or `)` at the table's entry indent.
pub(super) fn is_paren_row(trimmed: &str, indent: usize, min_entry_indent: Option<usize>) -> bool {
    matches!(trimmed.trim_end(), "(" | ")") && min_entry_indent == Some(indent)
}

/// The spec a bare paren row reads as: one spelling with no dashes.
pub(super) fn paren_spec(spec_text: &str) -> Option<FlagSpec> {
    let t = spec_text.trim();
    matches!(t, "(" | ")").then(|| FlagSpec {
        spellings: vec![Spelling::bare(t)],
        fully_consumed: true,
        ..FlagSpec::default()
    })
}

/// The whole placeholder when alphanumeric text is glued straight onto the
/// `)` that closes at `close` (`--chmod (+|-)x`), kept as printed.
/// See docs/shapes.md S-223.
pub(super) fn glued_parenthetical(spec_text: &str, close: usize) -> Option<String> {
    let open = spec_text.find('(')?;
    let after = &spec_text[close + 1..];
    let tail = after.split_whitespace().next()?;
    (tail.starts_with(char::is_alphanumeric) && !after.starts_with(char::is_whitespace))
        .then(|| format!("{}{tail}", &spec_text[open..=close]))
}
