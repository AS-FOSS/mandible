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

/// A name-mode query split into the dashes typed at its front and the rest.
struct Needle {
    /// Leading `-` characters of the query.
    dashes: usize,
    /// The query without them, lowercase.
    rest: String,
    /// The whole query, lowercase.
    whole: String,
}

impl Needle {
    fn new(query: &str) -> Needle {
        let whole = query.to_lowercase();
        let rest = whole.trim_start_matches('-');
        Needle {
            dashes: whole.len() - rest.len(),
            rest: rest.to_string(),
            whole: whole.clone(),
        }
    }
}

/// Where the query lines up with the start of a word of `text`, as a byte
/// offset: the first word (offset `lead`, just past the spelling's own
/// leading dashes) or one right after a `-`/`_` found at or past `lead`.
/// Dashes at the front of the query must be among the dashes just before
/// that word. `text` is lowercase.
fn word_start(text: &str, lead: usize, needle: &Needle) -> Option<usize> {
    let from = lead.min(text.len());
    let after_separators = text[from..]
        .match_indices(['-', '_'])
        .map(|(i, _)| from + i + 1);
    std::iter::once(from)
        .chain(after_separators)
        .find(|&start| {
            let before = text[..start]
                .chars()
                .rev()
                .take_while(|&c| c == '-')
                .count();
            needle.dashes <= before
                && (needle.dashes == 0 || !text[start..].starts_with(['-', '_']))
                && text[start..].starts_with(&needle.rest)
        })
}

/// The tier of `needle` within `text`, or `None`. `lead` is the length of
/// the spelling's own leading dashes in `text`, which are not separators.
fn tier_in(text: &str, lead: usize, needle: &Needle) -> Option<Tier> {
    let text = text.to_lowercase();
    if text == needle.whole {
        return Some(Tier::Exact);
    }
    let start = word_start(&text, lead, needle)?;
    Some(if start == lead.min(text.len()) {
        Tier::Prefix
    } else {
        Tier::Word
    })
}

/// The letters of one spelling a name-mode `query` matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marked {
    /// Leading dashes covered by a dash-led query.
    pub(crate) dashes: usize,
    /// Char positions within the spelling's bare name.
    pub(crate) name: Vec<usize>,
}

/// The spelling's text as the query is matched against it, and the length
/// of its leading dashes: with its dashes for a dash-led query, else the
/// bare name.
fn spelled(spelling: &Spelling, dash_led: bool) -> Option<(String, usize)> {
    let name = spelling.name.to_lowercase();
    if !dash_led {
        return Some((name, 0));
    }
    let lead = match spelling.dashes {
        Dashes::None => 0,
        Dashes::Single => 1,
        Dashes::Double => 2,
    };
    Some((format!("{}{name}", "-".repeat(lead)), lead))
}

/// The letters of `spelling` that `query` matched in name mode, or `None`
/// when the spelling does not match.
pub(crate) fn marked_in_spelling(spelling: &Spelling, query: &str) -> Option<Marked> {
    let needle = Needle::new(query);
    let (text, lead) = spelled(spelling, needle.dashes > 0)?;
    let start = word_start(&text, lead, &needle)?;
    let rest = needle.rest.chars().count();
    let at = text[..start].chars().count();
    if start == lead.min(text.len()) {
        // The first word: the typed dashes are the last ones of the
        // spelling's own, drawn outside its name.
        return Some(Marked {
            dashes: needle.dashes,
            name: (0..rest).collect(),
        });
    }
    // A later word: the dashes before it are part of the name.
    let first = at.saturating_sub(lead + needle.dashes);
    Some(Marked {
        dashes: 0,
        name: (first..first + needle.dashes + rest).collect(),
    })
}

/// The tier of one spelling.
fn spelling_tier(spelling: &Spelling, query: &str) -> Option<Tier> {
    let needle = Needle::new(query);
    let (text, lead) = spelled(spelling, needle.dashes > 0)?;
    tier_in(&text, lead, &needle)
}

/// The tier of a command name: a word start as for any spelling, else a
/// literal substring of it when the query has no dashes in front.
fn command_tier(name: &str, query: &str) -> Option<Tier> {
    let needle = Needle::new(query);
    tier_in(name, 0, &needle)
        .or_else(|| (needle.dashes == 0 && name_matches(name, query)).then_some(Tier::Substring))
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

/// Tier, kind (command first), path depth, display position.
type RankKey = (Tier, u8, usize, usize);

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
    let mut keyed: Vec<(RankKey, usize, NodeRef)> = hits
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

    fn tier(text: &str, query: &str) -> Option<Tier> {
        tier_in(text, 0, &Needle::new(query))
    }

    /// `query` is a case-insensitive prefix of `name` at the start or after
    /// a `-`/`_`: `run` never matches `--no-trunc`, while `no`, `trunc`, a
    /// bare modifier letter and either half of `NODE_DEBUG` do.
    fn word_prefix_matches(name: &str, query: &str) -> bool {
        tier(name, query).is_some()
    }

    fn long_tier(name: &str, query: &str) -> Option<Tier> {
        spelling_tier(&Spelling::long(name), query)
    }

    #[test]
    fn tiers_order_exact_prefix_word_substring() {
        assert_eq!(tier("rebase", "rebase"), Some(Tier::Exact));
        assert_eq!(tier("rebase", "reb"), Some(Tier::Prefix));
        assert_eq!(tier("cherry-pick", "pick"), Some(Tier::Word));
        assert_eq!(command_tier("grep", "re"), Some(Tier::Substring));
        assert!(Tier::Exact < Tier::Prefix && Tier::Prefix < Tier::Word);
        assert!(Tier::Word < Tier::Substring);
    }

    #[test]
    fn marked_letters_are_the_matched_prefix_of_a_word() {
        let s = Spelling::long("no-verify");
        assert_eq!(marked_in_spelling(&s, "ver").unwrap().name, vec![3, 4, 5]);
        assert_eq!(marked_in_spelling(&s, "no").unwrap().name, vec![0, 1]);
        let long = Spelling::long("squash");
        let m = marked_in_spelling(&long, "--sq").unwrap();
        assert_eq!((m.dashes, m.name), (2, vec![0, 1]));
        let m = marked_in_spelling(&long, "-s").unwrap();
        assert_eq!((m.dashes, m.name), (1, vec![0]));
    }

    #[test]
    fn dashes_typed_in_front_mark_the_dashes_before_a_later_word() {
        let s = Spelling::long("no-squash");
        let m = marked_in_spelling(&s, "-sq").unwrap();
        assert_eq!((m.dashes, m.name), (0, vec![2, 3, 4]));
        assert_eq!(marked_in_spelling(&s, "--sq"), None);
    }

    #[test]
    fn a_leading_dash_lines_up_with_the_dashes_before_a_word() {
        let long = Spelling::long("squash");
        let short = Spelling::short('s');
        assert_eq!(spelling_tier(&long, "--squash"), Some(Tier::Exact));
        assert_eq!(spelling_tier(&long, "--sq"), Some(Tier::Prefix));
        assert_eq!(spelling_tier(&short, "-s"), Some(Tier::Exact));
        assert_eq!(spelling_tier(&long, "-s"), Some(Tier::Prefix));
        assert_eq!(spelling_tier(&short, "--s"), None);
    }

    #[test]
    fn one_or_two_dashes_find_a_double_dash_flag() {
        assert_eq!(long_tier("ssl-reqd", "-ssl"), Some(Tier::Prefix));
        assert_eq!(long_tier("ssl-reqd", "--ssl"), Some(Tier::Prefix));
        assert_eq!(long_tier("no-squash", "-s"), Some(Tier::Word));
        assert_eq!(long_tier("no-squash", "--s"), None);
        assert_eq!(long_tier("proxy-ssl", "--ssl"), None);
    }

    #[test]
    fn a_short_flag_ranks_before_the_long_ones_for_the_same_letter() {
        let short = spelling_tier(&Spelling::short('s'), "-s");
        let long = long_tier("squash", "-s");
        let later = long_tier("no-squash", "-s");
        assert!(short < long && long < later);
    }

    #[test]
    fn a_dash_prefix_works_inside_a_dashed_spelling() {
        let no_trunc = Spelling::long("no-trunc");
        assert_eq!(spelling_tier(&no_trunc, "--no-"), Some(Tier::Prefix));
        assert_eq!(spelling_tier(&no_trunc, "--run"), None);
        assert_eq!(spelling_tier(&no_trunc, "run"), None);
        assert_eq!(spelling_tier(&no_trunc, "-run"), None);
    }

    #[test]
    fn a_word_starts_with_a_letter_not_a_separator() {
        assert_eq!(long_tier("a-_x", "-_x"), None);
    }

    #[test]
    fn a_dashless_spelling_never_matches_a_dash_led_query() {
        let letter = Spelling::bare("s");
        assert_eq!(spelling_tier(&letter, "-s"), None);
        assert_eq!(command_tier("squash", "-s"), None);
    }

    #[test]
    fn a_command_with_a_dash_in_its_name_matches_a_dash_led_query() {
        assert_eq!(command_tier("cherry-pick", "-pick"), Some(Tier::Word));
        assert_eq!(command_tier("cherry-pick", "--pick"), None);
        assert_eq!(command_tier("cherry-pick", "-ick"), None);
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
