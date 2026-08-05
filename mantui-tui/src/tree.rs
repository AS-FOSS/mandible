//! Flattening a [`CommandNode`] tree into a linear list of visible rows for
//! the tree pane, honoring expand/collapse state and an optional filter.
//!
//! Per spec §9, this list is meant to be built once per *structural* change
//! (expand/collapse, search change, lazy fill) and reused across pure
//! navigation (↑/↓) — see [`crate::app::App::ensure_rows_fresh`], which only
//! rebuilds when a dirty flag is set.

use mantui_core::CommandNode;
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
    /// needing lazy extraction — always true in this batch, since Tier A is
    /// non-incremental).
    pub children_filled: bool,
    /// True if this node is hidden by the source and being force-shown
    /// because the user toggled `.` (show-hidden).
    pub hidden: bool,
}

/// Flatten `root` into visible rows.
///
/// `expanded` holds the paths the user has explicitly opened.
/// `filter`, when non-empty, restricts the tree to nodes whose name or
/// summary contains it (case-insensitive) plus their ancestor chain, which
/// is force-shown-open regardless of `expanded` (spec §10: "matching a node
/// force-expands its ancestor chain"). This is a simple substring filter,
/// not the `nucleo`-backed, flag-aware, ranked search described in spec
/// §10 — that depends on `mantui-search`, which is a later-batch stub; see
/// this module's doc comment in `mantui-tui/src/lib.rs`.
pub fn flatten(
    root: &CommandNode,
    expanded: &HashSet<Vec<String>>,
    filter: Option<&str>,
    show_hidden: bool,
) -> Vec<TreeRow> {
    let mut out = Vec::new();
    let root_path = vec![root.name.clone()];
    match filter {
        Some(f) if !f.trim().is_empty() => {
            let needle = f.to_lowercase();
            push_filtered(root, root_path, 0, expanded, &needle, show_hidden, &mut out);
        }
        _ => push_plain(root, root_path, 0, expanded, show_hidden, &mut out),
    }
    out
}

fn node_matches(node: &CommandNode, needle: &str) -> bool {
    if node.name.to_lowercase().contains(needle) {
        return true;
    }
    if let Some(summary) = &node.summary {
        if summary.as_str().to_lowercase().contains(needle) {
            return true;
        }
    }
    false
}

fn make_row(node: &CommandNode, path: Vec<String>, depth: usize, expanded: bool) -> TreeRow {
    TreeRow {
        path,
        depth,
        name: node.name.clone(),
        summary: node.summary.as_ref().map(|t| t.single_line()),
        has_children: !node.subcommands.is_empty(),
        expanded,
        children_filled: node.children_filled,
        hidden: node.hidden,
    }
}

fn push_plain(
    node: &CommandNode,
    path: Vec<String>,
    depth: usize,
    expanded: &HashSet<Vec<String>>,
    show_hidden: bool,
    out: &mut Vec<TreeRow>,
) {
    if node.hidden && !show_hidden {
        return;
    }
    let is_expanded = expanded.contains(&path);
    out.push(make_row(node, path.clone(), depth, is_expanded));
    if is_expanded {
        for child in &node.subcommands {
            let mut child_path = path.clone();
            child_path.push(child.name.clone());
            push_plain(child, child_path, depth + 1, expanded, show_hidden, out);
        }
    }
}

/// Returns true if `node` or any descendant was included (i.e. matched or
/// is an ancestor of a match), in which case rows for it were appended to
/// `out`.
fn push_filtered(
    node: &CommandNode,
    path: Vec<String>,
    depth: usize,
    expanded: &HashSet<Vec<String>>,
    needle: &str,
    show_hidden: bool,
    out: &mut Vec<TreeRow>,
) -> bool {
    if node.hidden && !show_hidden {
        return false;
    }
    let self_match = node_matches(node, needle);

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
            needle,
            show_hidden,
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
    out.push(make_row(node, path, depth, effective_expanded));
    if effective_expanded {
        out.extend(child_rows);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use mantui_core::{Provenance, Source, Text};

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

    #[test]
    fn root_collapsed_shows_only_root() {
        let root = tree();
        let rows = flatten(&root, &HashSet::new(), None, false);
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
        let rows = flatten(&root, &expanded, None, false);
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
        let rows = flatten(&root, &expanded, None, false);
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
        let rows = flatten(&root, &expanded, None, false);
        assert!(!rows.iter().any(|r| r.name == "add"));
    }

    #[test]
    fn hidden_nodes_shown_when_toggled() {
        let mut root = tree();
        root.subcommands[0].hidden = true;
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let rows = flatten(&root, &expanded, None, true);
        assert!(rows.iter().any(|r| r.name == "add"));
    }

    #[test]
    fn filter_force_expands_ancestor_chain_to_a_match() {
        let root = tree();
        // Nothing manually expanded, but filtering for "onto" should reveal
        // git -> rebase -> --onto-helper.
        let rows = flatten(&root, &HashSet::new(), Some("onto"), false);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "rebase", "--onto-helper"]);
    }

    #[test]
    fn filter_matches_on_summary_text() {
        let root = tree();
        let rows = flatten(&root, &HashSet::new(), Some("reapply"), false);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "rebase"]);
    }

    #[test]
    fn filter_hides_non_matching_siblings() {
        let root = tree();
        let rows = flatten(&root, &HashSet::new(), Some("stash"), false);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["git", "stash"]);
    }

    #[test]
    fn empty_filter_behaves_like_no_filter() {
        let root = tree();
        let mut expanded = HashSet::new();
        expanded.insert(vec!["git".to_string()]);
        let with_empty = flatten(&root, &expanded, Some(""), false);
        let without = flatten(&root, &expanded, None, false);
        assert_eq!(with_empty, without);
    }
}
