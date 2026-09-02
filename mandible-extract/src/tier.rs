//! [`ExtractionTier`]: the node-scoped extraction trait every tier
//! implements. See spec §5.2.

use crate::errors::ExtractError;
use crate::resolve::ResolvedTool;
use mandible_core::{Authority, CommandNode};

/// Structural facts about the node being extracted that the runner already
/// knows but an individual tier's probe cannot determine on its own.
#[derive(Debug, Clone, Copy)]
pub struct NodeHints {
    /// True when this node's name (the last element of `path`) is known to
    /// come from a structural source (a recognized command heading) rather
    /// than a layout guess. The root is always `true`: the tool name came
    /// from the user, not a parser.
    ///
    /// Gates whether the node may be probed with `--help`/`-h` at all: send
    /// only when the word came from a structural source, never a prose
    /// heuristic. A non-attested node is never probed, in any shape; the
    /// tier declines with a per-node error instead. Spec §6 rule 0, §5.3.
    pub heading_attested: bool,
}

/// One source of `CommandNode` data: a known-spec catalog, `--help` grammar
/// parser, completion script parser, man page extractor, or native probe.
/// Node-scoped rather than whole-tree to keep lazy/incremental extraction
/// possible (spec §5.1, §5.2).
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
    /// positionals, and the names of its direct subcommands. Implementors
    /// must not recurse further, unless [`Self::is_incremental`] is
    /// `false`, in which case the whole known subtree may be returned in
    /// one call.
    ///
    /// `path` includes the tool's own name as its first element. `hints`
    /// carries structural facts the runner already knows — see
    /// [`NodeHints`].
    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
        hints: NodeHints,
    ) -> Result<CommandNode, ExtractError>;

    /// `false` when the source is already fully in memory (e.g. carapace),
    /// in which case the runner requests the whole tree in one call. `true`
    /// (the default) means the runner should defer descendants until they
    /// are expanded (batch 2).
    fn is_incremental(&self) -> bool {
        true
    }
}
