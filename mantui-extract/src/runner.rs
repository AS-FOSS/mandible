//! The extraction runner: drives every enabled tier for a tool and merges
//! their output (spec §5.2, §5.3).
//!
//! This batch's runner is intentionally the simple case described in spec
//! §5.2 step 1-2 plus the non-incremental fast path: it asks every
//! detecting tier for the root, and since the only tier wired up by default
//! in this batch (Tier A) is non-incremental, that single call already
//! returns the whole tree. Lazy per-node expansion on top of incremental
//! tiers (Tier B/C/E) is batch 2's job and slots in without changing this
//! trait or this struct's shape.

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
}
