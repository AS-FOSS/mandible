//! A grammar production that lists the commands (docs/shapes.md S-258):
//!
//! ```text
//! Usage: ip [ OPTIONS ] OBJECT { COMMAND | help }
//! where  OBJECT := { address | addrlabel | link | xfrm }
//! ```
//!
//! When a usage form puts a name in its command position and a
//! `NAME := { a | b | ... }` production lists lowercase words joined by `|`,
//! those words are the node's commands and the name is not an operand. A
//! list without `|` (`{ ctrl etc }`) or with nested groups is not one.

use super::*;
use mandible_core::is_command_name_shaped;

/// The usage form's command name, and whether its command slot shows `help`
/// as an alternative (`{ COMMAND | help }`, `[help]`).
struct CommandSlot {
    name: String,
    shows_help: bool,
}

/// Bracket depth change over `token`, counting `[]` and `{}` alike.
fn depth_delta(token: &str) -> i32 {
    token
        .chars()
        .map(|c| match c {
            '[' | '{' => 1,
            ']' | '}' => -1,
            _ => 0,
        })
        .sum()
}

/// One balanced group opening `tokens[0]`, joined, or `None`.
fn first_group(tokens: &[&str]) -> Option<String> {
    let mut depth = 0;
    for (n, token) in tokens.iter().enumerate() {
        depth += depth_delta(token);
        if depth <= 0 {
            return Some(tokens[..=n].join(" "));
        }
    }
    None
}

fn is_placeholder_name(token: &str) -> bool {
    token.starts_with(|c: char| c.is_ascii_uppercase())
        && token
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The command slot of one usage form: the program, any bracketed option
/// groups, then an upper-case name followed by `{..}`, `[..]` or `help`.
fn command_slot(form: &str) -> Option<CommandSlot> {
    let mut tokens: Vec<&str> = form.split_whitespace().collect();
    if tokens.first().is_some_and(|t| t.ends_with(':')) {
        tokens.remove(0);
    }
    let mut at = 1;
    while tokens.get(at).is_some_and(|t| t.starts_with('[')) {
        at += first_group(&tokens[at..])?.split_whitespace().count();
    }
    let name = *tokens.get(at)?;
    if !is_placeholder_name(name) {
        return None;
    }
    let rest = &tokens[at + 1..];
    let first = *rest.first()?;
    if first == "help" {
        return Some(CommandSlot {
            name: name.to_string(),
            shows_help: true,
        });
    }
    if !first.starts_with(['{', '[']) {
        return None;
    }
    let group = first_group(rest)?;
    let inner: String = group.chars().skip(1).collect();
    Some(CommandSlot {
        name: name.to_string(),
        shows_help: inner
            .split('|')
            .any(|alt| alt.trim().trim_end_matches(['}', ']']).trim() == "help"),
    })
}

/// The words of the production `name := { a | b | ... }`, in its order, or
/// `None` when it is absent, wraps past 20 lines, nests a group, or lists
/// fewer than two command-shaped words.
fn production_members(lines: &[&str], name: &str) -> Option<Vec<String>> {
    const MAX_WRAP: usize = 20;
    let (at, head) = lines.iter().enumerate().find_map(|(i, line)| {
        let t = line.trim_start();
        let t = t.strip_prefix("where").map_or(t, str::trim_start);
        let (label, rest) = t.split_once(":=")?;
        (label.trim_end() == name).then_some((i, rest))
    })?;
    let mut body = head.trim_start().to_string();
    for line in lines.iter().skip(at + 1).take(MAX_WRAP) {
        if !body.starts_with('{') || depth_delta(&body) <= 0 || line.trim().is_empty() {
            break;
        }
        body.push(' ');
        body.push_str(line.trim());
    }
    let inner = body.strip_prefix('{')?;
    let close = inner.find('}')?;
    let inner = &inner[..close];
    if inner.contains(['{', '[']) {
        return None;
    }
    let words: Vec<&str> = inner.split('|').map(str::trim).collect();
    (words.len() >= 2 && words.iter().all(|w| is_command_name_shaped(w)))
        .then(|| words.into_iter().map(str::to_string).collect())
}

/// Read the production behind a usage form's command position as the
/// node's commands, and drop the name from its operands.
pub(super) fn apply(result: &mut ParsedHelp, lines: &[&str]) {
    let found = result.usage.iter().find_map(|form| {
        let slot = command_slot(form)?;
        let members = production_members(lines, &slot.name)?;
        Some((slot, members))
    });
    let Some((slot, members)) = found else { return };
    result.positionals.retain(|p| p.primary_name() != slot.name);
    for word in members {
        // `help` is the usage form's own alternative, not an object.
        if slot.shows_help && word == "help" {
            continue;
        }
        let mut node = CommandNode::new(word, Provenance::single(Source::HelpText));
        node.heading_attested = true;
        node.help_slot_attested = slot.shows_help;
        result.try_push_subcommand(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ParsedHelp {
        parse_with_profile(text, None, Some("tool"))
    }

    fn names(p: &ParsedHelp) -> Vec<&str> {
        p.subcommands.iter().map(|c| c.name.as_str()).collect()
    }

    const PAGE: &str = "Usage: tool [ OPTIONS ] OBJECT { COMMAND | help }\n\
         where  OBJECT := { address | link |\n                    route | help }\n\
         \x20      OPTIONS := { -V[ersion] | -s[tatistics] }\n";

    #[test]
    fn the_members_become_commands_in_order_and_the_name_goes() {
        let p = parse(PAGE);
        assert_eq!(names(&p), ["address", "link", "route"]);
        assert!(p.positionals.iter().all(|o| o.primary_name() != "OBJECT"));
        assert!(p.positionals.iter().any(|o| o.primary_name() == "COMMAND"));
        assert!(p
            .subcommands
            .iter()
            .all(|c| c.heading_attested && c.help_slot_attested));
    }

    #[test]
    fn a_slot_without_help_makes_commands_that_are_not_help_probed() {
        let p = parse("Usage: tool OBJECT [ COMMAND ]\nwhere OBJECT := { a | b }\n");
        assert_eq!(names(&p), ["a", "b"]);
        assert!(p.subcommands.iter().all(|c| !c.help_slot_attested));
    }

    #[test]
    fn a_list_without_bars_is_not_a_command_list() {
        let p = parse("Usage: tool [ OPTIONS ] OBJECT [help] }\nwhere  OBJECT := { ctrl etc }\n");
        assert!(p.subcommands.is_empty());
        assert!(p.positionals.iter().any(|o| o.primary_name() == "OBJECT"));
    }

    #[test]
    fn a_name_outside_the_command_position_reads_nothing() {
        for usage in [
            "Usage: tool [ OPTIONS ] [ OBJECT ]",
            "Usage: tool OBJECT",
            "Usage: tool -f OBJECT { COMMAND | help }",
            "Usage: tool [ OPTIONS ] FILE OBJECT { COMMAND | help }",
        ] {
            let p = parse(&format!("{usage}\nwhere OBJECT := {{ a | b }}\n"));
            assert!(p.subcommands.is_empty(), "{usage}");
        }
    }

    #[test]
    fn nested_groups_and_placeholders_are_not_commands() {
        for list in ["{ a | { b | c } }", "{ a | B }", "{ a }", "{ a | b"] {
            let p = parse(&format!(
                "Usage: tool OBJECT {{ COMMAND | help }}\nwhere OBJECT := {list}\n"
            ));
            assert!(p.subcommands.is_empty(), "{list}");
        }
    }

    #[test]
    fn help_is_a_command_when_the_form_does_not_show_it() {
        let p = parse("Usage: tool OBJECT { COMMAND }\nwhere OBJECT := { a | help }\n");
        assert_eq!(names(&p), ["a", "help"]);
    }
}
