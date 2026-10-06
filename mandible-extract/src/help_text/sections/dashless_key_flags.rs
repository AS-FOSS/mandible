//! A block of `key=<placeholder>` rows under a heading is a group of
//! dashless flags (`jfr configure`'s `Options for default.jfc:`): each row
//! is an identifier, `=`, and one angle-bracket placeholder. The key is the
//! flag's one dashless spelling (`Spelling::bare`, like the `@file` argfile
//! flag of spec 4.5); `<a|b|c>` members are its choices, any other
//! placeholder its value name. The `=` joiner is not stored. See
//! docs/shapes.md S-202 and corpus/jfr/17.0.20-configure.

use super::key_value_operands::is_key_value;
use super::{leading_whitespace, MAX_RECOVERED_ENTRIES};
use mandible_core::{Choice, Entity, EntityKind, Provenance, Source, Spelling, ValueKind};

/// At least this many rows make a block; one stray `a=<b>` line is prose.
const MIN_ROWS: usize = 2;

/// True for a choice member that is a plain literal word (`off`, `60s`).
fn is_literal_member(m: &str) -> bool {
    !m.is_empty()
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

fn flag_for(key: &str, placeholder: &str, group: &str) -> Entity {
    let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
    e.spellings = vec![Spelling::bare(key)];
    e.group = Some(group.to_string());
    e.value_kind = ValueKind::Required;
    let inner = placeholder.trim_start_matches('<').trim_end_matches('>');
    let members: Vec<&str> = inner.split('|').collect();
    if members.len() >= 2 && members.iter().all(|m| is_literal_member(m)) {
        e.choices = members.iter().map(|m| Choice::bare(*m)).collect();
    } else {
        e.value_name = Some(placeholder.to_string());
    }
    e
}

/// Appends one dashless flag per `key=<placeholder>` row of every block.
pub(super) fn recover_dashless_key_flags(flags: &mut Vec<Entity>, lines: &[&str]) {
    let mut at = 0;
    while at < lines.len() {
        let heading = lines[at];
        let indent = leading_whitespace(heading);
        let heading_text = heading.trim();
        at += 1;
        if !heading_text.ends_with(':') || heading_text.split_whitespace().count() > 8 {
            continue;
        }
        let mut rows: Vec<(&str, &str)> = Vec::new();
        let mut j = at;
        while j < lines.len() {
            let row = lines[j];
            let text = row.trim();
            if text.is_empty() {
                j += 1;
                continue;
            }
            match text.split_once('=') {
                Some((key, value))
                    if leading_whitespace(row) > indent
                        && text.split_whitespace().count() == 1
                        && is_key_value(text)
                        && value.starts_with('<')
                        && value.ends_with('>') =>
                {
                    rows.push((key, value));
                    j += 1;
                }
                _ => break,
            }
        }
        if rows.len() < MIN_ROWS {
            continue;
        }
        at = j;
        for (key, value) in rows {
            if flags.len() >= MAX_RECOVERED_ENTRIES {
                return;
            }
            if flags
                .iter()
                .any(|f| f.spellings.iter().any(|s| s.name == key))
            {
                continue;
            }
            flags.push(flag_for(key, value, heading_text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_rows_under_a_heading_are_dashless_flags() {
        let lines = [
            "Options for x.jfc:",
            "",
            "  gc=<off|high>",
            "",
            "  limit=<timespan>",
        ];
        let mut out = Vec::new();
        recover_dashless_key_flags(&mut out, &lines);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].spelling(), "gc");
        assert_eq!(out[0].choices.len(), 2);
        assert_eq!(out[0].group.as_deref(), Some("Options for x.jfc:"));
        assert_eq!(out[1].value_name.as_deref(), Some("<timespan>"));
    }

    #[test]
    fn a_lone_row_is_left_alone() {
        let lines = ["Notes:", "  a=<b>"];
        let mut out = Vec::new();
        recover_dashless_key_flags(&mut out, &lines);
        assert!(out.is_empty());
    }
}
