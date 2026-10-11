//! A block under a heading that names one flag (`Valid units for [-u]
//! flag:`) whose every line is `Label: v1, v2, ...` lists that flag's
//! values grouped by label (speedtest). Every value of every line becomes
//! a choice of the owning flag, the line's label as its description. All or
//! nothing: one line of another shape leaves the block to the generic
//! bare-block reader. The heading must write the flag's own spelling (`[-u]`,
//! `--unit`): one that only shares a word with a flag (update-passwd's `File
//! locations used:`) names nothing. See docs/shapes.md S-232.

use super::ParsedHelp;
use mandible_core::{Choice, Dashes, Entity, Text};

/// `(label, values)` for a `Label: v1, v2, ...` line, `None` otherwise.
fn labelled_line(line: &str) -> Option<(&str, Vec<&str>)> {
    let (label, rest) = line.trim().split_once(':')?;
    let label = label.trim();
    if !label.starts_with(char::is_alphabetic) {
        return None;
    }
    let values: Vec<&str> = rest.split(',').map(str::trim).collect();
    let plain = values
        .iter()
        .all(|v| !v.is_empty() && !v.contains(char::is_whitespace));
    plain.then_some((label, values))
}

/// The index of the flag whose own spelling the heading writes out.
fn flag_spelled_in(heading: &str, flags: &[Entity]) -> Option<usize> {
    let tokens: Vec<&str> = heading
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .collect();
    flags.iter().position(|f| {
        f.spellings.iter().any(|sp| {
            let dashes = match sp.dashes {
                Dashes::Single => "-",
                Dashes::Double => "--",
                Dashes::None => return false,
            };
            tokens.contains(&format!("{dashes}{}", sp.name).as_str())
        })
    })
}

/// Attach the block's values to the flag `heading` names; `false` (nothing
/// changed) when the block is not wholly of this shape or no flag with
/// no choices of its own is named.
pub(super) fn take_labelled_values(heading: &str, block: &[&str], out: &mut ParsedHelp) -> bool {
    let rows: Vec<&str> = block
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let parsed: Option<Vec<_>> = rows.iter().map(|l| labelled_line(l)).collect();
    let Some(parsed) = parsed.filter(|p| p.len() >= 2) else {
        return false;
    };
    let Some(owner) = flag_spelled_in(heading, &out.flags) else {
        return false;
    };
    if !out.flags[owner].choices.is_empty() {
        return false;
    }
    for (label, values) in parsed {
        for value in values {
            if out.flags[owner].choices.iter().any(|c| c.name == value) {
                continue;
            }
            out.flags[owner]
                .choices
                .push(Choice::described(value, Text::sanitize(label)));
        }
    }
    true
}
