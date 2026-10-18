//! The command tree together with a version that moves on every write.
//!
//! Anything derived from the tree (the detail pane's page cache) keys on
//! [`TreeStore::version`]. The fields are private and the only mutable access
//! is [`TreeStore::edit`] or [`TreeStore::replace`], both of which advance the
//! version first, so a write that forgets to invalidate cannot compile.

use mandible_core::CommandNode;
use std::ops::Deref;

/// A [`CommandNode`] tree whose every write goes through the version bump.
pub struct TreeStore {
    root: CommandNode,
    version: u64,
}

impl TreeStore {
    /// Wrap a freshly extracted tree.
    pub fn new(root: CommandNode) -> Self {
        Self { root, version: 0 }
    }

    /// The tree's version; differs after any write access.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Mutable access to the tree. Advances the version even when the caller
    /// ends up changing nothing; a spurious bump only costs a page rebuild.
    pub fn edit(&mut self) -> &mut CommandNode {
        self.version = self.version.wrapping_add(1);
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
}
