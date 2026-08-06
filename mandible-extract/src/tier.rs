//! [`ExtractionTier`]: the node-scoped extraction trait every tier
//! implements. See spec §5.2.

use crate::errors::ExtractError;
use crate::resolve::ResolvedTool;
use mantui_core::{Authority, CommandNode};

/// One source of `CommandNode` data: a known-spec catalog, `--help` grammar
/// parser, completion script parser, man page extractor, or native probe.
///
/// Extraction is node-scoped rather than whole-tree: a whole-tree
/// `extract()` forecloses lazy/incremental extraction, which is required to
/// keep cobra-heavy tools (10-25s to walk eagerly, spec §5.1) interactive.
pub trait ExtractionTier: Send + Sync {
    /// A stable, human-readable identifier for this tier, e.g.
    /// `"known_specs::carapace"`. Shown in `--doctor` and the `?` overlay.
    fn name(&self) -> &'static str;

    /// This tier's two-axis authority, used when merging its output against
    /// other tiers' output for the same node (spec §4.4).
    fn authority(&self) -> Authority;

    /// Cheap, side-effect-free check: can this tier plausibly handle
    /// `tool`? Must obey spec §6 if it needs to probe anything. The result
    /// is cached per run by the caller.
    fn detect(&self, tool: &ResolvedTool) -> bool;

    /// Extract exactly one level: the node at `path`, its flags, its
    /// positionals, and the *names* of its direct subcommands. Implementors
    /// must not recurse further than that — unless [`Self::is_incremental`]
    /// is `false`, in which case the whole known subtree may be returned in
    /// one call because there is nothing to defer (e.g. an in-memory
    /// catalog).
    ///
    /// `path` includes the tool's own name as its first element, matching
    /// [`mantui_core::NodeRef::Command`]'s convention.
    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
    ) -> Result<CommandNode, ExtractError>;

    /// `false` when the source is already fully in memory (e.g. carapace),
    /// in which case the runner requests the whole tree in one call. `true`
    /// (the default) means the runner should defer descendants until they
    /// are expanded (batch 2).
    fn is_incremental(&self) -> bool {
        true
    }
}
