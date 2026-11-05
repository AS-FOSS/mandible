//! A legend that names a value placeholder (`STYLE is one of:`, `<color>:
//! black blue ...`) gives its values to every flag whose value names that
//! placeholder, not to the first one. See docs/shapes.md S-259 and
//! corpus/nl/9.4, corpus/setterm/audit-seed.

use super::ParsedHelp;
use mandible_core::{Choice, Entity, Text};

/// The lowercase alphanumeric words of `text`.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The word of `owner`'s value name that `heading` also writes, when the
/// owner was found by that word and not by its own `--name`.
pub(super) fn placeholder_word(heading: &str, owner: &Entity) -> Option<String> {
    let lower = heading.to_lowercase();
    if owner
        .long()
        .is_some_and(|l| lower.contains(&format!("--{}", l.to_lowercase())))
    {
        return None;
    }
    let heading_words = words(heading);
    words(owner.value_name.as_deref()?)
        .into_iter()
        .find(|w| w.chars().count() > 1 && heading_words.contains(w))
}

/// Give the flag `heading` names its values, descriptions kept, and then
/// every other flag whose value names the same placeholder.
pub(super) fn attach_to_owner(
    out: &mut ParsedHelp,
    owner: usize,
    heading: &str,
    candidates: Vec<(String, Option<String>)>,
) {
    for (name, desc) in candidates {
        if out.flags[owner].choices.iter().any(|c| c.name == name) {
            continue;
        }
        out.flags[owner].choices.push(Choice {
            name,
            description: desc.map(|d| Text::sanitize(&d)),
        });
    }
    if let Some(word) = placeholder_word(heading, &out.flags[owner]) {
        let choices = out.flags[owner].choices.clone();
        give_to_siblings(&mut out.flags, Some(owner), &word, &choices);
    }
}

/// Add `choices` the flag does not list yet to every flag but `skip`
/// whose value name holds `word`.
pub(super) fn give_to_siblings(
    flags: &mut [Entity],
    skip: Option<usize>,
    word: &str,
    choices: &[Choice],
) {
    for (i, flag) in flags.iter_mut().enumerate() {
        let names_it = flag
            .value_name
            .as_deref()
            .is_some_and(|v| words(v).iter().any(|w| w == word));
        if Some(i) == skip || !names_it {
            continue;
        }
        for choice in choices {
            if !flag.choices.iter().any(|c| c.name == choice.name) {
                flag.choices.push(choice.clone());
            }
        }
    }
}

/// `<word>: a b c ...` ending `desc`: the description before it, the
/// placeholder word, and the values, at least three plain lowercase words.
fn split_legend(desc: &str) -> Option<(&str, String, Vec<&str>)> {
    let open = desc.rfind(" <")?;
    let (head, legend) = desc.split_at(open);
    let (name, values) = legend.trim_start().strip_prefix('<')?.split_once(">:")?;
    let members: Vec<&str> = values.split_whitespace().collect();
    let plain = |m: &&str| {
        m.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && m.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (named && members.len() >= 3 && members.iter().all(plain))
        .then(|| (head.trim_end(), name.to_lowercase(), members))
}

/// The plain words of an `a|b|<word>` value name ahead of its placeholder.
fn literal_alternatives(value: &str) -> Vec<&str> {
    if !value.contains('|') {
        return Vec::new();
    }
    value
        .split('|')
        .filter(|a| !a.is_empty() && !a.contains(['<', '[', '{']))
        .collect()
}

/// A flag whose description ends in a `<word>: a b c` legend gives the
/// legend up: the values go to every flag whose value names `<word>`, with
/// the plain words its own `a|b|<word>` alternatives list ahead of them.
pub(super) fn attach_inline_legends(flags: &mut [Entity]) {
    for at in 0..flags.len() {
        let split = flags[at].description.as_ref().and_then(|d| {
            let (head, word, members) = split_legend(d.as_str())?;
            let members: Vec<String> = members.into_iter().map(str::to_string).collect();
            Some((head.to_string(), word, members))
        });
        let Some((head, word, members)) = split else {
            continue;
        };
        flags[at].description = (!head.is_empty()).then(|| Text::sanitize(&head));
        for flag in flags.iter_mut() {
            let Some(value) = flag.value_name.as_deref() else {
                continue;
            };
            if !words(value).contains(&word) || !flag.choices.is_empty() {
                continue;
            }
            let named: Vec<Choice> = literal_alternatives(value)
                .into_iter()
                .map(str::to_string)
                .chain(members.iter().cloned())
                .map(Choice::bare)
                .collect();
            flag.choices = named;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_angle_legend_is_split_off() {
        let (head, word, m) =
            split_legend("set half-bright text color <color>: black blue cyan").unwrap();
        assert_eq!(
            (head, word.as_str()),
            ("set half-bright text color", "color")
        );
        assert_eq!(m, ["black", "blue", "cyan"]);
    }

    #[test]
    fn prose_or_a_short_list_is_not_a_legend() {
        assert!(split_legend("use <file>: a b").is_none());
        assert!(split_legend("read <file>: See the Manual page.").is_none());
    }
}
