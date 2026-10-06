//! A synopsis-only single-dash word read as a short flag plus a fabricated
//! value (`cgi-fcgi -connect <connName>` as `-c` valued `onnect`, `-start`,
//! `-bind` likewise) is one multi-letter option when the document names the
//! whole word on two or more lines, standing alone each time. A short flag
//! with a glued value is never also written as a separate token on a second
//! line. See docs/shapes.md S-172.

use super::*;

/// Fewest characters the swallowed tail must carry: shorter is a letter pair
/// (`-ps`, `-Ss`) that is as likely a short flag with a glued value.
const MIN_TAIL_CHARS: usize = 3;

/// Fewest distinct document lines that must spell the whole word.
const MIN_LINES: usize = 2;

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

pub(super) fn repair(usage_lines: &[String], raw: &str, flags: &mut [Entity]) {
    for flag in flags.iter_mut() {
        if !flag
            .provenance
            .sources
            .iter()
            .all(|s| matches!(s, Source::HelpTextSynopsis))
        {
            continue;
        }
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
            continue;
        }
        let Some(tail) = flag.value_name.as_deref() else {
            continue;
        };
        if tail.chars().count() < MIN_TAIL_CHARS
            || !tail.chars().all(|c| c.is_ascii_lowercase())
            || !short.is_ascii_lowercase()
        {
            continue;
        }
        let name = format!("{short}{tail}");
        let word = format!("-{name}");
        if raw.lines().filter(|l| whole_word_on(l, &word)).count() < MIN_LINES {
            continue;
        }
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
