//! Three or more rows of one short letter, each glued to a different word
//! and each with a description, are separate single-dash options
//! (`-Bsymbolic`, `-Bno-symbolic`), not one `-B` flag with several values.
//! `docs/shapes.md` S-212, `corpus/ld/2.42-single-dash`.

use super::repair::{is_option_name_tail, spaced_value_placeholder, token_occurs_glued};
use mandible_core::{Entity, Source, Spelling, ValueKind};

/// Fewest rows of one letter that make a family.
const MIN_FAMILY_ROWS: usize = 3;

/// The letter and glued word of a table row that reads as `-<letter><word>`.
fn glued_word(flag: &Entity) -> Option<(char, &str)> {
    if !flag.provenance.sources.contains(&Source::HelpText)
        || flag.provenance.sources.contains(&Source::HelpTextSynopsis)
        || flag.long().is_some()
        || flag.value_kind != ValueKind::Required
        || flag.description.is_none()
    {
        return None;
    }
    let word = flag.value_name.as_deref()?;
    (is_option_name_tail(word) && word.chars().count() >= 2).then_some(())?;
    Some((flag.short()?, word))
}

/// The value the row spaces after `token`: a bracketed placeholder, or one
/// ALL-CAPS word before the description's column gap (`-Tbss ADDRESS`).
fn spaced_value(raw: &str, token: &str) -> Option<(String, ValueKind)> {
    if let Some(found) = spaced_value_placeholder(raw, token) {
        return Some(found);
    }
    raw.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(token)?.strip_prefix(' ')?;
        let word = rest.split_once("  ").map_or(rest, |(w, _)| w);
        let plain = word.chars().all(|c| c.is_ascii_uppercase() || c == '_');
        (plain && !word.is_empty()).then(|| (word.to_string(), ValueKind::Required))
    })
}

/// Rename each qualifying family member to its one single-dash spelling.
pub(super) fn split_short_letter_word_family(flags: &mut [Entity], raw: &str) {
    let rows: Vec<Option<(char, String)>> = flags
        .iter()
        .map(|f| {
            glued_word(f)
                .filter(|(c, w)| token_occurs_glued(raw, &format!("-{c}{w}")))
                .map(|(c, w)| (c, w.to_string()))
        })
        .collect();
    for (i, row) in rows.iter().enumerate() {
        let Some((letter, word)) = row else { continue };
        let mut members: Vec<&str> = rows
            .iter()
            .flatten()
            .filter(|(c, _)| c == letter)
            .map(|(_, w)| w.as_str())
            .collect();
        members.sort_unstable();
        members.dedup();
        if members.len() < MIN_FAMILY_ROWS {
            continue;
        }
        let name = format!("{letter}{word}");
        let value = spaced_value(raw, &format!("-{name}"));
        flags[i].spellings = vec![Spelling::single_dash(name)];
        flags[i].value_kind = value.as_ref().map_or(ValueKind::None, |v| v.1);
        flags[i].value_name = value.map(|v| v.0);
    }
}
