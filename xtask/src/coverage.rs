//! The extraction coverage harness (spec §13.1): runs the full tiered
//! pipeline against every executable on `PATH` and emits a scoreboard.
//!
//! This is the artifact that makes "universal, no per-tool patches"
//! measurable rather than aspirational — without it, a parser change is
//! only ever checked against whichever one tool the author happened to be
//! looking at, and there's no way to see that fixing `tar` regressed `xz`.

use mantui_extract::{default_tiers, Runner};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

/// One tool's row in the scoreboard.
struct Row {
    tool: String,
    tiers: String,
    nodes: usize,
    flags: usize,
    /// `None` when there are no flags to compute a percentage over.
    pct_described: Option<f64>,
    ms: u128,
    status: &'static str,
}

/// Aggregate stats used for the regression gate (spec §13.1: "`%described`
/// aggregate and `no-tier` count may not worsen").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aggregate {
    /// Total flags with a description, across every tool, divided by total
    /// flags across every tool (not an average of per-tool percentages,
    /// so a handful of huge catalogs don't get diluted by many small
    /// no-flag tools).
    pub pct_described: f64,
    /// Tools for which no tier produced a root node at all.
    pub no_tier_count: usize,
    /// Total tools scanned.
    pub total: usize,
}

/// Enumerate unique executable names on `PATH`, run the full extraction
/// pipeline against each (in parallel — this is dozens to low thousands of
/// subprocess spawns and would otherwise take a very long time
/// sequentially), and return the scoreboard rows plus aggregate stats, in
/// tool-name order.
pub fn run() -> (String, Aggregate) {
    let tools = unique_executables_on_path();
    let runner = Runner::new(default_tiers());

    let mut rows: Vec<Row> = tools
        .par_iter()
        .map(|tool| score_one(&runner, tool))
        .collect();
    rows.sort_by(|a, b| a.tool.cmp(&b.tool));

    let aggregate = compute_aggregate(&rows);
    let table = render_table(&rows, aggregate);
    (table, aggregate)
}

fn score_one(runner: &Runner, tool: &str) -> Row {
    let start = Instant::now();
    let result = runner.extract_full(tool);
    let ms = start.elapsed().as_millis();

    let tiers: Vec<&str> = result
        .tier_statuses
        .iter()
        .filter(|s| s.detected && s.error.is_none())
        .map(|s| short_tier_name(s.tier))
        .collect();
    let tiers_label = if tiers.is_empty() {
        "—".to_string()
    } else {
        tiers.join("+")
    };

    let nodes = result.node_count();
    let flags = result.flag_count();
    let pct_described = if flags == 0 {
        None
    } else {
        Some(result.flag_description_ratio() * 100.0)
    };

    let status = if result.root.is_none() {
        "no-tier"
    } else if pct_described.map(|p| p < 50.0).unwrap_or(false) {
        "low-confidence"
    } else {
        "ok"
    };

    Row {
        tool: tool.to_string(),
        tiers: tiers_label,
        nodes,
        flags,
        pct_described,
        ms,
        status,
    }
}

/// Shorten a tier's internal name (e.g. `"known_specs::carapace"`) to the
/// spec's scoreboard vocabulary (`"carapace"`, `"help"`).
fn short_tier_name(name: &str) -> &str {
    match name {
        "known_specs::carapace" => "carapace",
        "help_text" => "help",
        other => other,
    }
}

fn compute_aggregate(rows: &[Row]) -> Aggregate {
    let total_flags: usize = rows.iter().map(|r| r.flags).sum();
    let described_flags: f64 = rows
        .iter()
        .map(|r| {
            r.pct_described
                .map(|p| p / 100.0 * r.flags as f64)
                .unwrap_or(0.0)
        })
        .sum();
    let pct_described = if total_flags == 0 {
        0.0
    } else {
        described_flags / total_flags as f64 * 100.0
    };
    let no_tier_count = rows.iter().filter(|r| r.status == "no-tier").count();
    Aggregate {
        pct_described,
        no_tier_count,
        total: rows.len(),
    }
}

fn render_table(rows: &[Row], aggregate: Aggregate) -> String {
    // Note the literal space after each `{:<16}`: `{:<N}` only pads to a
    // *minimum* width, it never truncates or inserts a separator — a tool
    // name of 16+ characters (there are some real ones, e.g. compiler
    // toolchain binaries with long triples) would otherwise run directly
    // into the next column with no gap at all.
    let mut out = String::new();
    out.push_str(&format!(
        "{:<16} {:<16} {:>7}{:>8}{:>13}{:>7}  {}\n",
        "tool", "tier(s)", "nodes", "flags", "%described", "ms", "status"
    ));
    for row in rows {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{:<16} {:<16} {:>7}{:>8}{:>13}{:>7}  {}\n",
            row.tool, row.tiers, row.nodes, row.flags, pct, row.ms, row.status
        ));
    }
    out.push_str(&format!(
        "\n# aggregate: pct_described={:.2} no_tier_count={} total={}\n",
        aggregate.pct_described, aggregate.no_tier_count, aggregate.total
    ));
    out
}

/// Parse the `# aggregate: ...` footer line this module writes, so
/// `--check` can compare against a prior run without re-parsing the whole
/// table.
pub fn parse_aggregate_footer(scoreboard: &str) -> Option<Aggregate> {
    let line = scoreboard.lines().find(|l| l.starts_with("# aggregate:"))?;
    let mut pct_described = None;
    let mut no_tier_count = None;
    let mut total = None;
    for field in line.trim_start_matches("# aggregate:").split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "pct_described" => pct_described = value.parse::<f64>().ok(),
            "no_tier_count" => no_tier_count = value.parse::<usize>().ok(),
            "total" => total = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(Aggregate {
        pct_described: pct_described?,
        no_tier_count: no_tier_count?,
        total: total?,
    })
}

/// Every uniquely-named executable file found in a `PATH` directory,
/// deduplicated by basename (the first directory to have a given name
/// wins, matching normal `PATH` resolution order) and sorted.
fn unique_executables_on_path() -> Vec<String> {
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
mod tests {
    use super::*;

    #[test]
    fn parses_its_own_footer_format() {
        let table = "tool  tier(s)\nfoo   carapace\n\n# aggregate: pct_described=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_described, 42.5);
        assert_eq!(agg.no_tier_count, 3);
        assert_eq!(agg.total, 10);
    }

    #[test]
    fn missing_footer_returns_none() {
        assert!(parse_aggregate_footer("no footer here\n").is_none());
    }

    #[test]
    fn short_tier_name_maps_known_names() {
        assert_eq!(short_tier_name("known_specs::carapace"), "carapace");
        assert_eq!(short_tier_name("help_text"), "help");
        assert_eq!(short_tier_name("something_else"), "something_else");
    }

    #[test]
    fn aggregate_weights_by_flag_count_not_per_tool_average() {
        let rows = vec![
            Row {
                tool: "big".to_string(),
                tiers: "carapace".to_string(),
                nodes: 1,
                flags: 100,
                pct_described: Some(100.0),
                ms: 1,
                status: "ok",
            },
            Row {
                tool: "small".to_string(),
                tiers: "help".to_string(),
                nodes: 1,
                flags: 1,
                pct_described: Some(0.0),
                ms: 1,
                status: "ok",
            },
        ];
        let agg = compute_aggregate(&rows);
        // 100 described out of 101 total, not (100% + 0%)/2 = 50%.
        assert!((agg.pct_described - (100.0 / 101.0 * 100.0)).abs() < 0.01);
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
}
