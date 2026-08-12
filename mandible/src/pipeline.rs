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

use mandible_extract::Runner;

/// The result of extracting one tool's tree. An alias, not a distinct
/// struct: `LoadedTool` used to duplicate [`mandible_extract::ExtractionResult`]
/// field-for-field (`tool`, `root`, `tier_statuses`, `elapsed`), which is
/// exactly the shape of drift that let `--doctor` compute `%described`
/// from a hand-rolled tree walk instead of the runner's own
/// `flag_description_ratio` — a second copy of the same data invites a
/// second, divergent copy of the arithmetic over it. Callers (`--doctor`,
/// the TUI's background warmer) get the counting accessors
/// (`flag_count`, `describable_flag_count`, `flag_description_ratio`) for
/// free this way, and there is no longer a seam where the two could say
/// different things about the same extraction.
pub type LoadedTool = mandible_extract::ExtractionResult;

/// Extract `tool_name`'s tree, running the full extraction pipeline fresh
/// every time (spec §11: there is no cache to consult first).
pub fn load(tool_name: &str) -> LoadedTool {
    let runner = Runner::new(mandible_extract::default_tiers());
    runner.extract_full(tool_name)
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
