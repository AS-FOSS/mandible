//! The `<tool> help <COMMAND>` gate (spec §6 rule 2, docs/shapes.md S-187).
//!
//! A help text that prints a strict line `<tool> help <COMMAND>` (or the
//! `[COMMAND]`, bare `COMMAND` and trailing-parenthetical spellings) says the
//! tool answers `help <word>`. Prose that merely quotes the argv never
//! matches, because the line must open with the tool's own name.
//! Fixtures: `corpus/brew/7.0.2`, `corpus/jfr/*`.

use mandible_core::CommandNode;

/// True when `line` is exactly `<tool> help <operand>` with an optional
/// trailing `( ... )` aside.
fn is_strict_help_line(line: &str, tool: &str) -> bool {
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some(tool) || tokens.next() != Some("help") {
        return false;
    }
    let Some(operand) = tokens.next() else {
        return false;
    };
    if !is_command_operand(operand) {
        return false;
    }
    let rest: Vec<&str> = tokens.collect();
    rest.is_empty()
        || (rest[0].starts_with('(') && rest.last().is_some_and(|t| t.ends_with(')')))
        || description_column_follows(line, operand)
}

/// True when the text after `operand` opens with a column gap (two spaces
/// or more), as a table row's description does. One space reads as a
/// sentence continuing, which never qualifies.
fn description_column_follows(line: &str, operand: &str) -> bool {
    line.find(operand)
        .and_then(|at| line.get(at + operand.len()..))
        .is_some_and(|after| after.starts_with("  "))
}

/// `[COMMAND]`, `<command>`, `[<command>]`, `<command...>` or an all-caps
/// bare `COMMAND`. A lowercase bare word is a concrete example, not a slot.
fn is_command_operand(operand: &str) -> bool {
    let (inner, bracketed) = match operand.strip_prefix('[') {
        Some(rest) => match rest.strip_suffix(']') {
            Some(inner) => (inner, true),
            None => return false,
        },
        None => (operand, false),
    };
    let (word, angled) = match inner.strip_prefix('<') {
        Some(rest) => match rest.strip_suffix('>') {
            Some(w) => (w, true),
            None => return false,
        },
        None => (inner, false),
    };
    let word = word.strip_suffix("...").unwrap_or(word);
    let shaped = word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let upper = word.chars().all(|c| !c.is_ascii_lowercase());
    shaped && (bracketed || angled || upper)
}

/// True when `raw` carries a strict `<tool> help <COMMAND>` line.
pub fn has_strict_help_line(raw: &str, tool: &str) -> bool {
    raw.lines().any(|l| is_strict_help_line(l, tool))
}

/// Mark every subcommand this same text attested by invocation (and never
/// by heading) as reachable through `help <word>`. The word `help` itself
/// is left alone, since `help help` asks nothing new.
pub fn mark_help_word_attested(subcommands: &mut [CommandNode], raw: &str, tool: &str) {
    if !has_strict_help_line(raw, tool) {
        return;
    }
    for node in subcommands {
        if node.invocation_attested && !node.heading_attested && node.name != "help" {
            node.help_word_attested = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_forms_match() {
        for l in [
            "  brew help [COMMAND]",
            "gem help COMMAND",
            " jfr help [<command>] (aliases --help, -h, -?)",
            "    gem help <COMMAND>           show help on COMMAND",
            "tool help <command>",
            "tool help <command...>",
        ] {
            assert!(
                is_strict_help_line(l, l.split_whitespace().next().unwrap()),
                "{l}"
            );
        }
    }

    #[test]
    fn sentence_forms_never_match() {
        for l in [
            "See 'git help <command>' to read about a specific subcommand.",
            "See 'git help <command>'",
            "Use git help <command> for more",
            "git help <command> shows the manual page for a command",
            "tool help install",
            "tool help <command> to read more",
            "tool helper [COMMAND]",
        ] {
            assert!(
                !is_strict_help_line(l, "git") && !is_strict_help_line(l, "tool"),
                "{l}"
            );
        }
    }

    #[test]
    fn marks_only_invocation_attested_words() {
        let raw = "  tool help [COMMAND]\n";
        let mut a = CommandNode::new("a", mandible_core::Provenance::default());
        a.invocation_attested = true;
        let mut b = CommandNode::new("b", mandible_core::Provenance::default());
        b.heading_attested = true;
        let mut h = CommandNode::new("help", mandible_core::Provenance::default());
        h.invocation_attested = true;
        let mut nodes = vec![a, b, h];
        mark_help_word_attested(&mut nodes, raw, "tool");
        assert!(nodes[0].help_word_attested);
        assert!(!nodes[1].help_word_attested);
        assert!(!nodes[2].help_word_attested);
        let mut none = vec![nodes[0].clone()];
        none[0].help_word_attested = false;
        mark_help_word_attested(&mut none, "See 'tool help <command>'", "tool");
        assert!(!none[0].help_word_attested);
    }
}
