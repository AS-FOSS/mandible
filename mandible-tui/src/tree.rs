//! Flattening a [`CommandNode`] tree into a linear list of visible rows for
//! the tree pane, honoring expand/collapse state and an optional filter.
//!
//! Per spec §9, this list is meant to be built once per *structural* change
//! (expand/collapse, search change, lazy fill) and reused across pure
//! navigation (↑/↓) — see [`crate::app::App::ensure_rows_fresh`], which only
//! rebuilds when a dirty flag is set.

use mandible_core::CommandNode;
use std::collections::HashSet;

/// One visible row in the tree pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// This row's full name path from the root (spec §4.3 `NodeRef`
    /// convention: includes the root's own name).
    pub path: Vec<String>,
    /// Indentation depth; the root is depth 0.
    pub depth: usize,
    /// The node's own name (not sanitized via `Text` — see
    /// [`crate::sanitize::defensive_single_line`], applied at render time
    /// as a belt-and-braces measure since the IR types `name` as a plain
    /// `String`, not `Text`).
    pub name: String,
    /// A single-line summary preview, already collapsed via
    /// `Text::single_line`, if the node has one.
    pub summary: Option<String>,
    /// True if the node has any subcommands.
    pub has_children: bool,
    /// True if this row is currently showing its children.
    pub expanded: bool,
    /// True if the node's own children are known-complete (vs. still
    /// needing lazy extraction on expand — spec §5.2 step 3).
    pub children_filled: bool,
    /// True if this node is hidden by the source and being force-shown
    /// because the user toggled `.` (show-hidden).
    pub hidden: bool,
    /// True if a lazy fill (spec §5.2 step 3) is currently in flight for
    /// this node, so the tree pane can render a subtle spinner/placeholder
    /// row instead of a static chevron (spec §9 "designed degraded
    /// states").
    pub pending: bool,
    /// True when this node was found by the `<parent>-<sub>` PATH
    /// convention rather than documented by its parent's own help text
    /// (spec §5.4) — [`mandible_core::CommandNode::discovered_binary`]. The
    /// row says so (spec §9.2): the command is a guess made from a
    /// filename, and every other row on screen is not.
    pub unverified: bool,
}

/// Flatten `root` into visible rows.
///
/// `expanded` holds the paths the user has explicitly opened.
/// `matching_paths`, when `Some`, restricts the tree to nodes whose path is
/// in the set, plus their ancestor chain, which is force-shown-open
/// regardless of `expanded` (spec §10: "matching a node force-expands its
/// ancestor chain"). This function itself does no text matching — the
/// caller (`App`, backed by `mandible-search`'s `nucleo` index) decides what
/// matches and hands over the resulting set of command paths; a
/// [`mandible_core::NodeRef::Flag`] match is represented here by its *parent
/// command's* path, since flags aren't tree rows (spec §2: "Flags are not
/// tree rows").
pub fn flatten(
    root: &CommandNode,
    expanded: &HashSet<Vec<String>>,
    matching_paths: Option<&HashSet<Vec<String>>>,
    show_hidden: bool,
    pending: &HashSet<Vec<String>>,
) -> Vec<TreeRow> {
    let mut out = Vec::new();
    let root_path = vec![root.name.clone()];
    match matching_paths {
        Some(matches) if !matches.is_empty() => {
            push_filtered(
                root,
                root_path,
                0,
                expanded,
                matches,
                show_hidden,
                pending,
                &mut out,
            );
        }
        _ => push_plain(root, root_path, 0, expanded, show_hidden, pending, &mut out),
    }
    out
}

fn make_row(
    node: &CommandNode,
    path: Vec<String>,
    depth: usize,
    expanded: bool,
    pending: bool,
) -> TreeRow {
    TreeRow {
        path,
        depth,
        name: node.name.clone(),
        summary: node.summary.as_ref().map(|t| t.single_line()),
        has_children: !node.subcommands.is_empty(),
        expanded,
        children_filled: node.children_filled,
        hidden: node.hidden,
        pending,
        unverified: node.discovered_binary.is_some(),
    }
}

fn push_plain(
    node: &CommandNode,
    path: Vec<String>,
    depth: usize,
    expanded: &HashSet<Vec<String>>,
    show_hidden: bool,
    pending: &HashSet<Vec<String>>,
    out: &mut Vec<TreeRow>,
) {
    if node.hidden && !show_hidden {
        return;
    }
    let is_expanded = expanded.contains(&path);
    let is_pending = pending.contains(&path);
    out.push(make_row(node, path.clone(), depth, is_expanded, is_pending));
    if is_expanded {
        for child in &node.subcommands {
            let mut child_path = path.clone();
            child_path.push(child.name.clone());
            push_plain(
                child,
                child_path,
                depth + 1,
                expanded,
                show_hidden,
                pending,
                out,
            );
        }
    }
}

/// Returns true if `node` or any descendant was included (i.e. matched or
/// is an ancestor of a match), in which case rows for it were appended to
/// `out`.
#[allow(clippy::too_many_arguments)]
fn push_filtered(
    node: &CommandNode,
    path: Vec<String>,
    depth: usize,
    expanded: &HashSet<Vec<String>>,
    matches: &HashSet<Vec<String>>,
    show_hidden: bool,
    pending: &HashSet<Vec<String>>,
    out: &mut Vec<TreeRow>,
) -> bool {
    if node.hidden && !show_hidden {
        return false;
    }
    let self_match = matches.contains(&path);

    let mut child_rows = Vec::new();
    let mut any_child_match = false;
    for child in &node.subcommands {
        let mut child_path = path.clone();
        child_path.push(child.name.clone());
        if push_filtered(
            child,
            child_path,
            depth + 1,
            expanded,
            matches,
            show_hidden,
            pending,
            &mut child_rows,
        ) {
            any_child_match = true;
        }
    }

    if !self_match && !any_child_match {
        return false;
    }

    let user_expanded = expanded.contains(&path);
    let effective_expanded = user_expanded || any_child_match;
    let is_pending = pending.contains(&path);
    out.push(make_row(node, path, depth, effective_expanded, is_pending));
    if effective_expanded {
        out.extend(child_rows);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, Text};

    fn node(name: &str, children: Vec<CommandNode>) -> CommandNode {
        let mut n = CommandNode::new(name, Provenance::single(Source::HelpText));
        n.subcommands = children;
        n
    }

    fn tree() -> CommandNode {
        node(
            "git",
            vec![
                node("add", vec![]),
                {
                    let mut rebase = node("rebase", vec![node("--onto-helper", vec![])]);
                    rebase.summary = Some(Text::sanitize("Reapply commits on top of another base"));
                    rebase
                },
                node("stash", vec![]),
            ],
        )
    }

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    fn matches(paths: &[&[&str]]) -> HashSet<Vec<String>> {
        paths.iter().map(|p| path(p)).collect()
    }

    #[test]
    fn root_collapsed_shows_only_root() {
        let root = tree();
        let rows = flatten(&root, &HashSet::new(), None, false, &HashSet::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "git");
        assert!(rows[0].has_children);
        assert!(!rows[0].expanded);
    }

    #[test]
    fn expanding_root_shows_direct_children_only() {
        let root = tree();
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let rows = flatten(&root, &expanded, None, false, &HashSet::new());
        assert_eq!(rows.len(), 4); // git, add, rebase, stash
        assert_eq!(rows[1].name, "add");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].name, "rebase");
        assert!(!rows[2].expanded, "rebase itself not expanded yet");
    }

    #[test]
    fn nested_expansion() {
        let root = tree();
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        expanded.insert(vec!["git".to_string(), "rebase".to_string()]);
        let rows = flatten(&root, &expanded, None, false, &HashSet::new());
        // git, add, rebase, --onto-helper, stash
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[3].name, "--onto-helper");
        assert_eq!(rows[3].depth, 2);
    }

    #[test]
    fn hidden_nodes_excluded_by_default() {
        let mut root = tree();
        root.subcommands[0].hidden = true; // "add"
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let rows = flatten(&root, &expanded, None, false, &HashSet::new());
        assert!(!rows.iter().any(|r| r.name == "add"));
    }

    #[test]
    fn hidden_nodes_shown_when_toggled() {
        let mut root = tree();
        root.subcommands[0].hidden = true;
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let rows = flatten(&root, &expanded, None, true, &HashSet::new());
        assert!(rows.iter().any(|r| r.name == "add"));
    }

    #[test]
    fn filter_force_expands_ancestor_chain_to_a_match() {
        let root = tree();
        // Nothing manually expanded, but a match on --onto-helper should
        // reveal git -> rebase -> --onto-helper.
        let m = matches(&[&["git", "rebase", "--onto-helper"]]);
        let rows = flatten(&root, &HashSet::new(), Some(&m), false, &HashSet::new());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "rebase", "--onto-helper"]);
    }

    #[test]
    fn filter_shows_only_the_matched_node_when_it_has_no_matching_descendants() {
        let root = tree();
        // A match on "rebase" itself (e.g. its summary matched, or a flag
        // of its matched and got mapped to its parent path) should show
        // just git -> rebase, not its children.
        let m = matches(&[&["git", "rebase"]]);
        let rows = flatten(&root, &HashSet::new(), Some(&m), false, &HashSet::new());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "rebase"]);
    }

    #[test]
    fn filter_hides_non_matching_siblings() {
        let root = tree();
        let m = matches(&[&["git", "stash"]]);
        let rows = flatten(&root, &HashSet::new(), Some(&m), false, &HashSet::new());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "stash"]);
    }

    #[test]
    fn empty_matches_behaves_like_no_filter() {
        let root = tree();
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let empty = HashSet::new();
        let with_empty = flatten(&root, &expanded, Some(&empty), false, &HashSet::new());
        let without = flatten(&root, &expanded, None, false, &HashSet::new());
        assert_eq!(with_empty, without);
    }
}
