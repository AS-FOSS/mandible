//! Third ownership proof for a choices block: the introducing heading
//! names the plural of exactly one flag's value placeholder
//! (`Warning categories include:` under `-W, --warnings=CATEGORY`), or
//! names that flag's long spelling as a word, singular or plural
//! (`Languages include:` under `-l, --language=LANG`).
//! See docs/shapes.md S-203, corpus/automake/1.16.5, corpus/autom4te/2.71.

use super::ParsedHelp;
use mandible_core::Entity;

/// The lowercase alphabetic core of a value placeholder (`<files>` and
/// `FILES` both give `files`).
fn placeholder_word(value_name: &str) -> String {
    value_name
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase()
}

fn plural_forms(word: &str) -> Vec<String> {
    let mut forms = vec![format!("{word}s"), format!("{word}es")];
    if let Some(stem) = word.strip_suffix('y') {
        forms.push(format!("{stem}ies"));
    }
    forms
}

/// Words that may follow a noun naming the block's subject: the noun then ends
/// its phrase (`digest algorithm and ...`, `Languages include:`), where a
/// following content word makes it a modifier (`text segmentation`).
const PHRASE_END: [&str; 20] = [
    "and", "or", "include", "includes", "are", "is", "for", "of", "to", "in", "as", "by", "with",
    "that", "which", "choices", "values", "names", "can", "may",
];

/// True when `long` or one of its plurals is a whole word of `words` that ends
/// its noun phrase.
fn names_word(words: &[&str], long: &str) -> bool {
    let plurals = plural_forms(long);
    words.iter().enumerate().any(|(i, w)| {
        (*w == long || plurals.iter().any(|p| p == w))
            && words.get(i + 1).is_none_or(|n| PHRASE_END.contains(n))
    })
}

/// True when the heading names a long spelling of `flag` (`language`) and the
/// flag takes a value: a flag without one has no choices to own.
fn heading_names_long(words: &[&str], flag: &Entity) -> bool {
    flag.value_kind != mandible_core::ValueKind::None
        && flag.spellings.iter().any(|s| {
            let name = s.name.to_lowercase();
            s.dashes != mandible_core::Dashes::None
                && name.chars().count() > 3
                && name.chars().all(|c| c.is_ascii_alphabetic())
                && names_word(words, &name)
        })
}

/// The index of the one flag whose value placeholder's plural, or whose
/// long spelling, is a whole word of `heading`. `None` when no flag or several distinct flags match.
pub(super) fn plural_owner_index(heading: &str, flags: &[Entity]) -> Option<usize> {
    if !heading.trim_end().ends_with(':') {
        return None;
    }
    let lower = heading.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut hits = flags.iter().enumerate().filter(|(_, f)| {
        heading_names_long(&words, f)
            || f.value_name.as_deref().is_some_and(|vn| {
                let word = placeholder_word(vn);
                word.chars().count() > 3
                    && word.chars().all(char::is_alphabetic)
                    && plural_forms(&word)
                        .iter()
                        .any(|p| words.contains(&p.as_str()))
            })
    });
    let first = hits.next()?.0;
    hits.next().is_none().then_some(first)
}

/// `no-CATEGORY`: a lowercase prefix, a hyphen and an all-caps placeholder.
pub(super) fn is_prefixed_placeholder(name: &str) -> bool {
    let Some((prefix, rest)) = name.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_lowercase())
        && rest.chars().count() >= 2
        && rest.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

/// `'Autoconf'`: one alphanumeric word inside matching single or double
/// quotes. A quoted name is a value of the flag that owns its block, never a
/// guess for the last flag.
pub(super) fn is_quoted_word(name: &str) -> bool {
    let mut chars = name.chars();
    let (Some(open), Some(close)) = (chars.next(), chars.next_back()) else {
        return false;
    };
    matches!(open, '\'' | '"')
        && open == close
        && name.chars().count() > 2
        && name[1..name.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A row name only a proven owner may take: `no-CATEGORY` or `'Autoconf'`.
pub(super) fn is_owned_only_name(name: &str) -> bool {
    is_prefixed_placeholder(name) || is_quoted_word(name)
}

/// Drops the `prefix-PLACEHOLDER` rows whose placeholder is not the proven
/// owner's own value name, and the quoted rows nobody owns, and strips the
/// quotes off the rest; every other row stays, in order.
pub(super) fn keep_owned_placeholders(
    rows: Vec<(String, Option<String>)>,
    owner: Option<usize>,
    out: &ParsedHelp,
) -> Vec<(String, Option<String>)> {
    let owner_value = owner
        .and_then(|i| out.flags[i].value_name.as_deref())
        .map(placeholder_word);
    rows.into_iter()
        .filter(|(name, _)| {
            if is_quoted_word(name) {
                return owner.is_some();
            }
            !is_prefixed_placeholder(name)
                || name
                    .split_once('-')
                    .is_some_and(|(_, rest)| owner_value.as_deref() == Some(&rest.to_lowercase()))
        })
        .map(|(name, desc)| match is_quoted_word(&name) {
            true => (name[1..name.len() - 1].to_string(), desc),
            false => (name, desc),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, ValueKind};

    fn flag(long: &str, value: &str) -> Entity {
        let mut e = Entity::flag_long(long, Provenance::single(Source::HelpText));
        e.value_name = Some(value.to_string());
        e.value_kind = ValueKind::Required;
        e
    }

    #[test]
    fn plural_of_the_one_placeholder_owns_the_block() {
        let flags = vec![flag("libdir", "DIR"), flag("warnings", "CATEGORY")];
        assert_eq!(
            plural_owner_index("Warning categories include:", &flags),
            Some(1)
        );
    }

    #[test]
    fn the_long_spelling_as_a_heading_word_owns_the_block() {
        let flags = vec![flag("warnings", "CATEGORY"), flag("language", "LANG")];
        assert_eq!(plural_owner_index("Languages include:", &flags), Some(1));
        assert_eq!(plural_owner_index("Language choices:", &flags), Some(1));
        assert_eq!(plural_owner_index("Other things:", &flags), None);
    }

    #[test]
    fn a_long_spelling_that_modifies_another_noun_owns_nothing() {
        let flags = vec![flag("kind", "KIND"), flag("text", "STRING")];
        let h = "Show text segmentation as determined by Pango:";
        assert_eq!(plural_owner_index(h, &flags), None);
        let flags = vec![flag("algorithm", "TYPE")];
        let h = "DIGEST determines the digest algorithm and default output format:";
        assert_eq!(plural_owner_index(h, &flags), Some(0));
    }

    #[test]
    fn a_flag_without_a_value_owns_no_block() {
        let mut components = flag("components", "X");
        components.value_kind = ValueKind::None;
        components.value_name = None;
        assert_eq!(
            plural_owner_index("Typical components:", &[components]),
            None
        );
    }

    #[test]
    fn two_matching_flags_prove_nothing() {
        let both = vec![flag("a", "CATEGORY"), flag("b", "category")];
        assert_eq!(plural_owner_index("Categories include:", &both), None);
    }

    #[test]
    fn prefixed_placeholder_shape() {
        assert!(is_prefixed_placeholder("no-CATEGORY"));
        assert!(!is_prefixed_placeholder("no-category"));
        assert!(!is_prefixed_placeholder("NO-CATEGORY"));
        assert!(!is_prefixed_placeholder("CATEGORY"));
    }

    #[test]
    fn quoted_words_are_owned_only() {
        assert!(is_quoted_word("'Autoconf'"));
        assert!(is_quoted_word("\"M4sh\""));
        assert!(!is_quoted_word("''"));
        assert!(!is_quoted_word("'two words'"));
        let rows = vec![("'M4sh'".to_string(), None)];
        let out = ParsedHelp::default();
        assert!(keep_owned_placeholders(rows.clone(), None, &out).is_empty());
    }

    #[test]
    fn heading_without_a_colon_proves_nothing() {
        let flags = vec![flag("warnings", "CATEGORY")];
        assert_eq!(plural_owner_index("Warning categories", &flags), None);
    }
}
