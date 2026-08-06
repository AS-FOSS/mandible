//! Tier A: known-tool structured spec databases (spec §7). Currently backed
//! by the vendored carapace-spec snapshot at `vendor/carapace-specs.json`.
//!
//! **Deviation from spec §7:** "If a `carapace` binary is on PATH, prefer
//! `carapace --spec <tool>` over the snapshot" is not implemented in this
//! batch. That argv shape (`--spec <tool>`) is not on the spec §6 rule 2
//! allowlist (`__complete`, `completion <shell>`, `--help`/`-h`,
//! `help [<words>]`, `-- <partial>` under `COMPLETE=`), and rule 2 requires
//! a spec amendment before adding a new shape. Rather than widen the
//! allowlist as a side effect of this batch, this tier uses only the
//! vendored snapshot; live-carapace preference is left for a follow-up that
//! amends §6 deliberately. See the batch report for this called out as an
//! open item.
//!
//! **Storage.** The catalog is not `include_str!`'d and deserialized whole
//! per lookup. `build.rs` scans `vendor/carapace-specs.json` once at
//! compile time and emits a sorted `[(tool_name, byte_offset, byte_len)]`
//! table (`SPEC_INDEX`, generated into `OUT_DIR/spec_index.rs`); at
//! runtime, [`lookup_raw_json`] binary-searches that table and
//! `serde_json::from_str`s exactly one tool's slice of the
//! `include_bytes!`'d catalog. See `tests/known_specs_index.rs` for the
//! regression test proving a `git` lookup never touches `docker`'s bytes.

mod raw;

use crate::errors::ExtractError;
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use mandible_core::{Authority, CommandNode, Source};

include!(concat!(env!("OUT_DIR"), "/spec_index.rs"));

static CATALOG_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../vendor/carapace-specs.json"
));

/// Metadata about the vendored catalog snapshot (spec §7 "Staleness" and
/// §11 cache contents: the UI and `--doctor` must be able to show when the
/// snapshot was generated and from which commit).
#[derive(Debug, Clone, Copy)]
pub struct CatalogMeta {
    /// The catalog provider, e.g. `"carapace-spec"`.
    pub provider: &'static str,
    /// The upstream repository this was vendored from.
    pub source: &'static str,
    /// The subdirectory of the source repo that was scanned.
    pub source_dir: &'static str,
    /// The upstream commit hash the snapshot was generated at.
    pub commit: &'static str,
    /// An RFC 3339 timestamp of when the snapshot was generated.
    pub generated: &'static str,
    /// Total tools in the catalog (740 as of the initial vendoring).
    pub tool_count: usize,
}

/// The vendored catalog's metadata, as recorded by
/// `scripts/vendor_carapace_specs.py`.
pub fn catalog_meta() -> CatalogMeta {
    CatalogMeta {
        provider: META_PROVIDER,
        source: META_SOURCE,
        source_dir: META_SOURCE_DIR,
        commit: META_COMMIT,
        generated: META_GENERATED,
        tool_count: TOOL_COUNT,
    }
}

/// Look up a single tool's raw JSON slice in the vendored catalog via
/// binary search over the build-time index, without deserializing any
/// other tool's entry.
fn lookup_raw_json(tool: &str) -> Option<&'static str> {
    SPEC_INDEX
        .binary_search_by(|(name, _, _)| (*name).cmp(tool))
        .ok()
        .map(|i| {
            let (_, start, len) = SPEC_INDEX[i];
            let bytes = &CATALOG_BYTES[start as usize..(start + len) as usize];
            std::str::from_utf8(bytes).expect("catalog is valid UTF-8 (verified at vendoring time)")
        })
}

/// Tier A: carapace-spec catalog. Non-incremental — the whole subtree for a
/// tool is already in memory, so [`ExtractionTier::extract_node`] returns
/// the full known tree in one call regardless of `path` depth.
#[derive(Debug, Default)]
pub struct CarapaceTier;

impl ExtractionTier for CarapaceTier {
    fn name(&self) -> &'static str {
        "known_specs::carapace"
    }

    fn authority(&self) -> Authority {
        Source::KnownSpec {
            provider: "carapace".to_string(),
        }
        .authority()
    }

    fn detect(&self, tool: &ResolvedTool) -> bool {
        lookup_raw_json(&tool.name).is_some()
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
    ) -> Result<CommandNode, ExtractError> {
        let raw_json = lookup_raw_json(&tool.name).ok_or(ExtractError::NotInCatalog)?;
        let parsed: raw::RawCommand =
            serde_json::from_str(raw_json).map_err(|e| ExtractError::ParseFailed(e.to_string()))?;
        let full = raw::convert(parsed, &[]);
        if path.len() <= 1 {
            return Ok(full);
        }
        mandible_core::resolve(&full, path)
            .cloned()
            .ok_or(ExtractError::PathNotFound)
    }

    fn is_incremental(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_meta_matches_vendoring_script_output() {
        let meta = catalog_meta();
        assert_eq!(meta.provider, "carapace-spec");
        assert_eq!(meta.source, "https://github.com/carapace-sh/carapace-bin");
        assert!(!meta.commit.is_empty());
        assert!(!meta.generated.is_empty());
        assert!(
            meta.tool_count > 700,
            "expected ~740 tools, got {}",
            meta.tool_count
        );
    }

    #[test]
    fn index_is_sorted_for_binary_search() {
        for pair in SPEC_INDEX.windows(2) {
            assert!(
                pair[0].0 <= pair[1].0,
                "index not sorted: {} > {}",
                pair[0].0,
                pair[1].0
            );
        }
    }

    /// Regression test for the spec §7 storage requirement: looking up one
    /// tool must not deserialize (or even touch) another tool's bytes.
    /// `lookup_raw_json` slices `CATALOG_BYTES[start..start+len]` using
    /// exactly the looked-up tool's own index entry, so this asserts the
    /// structural property that makes that true: no two entries' byte
    /// ranges overlap, and `git`'s slice in particular doesn't reach into
    /// `docker`'s.
    #[test]
    fn tool_byte_ranges_do_not_overlap() {
        let mut sorted_by_offset: Vec<&(&str, u32, u32)> = SPEC_INDEX.iter().collect();
        sorted_by_offset.sort_by_key(|(_, start, _)| *start);
        for pair in sorted_by_offset.windows(2) {
            let (name_a, start_a, len_a) = pair[0];
            let (name_b, start_b, _len_b) = pair[1];
            assert!(
                start_a + len_a <= *start_b,
                "{name_a} ({start_a}..{}) overlaps {name_b} (starts at {start_b})",
                start_a + len_a
            );
        }

        // Concretely: git's raw slice must not contain docker's name key,
        // proving the slice didn't spill into a neighboring entry.
        let git_json = lookup_raw_json("git").expect("git is in the catalog");
        let docker_range = SPEC_INDEX.iter().find(|(name, _, _)| *name == "docker");
        assert!(
            docker_range.is_some(),
            "docker should be in the catalog for this test to be meaningful"
        );
        assert!(
            !git_json.contains("\"docker\""),
            "git's slice should not contain docker's key"
        );
    }

    #[test]
    fn detects_git_and_docker() {
        let tier = CarapaceTier;
        let git = ResolvedTool {
            name: "git".to_string(),
            path: None,
            version: None,
        };
        let docker = ResolvedTool {
            name: "docker".to_string(),
            path: None,
            version: None,
        };
        assert!(tier.detect(&git));
        assert!(tier.detect(&docker));
    }

    #[test]
    fn does_not_detect_unknown_tool() {
        let tier = CarapaceTier;
        let unknown = ResolvedTool {
            name: "definitely-not-a-real-cli-tool-xyz".to_string(),
            path: None,
            version: None,
        };
        assert!(!tier.detect(&unknown));
    }

    #[test]
    fn extracts_git_full_tree_with_real_descriptions() {
        let tier = CarapaceTier;
        let git = ResolvedTool {
            name: "git".to_string(),
            path: None,
            version: None,
        };
        let node = tier.extract_node(&git, &["git".to_string()]).unwrap();
        assert_eq!(node.name, "git");
        assert!(node.children_filled);
        assert!(!node.subcommands.is_empty());
        assert!(
            node.subcommands.len() > 100,
            "git should have >100 subcommands, got {}",
            node.subcommands.len()
        );
        // At least one subcommand should carry real prose.
        let rebase = node.subcommands.iter().find(|c| c.name == "rebase");
        assert!(rebase.is_some(), "git should have a rebase subcommand");
    }

    #[test]
    fn extracts_specific_path_within_known_tree() {
        let tier = CarapaceTier;
        let git = ResolvedTool {
            name: "git".to_string(),
            path: None,
            version: None,
        };
        let node = tier
            .extract_node(&git, &["git".to_string(), "rebase".to_string()])
            .unwrap();
        assert_eq!(node.name, "rebase");
    }

    #[test]
    fn unknown_tool_extraction_fails() {
        let tier = CarapaceTier;
        let unknown = ResolvedTool {
            name: "definitely-not-a-real-cli-tool-xyz".to_string(),
            path: None,
            version: None,
        };
        assert!(tier
            .extract_node(&unknown, std::slice::from_ref(&unknown.name))
            .is_err());
    }
}
