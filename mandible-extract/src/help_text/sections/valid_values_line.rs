//! A line `Valid <words> <noun>: v1, v2, ...` outside any table lists the
//! values of the flag whose description points at it (`see below for valid
//! formats`). The values become that flag's choices; a value's `(default)`
//! note is its description, parentheses kept as printed. The prose
//! paragraph right after the line (same indent, up to a blank line) joins
//! the flag's description. Exactly one choice-less flag must name
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
            [one] if one.starts_with('(') && one.ends_with(')') => Some((*one).to_string()),
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
/// The paragraph after `lines[at]`: blank lines skipped, then consecutive
/// lines at the same indent that are not themselves `Valid ...` lines.
fn trailing_paragraph(lines: &[&str], at: usize) -> Option<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let base = indent(lines[at]);
    let mut rest = lines[at + 1..].iter().skip_while(|l| l.trim().is_empty());
    let first = rest.next()?;
    if first.trim().starts_with("Valid ") || indent(first) != base {
        return None;
    }
    let mut words = vec![first.trim()];
    for l in rest {
        if l.trim().is_empty() || indent(l) != base {
            break;
        }
        words.push(l.trim());
    }
    Some(words.join(" "))
}

pub(super) fn attach(lines: &[&str], flags: &mut [Entity]) {
    for (at, line) in lines.iter().enumerate() {
        let Some((noun, values)) = valid_line(line) else {
            continue;
        };
        let mut owners = flags
            .iter_mut()
            .filter(|f| f.choices.is_empty() && mentions(f, &noun));
        let (Some(flag), None) = (owners.next(), owners.next()) else {
            continue;
        };
        if let Some(para) = trailing_paragraph(lines, at) {
            let own = flag.description.as_ref().map(|d| d.as_str().to_string());
            let joined = match own {
                Some(o) => format!("{o} {para}"),
                None => para,
            };
            flag.description = Some(Text::sanitize(&joined));
        }
        flag.choices = values
            .into_iter()
            .map(|(name, note)| match note {
                Some(n) => Choice::described(name, Text::sanitize(&n)),
                None => Choice::bare(name),
            })
            .collect();
    }
}
