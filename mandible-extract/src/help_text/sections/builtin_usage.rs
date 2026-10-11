//! A shell builtin's help, `cd: cd [-L|[-P [-e]]] [-@] [dir]`: the name is
//! written twice and the usage line is followed by the indented paragraphs
//! that describe the builtin, up to the `Options:` heading. Those paragraphs
//! are the description. `docs/shapes.md` S-227, `corpus/cd/user`.

use super::*;

/// The paragraphs between a `NAME: NAME ...` usage line and the first
/// section heading, dedented, when the usage line at `at` has that shape.
pub(super) fn description_after_usage(
    lines: &[&str],
    at: Option<usize>,
    tool_name: Option<&str>,
) -> Option<String> {
    let (at, name) = at.zip(tool_name)?;
    let first = lines.get(at)?.trim();
    let rest = first.strip_prefix(name)?.strip_prefix(": ")?;
    if starts_with_usage_prefix(rest) {
        return None;
    }
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut after_block = None;
    for (n, l) in lines[at + 1..].iter().enumerate() {
        let t = l.trim();
        if is_section_heading_line(t) {
            after_block = Some((at + 1 + n, leading_whitespace(l)));
            break;
        }
        if t.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(t);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    if let Some((heading_at, indent)) = after_block {
        paragraphs.extend(paragraphs_after_block(lines, heading_at, indent));
    }
    let text = paragraphs
        .iter()
        .map(|p| p.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// The paragraphs that follow the block under the heading at `heading_at`
/// (`Options:` and its more-indented rows), at the heading's own indent,
/// up to the next section heading: `cd`'s "The default is to follow
/// symbolic links ..." paragraph, text the options block must not hide.
fn paragraphs_after_block<'a>(
    lines: &[&'a str],
    heading_at: usize,
    indent: usize,
) -> Vec<Vec<&'a str>> {
    let mut j = heading_at + 1;
    while j < lines.len() && (lines[j].trim().is_empty() || leading_whitespace(lines[j]) > indent)
    {
        j += 1;
    }
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    while j < lines.len() {
        let l = lines[j];
        let t = l.trim();
        if t.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else if leading_whitespace(l) != indent || is_section_heading_line(t) {
            current.clear();
            break;
        } else {
            current.push(t);
        }
        j += 1;
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}

/// What joins a rescued wrapped-prose paragraph (S-027) to the description:
/// a blank line when the paragraph opens with its own section heading
/// (`Exit Status:` in `cd`'s help), a space otherwise.
pub(super) fn paragraph_separator(lines: &[&str], flag_line_idx: usize) -> &'static str {
    let mut start = flag_line_idx;
    while start > 0 && !lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    if is_section_heading_line(lines[start].trim()) {
        "\n\n"
    } else {
        " "
    }
}
