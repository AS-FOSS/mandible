//! Addressing: [`NodeRef`] and [`FlagKey`], the single addressing type used
//! by search, the clipboard, and the cache. See spec §4.3.

use crate::node::CommandNode;
use serde::{Deserialize, Serialize};

/// A reference to a command or a specific flag within the tree, by name path.
///
/// Paths are name-based: `["git", "rebase"]` addresses `git rebase`, and
/// include the root's own name as the first segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeRef {
    /// A command or subcommand, by full name path from the root.
    Command(Vec<String>),
    /// A flag on a command, by the command's name path plus the flag's key.
    Flag {
        /// The owning command's full name path from the root.
        path: Vec<String>,
        /// Which flag on that command.
        key: FlagKey,
    },
}

/// Identifies a flag within a node's flag list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlagKey {
    /// By long spelling, without the leading `--`.
    Long(String),
    /// By short spelling, without the leading `-`.
    Short(char),
}

/// Resolve a name path to the [`CommandNode`] it addresses, starting from
/// `root`. `path` includes `root`'s own name as its first element.
///
/// Walks `subcommands` by exact name (or alias) match at each level,
/// consuming exactly one path segment per level. This deliberately does
/// **not** special-case a subcommand sharing its parent's name — there is no
/// "skip a segment that matches the current node" shortcut, which would
/// silently mis-resolve that case (spec §4.3).
pub fn resolve<'a>(root: &'a CommandNode, path: &[String]) -> Option<&'a CommandNode> {
    let mut segments = path.iter();
    let first = segments.next()?;
    if !names_match(root, first) {
        return None;
    }
    let mut current = root;
    for seg in segments {
        current = current.subcommands.iter().find(|c| names_match(c, seg))?;
    }
    Some(current)
}

/// Mutable counterpart of [`resolve`], used by the extraction runner to
/// splice a newly-extracted subtree into the cached tree in place.
pub fn resolve_mut<'a>(root: &'a mut CommandNode, path: &[String]) -> Option<&'a mut CommandNode> {
    let mut segments = path.iter();
    let first = segments.next()?;
    if !names_match(root, first) {
        return None;
    }
    let mut current = root;
    for seg in segments {
        current = current
            .subcommands
            .iter_mut()
            .find(|c| names_match(c, seg))?;
    }
    Some(current)
}

fn names_match(node: &CommandNode, segment: &str) -> bool {
    node.name == segment || node.aliases.iter().any(|a| a == segment)
}

/// Resolve a [`NodeRef`] to the flag it addresses, if any.
pub fn resolve_flag<'a>(
    root: &'a CommandNode,
    path: &[String],
    key: &FlagKey,
) -> Option<&'a crate::node::Flag> {
    let node = resolve(root, path)?;
    node.flags.iter().find(|f| f.matches_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{Provenance, Source};

    fn leaf(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    #[test]
    fn resolves_root() {
        let root = leaf("git");
        let found = resolve(&root, &["git".to_string()]).unwrap();
        assert_eq!(found.name, "git");
    }

    #[test]
    fn resolves_nested_path() {
        let mut root = leaf("git");
        let mut rebase = leaf("rebase");
        rebase.subcommands.push(leaf("--onto-helper"));
        root.subcommands.push(rebase);
        let path = vec!["git".to_string(), "rebase".to_string()];
        let found = resolve(&root, &path).unwrap();
        assert_eq!(found.name, "rebase");
    }

    #[test]
    fn returns_none_for_unknown_segment() {
        let root = leaf("git");
        let path = vec!["git".to_string(), "nonexistent".to_string()];
        assert!(resolve(&root, &path).is_none());
    }

    /// A subcommand sharing its parent's name must resolve correctly: this
    /// is the exact regression case called out in spec §4.3. A buggy
    /// resolver that "skips a segment equal to the current node's name"
    /// would treat `["a", "a", "b"]` as if the second `a` were a no-op and
    /// incorrectly look for `b` directly under the outer `a`, or would
    /// resolve to the wrong `a`.
    #[test]
    fn subcommand_sharing_parent_name_resolves_correctly() {
        // a
        // └── a (a different node, deliberately also named "a")
        //     └── b
        let mut inner_a = leaf("a");
        inner_a.subcommands.push(leaf("b"));
        // Mark the inner node's child distinguishably so we can prove we
        // landed on the right subtree.
        inner_a.subcommands[0].summary = None;
        let mut outer_a = leaf("a");
        outer_a.subcommands.push(inner_a);
        // outer "a" has no "b" child directly - only inner "a" does.
        let path = vec!["a".to_string(), "a".to_string(), "b".to_string()];
        let found = resolve(&outer_a, &path).expect("must resolve through nested same-named node");
        assert_eq!(found.name, "b");

        // And a path that tries to skip the inner "a" must fail, proving
        // there's no skip-shortcut silently making ["a", "b"] work too.
        let bad_path = vec!["a".to_string(), "b".to_string()];
        assert!(resolve(&outer_a, &bad_path).is_none());
    }

    #[test]
    fn resolves_via_alias() {
        let mut root = leaf("git");
        let mut add = leaf("add");
        add.aliases.push("stage".to_string());
        root.subcommands.push(add);
        let path = vec!["git".to_string(), "stage".to_string()];
        let found = resolve(&root, &path).unwrap();
        assert_eq!(found.name, "add");
    }

    #[test]
    fn resolve_mut_allows_splicing() {
        let mut root = leaf("git");
        root.subcommands.push(leaf("rebase"));
        {
            let node = resolve_mut(&mut root, &["git".to_string(), "rebase".to_string()]).unwrap();
            node.children_filled = true;
        }
        assert!(root.subcommands[0].children_filled);
    }
}
