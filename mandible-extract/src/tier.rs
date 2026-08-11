//! [`ExtractionTier`]: the node-scoped extraction trait every tier
//! implements. See spec §5.2.

use crate::errors::ExtractError;
use crate::resolve::ResolvedTool;
use mandible_core::{Authority, CommandNode};

/// Structural facts about the node being extracted that the *runner* knows
/// (it holds the already-merged [`CommandNode`], see [`crate::Runner`]) but
/// an individual tier's own probe cannot determine on its own.
///
/// `Copy` and deliberately thin: it is passed to every tier's
/// [`ExtractionTier::extract_node`], most tiers ignore it entirely, and
/// today exactly one field exists because exactly one decision needs it.
#[derive(Debug, Clone, Copy)]
pub struct NodeHints {
    /// True when this node's own name — the last element of the `path`
    /// passed to [`ExtractionTier::extract_node`] — is known to have come
    /// from a *structural* source: a recognized command heading
    /// ([`mandible_core::CommandNode::heading_attested`]), rather than from
    /// a layout guess that merely looked plausible.
    ///
    /// The root is always `true` ([`crate::Runner::extract_full_for`]
    /// passes it that way): it is the tool name the *user typed*, not a
    /// word any parser invented, so it is structurally attested by
    /// definition — there is no heading to point to because none was
    /// needed.
    ///
    /// This is the provenance gate spec §6 rule 0's closing paragraph names
    /// as the general form of the hazard behind the never-probe list:
    /// "send `<word> --help` only when the word came from a structural
    /// source ... never from a prose heuristic." It started out gating
    /// only [M-16] sub-case (a)'s `-h` fallback for a subcommand's
    /// man-shaped `--help` output; it now also gates the positional
    /// `<words...> --help` probe itself
    /// ([`crate::help_text::probe_help_text_reporting_flag`]) — a
    /// non-attested node is never probed at all, in any shape, and the
    /// tier declines with a per-node error instead (spec §5.3) rather than
    /// the tree silently gaining an empty-but-successful node.
    pub heading_attested: bool,
}

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
    /// [`mandible_core::NodeRef::Command`]'s convention. `hints` carries
    /// structural facts about this node that the runner already knows —
    /// see [`NodeHints`].
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
