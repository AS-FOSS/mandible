//! Name-mode filtering and ranking of the search index's results (spec §10).
//!
//! A row matches when its names contain every word of the query. The tiers
//! only choose which match the cursor jumps to first; they never hide or
//! reorder the tree.

use mandible_core::{resolve, resolve_flag, CommandNode, FlagKey, NodeRef};
use std::collections::HashMap;

/// How closely a name relates to the query. Smaller is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    /// The name is the query, letter for letter, case included.
    ExactCase,
    /// The name is the query apart from case.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// A word of the name starts with the query: the name's start, or the
    /// character after a `-` or `_`.
    Word,
    /// The query is somewhere inside the name.
    Inside,
}

/// The words of a query: split on spaces, lowercase.
pub(crate) fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// The tier of the query's first word within one name, or `None` when the
/// word is not in the name. `first` is the word as typed.
fn name_tier(name: &str, first: &str) -> Option<Tier> {
    let lower = name.to_lowercase();
    let word = first.to_lowercase();
    if lower == word {
        return Some(if name == first {
            Tier::ExactCase
        } else {
            Tier::Exact
        });
    }
    if lower.starts_with(&word) {
        return Some(Tier::Prefix);
    }
    let mut inside = false;
    for (at, _) in lower.match_indices(&word) {
        inside = true;
        if lower[..at].ends_with(['-', '_']) {
            return Some(Tier::Word);
        }
    }
    inside.then_some(Tier::Inside)
}

/// Every name a hit can be found by: a command's own name, or every form
/// of every spelling of an entity.
fn names_of(root: &CommandNode, item: &NodeRef) -> Vec<String> {
    match item {
        NodeRef::Command(path) => vec![path.last().cloned().unwrap_or_default()],
        NodeRef::Flag { path, key } => resolve_flag(root, path, key)
            .map(mandible_search::entity_forms)
            .unwrap_or_default(),
    }
}

/// The names of a hit that carry the query's first word, best tier first
/// (document order breaks ties), when every word of the query is in the
/// names. `None` when the hit does not match.
fn matched_names(root: &CommandNode, item: &NodeRef, query: &str) -> Option<Vec<(Tier, String)>> {
    let all = words(query);
    let first = query.split_whitespace().next()?;
    let names = names_of(root, item);
    let text = names.join(" ").to_lowercase();
    if !all.iter().all(|w| text.contains(w.as_str())) {
        return None;
    }
    let mut tiered: Vec<(Tier, String)> = names
        .into_iter()
        .filter_map(|n| name_tier(&n, first).map(|t| (t, n)))
        .collect();
    tiered.sort_by_key(|(t, _)| *t);
    Some(tiered)
}

/// The spelling (or command name) the query matched best on a hit: the one
/// a suggestion completes.
pub(crate) fn best_name(root: &CommandNode, item: &NodeRef, query: &str) -> Option<String> {
    matched_names(root, item, query)?
        .into_iter()
        .next()
        .map(|(_, name)| name)
}

fn item_rank(root: &CommandNode, item: &NodeRef, query: &str) -> Option<(Tier, u8)> {
    let tier = matched_names(root, item, query)?.first()?.0;
    let kind = u8::from(matches!(item, NodeRef::Flag { .. }));
    Some((tier, kind))
}

/// Tier, kind (command first), path depth, display position, line on the
/// command's page.
type RankKey = (Tier, u8, usize, usize, usize);

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
    let mut pages = PagePositions::default();
    let mut keyed: Vec<(RankKey, usize, NodeRef)> = hits
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let (tier, kind) = item_rank(root, &item, query)?;
            let path = hit_path(&item);
            let pos = order.get(path).copied().unwrap_or(usize::MAX);
            let line = pages.of(root, &item);
            Some(((tier, kind, path.len(), pos, line), i, item))
        })
        .collect();
    keyed.sort_by_key(|(rank, i, _)| (*rank, *i));
    keyed.into_iter().map(|(_, _, item)| item).collect()
}

/// Entity positions on their command's page, read once per command.
#[derive(Default)]
pub(crate) struct PagePositions(HashMap<Vec<String>, HashMap<FlagKey, usize>>);

impl PagePositions {
    /// The line rank of a hit on its command's page: 0 for a command.
    pub(crate) fn of(&mut self, root: &CommandNode, item: &NodeRef) -> usize {
        let NodeRef::Flag { path, key } = item else {
            return 0;
        };
        let page = self.0.entry(path.clone()).or_insert_with(|| {
            resolve(root, path).map_or_else(HashMap::new, crate::render::page_positions)
        });
        page.get(key).copied().unwrap_or(usize::MAX)
    }
}

/// Wide mode's order with each command's entities of one rank group
/// reordered to stand top to bottom on its page, in the slots that group's
/// entities already held. `groups[i]` is the rank group of `hits[i]`.
pub(crate) fn page_order_within_groups(
    root: &CommandNode,
    hits: Vec<NodeRef>,
    groups: &[u8],
) -> Vec<NodeRef> {
    let mut pages = PagePositions::default();
    let mut slots: HashMap<(u8, Vec<String>), Vec<usize>> = HashMap::new();
    for (i, hit) in hits.iter().enumerate() {
        if matches!(hit, NodeRef::Flag { .. }) {
            slots
                .entry((groups[i], hit_path(hit).clone()))
                .or_default()
                .push(i);
        }
    }
    let mut out = hits.clone();
    for slot in slots.values() {
        let mut members: Vec<&NodeRef> = slot.iter().map(|&i| &hits[i]).collect();
        members.sort_by_key(|m| pages.of(root, m));
        for (&i, m) in slot.iter().zip(members) {
            out[i] = m.clone();
        }
    }
    out
}

/// The command path a hit lands on: a command's own, or the owning command
/// of an entity.
pub(crate) fn hit_path(item: &NodeRef) -> &Vec<String> {
    match item {
        NodeRef::Command(path) | NodeRef::Flag { path, .. } => path,
    }
}

/// The `c` node of a test tree, for page-order checks.
#[cfg(test)]
pub(crate) fn tests_node(app: &crate::app::App) -> CommandNode {
    resolve(&app.root, &[app.root.name.clone(), "c".to_string()])
        .cloned()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Entity, Provenance, Source};

    fn prov() -> Provenance {
        Provenance::single(Source::HelpText)
    }

    fn tier(name: &str, query: &str) -> Option<Tier> {
        name_tier(name, query.split_whitespace().next().unwrap())
    }

    #[test]
    fn tiers_order_exact_prefix_word_inside() {
        assert_eq!(tier("rebase", "rebase"), Some(Tier::ExactCase));
        assert_eq!(tier("rebase", "REBASE"), Some(Tier::Exact));
        assert_eq!(tier("rebase", "reb"), Some(Tier::Prefix));
        assert_eq!(tier("cherry-pick", "pick"), Some(Tier::Word));
        assert_eq!(tier("NODE_DEBUG", "debug"), Some(Tier::Word));
        assert_eq!(tier("grep", "re"), Some(Tier::Inside));
        assert_eq!(tier("--blocksize", "size"), Some(Tier::Inside));
        assert_eq!(tier("--sysroot", "root"), Some(Tier::Inside));
        assert_eq!(tier("rebase", "xyz"), None);
        assert!(Tier::ExactCase < Tier::Exact && Tier::Exact < Tier::Prefix);
        assert!(Tier::Prefix < Tier::Word && Tier::Word < Tier::Inside);
    }

    #[test]
    fn a_dash_is_an_ordinary_character() {
        assert_eq!(tier("--squash", "--squash"), Some(Tier::ExactCase));
        assert_eq!(tier("--squash", "--sq"), Some(Tier::Prefix));
        assert_eq!(tier("-s", "-s"), Some(Tier::ExactCase));
        assert_eq!(tier("-S", "-s"), Some(Tier::Exact));
        assert_eq!(tier("--squash", "-s"), Some(Tier::Word));
        assert_eq!(tier("--no-squash", "-s"), Some(Tier::Inside));
        assert_eq!(tier("--ssl-reqd", "-ssl"), Some(Tier::Word));
        assert_eq!(tier("--proxy-ssl", "--ssl"), None);
        assert_eq!(tier("--proxy-ssl", "-ssl"), Some(Tier::Inside));
        assert_eq!(tier("--no-trunc", "run"), Some(Tier::Inside));
        assert_eq!(tier("--no-alpn", "--no"), Some(Tier::Prefix));
        assert_eq!(tier("squash", "-s"), None);
    }

    fn node_with(flag: Entity) -> CommandNode {
        let mut node = CommandNode::new("c", prov());
        node.entities.push(flag);
        node
    }

    fn hit(root: &CommandNode, i: usize) -> NodeRef {
        NodeRef::Flag {
            path: vec![root.name.clone()],
            key: root.entities[i].key().unwrap(),
        }
    }

    #[test]
    fn every_word_must_be_in_the_names_of_the_hit() {
        let root = node_with(Entity::flag_long("no-verify", prov()));
        let h = hit(&root, 0);
        assert!(matched_names(&root, &h, "no verify").is_some());
        assert!(matched_names(&root, &h, "NO  Verify").is_some());
        assert!(matched_names(&root, &h, "no verifx").is_none());
        assert!(matched_names(&root, &h, "-no-").is_some());
        assert!(matched_names(&root, &h, "").is_none());
    }

    #[test]
    fn the_no_form_of_a_negatable_flag_is_a_name() {
        let e = Entity::flag_spelled(None, Some("squash".into()), false, true, prov());
        let root = node_with(e);
        let h = hit(&root, 0);
        assert_eq!(
            best_name(&root, &h, "--squash").as_deref(),
            Some("--squash")
        );
        assert_eq!(
            best_name(&root, &h, "--no-squash").as_deref(),
            Some("--no-squash")
        );
        assert!(matched_names(&root, &h, "no squash").is_some());
    }

    #[test]
    fn a_short_flag_ranks_before_the_long_ones_for_the_same_letter() {
        let mut root = CommandNode::new("c", prov());
        root.entities.push(Entity::flag_long("no-squash", prov()));
        root.entities.push(Entity::flag_long("squash", prov()));
        root.entities.push(Entity::flag_short('S', prov()));
        root.entities.push(Entity::flag_short('s', prov()));
        let hits: Vec<NodeRef> = (0..4).map(|i| hit(&root, i)).collect();
        let ranked = rank_by_name(&root, hits.clone(), "-s");
        assert_eq!(
            ranked,
            vec![
                hits[3].clone(),
                hits[2].clone(),
                hits[1].clone(),
                hits[0].clone()
            ]
        );
    }
}
