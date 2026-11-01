//! Alternation value specs (`y|n|ay`, `{auto|manual}`, `[none|zlib]`): a
//! value that names its own alternatives is that entity's `choices`, and
//! the page draws a `{…}` mark where a name would be (docs/shapes.md S-155,
//! S-215 family; design §9.3 rule 6).

use crate::entity::{Choice, Entity, EntityKind};
use crate::node::ValueKind;

/// What the page draws for a value that is only its alternatives.
pub const CHOICES_MARK: &str = "{…}";

/// One alternative: lowercase letters and digits, with `-` and `_` after
/// the first character. A capitalised word names a placeholder instead.
fn is_alternative(word: &str) -> bool {
    let mut chars = word.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// The alternatives of a value spec made of two or more words joined by
/// `|`, bare or wrapped once in `{}` or `[]`. `None` for anything else.
pub fn alternation_members(spec: &str) -> Option<Vec<&str>> {
    let inner = [('{', '}'), ('[', ']')]
        .iter()
        .find_map(|&(open, close)| spec.strip_prefix(open)?.strip_suffix(close))
        .unwrap_or(spec);
    let members: Vec<&str> = inner.split('|').collect();
    (members.len() >= 2 && members.iter().all(|m| is_alternative(m))).then_some(members)
}

/// The alternatives of a positional's name. Only the braced form counts:
/// a bare `device|mountpoint` or `<a>|<b>` operand names kinds of
/// argument, not literal values.
fn operand_members(name: &str) -> Option<Vec<&str>> {
    (name.starts_with('{') && name.ends_with('}'))
        .then(|| alternation_members(name))
        .flatten()
}

impl Entity {
    /// True for a positional whose own name is a braced alternation that
    /// its `choices` already list; the page draws `{…}` in its place.
    pub fn name_is_choices(&self) -> bool {
        self.kind == EntityKind::Positional
            && !self.choices.is_empty()
            && operand_members(self.primary_name()).is_some()
    }

    /// True when the page draws [`CHOICES_MARK`] as this flag's value: it
    /// takes a value, has no name for it, and lists choices.
    pub(crate) fn value_is_choices(&self) -> bool {
        self.value_name.is_none() && self.value_kind != ValueKind::None && !self.choices.is_empty()
    }
}

/// Move every alternation value into `choices`: a flag's value spec
/// becomes its choices and the name goes (the choices carry it), and a
/// positional named by an alternation keeps its name and gains the
/// choices.
pub(crate) fn fold_alternation_values(entities: &mut [Entity]) {
    for e in entities {
        let members = match e.kind {
            EntityKind::Flag => e.value_name.as_deref().and_then(alternation_members),
            EntityKind::Positional => operand_members(e.primary_name()),
            _ => None,
        };
        let Some(members) = members else { continue };
        let same = e.choices.len() == members.len()
            && e.choices.iter().zip(&members).all(|(c, m)| c.name == *m);
        if e.choices.is_empty() {
            e.choices = members.into_iter().map(Choice::bare).collect();
        } else if !same {
            continue;
        }
        if e.kind == EntityKind::Flag {
            e.value_name = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_joined_by_bars_are_alternatives() {
        for spec in [
            "y|n",
            "y|n|ay",
            "0|1",
            "{auto|manual}",
            "[no|yes]",
            "[none|zlib-gnu|a_b]",
        ] {
            assert!(alternation_members(spec).is_some(), "{spec}");
        }
    }

    #[test]
    fn placeholders_and_other_notation_are_not() {
        for spec in [
            "VG|Tag",
            "PID|pattern",
            "all|unmanaged|Number",
            "Size[m|UNIT]",
            "a|",
            "yes",
            "host+tpm2|auto",
            "<fetch|push>",
            "x=[a|b]",
            "a|b,c",
        ] {
            assert!(alternation_members(spec).is_none(), "{spec}");
        }
    }

    #[test]
    fn an_operand_counts_only_when_braced() {
        assert!(operand_members("{auto|manual}").is_some());
        for name in ["device|mountpoint", "[a|b]", "table|table_file"] {
            assert!(operand_members(name).is_none(), "{name}");
        }
    }

    #[test]
    fn the_wrapper_comes_off_once() {
        assert_eq!(
            alternation_members("[none|zlib]"),
            Some(vec!["none", "zlib"])
        );
        assert!(alternation_members("{[a|b]}").is_none());
    }
}
