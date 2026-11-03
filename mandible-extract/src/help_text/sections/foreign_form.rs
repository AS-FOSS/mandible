//! Usage forms that name a different command than the node (S-255).
//! `git log -h` lists `git show [<options>] <object>...` as its second
//! form; `object` is an operand of `show`, not of `log`.

use mandible_core::{CommandNode, Entity};

/// The words of a usage entry after its `usage:` / `or:` label.
fn form_words(entry: &str) -> Vec<&str> {
    let mut words = entry.split_whitespace().peekable();
    if words
        .peek()
        .is_some_and(|w| w.eq_ignore_ascii_case("usage:") || w.eq_ignore_ascii_case("or:"))
    {
        words.next();
    }
    words.collect()
}

fn is_command_word(w: &str) -> bool {
    w.starts_with(|c: char| c.is_ascii_lowercase())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Indices of the entries that open with the first entry's program word
/// and a different command word, when the first entry names `node`.
pub(crate) fn foreign_entries(entries: &[String], node: Option<&str>) -> Vec<usize> {
    let (Some(node), Some(first)) = (node, entries.first()) else {
        return Vec::new();
    };
    let first = form_words(first);
    let (Some(&program), Some(&own)) = (first.first(), first.get(1)) else {
        return Vec::new();
    };
    if own != node {
        return Vec::new();
    }
    (1..entries.len())
        .filter(|&k| {
            let words = form_words(&entries[k]);
            words.first() == Some(&program)
                && words
                    .get(1)
                    .is_some_and(|w| is_command_word(w) && *w != node)
        })
        .collect()
}

/// Whether some word of `entry` is `name` once its notation is stripped.
fn mentions(entry: &str, name: &str) -> bool {
    entry
        .split_whitespace()
        .any(|w| w.trim_matches(|c: char| "[]<>(){}|.,".contains(c)) == name)
}

/// Drop the operands that only a foreign form of `usage` names.
pub(crate) fn drop_foreign_operands(usage: &[String], node: &str, positionals: &mut Vec<Entity>) {
    let foreign = foreign_entries(usage, Some(node));
    if foreign.is_empty() {
        return;
    }
    positionals.retain(|p| {
        let name = p.primary_name();
        let named_by = |own: bool| {
            usage
                .iter()
                .enumerate()
                .any(|(k, e)| foreign.contains(&k) != own && mentions(e, name))
        };
        !(named_by(false) && !named_by(true))
    });
}

/// Rows that repeat the program and the node's own word (`ip link delete`)
/// read as a command named after the node. Its verbs are the node's own
/// commands, so they replace it. S-257.
pub(crate) fn lift_own_name_command(subcommands: &mut Vec<CommandNode>, own: &str) {
    let Some(at) = subcommands.iter().position(|c| c.name == own) else {
        return;
    };
    let echo = subcommands.remove(at);
    for verb in echo.subcommands {
        if subcommands.iter().all(|c| c.name != verb.name) {
            subcommands.push(verb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};

    fn node(name: &str, kids: &[&str]) -> CommandNode {
        let mut n = CommandNode::new(name, Provenance::single(Source::HelpText));
        n.subcommands = kids.iter().map(|k| node(k, &[])).collect();
        n
    }

    #[test]
    fn a_command_named_after_the_node_gives_way_to_its_verbs() {
        let mut subs = vec![node("link", &["set", "show"])];
        lift_own_name_command(&mut subs, "link");
        let names: Vec<_> = subs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["set", "show"]);
        let mut bare = vec![node("link", &[])];
        lift_own_name_command(&mut bare, "link");
        assert!(bare.is_empty());
        let mut other = vec![node("add", &[])];
        lift_own_name_command(&mut other, "link");
        assert_eq!(other.len(), 1);
    }
}
