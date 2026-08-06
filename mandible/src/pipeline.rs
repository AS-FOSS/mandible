//! Wiring the extraction runner: the shared "get me this tool's tree" path
//! used by both `--doctor` and the TUI.
//!
//! **There is no on-disk cache** (spec §11). Revision 2's cache keyed on
//! binary identity plus a build-time source fingerprint, but no fingerprint
//! over the binary can catch `docker` gaining subcommands from a plugin, or
//! `git` gaining subcommands from `~/.gitconfig` aliases — both change
//! `--help` output without changing the binary at all. A cache that is
//! *usually* fresh is a cache that will be confidently wrong at some point,
//! and this project already shipped one staleness bug whose only symptom
//! was a correct fix appearing not to work. Lazy root-only extraction (spec
//! §5.2) is what makes removing the cache affordable: a cold launch only
//! ever extracts the root node, not the whole tree.

use mandible_core::CommandNode;
use mandible_extract::{Runner, TierStatus};
use std::time::{Duration, Instant};

/// The result of extracting one tool's tree.
pub struct LoadedTool {
    /// The tool name.
    pub tool: String,
    /// The merged tree, or `None` if no tier could extract anything.
    pub root: Option<CommandNode>,
    /// Per-tier outcome, for `--doctor` and the `?` overlay.
    pub tier_statuses: Vec<TierStatus>,
    /// Wall-clock time this call spent extracting.
    pub elapsed: Duration,
}

/// Extract `tool_name`'s tree, running the full extraction pipeline fresh
/// every time (spec §11: there is no cache to consult first).
pub fn load(tool_name: &str) -> LoadedTool {
    let runner = Runner::new(mandible_extract::default_tiers());
    let start = Instant::now();
    let result = runner.extract_full(tool_name);
    let elapsed = start.elapsed();

    LoadedTool {
        tool: tool_name.to_string(),
        root: result.root,
        tier_statuses: result.tier_statuses,
        elapsed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_extracts_a_real_tool() {
        let loaded = load("git");
        assert!(loaded.root.is_some());
    }

    #[test]
    fn load_reports_no_root_for_an_unresolvable_tool() {
        let loaded = load("definitely-not-a-real-tool-xyz-123");
        assert!(loaded.root.is_none());
    }
}
