//! Root-description text the leading-prose scan misses: prose that follows
//! the usage block and a blank line, an indented lead paragraph above a
//! bare heading word, and the heading word itself (never a description).
//! `docs/shapes.md` S-235, S-236.

use super::*;

/// A column-0 single capitalised word on its own line whose next non-blank
/// line is indented (`Synopsis` above an indented usage): a heading, not
/// prose.
pub(super) fn is_bare_heading_word(lines: &[&str], idx: usize) -> bool {
    let Some(line) = lines.get(idx) else {
        return false;
    };
    let t = line.trim_end();
    let mut chars = t.chars();
    let word_shaped = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphabetic());
    word_shaped
        && lines[idx + 1..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| leading_whitespace(l) > 0)
}

/// The indented paragraph that opens a document (`   xdg-open - opens a
/// file or URL ...`), dedented, when a bare heading word follows it.
pub(super) fn indented_lead(lines: &[&str]) -> Option<String> {
    let start = lines.iter().position(|l| !l.trim().is_empty())?;
    let end = lines[start..]
        .iter()
        .position(|l| l.trim().is_empty())
        .map_or(lines.len(), |n| start + n);
    let para = &lines[start..end];
    if para
        .iter()
        .any(|l| leading_whitespace(l) == 0 || starts_with_usage_prefix(l.trim()))
    {
        return None;
    }
    let next = lines[end..].iter().position(|l| !l.trim().is_empty())? + end;
    if !is_bare_heading_word(lines, next) {
        return None;
    }
    Some(para.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n"))
}

/// Column-0 prose paragraphs from `from` on (after the usage block), up to
/// the first line that is not plain prose: a heading, a flag-shaped or
/// usage-shaped line, an indented line, or a line with a column gap.
pub(super) fn prose_after_usage(lines: &[&str], from: usize) -> Option<String> {
    // Prose is set off from the usage block by a blank line; rows right
    // under a bare `Usage:` label are forms, not prose.
    if lines.get(from).is_some_and(|l| !l.trim().is_empty()) {
        return None;
    }
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut j = from;
    'scan: while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            if is_block_label(&current, lines, j) {
                current.clear();
                break 'scan;
            }
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            j += 1;
            continue;
        }
        let t = l.trim_end();
        let plain = leading_whitespace(l) == 0
            && !t.starts_with('-')
            && !t.ends_with(':')
            && !t.contains("  ")
            && !t.contains('\t')
            && !is_section_heading_line(t)
            && !starts_with_usage_prefix(t)
            && !looks_like_bnf_production_line(t)
            && !is_bare_heading_word(lines, j)
            && !(current.is_empty() && looks_like_invocation_row(t))
            // A line with an indented line right beneath it labels a block.
            && !lines
                .get(j + 1)
                .is_some_and(|n| !n.trim().is_empty() && leading_whitespace(n) > 0);
        if !plain {
            // A paragraph cut short by a non-prose line is not whole prose.
            current.clear();
            break 'scan;
        }
        current.push(l);
        j += 1;
    }
    if !is_block_label(&current, lines, lines.len()) && !current.is_empty() {
        paragraphs.push(current);
    }
    let text = paragraphs
        .iter()
        .map(|p| p.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

/// `list [ --username=username ]`: a lowercase word then a flag or bracket
/// token, a command form rather than a sentence.
fn looks_like_invocation_row(t: &str) -> bool {
    let mut words = t.split_whitespace();
    let (Some(first), Some(second)) = (words.next(), words.next()) else {
        return false;
    };
    first.starts_with(|c: char| c.is_ascii_lowercase())
        && (second.starts_with('-') || second.starts_with('['))
}

/// A short unpunctuated single line (`General commands`) with an indented
/// block after the blank line is that block's label, not prose.
fn is_block_label(paragraph: &[&str], lines: &[&str], blank_at: usize) -> bool {
    let [only] = paragraph else {
        return false;
    };
    let t = only.trim();
    t.split_whitespace().count() <= 4
        && !t.ends_with(['.', '!', '?', ')', ';'])
        && lines
            .get(blank_at..)
            .and_then(|rest| rest.iter().find(|l| !l.trim().is_empty()))
            .is_some_and(|l| leading_whitespace(l) > 0)
}
