//! A short spelling made of one punctuation character (`-%`, `-_`, `-!`,
//! `-/`) beside a long spelling in an aligned spelling column is the same
//! option as that long spelling, like `-0, --zero`. `is_flag_shaped` stays
//! deaf to these tokens (they are too common as prose), so the aligned
//! column accepts them only here, where a long spelling and a recurring
//! column vouch for the row. See docs/shapes.md S-224.

/// True for `-` followed by exactly one ASCII punctuation character other
/// than another dash.
pub(super) fn is_punct_short(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next() == Some('-')
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_punctuation() && c != '-')
        && chars.next().is_none()
}
