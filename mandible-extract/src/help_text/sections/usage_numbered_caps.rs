//! Numbered ALL-CAPS operands in a verb node's own forms:
//! `compare VERSION1 OP VERSION2`, `sort [VERSION1 VERSION2 ...]`.
//!
//! The per-token operand test wants letters and underscores only, so a
//! digit-suffixed word is never read there, and allowing digits everywhere
//! is wrong (a lone `W0` is a register, not an operand). The evidence that
//! `VERSION1` is an operand is the same stem coming back with another
//! number in the same form. This pass runs only on a usage-verb node's own
//! forms (the root keeps what its other recoveries read), after the
//! ordinary extraction, and adds what that missed. See docs/shapes.md S-241.

use mandible_core::{Entity, Provenance, Source};

/// `(stem, number)` for `STEM<digits>` with a stem of two or more ASCII
/// capitals or underscores.
fn split_numbered_caps(word: &str) -> Option<(&str, &str)> {
    let digits_at = word.find(|c: char| c.is_ascii_digit())?;
    let (stem, digits) = word.split_at(digits_at);
    let stem_ok = stem.len() >= 2
        && stem.chars().all(|c| c.is_ascii_uppercase() || c == '_')
        && stem.chars().any(|c| c.is_ascii_uppercase());
    (stem_ok && digits.chars().all(|c| c.is_ascii_digit())).then_some((stem, digits))
}

fn clean(token: &str) -> &str {
    token.trim_matches(|c| c == '[' || c == ']' || c == '.')
}

/// Add the paired numbered ALL-CAPS operands of `forms` that `positionals`
/// lacks, and put the operands in the order the forms print them.
pub(super) fn add_numbered_operands(forms: &[String], positionals: &mut Vec<Entity>) {
    let words: Vec<&str> = forms
        .iter()
        .flat_map(|f| f.split_whitespace())
        .map(clean)
        .collect();
    let paired = |w: &str| {
        split_numbered_caps(w).is_some_and(|(stem, n)| {
            words
                .iter()
                .filter_map(|o| split_numbered_caps(o))
                .any(|(s, m)| s == stem && m != n)
        })
    };
    let mut added = false;
    for form in forms {
        let toks: Vec<&str> = form.split_whitespace().collect();
        let mut depth = 0i32;
        for (i, tok) in toks.iter().enumerate() {
            let before = depth;
            depth += tok.matches('[').count() as i32 - tok.matches(']').count() as i32;
            let name = clean(tok);
            if !paired(name) || positionals.iter().any(|p| p.primary_name() == name) {
                continue;
            }
            let optional = before > 0 || tok.contains('[');
            let repeats = tok.ends_with("...")
                || toks
                    .get(i + 1)
                    .is_some_and(|n| n.trim_end_matches(']').starts_with(".."));
            let mut p = Entity::positional(name.to_string(), Provenance::single(Source::HelpText));
            p.required = !optional;
            p.repeatable = repeats;
            positionals.push(p);
            added = true;
        }
    }
    if added {
        positionals.sort_by_key(|p| {
            words
                .iter()
                .position(|w| *w == p.primary_name())
                .unwrap_or(usize::MAX)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &[Entity]) -> Vec<String> {
        p.iter().map(|e| e.primary_name().to_string()).collect()
    }

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_stem_that_comes_back_with_another_number_is_an_operand_in_order() {
        let mut p = vec![Entity::positional(
            "OP".to_string(),
            Provenance::single(Source::HelpText),
        )];
        add_numbered_operands(&lines(&["compare VERSION1 OP VERSION2"]), &mut p);
        assert_eq!(names(&p), ["VERSION1", "OP", "VERSION2"]);
        assert!(p[0].required);
    }

    #[test]
    fn a_bracket_group_is_optional_and_a_trailing_dots_word_repeats() {
        let mut p = Vec::new();
        add_numbered_operands(&lines(&["sort [VERSION1 VERSION2 ...]"]), &mut p);
        assert_eq!(names(&p), ["VERSION1", "VERSION2"]);
        assert!(!p[0].required && !p[1].required);
        assert!(!p[0].repeatable && p[1].repeatable);
    }

    #[test]
    fn a_lone_numbered_word_or_a_one_letter_stem_is_not() {
        let mut p = Vec::new();
        add_numbered_operands(&lines(&["x VERSION1 OP", "y W0 W1"]), &mut p);
        assert!(p.is_empty());
    }
}
