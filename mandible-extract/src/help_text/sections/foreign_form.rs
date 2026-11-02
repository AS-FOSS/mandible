//! Usage forms that name a different command than the node (S-255).
//! `git log -h` lists `git show [<options>] <object>...` as its second
//! form; `object` is an operand of `show`, not of `log`.

use mandible_core::Entity;

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
