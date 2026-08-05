//! The extraction coverage harness (spec §13.1): runs the full tiered
//! pipeline against every executable on `PATH` and emits a scoreboard.
//!
//! This is the artifact that makes "universal, no per-tool patches"
//! measurable rather than aspirational — without it, a parser change is
//! only ever checked against whichever one tool the author happened to be
//! looking at, and there's no way to see that fixing `tar` regressed `xz`.

use mantui_core::{is_command_name_shaped, CommandNode};
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
    /// Structure-sanity count (spec §13.1): descendant nodes whose name
    /// fails [`is_command_name_shaped`], plus descendant nodes with no
    /// flags, no children, and no summary. Non-zero means `status` is
    /// forced to `"suspicious"` regardless of `%described` — the whole
    /// point of this column is that `%described` alone cannot detect
    /// fabricated structure, since invented nodes *inflate* it ([M-10]).
    suspicious_nodes: usize,
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
    /// Tools with at least one structurally-suspicious node (spec §13.1):
    /// a name failing [`is_command_name_shaped`], or a node with no flags,
    /// no children, and no summary. Gated exactly like `no_tier_count` —
    /// [M-10] shipped as `ok` at `100% described` because `%described`
    /// alone can't see fabricated structure; this is the column that can.
    pub suspicious_count: usize,
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

    let suspicious_nodes = result.root.as_ref().map(structure_sanity).unwrap_or(0);

    let status = if result.root.is_none() {
        "no-tier"
    } else if suspicious_nodes > 0 {
        // Checked before %described on purpose: a tool that's tripping
        // the structure-sanity check is suspicious regardless of how
        // "described" its (possibly fabricated) flags look — [M-10]
        // reported `tar` as `ok` at `100% described` while 39 of its 40
        // nodes were invented, because the invented nodes inflated the
        // metric instead of depressing it.
        "suspicious"
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
        suspicious_nodes,
        status,
    }
}

/// Count descendant nodes (not the root itself — see below) that fail
/// either half of spec §13.1's structure-sanity check: a name that
/// doesn't look like a real command (`is_command_name_shaped`), or a node
/// with no flags, no children, and no summary at all (a node that exists
/// but carries nothing is exactly what a mis-parsed continuation line or
/// enum-value-turned-subcommand looks like).
///
/// The root is deliberately excluded from the name-shape half: it's the
/// literal executable name resolved from `PATH`, never something a tier
/// guessed at, and plenty of completely legitimate real-world binaries
/// (`NetworkManager`, `FileCheck-18`, `aarch64-linux-gnu-cpp-13`) fail
/// `^[a-z][a-z0-9_.-]*$` on casing or a leading digit alone. Counting
/// those would swamp the signal this column exists to carry — a metric
/// too noisy to trust is exactly as useless as one that's gameable.
fn structure_sanity(root: &CommandNode) -> usize {
    root.subcommands.iter().map(count_suspicious).sum()
}

fn count_suspicious(node: &CommandNode) -> usize {
    let bad_name = !is_command_name_shaped(&node.name);
    let empty = node.flags.is_empty() && node.subcommands.is_empty() && node.summary.is_none();
    let this_node = usize::from(bad_name || empty);
    this_node + node.subcommands.iter().map(count_suspicious).sum::<usize>()
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
    let suspicious_count = rows.iter().filter(|r| r.status == "suspicious").count();
    Aggregate {
        pct_described,
        no_tier_count,
        suspicious_count,
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
        "{:<16} {:<16} {:>7}{:>8}{:>13}{:>7}{:>8}  {}\n",
        "tool", "tier(s)", "nodes", "flags", "%described", "ms", "suspect", "status"
    ));
    for row in rows {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{:<16} {:<16} {:>7}{:>8}{:>13}{:>7}{:>8}  {}\n",
            row.tool,
            row.tiers,
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            row.status
        ));
    }
    out.push_str(&format!(
        "\n# aggregate: pct_described={:.2} no_tier_count={} suspicious_count={} total={}\n",
        aggregate.pct_described,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.total
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
    // Older scoreboards (pre structure-sanity column) have no
    // `suspicious_count` field at all; default to 0 rather than failing
    // to parse, so `--check` against a not-yet-regenerated baseline still
    // works for the two fields that did exist.
    let mut suspicious_count = 0usize;
    let mut total = None;
    for field in line.trim_start_matches("# aggregate:").split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "pct_described" => pct_described = value.parse::<f64>().ok(),
            "no_tier_count" => no_tier_count = value.parse::<usize>().ok(),
            "suspicious_count" => suspicious_count = value.parse::<usize>().ok()?,
            "total" => total = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(Aggregate {
        pct_described: pct_described?,
        no_tier_count: no_tier_count?,
        suspicious_count,
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
        let table = "tool  tier(s)\nfoo   carapace\n\n# aggregate: pct_described=42.50 no_tier_count=3 suspicious_count=2 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_described, 42.5);
        assert_eq!(agg.no_tier_count, 3);
        assert_eq!(agg.suspicious_count, 2);
        assert_eq!(agg.total, 10);
    }

    /// A scoreboard written before the structure-sanity column existed has
    /// no `suspicious_count` field at all — `--check` against it must
    /// still work (defaulting to 0) rather than treating the whole footer
    /// as unparseable.
    #[test]
    fn footer_without_suspicious_count_defaults_to_zero() {
        let table = "# aggregate: pct_described=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.suspicious_count, 0);
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

    fn row(tool: &str, flags: usize, pct_described: Option<f64>, status: &'static str) -> Row {
        Row {
            tool: tool.to_string(),
            tiers: "help".to_string(),
            nodes: 1,
            flags,
            pct_described,
            ms: 1,
            suspicious_nodes: 0,
            status,
        }
    }

    #[test]
    fn aggregate_weights_by_flag_count_not_per_tool_average() {
        let rows = vec![
            row("big", 100, Some(100.0), "ok"),
            row("small", 1, Some(0.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        // 100 described out of 101 total, not (100% + 0%)/2 = 50%.
        assert!((agg.pct_described - (100.0 / 101.0 * 100.0)).abs() < 0.01);
    }

    #[test]
    fn aggregate_counts_suspicious_status_separately_from_no_tier() {
        let rows = vec![
            row("clean", 10, Some(100.0), "ok"),
            row("phantom", 40, Some(100.0), "suspicious"),
            row("nothing", 0, None, "no-tier"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.suspicious_count, 1);
        assert_eq!(agg.no_tier_count, 1);
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

    fn leaf(name: &str) -> CommandNode {
        CommandNode::new(
            name,
            mantui_core::Provenance::single(mantui_core::Source::HelpText),
        )
    }

    /// The exact regression this column exists for: a tool whose
    /// subcommands are all fabricated fragments must not be structurally
    /// clean just because each fragment happens to have "a description"
    /// (its own trailing prose, in [M-10]'s case).
    #[test]
    fn structure_sanity_flags_fabricated_names() {
        let mut root = leaf("tar");
        let mut phantom = leaf("treat them as errors");
        phantom.summary = Some(mantui_core::Text::sanitize(
            "some trailing description text",
        ));
        root.subcommands.push(phantom);
        assert_eq!(structure_sanity(&root), 1);
    }

    /// A node that exists but carries nothing (no flags, no children, no
    /// summary) is exactly the shape a mis-parsed continuation line takes
    /// even when its *name* happens to pass the shape test (an enum value
    /// like `gnu` parses as a fine identifier; it's still not a command).
    #[test]
    fn structure_sanity_flags_empty_nodes_with_valid_names() {
        let mut root = leaf("tar");
        root.subcommands.push(leaf("gnu"));
        assert_eq!(structure_sanity(&root), 1);
    }

    #[test]
    fn structure_sanity_ignores_the_roots_own_name() {
        // Root names are real executable filenames from PATH, not
        // something a tier guessed — e.g. "NetworkManager" fails the
        // lowercase-start regex but is a completely real binary.
        let mut root = leaf("NetworkManager");
        let mut child = leaf("status");
        child.summary = Some(mantui_core::Text::sanitize("Show status"));
        root.subcommands.push(child);
        assert_eq!(structure_sanity(&root), 0);
    }

    #[test]
    fn structure_sanity_is_zero_for_a_clean_tree() {
        let mut root = leaf("git");
        let mut child = leaf("commit");
        child.summary = Some(mantui_core::Text::sanitize("Record changes"));
        root.subcommands.push(child);
        assert_eq!(structure_sanity(&root), 0);
    }
}
