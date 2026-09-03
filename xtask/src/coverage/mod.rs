//! The extraction coverage harness (spec §13.1): runs the full tiered
//! pipeline against every executable on `PATH` and emits a scoreboard, so a
//! parser change is checked fleet-wide rather than against one tool.
//!
//! Columns include a `framework` field (spec §7 Tier A′), a `verbatim`
//! status (spec §7 Tier B step 3), and a `--format markdown` mode the
//! framework-support CI workflow (spec §13.1a) consumes.

mod aggregate;
mod fingerprint;
mod render_markdown;
mod render_text;
mod score;

use aggregate::compute_aggregate;
pub use aggregate::{parse_aggregate_footer, Aggregate};
use render_markdown::render_markdown;
use render_text::{render_text, Row};
pub(crate) use render_text::{
    BUNDLE_COL_WIDTH, EXISTENCE_COL_WIDTH, FLAGS_COL_WIDTH, FRAMEWORK_COL_WIDTH, MAN_COL_WIDTH,
    MISATTR_COL_WIDTH, MS_COL_WIDTH, NODES_COL_WIDTH, PCT_COL_WIDTH, SUSPECT_COL_WIDTH,
    TIER_COL_WIDTH, TOOL_COL_WIDTH,
};
use score::score_one;

use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Output format for the rendered scoreboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ScoreFormat {
    /// Fixed-width plain text (the format checked into
    /// `coverage-scoreboard.txt`).
    Text,
    /// GitHub-flavored markdown, for `$GITHUB_STEP_SUMMARY` (spec
    /// §13.1a, batch 6 part 6).
    Markdown,
}

/// Keep every `total`-th tool starting at `index` — a stride, not a
/// contiguous block.
///
/// Contiguous slicing balances badly because expensive tools cluster
/// alphabetically: a machine with 23 `qemu-*-static` binaries (4 MB each,
/// and the artifact scanner reads deep into every one) puts them all in a
/// single chunk, which then takes longer than every other chunk combined.
/// A stride interleaves them, so each shard gets a comparable share of the
/// expensive ones and the slowest shard sets a much lower ceiling.
fn select_shard(tools: Vec<String>, index: usize, total: usize) -> Vec<String> {
    tools
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % total == index)
        .map(|(_, t)| t)
        .collect()
}

/// Enumerate unique executable names on `PATH`, run the full extraction
/// pipeline against each (in parallel — this is dozens to low thousands of
/// subprocess spawns and would otherwise take a very long time
/// sequentially), and return the scoreboard rows plus aggregate stats, in
/// tool-name order.
pub fn run(
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
) -> (String, Aggregate) {
    run_over(unique_executables_on_path(), shard, progress, format)
}

/// Same as [`run`], but over a caller-supplied tool list instead of
/// scanning `PATH`. Used by `--tools` to pin a fixed, reproducible set —
/// necessary for CI (spec §13.1's regression gate needs a tool inventory
/// that doesn't vary with the runner image) — and by tests.
pub fn run_over(
    mut tools: Vec<String>,
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
) -> (String, Aggregate) {
    tools.sort();
    tools.dedup();
    if let Some((index, total)) = shard {
        tools = select_shard(tools, index, total);
    }
    let mut rows: Vec<Row> = tools
        .par_iter()
        .map(|tool| {
            // Logged on both sides, flushed immediately, because the
            // *unmatched* line is the diagnosis. Several tools are in
            // flight at once, so "the last tool logged" is only ever a
            // shortlist — but a tool that started and never finished is
            // the one that took the process down. Start-only logging
            // narrowed three killed CI shards to two suspects each and
            // could not pick between them.
            if progress {
                use std::io::Write;
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err, "probe-start: {tool}");
                let _ = err.flush();
            }
            let row = score_one(tool);
            if progress {
                use std::io::Write;
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err, "probe-done:  {tool}");
                let _ = err.flush();
            }
            row
        })
        .collect();
    rows.sort_by(|a, b| a.tool.cmp(&b.tool));

    let aggregate = compute_aggregate(&rows);
    let table = match format {
        ScoreFormat::Text => render_text(&rows, &aggregate),
        ScoreFormat::Markdown => render_markdown(&rows, &aggregate),
    };
    (table, aggregate)
}

/// Every uniquely-named executable file found in a `PATH` directory,
/// deduplicated by basename (the first directory to have a given name
/// wins, matching normal `PATH` resolution order) and sorted.
///
/// `pub(crate)`, not `pub`: `crate::audit`'s `sample` subcommand needs the
/// same population this module's own `run` scans by default (spec's audit
/// brief: "a deterministic draw from the tools on `PATH`"), and re-walking
/// `PATH` a second, independent way would risk the two enumerations quietly
/// disagreeing about what "every tool" means.
pub(crate) fn unique_executables_on_path() -> Vec<String> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_executable_file(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            seen.entry(name.to_string()).or_insert(path);
        }
    }
    seen.into_keys().collect()
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
/// `describable` defaults to `flags` — most tests here aren't about the
/// synopsis-exclusion split itself, so every flag is describable unless
/// a test overrides `.describable` afterwards (same pattern as
/// `.verbatim`/`.man_shaped` below).
fn row(tool: &str, flags: usize, pct_flags_with_text: Option<f64>, status: &'static str) -> Row {
    Row {
        tool: tool.to_string(),
        tiers: "help".to_string(),
        framework: "—".to_string(),
        command_table_count: 0,
        nodes: 1,
        flags,
        describable: flags,
        pct_flags_with_text,
        ms: 1,
        suspicious_nodes: 0,
        verbatim: false,
        man_shaped: false,
        misattribution_suspect_count: 0,
        misattribution_column_aligned: false,
        misattribution_samples: Vec::new(),
        existence_fabrication_count: 0,
        existence_samples: Vec::new(),
        bundle_collapse_count: 0,
        bundle_destroyed_flags: 0,
        bundle_samples: Vec::new(),
        alternation_defect_count: 0,
        alternation_samples: Vec::new(),
        single_dash_split_count: 0,
        single_dash_samples: Vec::new(),
        repeated_char_misread_count: 0,
        repeated_char_samples: Vec::new(),
        wrapped_prose_count: 0,
        wrapped_prose_samples: Vec::new(),
        tail_operand_count: 0,
        tail_operand_samples: Vec::new(),
        ragged_command_count: 0,
        ragged_command_samples: Vec::new(),
        wrapped_command_count: 0,
        wrapped_command_samples: Vec::new(),
        status,
        fingerprint: fingerprint::ToolFingerprint::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_partition_the_tool_list_exactly_once_each() {
        let tools: Vec<String> = (0..20).map(|i| format!("tool{i:02}")).collect();
        let total = 4;
        let mut seen: Vec<String> = Vec::new();
        for index in 0..total {
            seen.extend(select_shard(tools.clone(), index, total));
        }
        seen.sort();
        // Every tool appears in exactly one shard: none dropped, none
        // counted twice. A sharded scoreboard that silently loses tools
        // would understate coverage without looking wrong.
        assert_eq!(seen, tools);
    }

    #[test]
    fn shards_are_a_stride_not_a_contiguous_block() {
        let tools: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
        assert_eq!(select_shard(tools, 0, 3), vec!["t0", "t3"]);
    }

    #[test]
    fn unique_executables_on_path_finds_something_real() {
        // `sh` is present on every POSIX system this test would run on;
        // this is a sanity check that PATH scanning works at all, not an
        // exhaustive test of the harness (that's what running it for real
        // and inspecting the checked-in scoreboard is for).
        let tools = unique_executables_on_path();
        assert!(tools.iter().any(|t| t == "sh"));
    }

    /// `run_over` (the `--tools` path CI uses) scans exactly the given
    /// list, deduplicated — not every executable on `PATH` — so the
    /// aggregate's `total` is deterministic regardless of what else
    /// happens to be installed on the machine running it.
    #[test]
    fn run_over_scans_exactly_the_given_tools() {
        let (table, aggregate) = run_over(
            vec![
                "sh".to_string(),
                "sh".to_string(), // duplicate, must be deduped
                "true".to_string(),
            ],
            None,
            false,
            ScoreFormat::Text,
        );
        assert_eq!(aggregate.total, 2);
        assert!(table.contains("sh"));
        assert!(table.contains("true"));
    }

    #[test]
    fn run_over_markdown_format_produces_a_table() {
        let (table, _aggregate) =
            run_over(vec!["sh".to_string()], None, false, ScoreFormat::Markdown);
        assert!(table.starts_with("| tool |"));
    }
}
