//! A table under a heading that is a usage operand's own name (`Libraries:`
//! beside `Usage: krb5-config [OPTIONS] [LIBRARIES]`) lists the values of
//! the one flag whose description names that operand (`--libs  List
//! libraries required to link [LIBRARIES]`). Each row is a choice of that
//! flag, its text the choice's description. Operands carry no choices on
//! screen, the flag does. The heading must equal the operand's name
//! (case-insensitive) and exactly one flag may name it. See
//! docs/shapes.md S-231.

use super::ParsedHelp;
use mandible_core::{Choice, Text};

/// True when `word` is a whole word of `text` (case-sensitive).
fn names_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .any(|w| w == word)
}

/// Attach `entries` to the flag whose description names the operand
/// `heading` is; `false` (nothing changed) when there is no such single flag.
pub(super) fn take_operand_values(
    heading: &str,
    entries: &[(&str, String)],
    out: &mut ParsedHelp,
) -> bool {
    let word = heading.trim().trim_end_matches(':').trim();
    if word.is_empty() || entries.is_empty() {
        return false;
    }
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(n, d)| (n.trim(), d.as_str()))
        .collect();
    let plain = entries.iter().all(|(n, _)| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    });
    if !plain {
        return false;
    }
    let Some(operand) = out.positionals.iter().find_map(|p| {
        let name = p.spellings.first()?.name.trim_end_matches("...");
        name.eq_ignore_ascii_case(word).then(|| name.to_string())
    }) else {
        return false;
    };
    let mut owners = out.flags.iter().enumerate().filter(|(_, f)| {
        f.description
            .as_ref()
            .is_some_and(|d| names_word(d.as_str(), &operand))
    });
    let (Some((idx, _)), None) = (owners.next(), owners.next()) else {
        return false;
    };
    if !out.flags[idx].choices.is_empty() {
        return false;
    }
    for (name, desc) in entries {
        let desc = desc.trim();
        out.flags[idx].choices.push(Choice {
            name: (*name).to_string(),
            description: (!desc.is_empty()).then(|| Text::sanitize(desc)),
        });
    }
    true
}
