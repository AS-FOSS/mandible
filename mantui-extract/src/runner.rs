//! The extraction runner: drives every enabled tier for a tool and merges
//! their output (spec §5.2, §5.3).
//!
//! [`Runner::extract_full`] does spec §5.2 step 1 (root-only extraction
//! from every detecting tier). [`Runner::fill_node`] does step 3 (lazy
//! per-node expansion): it only re-probes *incremental* tiers, since a
//! non-incremental tier (Tier A) already contributed everything it ever
//! will at root extraction. Background warming of the next depth (step 4)
//! lives above this crate, in the binary's event loop, since it's about
//! *when* to call `fill_node` (on a bounded background pool, cancelled on
//! quit), not about extraction logic itself.

use crate::resolve::{resolve_tool, ResolvedTool};
use crate::tier::ExtractionTier;
use mantui_core::{merge_nodes, CommandNode};
use std::time::{Duration, Instant};

/// The result of one tier's attempt to contribute to a node, for display in
/// `--doctor` and the `?` overlay (spec §5.3).
#[derive(Debug, Clone)]
pub struct TierStatus {
    /// The tier's [`ExtractionTier::name`].
    pub tier: &'static str,
    /// Whether [`ExtractionTier::detect`] returned true.
    pub detected: bool,
    /// `Some(message)` if the tier detected but then failed to extract.
    pub error: Option<String>,
}

/// The outcome of running the full pipeline against one tool.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    /// The tool name that was requested.
    pub tool: String,
    /// The merged tree, or `None` if no tier produced a root node (spec
    /// §5.3: "The runner errors only when no tier produced a root node.").
    pub root: Option<CommandNode>,
    /// Per-tier status for this extraction.
    pub tier_statuses: Vec<TierStatus>,
    /// Wall-clock time spent extracting.
    pub elapsed: Duration,
}

impl ExtractionResult {
    /// Total flags in the merged tree, including inherited ones, counted
    /// recursively.
    pub fn flag_count(&self) -> usize {
        self.root.as_ref().map(count_flags).unwrap_or(0)
    }

    /// Total nodes in the merged tree (including the root), counted
    /// recursively.
    pub fn node_count(&self) -> usize {
        self.root.as_ref().map(count_nodes).unwrap_or(0)
    }

    /// Fraction (0.0-1.0) of flags in the merged tree that have a
    /// description. `0.0` if there are no flags.
    pub fn flag_description_ratio(&self) -> f64 {
        let total = self.flag_count();
        if total == 0 {
            return 0.0;
        }
        let described = self.root.as_ref().map(count_described_flags).unwrap_or(0);
        described as f64 / total as f64
    }
}

fn count_flags(node: &CommandNode) -> usize {
    node.flags.len() + node.subcommands.iter().map(count_flags).sum::<usize>()
}

fn count_described_flags(node: &CommandNode) -> usize {
    node.flags
        .iter()
        .filter(|f| f.description.is_some())
        .count()
        + node
            .subcommands
            .iter()
            .map(count_described_flags)
            .sum::<usize>()
}

fn count_nodes(node: &CommandNode) -> usize {
    1 + node.subcommands.iter().map(count_nodes).sum::<usize>()
}

/// Drives a fixed set of tiers against tools.
pub struct Runner {
    tiers: Vec<Box<dyn ExtractionTier>>,
}

impl Runner {
    /// Build a runner over the given tiers, attempted in the given order
    /// (a cost ordering, per spec §7 — conflict resolution is by
    /// [`mantui_core::Authority`], not attempt order).
    pub fn new(tiers: Vec<Box<dyn ExtractionTier>>) -> Runner {
        Runner { tiers }
    }

    /// The configured tiers' statuses without running extraction — used by
    /// `--doctor` to show which tiers exist even before knowing about a
    /// specific tool.
    pub fn tier_names(&self) -> Vec<&'static str> {
        self.tiers.iter().map(|t| t.name()).collect()
    }

    /// Extract the full tree for `tool_name`.
    ///
    /// Requests only the root from every detecting tier (spec §5.2 step 1),
    /// which for the non-incremental Tier A already yields the complete
    /// tree. A tier that detects but then fails does not abort the run —
    /// its failure is recorded in `tier_statuses` and whatever the other
    /// tiers produced is still merged and returned (spec §5.3).
    pub fn extract_full(&self, tool_name: &str) -> ExtractionResult {
        let resolved = resolve_tool(tool_name);
        self.extract_full_for(&resolved)
    }

    /// Same as [`Self::extract_full`] but for an already-resolved tool,
    /// useful when the caller already paid for `PATH` resolution.
    pub fn extract_full_for(&self, resolved: &ResolvedTool) -> ExtractionResult {
        let start = Instant::now();
        let root_path = vec![resolved.name.clone()];
        let mut statuses = Vec::with_capacity(self.tiers.len());
        let mut candidates = Vec::new();

        for tier in &self.tiers {
            let detected = tier.detect(resolved);
            let mut error = None;
            if detected {
                match tier.extract_node(resolved, &root_path) {
                    Ok(node) => candidates.push(node),
                    Err(e) => error = Some(e.to_string()),
                }
            }
            statuses.push(TierStatus {
                tier: tier.name(),
                detected,
                error,
            });
        }

        let root = if candidates.is_empty() {
            None
        } else {
            match merge_nodes(candidates) {
                Ok(node) => Some(node),
                Err(e) => {
                    statuses.push(TierStatus {
                        tier: "merge",
                        detected: true,
                        error: Some(e.to_string()),
                    });
                    None
                }
            }
        };

        ExtractionResult {
            tool: resolved.name.clone(),
            root,
            tier_statuses: statuses,
            elapsed: start.elapsed(),
        }
    }

    /// Lazy per-node expansion (spec §5.2 step 3): re-probe `path` against
    /// every *incremental* tier only (a non-incremental tier like Tier A
    /// already contributed everything it ever will, at root extraction —
    /// there's nothing to defer, so nothing to re-request), and merge the
    /// result with `existing` (the node already in the tree at `path`,
    /// which may itself carry deep, already-complete data from a
    /// non-incremental tier).
    ///
    /// `existing` is always included as a merge candidate, so the result
    /// is never worse than what was already known — a tier that fails or
    /// isn't detected just doesn't contribute (spec §5.3), it doesn't
    /// erase prior data.
    pub fn fill_node(
        &self,
        resolved: &ResolvedTool,
        path: &[String],
        existing: CommandNode,
    ) -> FillResult {
        let start = Instant::now();
        let mut candidates = vec![existing];
        let mut statuses = Vec::new();

        for tier in &self.tiers {
            if !tier.is_incremental() {
                continue;
            }
            let detected = tier.detect(resolved);
            let mut error = None;
            if detected {
                match tier.extract_node(resolved, path) {
                    Ok(node) => candidates.push(node),
                    Err(e) => error = Some(e.to_string()),
                }
            }
            statuses.push(TierStatus {
                tier: tier.name(),
                detected,
                error,
            });
        }

        // `candidates` always has at least the `existing` node pushed
        // above, so this can only fail via a merge-internal bug, never
        // MergeError::Empty.
        let node = merge_nodes(candidates)
            .expect("existing is always a candidate, so candidates is never empty");

        FillResult {
            node,
            tier_statuses: statuses,
            elapsed: start.elapsed(),
        }
    }
}

/// The outcome of [`Runner::fill_node`].
#[derive(Debug, Clone)]
pub struct FillResult {
    /// The merged node at the requested path, now with this level's data
    /// refreshed from every incremental tier.
    pub node: CommandNode,
    /// Per-tier status for this fill.
    pub tier_statuses: Vec<TierStatus>,
    /// Wall-clock time spent filling.
    pub elapsed: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ExtractError;
    use mantui_core::{Authority, Provenance, Source};

    struct AlwaysOk;
    impl ExtractionTier for AlwaysOk {
        fn name(&self) -> &'static str {
            "test::always_ok"
        }
        fn authority(&self) -> Authority {
            Source::HelpText.authority()
        }
        fn detect(&self, _tool: &ResolvedTool) -> bool {
            true
        }
        fn extract_node(
            &self,
            _tool: &ResolvedTool,
            path: &[String],
        ) -> Result<CommandNode, ExtractError> {
            Ok(CommandNode::new(
                path.last().cloned().unwrap_or_default(),
                Provenance::single(Source::HelpText),
            ))
        }
        fn is_incremental(&self) -> bool {
            false
        }
    }

    struct AlwaysFails;
    impl ExtractionTier for AlwaysFails {
        fn name(&self) -> &'static str {
            "test::always_fails"
        }
        fn authority(&self) -> Authority {
            Source::HelpText.authority()
        }
        fn detect(&self, _tool: &ResolvedTool) -> bool {
            true
        }
        fn extract_node(
            &self,
            _tool: &ResolvedTool,
            _path: &[String],
        ) -> Result<CommandNode, ExtractError> {
            Err(ExtractError::Other("boom".to_string()))
        }
    }

    struct NeverDetects;
    impl ExtractionTier for NeverDetects {
        fn name(&self) -> &'static str {
            "test::never_detects"
        }
        fn authority(&self) -> Authority {
            Source::HelpText.authority()
        }
        fn detect(&self, _tool: &ResolvedTool) -> bool {
            false
        }
        fn extract_node(
            &self,
            _tool: &ResolvedTool,
            _path: &[String],
        ) -> Result<CommandNode, ExtractError> {
            unreachable!("must not be called when detect() is false")
        }
    }

    #[test]
    fn succeeds_when_one_tier_works() {
        let runner = Runner::new(vec![Box::new(AlwaysOk)]);
        let result = runner.extract_full("sometool");
        assert!(result.root.is_some());
        assert_eq!(result.tier_statuses.len(), 1);
        assert!(result.tier_statuses[0].detected);
        assert!(result.tier_statuses[0].error.is_none());
    }

    #[test]
    fn a_failing_tier_does_not_block_a_working_one() {
        let runner = Runner::new(vec![Box::new(AlwaysFails), Box::new(AlwaysOk)]);
        let result = runner.extract_full("sometool");
        assert!(result.root.is_some(), "one working tier should be enough");
        let failing = result
            .tier_statuses
            .iter()
            .find(|s| s.tier == "test::always_fails")
            .unwrap();
        assert!(failing.detected);
        assert!(failing.error.is_some());
    }

    #[test]
    fn errors_only_when_no_tier_produces_a_root() {
        let runner = Runner::new(vec![Box::new(AlwaysFails), Box::new(NeverDetects)]);
        let result = runner.extract_full("sometool");
        assert!(result.root.is_none());
    }

    #[test]
    fn undetected_tier_is_never_extracted() {
        // NeverDetects::extract_node unreachable!()s if called; this test
        // passing without panicking is the assertion.
        let runner = Runner::new(vec![Box::new(NeverDetects)]);
        let result = runner.extract_full("sometool");
        assert!(result.root.is_none());
        assert!(!result.tier_statuses[0].detected);
    }

    /// An incremental tier whose `extract_node` returns a node carrying a
    /// description, so tests can tell whether `fill_node` actually called
    /// it and merged the result.
    struct IncrementalWithDescription;
    impl ExtractionTier for IncrementalWithDescription {
        fn name(&self) -> &'static str {
            "test::incremental_with_description"
        }
        fn authority(&self) -> Authority {
            Source::HelpText.authority()
        }
        fn detect(&self, _tool: &ResolvedTool) -> bool {
            true
        }
        fn extract_node(
            &self,
            _tool: &ResolvedTool,
            path: &[String],
        ) -> Result<CommandNode, ExtractError> {
            let mut node = CommandNode::new(
                path.last().cloned().unwrap_or_default(),
                Provenance::single(Source::HelpText),
            );
            node.description = Some(mantui_core::Text::sanitize("filled in lazily"));
            Ok(node)
        }
    }

    #[test]
    fn fill_node_skips_non_incremental_tiers() {
        // AlwaysOk is non-incremental: it already contributed everything
        // at root extraction, so fill_node must not call it again.
        let runner = Runner::new(vec![Box::new(AlwaysOk)]);
        let resolved = resolve_tool("sometool");
        let existing = CommandNode::new("child", Provenance::single(Source::HelpText));
        let result = runner.fill_node(
            &resolved,
            &["sometool".to_string(), "child".to_string()],
            existing,
        );
        assert!(
            result.tier_statuses.is_empty(),
            "non-incremental tier must not be probed by fill_node"
        );
    }

    #[test]
    fn fill_node_merges_incremental_tier_output_with_existing() {
        let runner = Runner::new(vec![Box::new(IncrementalWithDescription)]);
        let resolved = resolve_tool("sometool");
        let mut existing = CommandNode::new("child", Provenance::single(Source::HelpText));
        existing.summary = Some(mantui_core::Text::sanitize("already known summary"));
        let result = runner.fill_node(
            &resolved,
            &["sometool".to_string(), "child".to_string()],
            existing,
        );
        // The pre-existing field survives...
        assert_eq!(
            result.node.summary.unwrap().as_str(),
            "already known summary"
        );
        // ...and the newly-filled field is present too.
        assert_eq!(
            result.node.description.unwrap().as_str(),
            "filled in lazily"
        );
        assert_eq!(result.tier_statuses.len(), 1);
        assert!(result.tier_statuses[0].detected);
    }

    #[test]
    fn fill_node_never_fails_even_if_every_tier_fails() {
        // existing is always a merge candidate, so the result is always
        // Some — a tier failing here must degrade to "unchanged," not to
        // "no result at all" (unlike extract_full, which can return
        // root: None when nothing contributes anything).
        let runner = Runner::new(vec![Box::new(AlwaysFails)]);
        let resolved = resolve_tool("sometool");
        let mut existing = CommandNode::new("child", Provenance::single(Source::HelpText));
        existing.summary = Some(mantui_core::Text::sanitize("kept"));
        let result = runner.fill_node(
            &resolved,
            &["sometool".to_string(), "child".to_string()],
            existing,
        );
        assert_eq!(result.node.summary.unwrap().as_str(), "kept");
        assert!(result.tier_statuses[0].error.is_some());
    }
}
