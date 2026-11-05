//! A production flag row whose value is a braced lowercase list (S-258):
//! `-f[amily] { inet | inet6 | link } |` becomes `-f[amily] {inet|inet6|link}`.
//! A braced list of dash words is one flag with those aliases and the value
//! after it: `-nm | -nam[es] | { -cf | -conf } path`. A lone closing brace
//! after a flag is the production's own, never a description.

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

/// `{ -cf | -conf } path` as one spec, `-cf | -conf path`: a braced list of
/// bare dash words, then an optional plain value.
fn braced_aliases(segment: &str) -> Option<String> {
    let rest = segment.trim().strip_prefix('{')?;
    let close = rest.find('}')?;
    let (inner, tail) = (&rest[..close], rest[close + 1..].trim());
    let words: Vec<&str> = inner.split('|').map(str::trim).collect();
    let dashed = |w: &&str| w.len() > 1 && w.starts_with('-') && !w.contains(char::is_whitespace);
    if words.len() < 2 || !words.iter().all(dashed) || tail.contains(['{', '}', '|']) {
        return None;
    }
    if tail.starts_with('-') {
        return None;
    }
    let spec = format!("{} {tail}", words.join(" | "));
    let parsed = parse_flag_spec(spec.trim_end());
    (parsed.fully_consumed && parsed.value_name.is_some() == !tail.is_empty())
        .then(|| spec.trim_end().to_string())
}

/// `segment` cut before each top-level word that starts with a dash: one
/// piece per flag run together by a bare space. Words inside brackets or
/// braces (`{ -cf | -conf } path`) never start a piece.
pub(super) fn split_at_dash_words(segment: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut depth = 0i32;
    for word in segment.split_whitespace() {
        let opens_flag = depth <= 0 && word.len() > 1 && word.starts_with('-');
        match pieces.last_mut() {
            Some(last) if !opens_flag => {
                last.push(' ');
                last.push_str(word);
            }
            _ => pieces.push(word.to_string()),
        }
        depth += word
            .chars()
            .map(|c| i32::from(matches!(c, '[' | '{')) - i32::from(matches!(c, ']' | '}')))
            .sum::<i32>();
    }
    pieces
}

/// The opening delimiter that would match a given closing one.
fn matching_open(close: char) -> char {
    match close {
        '}' => '{',
        ')' => '(',
        _ => '[',
    }
}

/// Trims a trailing closing bracket (`}`/`)`/`]`) that has no opening
/// counterpart earlier in the same segment — the residue of an *enclosing*
/// BNF alternation group's own closer landing on the row's last
/// alternative once the row has been split on `|`: `vdpa`'s `-p[retty] }`
/// and `dcb`'s glued `--verbose]` are the same shape at two distances.
///
/// The no-matching-opener test tells this apart from a bracket that really
/// belongs to the segment: `-b[atch] [filename]`'s trailing `]` closes a
/// `[` earlier in the same segment and is left untouched. See
/// docs/shapes.md S-043.
pub(super) fn strip_trailing_stray_bracket(segment: &str) -> &str {
    let trimmed = segment.trim_end();
    let Some(last) = trimmed.chars().next_back() else {
        return trimmed;
    };
    if !matches!(last, '}' | ')' | ']') {
        return trimmed;
    }
    let before = &trimmed[..trimmed.len() - last.len_utf8()];
    if before.contains(matching_open(last)) {
        trimmed
    } else {
        before.trim_end()
    }
}

/// A row that ends in a braced alias list: the flags before it, then the
/// list's own flag. The part before it is one alias run when it is exactly
/// a lower-case single-dash word and a second dash word (`-nm | -nam[es]`),
/// else the flags S-043 splits.
fn entries_before_braced_aliases(line: &str) -> Option<Vec<(String, String)>> {
    let segments = split_alternatives(line.trim().trim_end_matches('|').trim_end());
    let (last, head) = segments.split_last()?;
    let tail = braced_aliases(last)?;
    let mut entries = match head {
        [] => Vec::new(),
        [first, second]
            if first.len() >= 3
                && first[1..].chars().all(|c| c.is_ascii_lowercase())
                && first.starts_with('-')
                && second.starts_with('-') =>
        {
            let run = format!("{first} | {second}");
            let parsed = parse_flag_spec(&run);
            if !(parsed.fully_consumed && parsed.value_name.is_none()) {
                return None;
            }
            vec![(run, String::new())]
        }
        _ => alternatives(&head.join(" | "))?,
    };
    entries.push((tail, String::new()));
    Some(entries)
}

/// One flag followed by the production's own closing bracket (`-br[ief] }`).
fn flag_before_stray_closer(line: &str) -> Option<(String, String)> {
    let text = line.trim();
    let bare = strip_trailing_stray_bracket(text);
    if bare.len() == text.len() || bare.contains('|') || !looks_like_flag_start(bare) {
        return None;
    }
    parse_flag_spec(bare)
        .fully_consumed
        .then(|| (bare.to_string(), String::new()))
}

/// [`braced_value_entry`], else the list-ending and closer rows, else
/// S-043's alternatives.
pub(super) fn alternatives(line: &str) -> Option<Vec<(String, String)>> {
    if let Some(entry) = braced_value_entry(line) {
        return Some(vec![entry]);
    }
    if let Some(entries) = entries_before_braced_aliases(line) {
        return Some(entries);
    }
    if let Some(entry) = flag_before_stray_closer(line) {
        return Some(vec![entry]);
    }
    split_bnf_alternation_row(line)
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
    fn a_braced_dash_list_is_one_flag_with_aliases_and_the_value_after_it() {
        assert_eq!(
            alternatives("-nm | -nam[es] | { -cf | -conf } path"),
            Some(vec![
                ("-nm | -nam[es]".to_string(), String::new()),
                ("-cf | -conf path".to_string(), String::new()),
            ])
        );
        assert_eq!(
            alternatives("{ -cf | -conf } path"),
            Some(vec![("-cf | -conf path".to_string(), String::new())])
        );
    }

    #[test]
    fn a_lone_closing_brace_after_a_flag_is_not_its_description() {
        assert_eq!(
            alternatives("-br[ief] }"),
            Some(vec![("-br[ief]".to_string(), String::new())])
        );
        assert_eq!(alternatives("-b[atch] [filename]"), None);
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
