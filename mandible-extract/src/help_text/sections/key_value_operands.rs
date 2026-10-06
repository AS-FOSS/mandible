//! `key=value` operands a usage line declares (`[option=value]*`): the
//! described table rows become positionals. The `key=<value>` list under a
//! heading is a different shape (dashless flags, `dashless_key_flags`).
//! See docs/shapes.md S-202 and corpus/jfr/17.0.20-configure.

use super::{leading_whitespace, non_empty_text};
use mandible_core::{Entity, Provenance, Source};

/// True for `word=word` with an identifier-like key and a non-empty value.
pub(super) fn is_key_value(token: &str) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_rows_become_operands() {
        let usage = vec!["jfr configure [option=value]*".to_string()];
        let lines = [
            "  option=value     The option value.",
            "                   Wrapped.",
            "",
            "Options for x.jfc:",
            "",
            "  gc=<off|high>",
        ];
        let mut out = Vec::new();
        recover_key_value_operands(&mut out, &usage, &lines);
        let names: Vec<_> = out.iter().map(|p| p.primary_name().to_string()).collect();
        assert_eq!(names, ["option=value"]);
        assert_eq!(
            out[0].description.as_ref().unwrap().as_str(),
            "The option value. Wrapped."
        );
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
