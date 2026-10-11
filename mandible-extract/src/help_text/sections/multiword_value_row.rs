//! A flag row whose value is two plain lowercase words inside the spec cell
//! (`jq`'s `--arg name value`, `--slurpfile name file`) is one value named
//! `name value`. `parse_flag_spec` reads the first word as the value and
//! leaves the rest unconsumed, where nothing took it up and the second word
//! vanished. See docs/shapes.md S-225.

use super::*;

/// Most plain words after the first one that still read as part of the
/// value.
const MAX_EXTRA_WORDS: usize = 1;

fn plain_word(w: &str) -> bool {
    w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Extends `spec`'s bare one-word value to the whole run of plain lowercase
/// words that follows the spellings in `spec_text`, when the grammar left
/// that run unconsumed.
fn extend_multiword_value(spec: &mut FlagSpec, spec_text: &str) {
    if spec.fully_consumed || spec.value_kind != ValueKind::Required {
        return;
    }
    let Some(value) = spec.value_name.as_deref() else {
        return;
    };
    let tokens: Vec<&str> = spec_text.split_whitespace().collect();
    let Some(at) = tokens.iter().position(|t| !t.starts_with('-')) else {
        return;
    };
    let extra = &tokens[at + 1..];
    if at == 0
        || tokens[at] != value
        || !plain_word(value)
        || extra.is_empty()
        || extra.len() > MAX_EXTRA_WORDS
        || !extra.iter().all(|w| plain_word(w))
    {
        return;
    }
    spec.value_name = Some(tokens[at..].join(" "));
}

/// Both spec repairs `emit_flags_with` applies after parsing a row's spec:
/// S-221's caps single dash and S-225's two-word value.
pub(super) fn repair_row_spec(spec: &mut FlagSpec, spec_text: &str, description: &str) {
    super::caps_single_dash::repair_caps_single_dash(spec, spec_text, description);
    extend_multiword_value(spec, spec_text);
}
