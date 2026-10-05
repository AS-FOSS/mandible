//! `key=value` operands a usage line declares (`[option=value]*`): the
//! described table rows and the heading-introduced key list become
//! positionals. A `key=value` word is never a flag. See docs/shapes.md
//! S-202 and corpus/jfr/17.0.20-configure.

use super::{leading_whitespace, non_empty_text};
use mandible_core::{Choice, Entity, Provenance, Source, ValueKind};

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

/// Names of the `key=value` operands the usage lines declare, with
/// the brackets and repeat marks stripped.
fn declared_tokens(usage: &[String]) -> Vec<String> {
    usage
        .iter()
        .flat_map(|u| u.split_whitespace())
        .map(|t| t.trim_matches(|c| matches!(c, '[' | ']' | '*' | '.')))
        .filter(|t| is_key_value(t))
        .map(str::to_string)
        .collect()
}

/// The text of the lines after `at` that continue a description starting
/// at column `col`.
fn continuation(lines: &[&str], at: usize, col: usize, first: &str) -> String {
    let mut text = first.trim().to_string();
    for next in &lines[at + 1..] {
        if next.trim().is_empty() || leading_whitespace(next) < col {
            break;
        }
        text.push(' ');
        text.push_str(next.trim());
    }
    text
}

fn operand(name: &str, repeatable: bool) -> Entity {
    let mut e = Entity::positional(name, Provenance::single(Source::HelpText));
    e.repeatable = repeatable;
    e
}

/// Described rows: `option=value` then a column gap and a description.
fn described_rows(lines: &[&str], declared: &[String], out: &mut Vec<Entity>) {
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(token) = trimmed.split_whitespace().next() else {
            continue;
        };
        let rest = &trimmed[token.len()..];
        if leading_whitespace(line) == 0
            || !rest.starts_with("  ")
            || !declared.iter().any(|d| d == token)
            || out.iter().any(|p| p.primary_name() == token)
        {
            continue;
        }
        let col = line.len() - rest.trim_start().len();
        let mut e = operand(token, true);
        e.description = non_empty_text(&continuation(lines, at, col, rest));
        out.push(e);
    }
}

/// `key=<a|b|c>` or `key=<placeholder>` under a heading that ends in `:`.
fn key_rows(lines: &[&str], out: &mut Vec<Entity>) {
    let mut at = 0;
    while at < lines.len() {
        let heading = lines[at];
        let indent = leading_whitespace(heading);
        let heading_text = heading.trim();
        at += 1;
        if !heading_text.ends_with(':') || heading_text.contains(' ') && heading_text.len() > 60 {
            continue;
        }
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut j = at;
        while j < lines.len() {
            let row = lines[j];
            if row.trim().is_empty() {
                j += 1;
                continue;
            }
            let row_text = row.trim();
            match row_text.split_once('=') {
                Some((key, value))
                    if leading_whitespace(row) > indent
                        && row_text.split_whitespace().count() == 1
                        && is_key_value(row_text)
                        && value.starts_with('<')
                        && value.ends_with('>') =>
                {
                    rows.push((key.to_string(), value.to_string()));
                    j += 1;
                }
                _ => break,
            }
        }
        if rows.is_empty() {
            continue;
        }
        at = j;
        for (key, value) in rows {
            if out.iter().any(|p| p.primary_name() == key) {
                continue;
            }
            let mut e = operand(&key, false);
            e.group = Some(heading_text.to_string());
            let inner = value.trim_start_matches('<').trim_end_matches('>');
            if inner.contains('|') {
                e.choices = inner
                    .split('|')
                    .map(|c| Choice {
                        name: c.to_string(),
                        description: None,
                    })
                    .collect();
            } else {
                e.value_name = Some(value.clone());
                e.value_kind = ValueKind::Required;
            }
            out.push(e);
        }
    }
}

/// Appends the `key=value` operands a usage declaration backs.
pub(super) fn recover_key_value_operands(
    positionals: &mut Vec<Entity>,
    usage: &[String],
    lines: &[&str],
) {
    let declared = declared_tokens(usage);
    if declared.is_empty() {
        return;
    }
    described_rows(lines, &declared, positionals);
    key_rows(lines, positionals);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_rows_and_key_block_become_operands() {
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
        let mut out = Vec::new();
        recover_key_value_operands(&mut out, &usage, &lines);
        let names: Vec<_> = out.iter().map(|p| p.primary_name().to_string()).collect();
        assert_eq!(names, ["option=value", "gc", "limit"]);
        assert_eq!(
            out[0].description.as_ref().unwrap().as_str(),
            "The option value. Wrapped."
        );
        assert_eq!(out[1].choices.len(), 2);
        assert_eq!(out[1].group.as_deref(), Some("Options for x.jfc:"));
        assert_eq!(out[2].value_name.as_deref(), Some("<timespan>"));
    }

    #[test]
    fn undeclared_pairs_are_left_alone() {
        let usage = vec!["tool [OPTION]...".to_string()];
        let lines = ["  bs=BYTES   read and write"];
        let mut out = Vec::new();
        recover_key_value_operands(&mut out, &usage, &lines);
        assert!(out.is_empty());
    }
}
