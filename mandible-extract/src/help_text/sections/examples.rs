//! A headed `Example`/`Examples` block, kept as the tool printed it.
//! The block was already fenced off so its rows never become flags or
//! subcommands (`docs/shapes.md` S-071); this reads the same region into
//! [`ParsedHelp::examples`] instead of discarding it. `corpus/tar/1.35`.
//!
//! The heading sits at the document's own left edge. Content deeper than
//! the heading runs until the first line indented less than it began.
//! Content flush with the heading runs while each flush line is shaped
//! like a command invocation (`mandible_core::looks_like_invocation`), so
//! a following heading or prose sentence ends it. Paragraphs split at
//! blank lines and lose the block's common indent, nothing else.

use mandible_core::looks_like_invocation;

/// Most paragraphs one document contributes.
const MAX_PARAGRAPHS: usize = 256;

/// Width of the line's leading whitespace, tabs counted to 8-column stops.
fn indent_of(line: &str) -> usize {
    let mut col = 0;
    for c in line.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += 8 - col % 8,
            _ => break,
        }
    }
    col
}

/// The line with its leading whitespace rewritten as `drop` fewer spaces.
fn dedented(line: &str, drop: usize) -> String {
    let width = indent_of(line);
    let body = line.trim_start_matches([' ', '\t']);
    format!("{}{}", " ".repeat(width.saturating_sub(drop)), body)
}

/// True for a line holding only the word `Example` or `Examples`, with an
/// optional colon, in any case.
fn is_examples_heading(line: &str) -> bool {
    let t = line.trim();
    let word = t.strip_suffix(':').unwrap_or(t).trim_end();
    word.eq_ignore_ascii_case("example") || word.eq_ignore_ascii_case("examples")
}

/// The lines under the heading at `at`, up to where its block ends.
fn block_lines<'a>(lines: &[&'a str], at: usize) -> Vec<&'a str> {
    let heading_indent = indent_of(lines[at]);
    let Some(first) = (at + 1..lines.len()).find(|&j| !lines[j].trim().is_empty()) else {
        return Vec::new();
    };
    let base = indent_of(lines[first]);
    let flush = base <= heading_indent;
    let mut block = Vec::new();
    for &line in &lines[first..] {
        if !line.trim().is_empty() {
            let indent = indent_of(line);
            let ends = if flush {
                indent <= heading_indent && !looks_like_invocation(line.trim())
            } else {
                indent < base
            };
            if ends {
                break;
            }
        }
        block.push(line);
    }
    while block.last().is_some_and(|l| l.trim().is_empty()) {
        block.pop();
    }
    block
}

/// Every example block of `raw`, as paragraphs of lines.
pub(super) fn capture(raw: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = raw.lines().collect();
    if super::looks_like_man_page(&lines) {
        return Vec::new();
    }
    let edge = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    let mut out: Vec<Vec<String>> = Vec::new();
    for at in 0..lines.len() {
        if !is_examples_heading(lines[at]) || indent_of(lines[at]) != edge {
            continue;
        }
        let block = block_lines(&lines, at);
        let common = block
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(0);
        let mut paragraph: Vec<String> = Vec::new();
        for line in block {
            if line.trim().is_empty() {
                if !paragraph.is_empty() {
                    out.push(std::mem::take(&mut paragraph));
                }
            } else {
                paragraph.push(dedented(line, common));
            }
        }
        if !paragraph.is_empty() {
            out.push(paragraph);
        }
    }
    out.truncate(MAX_PARAGRAPHS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(raw: &str) -> Vec<Vec<String>> {
        capture(raw)
    }

    #[test]
    fn an_indented_block_ends_at_the_first_shallower_line() {
        let raw = "Examples:\n  tar -cf a.tar x  # make\n  tar -xf a.tar    # extract\n\n Main mode:\n  -c  create\n";
        assert_eq!(
            got(raw),
            vec![vec![
                "tar -cf a.tar x  # make".to_string(),
                "tar -xf a.tar    # extract".to_string()
            ]]
        );
    }

    #[test]
    fn a_flush_block_keeps_indented_explanations_and_ends_at_a_heading() {
        let raw = "EXAMPLES:\ntool -l '*x*'\n    list probes\n\ntool -e 'p'\n    trace\nSee also:\nmore\n";
        assert_eq!(
            got(raw),
            vec![
                vec!["tool -l '*x*'".to_string(), "    list probes".to_string()],
                vec!["tool -e 'p'".to_string(), "    trace".to_string()],
            ]
        );
    }

    #[test]
    fn a_flush_block_of_prose_is_not_captured() {
        assert!(got("Example:\nThese are paths in the store.\nmore text\n").is_empty());
    }

    #[test]
    fn a_heading_indented_inside_the_document_is_not_captured() {
        assert!(got("  -x  does a thing\n        Example:\n            tool -x y\n").is_empty());
    }

    #[test]
    fn tabs_count_to_eight_column_stops_and_the_common_indent_goes() {
        let raw = "Example:\n\n\t$ echo x | jq .\n\t{\n\t  \"a\": 0\n\t}\n\nOptions:\n";
        assert_eq!(
            got(raw),
            vec![vec![
                "$ echo x | jq .".to_string(),
                "{".to_string(),
                "  \"a\": 0".to_string(),
                "}".to_string()
            ]]
        );
    }
}
