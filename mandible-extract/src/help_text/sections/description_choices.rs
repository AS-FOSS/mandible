//! [S-156] A flag description that ends in a labelled run of literal
//! values (`available formats: a, b, c`) names that flag's `choices`.
//! See docs/shapes.md S-156 and corpus/grub-mkimage/2.12.

use mandible_core::{Choice, Entity, Text};

const MIN_MEMBERS: usize = 3;
const FIXED_LABELS: [&str; 3] = ["possible values:", "valid values:", "one of:"];

/// Byte range of the last list label in `lower`: a fixed label, or
/// `available <word>:`. The range is `(start, end after the colon)`.
fn last_label(lower: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for label in FIXED_LABELS {
        if let Some(at) = lower.rfind(label) {
            best = best.max(Some((at, at + label.len())));
        }
    }
    let mut from = 0;
    while let Some(rel) = lower[from..].find("available ") {
        let at = from + rel;
        let rest = &lower[at + "available ".len()..];
        if let Some(word) = rest.split(':').next() {
            let plain = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphabetic());
            if plain && rest.len() > word.len() {
                best = best.max(Some((at, at + "available ".len() + word.len() + 1)));
            }
        }
        from = at + 1;
    }
    best
}

fn is_member(token: &str) -> bool {
    let mut chars = token.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.+-".contains(c))
}

/// The description with its list removed, and the list's members, when
/// `desc` ends in a labelled run of at least three literal values.
fn split_tail(desc: &str) -> Option<(String, Vec<String>)> {
    let lower = desc.to_ascii_lowercase();
    let (start, end) = last_label(&lower)?;
    let tail = desc[end..].trim().trim_end_matches('.');
    let members: Vec<&str> = tail.split(',').map(str::trim).collect();
    if members.len() < MIN_MEMBERS || !members.iter().all(|m| is_member(m)) {
        return None;
    }
    let head = desc[..start]
        .trim_end()
        .trim_end_matches([';', ','])
        .trim_end();
    Some((
        head.to_string(),
        members.into_iter().map(str::to_string).collect(),
    ))
}

/// Move the trailing value list of each flag's description into its
/// `choices`, for a flag that has none yet.
pub(super) fn attach_description_tail_choices(flags: &mut [Entity]) {
    for flag in flags {
        if !flag.choices.is_empty() {
            continue;
        }
        let Some(desc) = flag.description.as_ref() else {
            continue;
        };
        let Some((head, members)) = split_tail(desc.as_str()) else {
            continue;
        };
        if head.is_empty() {
            continue;
        }
        flag.description = Some(Text::sanitize(&head));
        flag.choices = members.into_iter().map(Choice::bare).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_labelled_run_to_the_end_is_split() {
        let (head, m) =
            split_tail("generate an image in FORMAT available formats: a-b, c, d.").unwrap();
        assert_eq!(head, "generate an image in FORMAT");
        assert_eq!(m, ["a-b", "c", "d"]);
    }

    #[test]
    fn prose_after_the_list_or_a_short_list_is_left() {
        assert!(split_tail("one of: a, b").is_none());
        assert!(split_tail("valid values: a, b, c and then more").is_none());
        assert!(split_tail("valid values: a, B, c").is_none());
        assert!(split_tail("no label: a, b, c").is_none());
    }
}
