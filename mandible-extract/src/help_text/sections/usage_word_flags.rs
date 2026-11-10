//! A synopsis-only single-dash word read as a short flag plus a fabricated
//! value (`cgi-fcgi -connect <connName>` as `-c` valued `onnect`, `-start`,
//! `-bind` likewise) is one multi-letter option when the document names the
//! whole word on two or more lines, standing alone each time. A short flag
//! with a glued value is never also written as a separate token on a second
//! line. A document that already spells options as abbreviated single-dash
//! words (`-V[ersion]`, `-s[tatistics]`) vouches for the word on one line.
//! See docs/shapes.md S-172.

use super::*;

/// Fewest characters the swallowed tail must carry: shorter is a letter pair
/// (`-ps`, `-Ss`) that is as likely a short flag with a glued value.
const MIN_TAIL_CHARS: usize = 3;

/// Fewest distinct document lines that must spell the whole word.
const MIN_LINES: usize = 2;

/// Fewest abbreviated single-dash spellings that make the document's own
/// dialect single-dash words.
const MIN_WORD_DIALECT: usize = 3;

fn word_dialect(flags: &[Entity]) -> bool {
    let words = flags
        .iter()
        .flat_map(|f| f.spellings.iter())
        .filter(|s| matches!(s.dashes, Dashes::Single) && s.abbrev.is_some())
        .count();
    words >= MIN_WORD_DIALECT
}

fn whole_word_on(line: &str, word: &str) -> bool {
    line.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .any(|t| t == word)
}

/// The `<placeholder>` token that follows the word at each usage-line
/// occurrence, `None` if any occurrence is followed by something else.
fn angle_value_after(usage_lines: &[String], word: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in usage_lines {
        let toks: Vec<&str> = line
            .split_whitespace()
            .map(|t| t.trim_matches(|c| c == '[' || c == ']' || c == ','))
            .collect();
        for (i, t) in toks.iter().enumerate() {
            if *t != word {
                continue;
            }
            match toks.get(i + 1) {
                Some(n) if n.starts_with('<') && n.ends_with('>') => {
                    found.get_or_insert_with(|| (*n).to_string());
                }
                _ => return None,
            }
        }
    }
    found
}

/// The single-dash word a synopsis-only short-plus-tail flag spells, when
/// the document names the whole word on `min_lines` lines.
fn word_of(flag: &Entity, raw: &str, min_lines: usize) -> Option<String> {
    if !flag
        .provenance
        .sources
        .iter()
        .all(|s| matches!(s, Source::HelpTextSynopsis))
    {
        return None;
    }
    let short = flag.short()?;
    if flag.long().is_some() || flag.value_kind != ValueKind::Required {
        return None;
    }
    let tail = flag.value_name.as_deref()?;
    if tail.chars().count() < MIN_TAIL_CHARS
        || !tail.chars().all(|c| c.is_ascii_lowercase())
        || !short.is_ascii_lowercase()
    {
        return None;
    }
    let name = format!("{short}{tail}");
    let word = format!("-{name}");
    (raw.lines().filter(|l| whole_word_on(l, &word)).count() >= min_lines).then_some(name)
}

/// True when `letter` is a short flag of `flags`, written `-f` or as the
/// abbreviation `-f[amily]`.
fn lists_short(flags: &[Entity], letter: char) -> bool {
    flags.iter().flat_map(|f| f.spellings.iter()).any(|s| {
        matches!(s.dashes, Dashes::Single)
            && (s.name.chars().count() == 1 || s.abbrev == Some(1))
            && s.name.starts_with(letter)
    })
}

/// True when `flags` already spell `word` with one dash, whole or as the
/// abbreviation `-b[atch]`.
fn lists_word(flags: &[Entity], word: &str) -> bool {
    flags
        .iter()
        .flat_map(|f| f.spellings.iter())
        .any(|s| matches!(s.dashes, Dashes::Single) && s.name == word)
}

/// True when a usage-derived `flag` is a single-dash word the page does not
/// already list: the word is not a spelling of `existing`, and some letter
/// of it is not a short flag there. A word whose letters are all listed
/// shorts is a bundle. S-272.
pub(super) fn is_unlisted_word(flag: &Entity, existing: &[Entity], raw: &str) -> bool {
    let min_lines = if word_dialect(existing) { 1 } else { MIN_LINES };
    word_of(flag, raw, min_lines).is_some_and(|name| {
        let same_reading = existing
            .iter()
            .any(|f| f.short() == flag.short() && f.value_name == flag.value_name);
        !lists_word(existing, &name)
            && !same_reading
            && !name.chars().all(|c| lists_short(existing, c))
    })
}

pub(super) fn repair(usage_lines: &[String], raw: &str, flags: &mut [Entity]) {
    let min_lines = if word_dialect(flags) { 1 } else { MIN_LINES };
    for flag in flags.iter_mut() {
        let Some(name) = word_of(flag, raw, min_lines) else {
            continue;
        };
        let word = format!("-{name}");
        flag.spellings = vec![Spelling::single_dash(&name)];
        match angle_value_after(usage_lines, &word) {
            Some(value) => flag.value_name = Some(value),
            None => {
                flag.value_name = None;
                flag.value_kind = ValueKind::None;
            }
        }
    }
}
