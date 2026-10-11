//! A short spelling of one punctuation character (`-%`, `-_`, `-!`, `-/`)
//! beside a long spelling in an aligned spelling column pairs with it, like
//! `-0, --zero`. `is_flag_shaped` stays deaf to it. docs/shapes.md S-224.

/// `-` plus exactly one ASCII punctuation character other than `-`.
pub(super) fn is_punct_short(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next() == Some('-')
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_punctuation() && c != '-')
        && chars.next().is_none()
}
