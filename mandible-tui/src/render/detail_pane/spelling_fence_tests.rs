//! Every spelling the detail pane draws is findable by typing it: for each
//! entity of each corpus fixture, the spellings read back off the drawn
//! row are searched in names mode and must find the entity, and every
//! command is found by its name and aliases.

use super::identity_tests::corpus_nodes;
use super::*;
use crate::search_rank::rank_by_name;
use mandible_core::{CommandNode, NodeRef};
use mandible_search::SearchIndex;

/// What the row draws, as separate things to type: each comma-separated
/// spelling, the two ends of a range, both forms of a `[no-]` spelling,
/// an abbreviation bracket as drawn and in full, and the spelling with a
/// value that sits against it.
fn drawn_spellings(entity: &Entity) -> Vec<String> {
    let mut out = Vec::new();
    let parts: Vec<String> = entity_name_spec(entity)
        .split(", ")
        .map(str::to_string)
        .collect();
    for (n, part) in parts.iter().enumerate() {
        let ends: Vec<&str> = part.split(" ... ").collect();
        for (m, end) in ends.iter().enumerate() {
            let mut forms = vec![end.to_string()];
            if end.contains("[no-]") {
                forms = vec![end.replace("[no-]", ""), end.replace("[no-]", "no-")];
            }
            for s in &entity.spellings {
                if s.abbrev.is_some() && s.render() == *end {
                    forms.push(s.typed());
                }
            }
            let last = n + 1 == parts.len() && m + 1 == ends.len();
            for form in forms {
                if let (Some(value), "", true) = (entity.value_text(), entity.value_gap(), last) {
                    out.push(format!("{form}{value}"));
                }
                out.push(form);
            }
        }
    }
    out
}

fn finds(root: &CommandNode, index: &mut SearchIndex, query: &str, item: &NodeRef) -> bool {
    index.set_query(query);
    rank_by_name(root, index.results(), query).contains(item)
}

fn missing(root: &CommandNode, tool: &str) -> Vec<String> {
    let mut index = SearchIndex::new();
    index.populate(root);
    let mut out = Vec::new();
    let mut stack = vec![(root, vec![root.name.clone()])];
    while let Some((node, path)) = stack.pop() {
        let command = NodeRef::Command(path.clone());
        for name in std::iter::once(&node.name).chain(&node.aliases) {
            if !finds(root, &mut index, name, &command) {
                out.push(format!("{tool}: command {name:?} at {path:?}"));
            }
        }
        for entity in &node.entities {
            let Some(key) = entity.key() else { continue };
            let item = NodeRef::Flag {
                path: path.clone(),
                key,
            };
            for query in drawn_spellings(entity) {
                if !query.trim().is_empty() && !finds(root, &mut index, &query, &item) {
                    out.push(format!("{tool}: {query:?} at {path:?}"));
                }
            }
        }
        for child in &node.subcommands {
            let mut p = path.clone();
            p.push(child.name.clone());
            stack.push((child, p));
        }
    }
    out
}

fn check(sampled: bool) {
    let (nodes, total) = corpus_nodes(sampled);
    assert!(!nodes.is_empty() && nodes.len() * 2 > total.min(40));
    let gaps: Vec<String> = nodes
        .iter()
        .flat_map(|(tool, root)| missing(root, tool))
        .collect();
    assert!(
        gaps.is_empty(),
        "{} drawn spellings are not found:\n{}",
        gaps.len(),
        gaps.iter().take(30).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn every_drawn_spelling_of_a_corpus_sample_is_found_in_names_mode() {
    check(true);
}

/// Every fixture with an `expected.snap`, which is every one that is not
/// an `[xfail]`. Minutes in a debug build, so it is opt-in.
#[test]
#[ignore = "whole corpus; run in release"]
fn every_drawn_spelling_of_the_whole_corpus_is_found_in_names_mode() {
    check(false);
}
