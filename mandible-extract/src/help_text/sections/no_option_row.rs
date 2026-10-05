//! A table row whose spelling is a bracketed "no option given" phrase
//! (`<no option>`, `(no options)`) describes the bare invocation. It is
//! never a flag, so its text joins the node description. See docs/shapes.md
//! S-201 and corpus/jinfo/17.0.20-no-option.

use super::{leading_whitespace, ParsedHelp};

/// The `("With no option", description)` pair a row spells out, with the
/// row's own wrapped description lines folded in, and the index of the
/// first line after the row. `None` when `lines[i]` is not such a row.
fn no_option_row(lines: &[&str], i: usize) -> Option<(usize, String)> {
    let line = lines.get(i)?;
    let trimmed = line.trim_start();
    let close = match trimmed.chars().next()? {
        '<' => '>',
        '(' => ')',
        '[' => ']',
        _ => return None,
    };
    let end = trimmed.find(close)?;
    let label = trimmed.get(1..end)?;
    if !names_no_option(label) {
        return None;
    }
    let rest = trimmed.get(end + 1..)?;
    // A single space after the label is prose, not a column gap.
    if !rest.starts_with("  ") && !rest.starts_with('\t') {
        return None;
    }
    let mut text = rest.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let indent = leading_whitespace(line);
    let mut next = i + 1;
    while let Some(cont) = lines.get(next) {
        if cont.trim().is_empty() || leading_whitespace(cont) <= indent {
            break;
        }
        text.push(' ');
        text.push_str(cont.trim());
        next += 1;
    }
    Some((next, format!("With {label}: {text}")))
}

/// Appends the row at `lines[i]` to the node description as
/// `With no option: ...` and returns the index after it. `None` when the
/// line is no such row or sits in an ignorable section.
pub(super) fn take_no_option_row(
    lines: &[&str],
    i: usize,
    out: &mut ParsedHelp,
    ignorable: bool,
) -> Option<usize> {
    if ignorable {
        return None;
    }
    let (end, text) = no_option_row(lines, i)?;
    out.description = Some(match out.description.take() {
        Some(d) => format!("{d} {text}"),
        None => text,
    });
    Some(end)
}

/// `no option`, `no options`, `without options`, `no arguments`, `no flags`.
fn names_no_option(label: &str) -> bool {
    let mut words = label.split_whitespace();
    let (Some(first), Some(second), None) = (words.next(), words.next(), words.next()) else {
        return false;
    };
    matches!(first, "no" | "without")
        && matches!(
            second,
            "option" | "options" | "argument" | "arguments" | "flag" | "flags"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_with_column_gap_is_read() {
        let lines = ["    <no option>          to print both"];
        let (next, text) = no_option_row(&lines, 0).unwrap();
        assert_eq!(next, 1);
        assert_eq!(text, "With no option: to print both");
    }

    #[test]
    fn prose_mention_is_refused() {
        let lines = ["  (no options) (deprecated - no kernel support)"];
        assert!(no_option_row(&lines, 0).is_none());
    }
}
