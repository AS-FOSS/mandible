//! Third ownership proof for a choices block: the introducing heading
//! names the plural of exactly one flag's value placeholder
//! (`Warning categories include:` under `-W, --warnings=CATEGORY`).
//! See docs/shapes.md S-203 and corpus/automake/1.16.5.

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

/// The index of the one flag whose value placeholder's plural is a whole
/// word of `heading`. `None` when no flag or several distinct flags match.
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
        f.value_name.as_deref().is_some_and(|vn| {
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

/// Drops the `prefix-PLACEHOLDER` rows whose placeholder is not the proven
/// owner's own value name; every other row stays, in order.
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
            !is_prefixed_placeholder(name)
                || name
                    .split_once('-')
                    .is_some_and(|(_, rest)| owner_value.as_deref() == Some(&rest.to_lowercase()))
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
    fn heading_without_a_colon_proves_nothing() {
        let flags = vec![flag("warnings", "CATEGORY")];
        assert_eq!(plural_owner_index("Warning categories", &flags), None);
    }
}
