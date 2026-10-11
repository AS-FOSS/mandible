//! A usage block whose forms each end in a tab-separated description, with
//! indented setting rows under some of them (`ethtool [ FLAGS ] -s|--change
//! DEVNAME<TAB>Change generic options`, then `[ speed %d ]`). The text after
//! the tab describes the form's operation flag, the rows are that
//! operation's own settings, and neither is an operand. Rule: three or more
//! forms open with the same command word and carry a tab description; a
//! wrapped description at column zero that itself holds a tab continues the
//! row above instead of ending the block. See docs/shapes.md S-243 and
//! corpus/ethtool/6.7.

use super::*;

/// Fewest tab-described forms that make a table.
const MIN_TAB_FORMS: usize = 3;

/// `(head, description)` split at the first tab. `None` when the line
/// carries no description.
fn split_description(line: &str) -> Option<(&str, &str)> {
    let at = line.find('\t')?;
    let desc = line[at..].trim();
    (!desc.is_empty()).then(|| (line[..at].trim_end(), desc))
}

/// [`split_description`], else the same split at the first run of three
/// spaces. Used on a form of a detected table only.
fn split_form(line: &str) -> Option<(&str, &str)> {
    split_description(line).or_else(|| {
        let at = line.find("   ")?;
        let desc = line[at..].trim();
        (!desc.is_empty()).then(|| (line[..at].trim_end(), desc))
    })
}

fn first_word(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// The command word the table's forms open with, when at least
/// [`MIN_TAB_FORMS`] lines opening with it carry a tab description.
fn table_head(usage_lines: &[String]) -> Option<&str> {
    let head = usage_lines
        .iter()
        .find(|l| split_description(l).is_some())
        .and_then(|l| first_word(l))?;
    let count = usage_lines
        .iter()
        .filter(|l| first_word(l) == Some(head) && split_description(l).is_some())
        .count();
    (count >= MIN_TAB_FORMS).then_some(head)
}

/// True when the flush-left line `i` still belongs to the usage block: it
/// reads as usage grammar, or it continues the description of the
/// tab-described form above it.
pub(super) fn continues_block(lines: &[&str], i: usize) -> bool {
    looks_like_usage_fragment(lines[i].trim_start()) || continues_tab_description(lines, i)
}

/// True when line `i` (flush left, holding a tab) continues the description
/// of the tab-described form directly above it.
fn continues_tab_description(lines: &[&str], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).and_then(|p| lines.get(p)) else {
        return false;
    };
    let cur = lines[i];
    !cur.starts_with(char::is_whitespace)
        && cur.trim_end().contains('\t')
        && split_description(prev.trim()).is_some()
        && prev.starts_with(char::is_whitespace)
}

fn is_operand_word(w: &str) -> bool {
    w.len() > 1
        && w.chars()
            .all(|c| c.is_uppercase() || matches!(c, '_' | '-'))
}

/// The usage lines with each form cut before its description. Rows are kept,
/// so the flags they spell (`[ --src a | b ]`) are still read.
pub(super) fn flag_lines(usage_lines: &[String]) -> Option<Vec<String>> {
    let head = table_head(usage_lines)?;
    Some(
        usage_lines
            .iter()
            .map(|l| match split_form(l) {
                Some((form, _)) if first_word(l) == Some(head) => form.to_string(),
                _ => l.clone(),
            })
            .collect(),
    )
}

/// The usage lines with each form cut before its description and each
/// setting row dropped, except a row opening with an UPPERCASE operand
/// word. One output line per input line, so line indexes stay aligned.
pub(super) fn positional_lines(usage_lines: &[String]) -> Option<Vec<String>> {
    let head = table_head(usage_lines)?;
    Some(
        usage_lines
            .iter()
            .map(|l| {
                if first_word(l) == Some(head) {
                    split_form(l).map_or(l.clone(), |(form, _)| form.to_string())
                } else if first_word(l).is_some_and(is_operand_word) {
                    l.clone()
                } else {
                    String::new()
                }
            })
            .collect(),
    )
}

/// The `-s|--change` alias run of a form head, as bare names.
fn operation_names(form: &str) -> Vec<String> {
    let mut toks = form.split_whitespace().skip(1).peekable();
    if toks.peek() == Some(&"[") {
        for t in toks.by_ref() {
            if t == "]" {
                break;
            }
        }
    }
    match toks.next() {
        Some(run) if run.starts_with('-') => run
            .split('|')
            .map(|s| s.trim_start_matches('-').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Gives each operation flag the description printed after its form, then
/// the setting rows under it, as printed and in order.
pub(super) fn attach_descriptions(usage_lines: &[String], flags: &mut [Entity]) {
    let Some(head) = table_head(usage_lines) else {
        return;
    };
    let mut owner: Option<(Vec<String>, Vec<&str>)> = None;
    let mut finished = Vec::new();
    for l in usage_lines {
        if first_word(l) == Some(head) {
            finished.extend(owner.take());
            let names = split_form(l).map(|(form, desc)| (operation_names(form), desc));
            owner = names.map(|(n, d)| (n, vec![d]));
        } else if let Some((_, parts)) = owner.as_mut() {
            parts.push(l.as_str());
        }
    }
    finished.extend(owner);
    for (names, parts) in finished {
        let text = parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let line = p.split_whitespace().collect::<Vec<_>>().join(" ");
                let row = i > 0 && !p.contains('\t');
                if row {
                    format!("  {line}")
                } else {
                    line
                }
            })
            .filter(|p| !p.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let target = flags.iter_mut().find(|f| {
            f.kind == EntityKind::Flag
                && f.description.is_none()
                && f.spellings.iter().any(|s| names.contains(&s.name))
        });
        if let (Some(flag), false) = (target, names.is_empty()) {
            flag.description = non_empty_text(&text);
        }
    }
}
