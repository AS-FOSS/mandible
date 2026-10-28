//! Name-mode filtering and ranking of the search index's results (spec §10).
//!
//! The index hands back every fuzzy hit; name mode keeps the ones whose own
//! names relate to the query and orders them in tiers, so the first entry is
//! the row the cursor jumps to and the tree shows.

use mandible_core::{resolve_flag, CommandNode, Dashes, NodeRef, Spelling};

/// How closely a name relates to the query. Smaller is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    /// The name is the query.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// A `-`/`_` word of the name starts with the query.
    Word,
    /// A command name that only contains the query.
    Substring,
}

/// Case-insensitive **substring** match of `query` against a command's own
/// `name`.
///
/// Deliberately literal, not a subsequence: subsequence matching is what
/// made the narrow mode feel broken (`run` in `docker` surfaced `ps` and
/// `build` through `--no-trunc`'s r…u…n).
pub(crate) fn name_matches(name: &str, query: &str) -> bool {
    name.to_lowercase().contains(&query.to_lowercase())
}

/// The tier of `query` within `text`, or `None`. Word starts are offset 0
/// and the position after each `-`/`_` found at or past `skip`; `skip`
/// keeps a spelling's leading dashes from counting as separators.
fn tier_in(text: &str, skip: usize, query: &str) -> Option<Tier> {
    let text = text.to_lowercase();
    let query = query.to_lowercase();
    if text == query {
        return Some(Tier::Exact);
    }
    if text.starts_with(&query) {
        return Some(Tier::Prefix);
    }
    let from = skip.min(text.len());
    let word = text[from..]
        .match_indices(['-', '_'])
        .any(|(i, _)| text[from + i + 1..].starts_with(&query));
    word.then_some(Tier::Word)
}

/// The tier of one spelling. A query starting with `-` is matched against
/// the spelling with its dashes; any other query against the bare name.
fn spelling_tier(spelling: &Spelling, query: &str) -> Option<Tier> {
    if !query.starts_with('-') {
        return tier_in(&spelling.name, 0, query);
    }
    let dashes = match spelling.dashes {
        Dashes::None => return None,
        Dashes::Single => "-",
        Dashes::Double => "--",
    };
    tier_in(&format!("{dashes}{}", spelling.name), dashes.len(), query)
}

/// The tier of a command name. A query starting with `-` names a flag
/// spelling, so no command matches it.
fn command_tier(name: &str, query: &str) -> Option<Tier> {
    if query.starts_with('-') {
        return None;
    }
    tier_in(name, 0, query).or_else(|| name_matches(name, query).then_some(Tier::Substring))
}

fn item_rank(root: &CommandNode, item: &NodeRef, query: &str) -> Option<(Tier, u8)> {
    match item {
        NodeRef::Command(path) => {
            let name = path.last().map_or("", String::as_str);
            command_tier(name, query).map(|tier| (tier, 0))
        }
        NodeRef::Flag { path, key } => {
            let entity = resolve_flag(root, path, key)?;
            entity
                .spellings
                .iter()
                .chain(entity.range().map(|r| &r.end))
                .filter_map(|s| spelling_tier(s, query))
                .min()
                .map(|tier| (tier, 1))
        }
    }
}

/// Each command path's position in the tree's top-to-bottom order.
fn display_order(root: &CommandNode) -> std::collections::HashMap<Vec<String>, usize> {
    fn walk(
        node: &CommandNode,
        path: Vec<String>,
        out: &mut std::collections::HashMap<Vec<String>, usize>,
    ) {
        out.insert(path.clone(), out.len());
        for child in &node.subcommands {
            let mut p = path.clone();
            p.push(child.name.clone());
            walk(child, p, out);
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(root, vec![root.name.clone()], &mut out);
    out
}

/// Keep the hits whose names relate to `query` and order them: by tier,
/// commands before entities within a tier, then the shallower path, then
/// the tree's display order, then the index's own order.
pub(crate) fn rank_by_name(root: &CommandNode, hits: Vec<NodeRef>, query: &str) -> Vec<NodeRef> {
    let order = display_order(root);
    let mut keyed: Vec<((Tier, u8, usize, usize), usize, NodeRef)> = hits
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let (tier, kind) = item_rank(root, &item, query)?;
            let path = hit_path(&item);
            let pos = order.get(path).copied().unwrap_or(usize::MAX);
            Some(((tier, kind, path.len(), pos), i, item))
        })
        .collect();
    keyed.sort_by_key(|(rank, i, _)| (*rank, *i));
    keyed.into_iter().map(|(_, _, item)| item).collect()
}

/// The command path a hit lands on: a command's own, or the owning command
/// of an entity.
pub(crate) fn hit_path(item: &NodeRef) -> &Vec<String> {
    match item {
        NodeRef::Command(path) | NodeRef::Flag { path, .. } => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `query` is a case-insensitive prefix of `name` at the start or after
    /// a `-`/`_`: `run` never matches `--no-trunc`, while `no`, `trunc`, a
    /// bare modifier letter and either half of `NODE_DEBUG` do.
    fn word_prefix_matches(name: &str, query: &str) -> bool {
        tier_in(name, 0, query).is_some()
    }

    #[test]
    fn tiers_order_exact_prefix_word_substring() {
        assert_eq!(tier_in("rebase", 0, "rebase"), Some(Tier::Exact));
        assert_eq!(tier_in("rebase", 0, "reb"), Some(Tier::Prefix));
        assert_eq!(tier_in("cherry-pick", 0, "pick"), Some(Tier::Word));
        assert_eq!(command_tier("grep", "re"), Some(Tier::Substring));
        assert!(Tier::Exact < Tier::Prefix && Tier::Prefix < Tier::Word);
        assert!(Tier::Word < Tier::Substring);
    }

    #[test]
    fn a_leading_dash_matches_the_spelling_with_its_dashes() {
        let long = Spelling::long("squash");
        let short = Spelling::short('s');
        assert_eq!(spelling_tier(&long, "--squash"), Some(Tier::Exact));
        assert_eq!(spelling_tier(&long, "--sq"), Some(Tier::Prefix));
        assert_eq!(spelling_tier(&short, "-s"), Some(Tier::Exact));
        assert_eq!(spelling_tier(&long, "-s"), None);
        assert_eq!(spelling_tier(&short, "--s"), None);
    }

    #[test]
    fn a_dash_prefix_works_inside_a_dashed_spelling() {
        let no_trunc = Spelling::long("no-trunc");
        assert_eq!(spelling_tier(&no_trunc, "--no-"), Some(Tier::Prefix));
        assert_eq!(spelling_tier(&no_trunc, "--run"), None);
    }

    #[test]
    fn a_dashless_spelling_never_matches_a_dash_led_query() {
        let letter = Spelling::bare("s");
        assert_eq!(spelling_tier(&letter, "-s"), None);
        assert_eq!(command_tier("squash", "-s"), None);
    }

    /// The exact docker guard `word_prefix_matches` exists to preserve:
    /// `run` must never match `--no-trunc`, because `run` is a prefix of
    /// neither of its `-`-separated words (`no`, `trunc`) nor of the whole
    /// name.
    #[test]
    fn word_prefix_matches_refuses_the_docker_run_no_trunc_case() {
        assert!(!word_prefix_matches("no-trunc", "run"));
    }

    /// The two `-`-separated words of `--no-trunc` each match their own
    /// prefix.
    #[test]
    fn word_prefix_matches_finds_either_word_of_a_dashed_name() {
        assert!(word_prefix_matches("no-trunc", "trunc"));
        assert!(word_prefix_matches("no-trunc", "no"));
    }

    /// A one-letter dashless name (a modifier letter) matches itself.
    #[test]
    fn word_prefix_matches_finds_a_bare_modifier_letter() {
        assert!(word_prefix_matches("d", "d"));
    }

    /// An underscore-separated env var name matches both from the start of
    /// the whole name and from after the separator.
    #[test]
    fn word_prefix_matches_finds_either_half_of_an_underscored_name() {
        assert!(word_prefix_matches("NODE_DEBUG", "NODE_D"));
        assert!(word_prefix_matches("NODE_DEBUG", "debug"));
    }

    /// Case-insensitive, both sides.
    #[test]
    fn word_prefix_matches_is_case_insensitive() {
        assert!(word_prefix_matches("NODE_DEBUG", "node_d"));
        assert!(word_prefix_matches("node_debug", "DEBUG"));
    }

    /// A query longer than the word it's being compared against is not a
    /// prefix of it — `word_prefix_matches` must not panic on a slice
    /// that runs past the word's own end.
    #[test]
    fn word_prefix_matches_refuses_a_query_longer_than_the_word() {
        assert!(!word_prefix_matches("no-trunc", "truncated"));
    }
}
