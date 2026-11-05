//! `NAME := text` legends under a command page's usage forms (docs/shapes.md
//! S-258):
//!
//! ```text
//! where  OPTS := [ pn <u32> | xpn <u64> ] [ salt SALT ]
//!        ID   := 128-bit hex string
//!        SCI  := { sci <u64> | port { 1..2^16-1 } address <lladdr> }
//! ```
//!
//! Each legend line describes the placeholder it names, without the `:=`.
//! A name the usage forms or another legend use becomes an operand when it
//! was not one already; a `{ a | b }` list of plain lowercase words is its
//! values. Legends are listed in their own order after the other operands.
//! A list of dash-led words is an option production, not a legend.

use super::production_commands::is_placeholder_name;
use super::*;
use mandible_core::alternation_members;

struct Legend {
    name: String,
    body: String,
}

/// Bracket depth change over `text`, counting `[]` and `{}` alike.
fn depth(text: &str) -> i32 {
    text.chars()
        .map(|c| i32::from(matches!(c, '[' | '{')) - i32::from(matches!(c, ']' | '}')))
        .sum()
}

/// The legend lines of `raw`: an optional `where` / `Where:`, a placeholder
/// name, `:=` and the text, which continues over wrapped lines while its
/// brackets are open.
fn legends(raw: &str) -> Vec<Legend> {
    const MAX_WRAP: usize = 20;
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let mut t = line.trim_start();
        if let Some(rest) = t.strip_prefix("where").or_else(|| t.strip_prefix("Where")) {
            t = rest.strip_prefix(':').unwrap_or(rest).trim_start();
        }
        let Some((name, body)) = t.split_once(":=") else {
            continue;
        };
        let name = name.trim();
        let mut body = body.trim().to_string();
        for next in lines.iter().skip(at + 1).take(MAX_WRAP) {
            if depth(&body) <= 0 || next.trim().is_empty() || next.contains(":=") {
                break;
            }
            body.push(' ');
            body.push_str(next.trim());
        }
        let is_options = body.trim_start_matches(['{', '[', ' ']).starts_with('-');
        if is_placeholder_name(name) && !body.is_empty() && !is_options {
            out.push(Legend {
                name: name.to_string(),
                body,
            });
        }
    }
    out
}

/// True when `name` stands alone as a word of `text`, ignoring brackets.
fn uses(text: &str, name: &str) -> bool {
    text.split_whitespace()
        .any(|w| w.trim_matches(['[', ']', '{', '}', '(', ')', ',', '.']) == name)
}

/// The legend's values: a braced list of plain lowercase words.
fn values(body: &str) -> Option<Vec<String>> {
    let inner = body.trim().strip_prefix('{')?.strip_suffix('}')?;
    let compact = format!(
        "{{{}}}",
        inner
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("|")
    );
    let members = alternation_members(&compact)?;
    Some(members.into_iter().map(str::to_string).collect())
}

/// Describe the operands the legends name, add the ones the page uses, and
/// list them in legend order after the rest. `usage` holds the page's forms.
pub(crate) fn apply(raw: &str, usage: &[String], positionals: &mut Vec<Entity>) {
    let legends = legends(raw);
    let mut described: Vec<Entity> = Vec::new();
    for legend in &legends {
        let at = positionals
            .iter()
            .position(|p| p.primary_name() == legend.name);
        let used = at.is_some()
            || usage.iter().any(|form| uses(form, &legend.name))
            || legends
                .iter()
                .any(|other| other.name != legend.name && uses(&other.body, &legend.name));
        if !used || described.iter().any(|p| p.primary_name() == legend.name) {
            continue;
        }
        let mut positional = match at {
            Some(k) => positionals.remove(k),
            None => {
                let shape = usage_operand_shape(usage, &legend.name);
                let mut p = Entity::positional(&legend.name, Provenance::single(Source::HelpText));
                p.required = shape.is_some_and(|(required, _)| required);
                p.repeatable = shape.is_some_and(|(_, variadic)| variadic);
                p
            }
        };
        match values(&legend.body) {
            Some(names) => {
                positional.choices = names.into_iter().map(Choice::bare).collect();
                positional.description = None;
            }
            None => positional.description = non_empty_text(&legend.body),
        }
        described.push(positional);
    }
    positionals.extend(described);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "Usage: ip x add DEV [ OPTS ] key ID\n\
        where  OPTS := [ pn <u32> | xpn <u64> ] [ salt SALT ]\n\
        \x20      ID   := 128-bit hex string\n\
        \x20      SALT := 96-bit hex string\n\
        \x20      MODE := { none | default }\n";

    fn run(positionals: &[&str]) -> Vec<Entity> {
        let usage = vec!["Usage: ip x add DEV [ OPTS ] key ID [ mode MODE ]".to_string()];
        let mut found: Vec<Entity> = positionals
            .iter()
            .map(|n| Entity::positional(*n, Provenance::single(Source::HelpText)))
            .collect();
        apply(PAGE, &usage, &mut found);
        found
    }

    #[test]
    fn a_legend_describes_its_name_without_the_operator_and_follows_the_other_operands() {
        let found = run(&["ID", "DEV", "OPTS"]);
        let names: Vec<&str> = found.iter().map(|p| p.primary_name()).collect();
        assert_eq!(names, ["DEV", "OPTS", "ID", "SALT", "MODE"]);
        let text = |n: &str| {
            found
                .iter()
                .find(|p| p.primary_name() == n)
                .and_then(|p| p.description.as_ref().map(|t| t.as_str().to_string()))
        };
        assert_eq!(text("ID").as_deref(), Some("128-bit hex string"));
        assert_eq!(text("SALT").as_deref(), Some("96-bit hex string"));
        assert_eq!(text("DEV"), None);
    }

    #[test]
    fn a_list_of_plain_lowercase_words_is_values_and_an_option_list_is_no_legend() {
        let found = run(&[]);
        let mode = found.iter().find(|p| p.primary_name() == "MODE").unwrap();
        assert_eq!(mode.choices.len(), 2);
        assert!(mode.description.is_none());
        assert!(legends("OPTIONS := { -V | -s }").is_empty());
    }
}
