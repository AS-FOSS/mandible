//! A line `Valid <words> <noun>: v1, v2, ...` outside any table lists the
//! values of the flag whose description points at it (`see below for valid
//! formats`). The values become that flag's choices; a value's `(default)`
//! note is its description. Exactly one choice-less flag must name
//! `valid <noun>`. See docs/shapes.md S-246 and corpus/speedtest/1.2.0.84.

use mandible_core::{Choice, Entity, Text};

/// One listed value and its parenthesised note.
type Listed = (String, Option<String>);

/// `(noun, values)` for a `Valid ...: v1, v2` line.
fn valid_line(line: &str) -> Option<(String, Vec<Listed>)> {
    let rest = line.trim().strip_prefix("Valid ")?;
    let (phrase, list) = rest.split_once(':')?;
    let noun = phrase.split_whitespace().last()?.to_lowercase();
    let mut values = Vec::new();
    for item in list.split(',') {
        let mut words = item.split_whitespace();
        let name = words.next()?;
        let note: Vec<&str> = words.collect();
        let note = match note.as_slice() {
            [] => None,
            [one] if one.starts_with('(') && one.ends_with(')') => {
                Some(one.trim_matches(['(', ')']).to_string())
            }
            _ => return None,
        };
        values.push((name.to_string(), note));
    }
    (values.len() >= 2).then_some((noun, values))
}

fn mentions(flag: &Entity, noun: &str) -> bool {
    flag.description.as_ref().is_some_and(|d| {
        let d = d.as_str().to_lowercase();
        d.contains(&format!("valid {noun}"))
    })
}

/// Attaches each `Valid <noun>:` line's values to the one flag naming it.
pub(super) fn attach(lines: &[&str], flags: &mut [Entity]) {
    for line in lines {
        let Some((noun, values)) = valid_line(line) else {
            continue;
        };
        let mut owners = flags
            .iter_mut()
            .filter(|f| f.choices.is_empty() && mentions(f, &noun));
        let (Some(flag), None) = (owners.next(), owners.next()) else {
            continue;
        };
        flag.choices = values
            .into_iter()
            .map(|(name, note)| match note {
                Some(n) => Choice::described(name, Text::sanitize(&n)),
                None => Choice::bare(name),
            })
            .collect();
    }
}
