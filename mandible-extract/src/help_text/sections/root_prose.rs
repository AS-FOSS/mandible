//! Root-description text the leading-prose scan misses: prose that follows
//! the usage block and a blank line, an indented lead paragraph above a
//! bare heading word, and the heading word itself (never a description),
//! including a colon label (`Options:`) at the edge of the description.
//! `docs/shapes.md` S-235, S-236, S-249.

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

/// A short column-0 label ending in a colon (`Options:`, `Commands are:`,
/// `where possible options include:`) whose next non-blank line is
/// indented: it labels the block below it, it is not prose. A sentence
/// lead-in (more than four words) is not one, nor is an identifier or a
/// line carrying a number (`armelf_linux_eabi:`, `1404 packages installed:`).
/// Its text still renders as the group label of the block it heads.
/// `docs/shapes.md` S-249.
pub(super) fn is_colon_block_label(lines: &[&str], idx: usize) -> bool {
    let Some(line) = lines.get(idx) else {
        return false;
    };
    let t = line.trim_end();
    leading_whitespace(line) == 0
        && t.split_whitespace().count() <= 4
        && is_section_heading_line(t)
        && t.trim_end_matches(':')
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == ' ')
        && lines[idx + 1..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| leading_whitespace(l) > 0)
}

/// The line indices of block labels at the two edges of the description
/// candidates (`candidate` says which column-0 lines may be description):
/// every label before the first prose line and every label after the last.
/// A label amid prose is left alone. `docs/shapes.md` S-249.
pub(super) fn edge_label_lines(
    lines: &[&str],
    bound: usize,
    candidate: impl Fn(usize) -> bool,
) -> std::collections::HashSet<usize> {
    let kept: Vec<usize> = (0..lines.len().min(bound))
        .filter(|&j| !lines[j].trim().is_empty() && candidate(j))
        .collect();
    let is_label = |j: &usize| is_colon_block_label(lines, *j);
    let lead = kept.iter().take_while(|j| is_label(j)).count();
    let trail = kept[lead..]
        .iter()
        .rev()
        .take_while(|j| is_label(j))
        .count();
    kept[..lead]
        .iter()
        .chain(&kept[kept.len() - trail..])
        .copied()
        .collect()
}

/// Where a headingless flag table starts inside the leading paragraph
/// `from..bound`, and the line its colon label sits on (the start itself
/// when there is no label). `corpus/python3/3.12`, `docs/shapes.md` S-279.
pub(super) fn flag_table_start(
    lines: &[&str],
    from: usize,
    bound: usize,
) -> Option<(usize, usize)> {
    let start = (from..bound.min(lines.len()))
        .find(|&j| starts_attested_headingless_flag_block(lines, j))?;
    let label = start
        .checked_sub(1)
        .filter(|&j| j >= from)
        .filter(|&j| leading_whitespace(lines[j]) == 0 && lines[j].trim_end().ends_with(':'));
    Some((start, label.unwrap_or(start)))
}

/// True when `lines[idx]` is the colon label of a flag table or one of its
/// own column-0 rows: those render as flags and a group, not as prose.
pub(super) fn is_flag_table_line(
    lines: &[&str],
    table: Option<(usize, usize)>,
    idx: usize,
) -> bool {
    table.is_some_and(|(start, label)| {
        idx == label && label != start
            || idx >= start && looks_like_flag_start(lines[idx].trim_start())
    })
}

/// A full-stop sentence that heads a block is no group label
/// ([`heading_can_name_a_group`]); its paragraph ends the node description
/// unless the description holds it already. A line under a differently
/// indented line is a table row, not a paragraph, and stays out.
/// `corpus/cp/9.4`, `docs/shapes.md` S-277.
pub(super) fn relocate_heading_sentence(lines: &[&str], at: usize, result: &mut ParsedHelp) {
    if !is_prose_sentence(lines[at].trim()) {
        return;
    }
    let indent = leading_whitespace(lines[at]);
    let mut first = at;
    while first > 0
        && !lines[first - 1].trim().is_empty()
        && !lines[first - 1].trim_end().ends_with(':')
    {
        if leading_whitespace(lines[first - 1]) != indent {
            return;
        }
        first -= 1;
    }
    if lines[first..=at]
        .iter()
        .any(|l| starts_with_usage_prefix(l.trim()))
    {
        return;
    }
    let text = lines[first..=at]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let held = result
        .description
        .as_deref()
        .is_some_and(|d| squash(d).contains(&squash(&text)));
    if held {
        return;
    }
    result.description = Some(match result.description.take() {
        Some(d) => format!("{d}\n\n{text}"),
        None => text,
    });
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
    let mut paragraphs: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_example = false;
    let mut j = from;
    'scan: while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            in_example = false;
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
        // An example block opens on a `$ command` line at the start of a
        // paragraph; the lines under it are that command's printed output,
        // columns and all, until the blank line. S-250.
        if current.is_empty() && is_shell_example_line(t) {
            in_example = true;
        }
        if in_example && leading_whitespace(l) == 0 {
            // The printed output is kept as its own lines, set in from the
            // command that printed it: `Text::sanitize` keeps a line
            // indented within its paragraph on a line of its own.
            current.push(if is_shell_example_line(t) {
                t.to_string()
            } else {
                format!("  {t}")
            });
            j += 1;
            continue;
        }
        let lead_in = current.is_empty() && is_example_lead_in(lines, j);
        let plain = leading_whitespace(l) == 0
            && !t.starts_with('-')
            && (lead_in || !t.ends_with(':'))
            && !has_column_gap(t)
            && !t.contains('\t')
            && (lead_in || !is_section_heading_line(t))
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
        current.push(l.to_string());
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

/// A run of two or more spaces inside the line that is not sentence
/// spacing: two spaces after a full stop, `?` or `!` (optionally closed by
/// `)` or a quote) before a capital letter are prose, not a column gap.
/// `docs/shapes.md` S-266.
fn has_column_gap(t: &str) -> bool {
    let b = t.trim().as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] != b' ' || b[i + 1] != b' ' {
            i += 1;
            continue;
        }
        let mut end = i;
        while b.get(end) == Some(&b' ') {
            end += 1;
        }
        let before = b[..i]
            .iter()
            .rev()
            .find(|c| !matches!(c, b')' | b'"' | b'\''));
        let sentence_gap = end - i == 2
            && before.is_some_and(|c| matches!(c, b'.' | b'?' | b'!'))
            && b.get(end).is_some_and(|c| c.is_ascii_uppercase());
        if !sentence_gap {
            return true;
        }
        i = end;
    }
    false
}

/// A shell-prompt example line: `$ snap get snap-name username`.
fn is_shell_example_line(t: &str) -> bool {
    t.strip_prefix("$ ")
        .is_some_and(|rest| !rest.trim().is_empty())
}

/// A one-line paragraph ending in a colon that introduces the example block
/// right after it (`Nested values may be retrieved via a dotted path:`).
fn is_example_lead_in(lines: &[&str], j: usize) -> bool {
    let t = lines[j].trim_end();
    if !t.ends_with(':') || t.split_whitespace().count() < 3 {
        return false;
    }
    let mut rest = lines[j + 1..].iter();
    rest.next().is_some_and(|l| l.trim().is_empty())
        && rest
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| is_shell_example_line(l.trim_end()))
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
fn is_block_label(paragraph: &[String], lines: &[&str], blank_at: usize) -> bool {
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
