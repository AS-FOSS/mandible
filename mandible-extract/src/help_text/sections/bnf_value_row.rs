//! A production flag row whose value is a braced lowercase list (S-258):
//! `-f[amily] { inet | inet6 | link } |` becomes `-f[amily] {inet|inet6|link}`.

use super::*;
use mandible_core::alternation_members;

/// The entry for one flag plus one braced alternation, else `None`.
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

/// [`braced_value_entry`], else S-043's alternatives.
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
