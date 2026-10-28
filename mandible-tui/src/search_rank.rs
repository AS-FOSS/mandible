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

/// True if `query` is a case-insensitive prefix of `name` starting at any
/// `-`/`_`-separated word boundary (including the start of `name` itself).
///
/// `run` never matches `--no-trunc` (its words are `no` and `trunc`), while
/// `no`, `trunc`, a bare modifier letter and either half of `NODE_DEBUG`
/// still do.
#[cfg(test)]
pub(crate) fn word_prefix_matches(name: &str, query: &str) -> bool {
    tier_in(name, 0, query).is_some()
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

/// Keep the hits whose names relate to `query` and order them: by tier,
/// commands before entities within a tier, and the index's own order within
/// both. `hits` arrives in that index order.
pub(crate) fn rank_by_name(root: &CommandNode, hits: Vec<NodeRef>, query: &str) -> Vec<NodeRef> {
    let mut keyed: Vec<((Tier, u8), usize, NodeRef)> = hits
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| Some((item_rank(root, &item, query)?, i, item)))
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
}
