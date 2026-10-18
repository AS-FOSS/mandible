//! The command tree together with a version that moves on every write.
//!
//! Anything derived from the tree (the detail pane's page cache) keys on
//! [`TreeStore::version`]. The fields are private and the only mutable access
//! is [`TreeStore::edit`] or [`TreeStore::replace`], both of which take a new
//! version first, so a write that forgets to invalidate cannot compile.
//! Versions come from one process-wide counter, so no two stores and no two
//! writes share one, even when a whole store is assigned over another.

use mandible_core::CommandNode;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

/// A [`CommandNode`] tree whose every write goes through the version bump.
pub struct TreeStore {
    root: CommandNode,
    version: u64,
}

impl TreeStore {
    /// Wrap a freshly extracted tree.
    pub fn new(root: CommandNode) -> Self {
        Self {
            root,
            version: next_version(),
        }
    }

    /// The tree's version; unique across all stores; new after any write access.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Mutable access to the tree. Advances the version even when the caller
    /// ends up changing nothing; a spurious bump only costs a page rebuild.
    pub fn edit(&mut self) -> &mut CommandNode {
        self.version = next_version();
        &mut self.root
    }

    /// Swap the whole tree.
    pub fn replace(&mut self, root: CommandNode) {
        *self.edit() = root;
    }
}

impl Deref for TreeStore {
    type Target = CommandNode;

    fn deref(&self) -> &CommandNode {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    #[test]
    fn every_write_access_moves_the_version_and_reads_do_not() {
        let mut t = TreeStore::new(node("a"));
        let v0 = t.version();
        let _ = t.name.len();
        assert_eq!(t.version(), v0);
        t.edit();
        let v1 = t.version();
        assert_ne!(v1, v0);
        t.replace(node("b"));
        assert_ne!(t.version(), v1);
        assert_eq!(t.name, "b");
    }

    #[test]
    fn versions_are_never_shared_between_stores_or_after_wholesale_replacement() {
        let a = TreeStore::new(node("a"));
        let b = TreeStore::new(node("a"));
        assert_ne!(a.version(), b.version());
        let mut seen = vec![a.version()];
        let mut c = a;
        for _ in 0..3 {
            c.edit();
            seen.push(c.version());
        }
        c = TreeStore::new(node("c"));
        assert!(!seen.contains(&c.version()));
    }
}
