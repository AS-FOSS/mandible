//! The plus-or-minus flag row: lsof's `+|-w` and `+f|-f`. See
//! docs/shapes.md S-086.

use super::*;

// --- S-086: the `+|-x` and `+x|-x` plus-or-minus row ----------------------
//
// lsof spells a flag that exists in both polarities `+|-w` (or `+f|-f`):
// one entity carrying both spellings, `+w` and `-w`, and the row's own
// description verbatim. The sigil is unambiguous on its own, so the row
// needs no neighbor evidence. See docs/shapes.md S-086.

/// True when `line`'s leading token is a plus-or-minus flag token
/// ([`plus_minus_pair_rest`]).
pub(super) fn starts_with_plus_minus_pair(line: &str) -> bool {
    plus_minus_pair_rest(first_word(line.trim_start()).trim_end_matches(',')).is_some()
}

/// Byte length of the leading bracket group of `text` (`[t[m<fmt>]]`),
/// nested brackets counted, or `None` when `text` does not open with a
/// group that closes.
fn leading_bracket_group_len(text: &str) -> Option<usize> {
    if !text.starts_with('[') {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a plus-or-minus row's spec text (`+|-e s`, `+f|-f`, `+|-f[gG]`,
/// `+|-r [t[m<fmt>]]`) into one [`FlagSpec`] carrying both spellings, `+x`
/// then `-x`, and the row's own value, plus whatever text the column
/// splitter left after the value (a description that began without a
/// column gap). `None` when the leading token is not a plus-or-minus
/// token. See docs/shapes.md S-086.
pub(super) fn parse_plus_minus_pair_spec(spec_text: &str) -> Option<(FlagSpec, String)> {
    let trimmed = spec_text.trim();
    let token = first_word(trimmed);
    let rest_token = plus_minus_pair_rest(token)?;
    let name_end = rest_token.find('[').unwrap_or(rest_token.len());
    let name = &rest_token[..name_end];
    if name.is_empty() {
        return None;
    }
    let mut spec = FlagSpec {
        spellings: vec![
            Spelling::bare(format!("+{name}")),
            Spelling::single_dash(name.to_string()),
        ],
        ..FlagSpec::default()
    };
    let glued = &rest_token[name_end..];
    let after = trimmed[token.len()..].trim_start();
    if let Some(inner) = glued
        .strip_prefix('[')
        .and_then(|g| g.strip_suffix(']'))
        .filter(|inner| !inner.is_empty())
    {
        spec.value_name = Some(inner.to_string());
        spec.value_kind = ValueKind::Optional;
        spec.fully_consumed = true;
        return Some((spec, after.to_string()));
    }
    if after.is_empty() {
        spec.fully_consumed = glued.is_empty();
        return Some((spec, String::new()));
    }
    // A whole optional value in brackets, possibly nested
    // (`[t[m<fmt>]]`), is read as written, minus the outer pair.
    if let Some(len) = leading_bracket_group_len(after) {
        let inner = &after[1..len - 1];
        if !inner.is_empty() {
            spec.value_name = Some(inner.to_string());
            spec.value_kind = ValueKind::Optional;
            spec.fully_consumed = true;
            return Some((spec, after[len..].trim_start().to_string()));
        }
    }
    let tail = parse_flag_spec(&format!("-{name} {after}"));
    spec.value_name = tail.value_name;
    spec.value_kind = tail.value_kind;
    spec.fully_consumed = tail.fully_consumed;
    Some((spec, String::new()))
}
