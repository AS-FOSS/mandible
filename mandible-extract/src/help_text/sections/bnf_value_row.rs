//! A flag row inside a grammar production whose value is a braced list of
//! lowercase alternatives (docs/shapes.md S-257):
//!
//! ```text
//! OPTIONS := { -f[amily] { inet | inet6 | link | help } |
//! ```
//!
//! The list is the flag's values. The row is rewritten to the compact
//! `-f[amily] {inet|inet6|link|help}` spec, which the alternation reading
//! (S-253) turns into choices.

use super::*;
use mandible_core::alternation_members;

/// The `(spec, description)` entry for `line`, or `None` when it is not one
/// flag followed by one braced alternation and nothing else.
pub(super) fn braced_value_entry(line: &str) -> Option<(String, String)> {
    let text = line.trim().trim_end_matches('|').trim_end();
    let open = text.find('{')?;
    let head = text[..open].trim_end();
    let list = text[open + 1..].strip_suffix('}')?;
    if list.contains(['{', '}']) || !looks_like_flag_start(head) || head.contains('|') {
        return None;
    }
    let compact = format!(
        "{{{}}}",
        list.split('|').map(str::trim).collect::<Vec<_>>().join("|")
    );
    alternation_members(&compact)?;
    let spec = parse_flag_spec(head);
    (spec.fully_consumed && spec.value_name.is_none())
        .then(|| (format!("{head} {compact}"), String::new()))
}

/// The entries a production row splits into: the one valued flag of
/// [`braced_value_entry`], else the alternatives of S-043.
pub(super) fn alternatives(line: &str) -> Option<Vec<(String, String)>> {
    match braced_value_entry(line) {
        Some(entry) => Some(vec![entry]),
        None => split_bnf_alternation_row(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_and_its_braced_list_become_one_valued_spec() {
        assert_eq!(
            braced_value_entry("-f[amily] { inet | inet6 | link | help } |"),
            Some((
                "-f[amily] {inet|inet6|link|help}".to_string(),
                String::new()
            ))
        );
    }

    #[test]
    fn anything_else_on_the_row_is_refused() {
        for row in [
            "-f[amily] { inet } |",
            "-f[amily] { inet | Inet6 } |",
            "-4 | -f[amily] { inet | inet6 } |",
            "-l[oops] { maximum-addr-flush-attempts } | -br[ief] |",
            "-f[amily] { inet | { a | b } } |",
            "-n[etns] name | { a | b }",
        ] {
            assert_eq!(braced_value_entry(row), None, "{row}");
        }
    }
}
