//! An ALL-CAPS word glued to one dash whose own description names the whole
//! word (`-NUM  shortcut for -C NUM`, `-NUM  same as --context=NUM`) is one
//! single-dash spelling, never `-N` with the value `UM`. The description
//! tells it from a short flag with a glued value (`-DNAME`, whose
//! description never repeats `DNAME`) — `docs/shapes.md` S-221.

use super::*;

/// Rewrites `spec` in place when `spec_text` is exactly `-RUN` (an
/// all-caps run of three or more letters) and `description` mentions `RUN`
/// as a whole word.
pub(super) fn repair_caps_single_dash(spec: &mut FlagSpec, spec_text: &str, description: &str) {
    let [only] = spec.spellings.as_slice() else {
        return;
    };
    let (Some(letter), Some(rest)) = (spec.short(), spec.value_name.as_deref()) else {
        return;
    };
    let run = format!("{letter}{rest}");
    let caps = |c: char| c.is_ascii_uppercase() || c == '_';
    if run.chars().count() < 3 || !run.chars().all(caps) || only.name.chars().count() != 1 {
        return;
    }
    if spec_text.trim() != format!("-{run}") || !mentions_whole_word(description, &run) {
        return;
    }
    spec.spellings = vec![Spelling::single_dash(run)];
    spec.value_name = None;
    spec.value_kind = ValueKind::None;
}

fn mentions_whole_word(text: &str, word: &str) -> bool {
    let word_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.split(|c: char| !word_char(c)).any(|w| w == word)
}
