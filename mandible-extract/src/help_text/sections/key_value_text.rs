//! `key=value` text a usage line declares (`[option=value]*`) and the
//! `key=<placeholder>` list under a heading (`jfr configure`'s `Options for
//! default.jfc:`): both are the node's description, printed as the help
//! prints them, never operands or flags. `docs/shapes.md` S-237 (replaces
//! the operand and dashless-flag readings of S-202 and S-205),
//! `corpus/jfr/17.0.20-configure`.

use super::*;

/// True for `word=word` with an identifier-like key and a non-empty value.
fn is_key_value(token: &str) -> bool {
    let Some((key, value)) = token.split_once('=') else {
        return false;
    };
    key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !value.is_empty()
}

/// The `key=value` words the usage lines declare, brackets and repeat
/// marks stripped.
fn declared_tokens(usage: &[String]) -> Vec<String> {
    usage
        .iter()
        .flat_map(|u| u.split_whitespace())
        .map(|t| t.trim_matches(|c| matches!(c, '[' | ']' | '*' | '.')))
        .filter(|t| is_key_value(t))
        .map(str::to_string)
        .collect()
}

/// The row at `at` joined with its wrapped continuation lines at or past
/// the description column `col`.
fn row_with_continuation(lines: &[&str], at: usize, col: usize) -> String {
    let mut text = lines[at].trim().to_string();
    for next in &lines[at + 1..] {
        if next.trim().is_empty() || leading_whitespace(next) < col {
            break;
        }
        text.push(' ');
        text.push_str(next.trim());
    }
    text
}

/// The flush (column 0) prose paragraph right after line `after`, past
/// blank lines: the sentence the help prints under the described rows
/// before its next heading. A line ending in `:` is a heading, not prose.
fn flush_paragraph_after(lines: &[&str], after: usize) -> Option<String> {
    let mut at = after + 1;
    while at < lines.len() && lines[at].trim().is_empty() {
        at += 1;
    }
    let mut words: Vec<&str> = Vec::new();
    while at < lines.len() && !lines[at].trim().is_empty() && leading_whitespace(lines[at]) == 0 {
        words.push(lines[at].trim());
        at += 1;
    }
    let text = words.join(" ");
    (!words.is_empty() && !text.ends_with(':') && text.ends_with(['.', '!', '?'])).then_some(text)
}

/// The described rows `option=value  description` of a declared word,
/// then the flush prose paragraph that follows the last of them.
fn described_rows(lines: &[&str], declared: &[String]) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut rows = Vec::new();
    let mut last_end = None;
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(token) = trimmed.split_whitespace().next() else {
            continue;
        };
        let rest = &trimmed[token.len()..];
        if leading_whitespace(line) == 0
            || !rest.starts_with("  ")
            || !declared.iter().any(|d| d == token)
            || seen.contains(&token)
        {
            continue;
        }
        seen.push(token);
        let col = line.len() - rest.trim_start().len();
        let text = row_with_continuation(lines, at, col);
        let mut end = at;
        while end + 1 < lines.len()
            && !lines[end + 1].trim().is_empty()
            && leading_whitespace(lines[end + 1]) >= col
        {
            end += 1;
        }
        last_end = Some(end);
        rows.push(match text.split_once(char::is_whitespace) {
            Some((key, tail)) => format!("{key}  {}", tail.trim()),
            None => text,
        });
    }
    if let Some(prose) = last_end.and_then(|end| flush_paragraph_after(lines, end)) {
        rows.push(prose);
    }
    rows
}

/// Headed blocks after line `from`: a flush heading ending in `:` whose
/// indented lines all start with the usage's command path (`jfr configure
/// ...`). The heading and its lines, one paragraph each, as printed.
fn trailing_invocation_blocks(lines: &[&str], from: usize, path: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut at = from;
    while at < lines.len() {
        let heading = lines[at].trim();
        if leading_whitespace(lines[at]) != 0 || !heading.ends_with(':') {
            at += 1;
            continue;
        }
        let mut rows: Vec<&str> = Vec::new();
        let mut j = at + 1;
        while j < lines.len() {
            let text = lines[j].trim();
            if text.is_empty() {
                j += 1;
            } else if leading_whitespace(lines[j]) > 0 && text.starts_with(path) {
                rows.push(text);
                j += 1;
            } else {
                break;
            }
        }
        if rows.is_empty() {
            at += 1;
        } else {
            blocks.push(format!("{heading}\n\n{}", rows.join("\n\n")));
            at = j;
        }
    }
    blocks
}

/// Every heading ending in `:` with two or more rows that are each one
/// `key=<placeholder>`: the heading, then the rows one per line.
fn placeholder_blocks(lines: &[&str]) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let heading = lines[at];
        let indent = leading_whitespace(heading);
        let heading_text = heading.trim();
        at += 1;
        if !heading_text.ends_with(':') || heading_text.split_whitespace().count() > 8 {
            continue;
        }
        let mut rows: Vec<&str> = Vec::new();
        let mut j = at;
        while j < lines.len() {
            let text = lines[j].trim();
            if text.is_empty() {
                j += 1;
                continue;
            }
            match text.split_once('=') {
                Some((_, value))
                    if leading_whitespace(lines[j]) > indent
                        && text.split_whitespace().count() == 1
                        && is_key_value(text)
                        && value.starts_with('<')
                        && value.ends_with('>') =>
                {
                    rows.push(text);
                    j += 1;
                }
                _ => break,
            }
        }
        if rows.len() >= 2 {
            at = j;
            blocks.push(format!("{heading_text}\n\n{}", rows.join("\n\n")));
        }
    }
    blocks
}

/// The command path of the first usage line: its words before the first
/// flag, bracket or `key=value` word.
fn usage_path(usage: &[String]) -> String {
    let words: Vec<&str> = usage
        .first()
        .map(|u| u.split_whitespace())
        .into_iter()
        .flatten()
        .take_while(|w| !w.starts_with(['-', '[', '<']) && !w.contains('='))
        .collect();
    words.join(" ")
}

/// The paragraphs of `key=value` text to append to the description: the
/// described rows of the usage's declared words, then each placeholder
/// block, in the order printed.
pub(super) fn key_value_paragraphs(usage: &[String], lines: &[&str]) -> Option<String> {
    let mut paragraphs = Vec::new();
    let declared = declared_tokens(usage);
    if !declared.is_empty() {
        let rows = described_rows(lines, &declared);
        if !rows.is_empty() {
            paragraphs.push(rows.join("\n\n"));
        }
    }
    let blocks = placeholder_blocks(lines);
    let had_blocks = !blocks.is_empty();
    paragraphs.extend(blocks);
    if had_blocks {
        let path = usage_path(usage);
        let from = lines
            .iter()
            .rposition(|l| l.trim().contains("=<"))
            .map_or(0, |i| i + 1);
        paragraphs.extend(trailing_invocation_blocks(lines, from, &path));
    }
    (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_rows_and_placeholder_blocks_become_text() {
        let usage = vec!["jfr configure [option=value]*".to_string()];
        let lines = [
            "  option=value     The option value.",
            "                   Wrapped.",
            "",
            "Options for x.jfc:",
            "",
            "  gc=<off|high>",
            "",
            "  limit=<timespan>",
        ];
        let text = key_value_paragraphs(&usage, &lines).unwrap();
        assert_eq!(
            text,
            "option=value  The option value. Wrapped.\n\nOptions for x.jfc:\n\ngc=<off|high>\n\nlimit=<timespan>"
        );
    }

    #[test]
    fn flush_prose_after_the_rows_is_kept() {
        let usage = vec!["jfr configure [option=value]*".to_string()];
        let lines = [
            "  option=value     The option value.",
            "",
            "The delimiter can be omitted. See",
            "the docs.",
            "",
            "Notes:",
        ];
        let text = key_value_paragraphs(&usage, &lines).unwrap();
        assert_eq!(
            text,
            "option=value  The option value.\n\nThe delimiter can be omitted. See the docs."
        );
    }

    #[test]
    fn headed_invocation_blocks_after_the_placeholders_are_kept() {
        let usage = vec!["jfr configure [option=value]*".to_string()];
        let lines = [
            "Options for x:",
            "  a=<b>",
            "  c=<d>",
            "",
            "To run it:",
            "",
            "  jfr configure --interactive",
            "",
            "Other notes:",
            "  something else",
        ];
        let text = key_value_paragraphs(&usage, &lines).unwrap();
        assert!(text.ends_with("c=<d>\n\nTo run it:\n\njfr configure --interactive"));
    }

    #[test]
    fn undeclared_pairs_and_a_lone_row_are_left_alone() {
        let usage = vec!["tool [OPTION]...".to_string()];
        let lines = ["  bs=BYTES   read and write", "Notes:", "  a=<b>"];
        assert!(key_value_paragraphs(&usage, &lines).is_none());
    }
}
