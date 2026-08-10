//! The extraction coverage harness (spec §13.1): runs the full tiered
//! pipeline against every executable on `PATH` and emits a scoreboard.
//!
//! This is the artifact that makes "universal, no per-tool patches"
//! measurable rather than aspirational — without it, a parser change is
//! only ever checked against whichever one tool the author happened to be
//! looking at, and there's no way to see that fixing `tar` regressed `xz`.
//!
//! Batch 6 part 5 adds a `framework` column (spec §7 Tier A′) and a
//! `verbatim` status (spec §7 Tier B step 3) on top of the existing
//! scoreboard, plus a `--format markdown` mode the framework-support CI
//! workflow (batch 6 part 6, spec §13.1a) consumes.

use mandible_extract::{default_tiers, resolve_tool, ExtractionResult, Runner};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

/// Fixed display width for the `tool` column in [`ScoreFormat::Text`]
/// output. Truncated (never just left unbounded) because real tool names
/// on a real `PATH` blow past any reasonable assumption —
/// `aarch64-linux-gnu-cpp-13` (24 chars), `UnicodeNameMappingGenerator-18`
/// (31 chars) — and an untruncated long name shoves every column after it
/// out of alignment for that one row, which is exactly the bug this
/// constant (and [`truncate_col`]) exists to fix.
const TOOL_COL_WIDTH: usize = 24;
/// Fixed display width for the `tier(s)` column, same reasoning.
const TIER_COL_WIDTH: usize = 18;
/// Fixed display width for the new `framework` column, same reasoning.
const FRAMEWORK_COL_WIDTH: usize = 26;

/// One tool's row in the scoreboard.
struct Row {
    tool: String,
    tiers: String,
    /// The detected framework (spec §7 Tier A′) plus how it was detected,
    /// e.g. `"clap (v3/v4) (artifact)"`, or `"—"` when unidentified. See
    /// [`framework_label`].
    framework: String,
    nodes: usize,
    flags: usize,
    /// `None` when there are no flags to compute a percentage over.
    pct_described: Option<f64>,
    ms: u128,
    /// Structure-sanity count (spec §13.1): descendant nodes whose name
    /// fails [`mandible_core::is_command_name_shaped`], plus descendant nodes with no
    /// flags, no children, and no summary. Non-zero means `status` is
    /// forced to `"suspicious"` regardless of `%described` — the whole
    /// point of this column is that `%described` alone cannot detect
    /// fabricated structure, since invented nodes *inflate* it ([M-10]).
    suspicious_nodes: usize,
    /// True when the root node degraded to spec §7 Tier B step 3's
    /// verbatim rendering (`CommandNode::unparsed` non-empty) rather than
    /// producing any structure at all.
    verbatim: bool,
    status: &'static str,
}

/// Aggregate stats. `pct_described`, `no_tier_count`, and
/// `suspicious_count` are the regression gate (spec §13.1: "may not
/// worsen"); `verbatim_count`, `framework_detected_count`, and
/// `framework_counts` are reported for visibility but deliberately **not**
/// part of that gate — see [`compute_aggregate`]'s doc comment on why.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    /// Total flags with a description, across every tool, divided by total
    /// flags across every tool (not an average of per-tool percentages,
    /// so a handful of huge catalogs don't get diluted by many small
    /// no-flag tools).
    pub pct_described: f64,
    /// Tools for which no tier produced a root node at all.
    pub no_tier_count: usize,
    /// Tools with at least one structurally-suspicious node (spec §13.1):
    /// a name failing [`mandible_core::is_command_name_shaped`], or a node with no flags,
    /// no children, and no summary. Gated exactly like `no_tier_count` —
    /// [M-10] shipped as `ok` at `100% described` because `%described`
    /// alone can't see fabricated structure; this is the column that can.
    pub suspicious_count: usize,
    /// Tools whose root degraded to verbatim (spec §7 Tier B step 3).
    /// **Not gated** — see [`compute_aggregate`].
    pub verbatim_count: usize,
    /// Tools for which Tier A′ identified a framework at all (spec §7
    /// Tier A′), regardless of method.
    pub framework_detected_count: usize,
    /// Per-framework tool counts (the framework's `Framework::name()`,
    /// without the detection-method suffix `[`framework_label`] adds to
    /// the per-row column), sorted by name for a stable, diffable
    /// scoreboard file.
    pub framework_counts: BTreeMap<String, usize>,
    /// Total tools scanned.
    pub total: usize,
    /// Raw numerator/denominator behind `pct_described`, carried in the
    /// footer so a scoreboard produced in *shards* can be merged exactly.
    /// Recomputing the aggregate from the per-row `%described` column
    /// cannot be exact — that column is rounded to whole percent — and a
    /// gated regression baseline must not be approximate. A full-PATH
    /// sweep is long enough to be worth running in shards, and CI's PATH
    /// sweep will want the same.
    pub described_flags: f64,
    /// Denominator for [`Self::described_flags`].
    pub total_flags: usize,
}

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
    let runner = Runner::new(default_tiers());

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
            let row = score_one(&runner, tool);
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

    let framework = framework_label(tool, &result);
    let nodes = result.node_count();
    let flags = result.flag_count();

    // Status derivation (structure-sanity count, verbatim flag,
    // %described, and the final label) is computed once in `status.rs`
    // and shared verbatim with the corpus runner — see that module's doc
    // comment for why an independent second definition here would be a
    // drift risk, not a convenience.
    let status = crate::status::compute(&result);

    Row {
        tool: tool.to_string(),
        tiers: tiers_label,
        framework,
        nodes,
        flags,
        pct_described: status.pct_described,
        ms,
        suspicious_nodes: status.suspicious_nodes,
        verbatim: status.verbatim,
        status: status.label,
    }
}

/// Compact `"<framework name> (<method>)"` label for the scoreboard's
/// `framework` column, or `"—"` when Tier A′ didn't identify one (spec §7
/// Tier A′ step 3, "Unidentified"). The framework name itself comes from
/// `CommandNode::detected_framework` on the merged root (set only by Tier
/// B — see `help_text::build_node`'s doc comment — so this is accurate
/// even when a higher-structural-authority tier like native won the rest
/// of the merge, since per-field authority resolution never lets a `None`
/// contributor displace a `Some` one).
///
/// The *method* (artifact vs. help-text signature) isn't itself carried on
/// `CommandNode` (spec §4.2 keeps `Source`/`Provenance` framework-agnostic
/// on purpose), so this re-derives it with one extra call to
/// `framework::identify_from_artifact` — which never spawns a process and
/// is memoized per binary path (see that function's own doc comment), so
/// this costs nothing beyond what `extract_full` already paid for and
/// never double-probes the tool.
fn framework_label(tool: &str, result: &ExtractionResult) -> String {
    let Some(name) = result
        .root
        .as_ref()
        .and_then(|r| r.detected_framework.clone())
    else {
        return "—".to_string();
    };
    let resolved = resolve_tool(tool);
    let method = if mandible_extract::framework::identify_from_artifact(&resolved)
        .is_some_and(|f| f.name() == name)
    {
        "artifact"
    } else {
        "help-text"
    };
    format!("{name} ({method})")
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

/// Compute aggregate stats over `rows`.
///
/// **`verbatim_count` is reported but deliberately not part of the
/// regression gate** (spec §13.1's `--check`, wired in `xtask/src/main.rs`):
/// unlike `no_tier_count` and `suspicious_count`, a *growing* `verbatim`
/// count is not on its own evidence of a regression. A correct new
/// framework grammar can legitimately move a tool from fabricated
/// structure (`suspicious`, or a low-confidence guess reported as `ok`) to
/// honest verbatim — that is exactly spec §7 Tier B step 3's intended
/// behavior, "never fabricate, degrade to verbatim" — and gating on
/// `verbatim` growing would block precisely the fix this whole batch is
/// about. `framework_detected_count`/`framework_counts` are reported for
/// the same reason: identifying *more* frameworks over time is progress,
/// never a regression to block on.
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
    let verbatim_count = rows.iter().filter(|r| r.verbatim).count();

    let mut framework_counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        if let Some(name) = framework_name_only(&row.framework) {
            *framework_counts.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    let framework_detected_count: usize = framework_counts.values().sum();

    Aggregate {
        pct_described,
        no_tier_count,
        suspicious_count,
        verbatim_count,
        framework_detected_count,
        framework_counts,
        total: rows.len(),
        described_flags,
        total_flags,
    }
}

/// Strip a row's `"<name> (<method>)"` framework label back down to just
/// the name, for aggregation; `None` for the unidentified sentinel `"—"`.
fn framework_name_only(label: &str) -> Option<&str> {
    if label == "—" {
        return None;
    }
    label.rsplit_once(" (").map(|(name, _)| name)
}

/// Truncate `s` to at most `width` characters, replacing the tail with a
/// single `…` marker when it doesn't fit. Character count, not
/// `unicode-width` — unlike `mandible-tui`'s rendering (which the
/// project's own invariants require display-width-safe truncation for,
/// since it draws into fixed terminal cells the user is actually looking
/// at), this is a plain-text developer report over tool names that are
/// overwhelmingly ASCII, so the extra dependency isn't justified here.
fn truncate_col(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let keep = width.saturating_sub(1);
    let mut truncated: String = chars[..keep].iter().collect();
    truncated.push('…');
    truncated
}

fn render_text(rows: &[Row], aggregate: &Aggregate) -> String {
    // Note the fixed `{:<N}` widths on the *truncated* string, not the
    // raw one: `{:<N}` only ever pads to a minimum width, it never
    // truncates — feeding it an untruncated long tool/tier/framework name
    // is exactly what let one long name shove every later column out of
    // alignment for that row.
    let mut out = String::new();
    out.push_str(&format!(
        "{:<tw$} {:<iw$} {:<fw$} {:>7}{:>8}{:>13}{:>7}{:>8}  {}\n",
        "tool",
        "tier(s)",
        "framework",
        "nodes",
        "flags",
        "%described",
        "ms",
        "suspect",
        "status",
        tw = TOOL_COL_WIDTH,
        iw = TIER_COL_WIDTH,
        fw = FRAMEWORK_COL_WIDTH,
    ));
    for row in rows {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{:<tw$} {:<iw$} {:<fw$} {:>7}{:>8}{:>13}{:>7}{:>8}  {}\n",
            truncate_col(&row.tool, TOOL_COL_WIDTH),
            truncate_col(&row.tiers, TIER_COL_WIDTH),
            truncate_col(&row.framework, FRAMEWORK_COL_WIDTH),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            row.status,
            tw = TOOL_COL_WIDTH,
            iw = TIER_COL_WIDTH,
            fw = FRAMEWORK_COL_WIDTH,
        ));
    }
    out.push_str(&aggregate_footer_line(aggregate));
    out.push('\n');
    out.push_str(&framework_summary_lines(aggregate));
    out.push_str(&worst_parsed_lines_text(&worst_parsed(rows)));
    out
}

/// Cap on the worst-parsed audit section. Not load-bearing (this is a
/// work-queue aid, not a gated metric); 25 keeps the footer scannable
/// rather than dumping every imperfect tool on a full-`PATH` sweep.
const WORST_PARSED_LIMIT: usize = 25;

/// How many of a tool's flags the grammar failed to find a description
/// for. The ranking key below.
fn undescribed_flags(row: &Row) -> usize {
    match row.pct_described {
        Some(pct) => {
            let described = (row.flags as f64) * (pct / 100.0);
            row.flags.saturating_sub(described.round() as usize)
        }
        // No flags at all, so nothing was missed.
        None => 0,
    }
}

/// The tools this harness parsed worst, ranked by how many flag
/// descriptions went missing, capped to [`WORST_PARSED_LIMIT`].
///
/// This section used to rank *unidentified* tools by flag count, on the
/// theory that rich help text with no framework behind it was the best
/// candidate for a new fingerprint. Measurement killed that theory: across
/// a real `PATH`, unidentified tools average ~92% described and identified
/// ones ~90%. Detection is not what separates a good result from a bad
/// one, so a list of undetected tools is not a work queue, and acting on it
/// would mean adding fingerprints that raise the detection rate without
/// parsing anything better (spec §7's note on why that is worse than
/// leaving the number alone).
///
/// What does separate them is how much of a tool the grammar actually
/// understood. Ranking by *undescribed flags* rather than by percentage
/// alone keeps the list actionable: a tool with 150 flags at 60% has more
/// missing documentation behind it than one with 3 flags at 0%, and is a
/// better use of the next hour. Ties broken by tool name for a stable,
/// diffable scoreboard.
fn worst_parsed(rows: &[Row]) -> Vec<&Row> {
    let mut worst: Vec<&Row> = rows.iter().filter(|r| undescribed_flags(r) > 0).collect();
    worst.sort_by(|a, b| {
        undescribed_flags(b)
            .cmp(&undescribed_flags(a))
            .then_with(|| a.tool.cmp(&b.tool))
    });
    worst.truncate(WORST_PARSED_LIMIT);
    worst
}

/// Plain-text rendering of [`worst_parsed`]'s result, as
/// `#`-prefixed lines matching this module's other informational footer
/// sections (`framework_summary_lines`) — reported for visibility, not
/// re-parsed by `--check`, so the exact format isn't load-bearing.
fn worst_parsed_lines_text(worst: &[&Row]) -> String {
    if worst.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("# worst-parsed (most missing flag descriptions — the real work queue):\n");
    for (rank, row) in worst.iter().enumerate() {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "#   {:>2}. {:<30} {:>5} of {:>5} flags undescribed ({:>4}) {}\n",
            rank + 1,
            row.tool,
            undescribed_flags(row),
            row.flags,
            pct,
            row.framework,
        ));
    }
    out
}

/// Markdown rendering of [`worst_parsed`]'s result, for
/// [`render_markdown`].
fn worst_parsed_section_markdown(worst: &[&Row]) -> String {
    if worst.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Worst-parsed tools** (most missing flag descriptions, which is where grammar work pays off):\n\n| tool | undescribed | flags | %described | framework |\n|---|---|---|---|---|\n",
    );
    for row in worst {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            undescribed_flags(row),
            row.flags,
            pct,
            md_escape(&row.framework),
        ));
    }
    out
}

/// GitHub-flavored markdown table plus the same aggregate footer,
/// rendered as prose — spec §13.1a's framework-support workflow (batch 6
/// part 6) writes this straight to `$GITHUB_STEP_SUMMARY`, which GitHub
/// renders as markdown in the run's summary UI.
fn render_markdown(rows: &[Row], aggregate: &Aggregate) -> String {
    let mut out = String::new();
    out.push_str(
        "| tool | tier(s) | framework | nodes | flags | %described | ms | suspect | status |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for row in rows {
        let pct = row
            .pct_described
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            md_escape(&row.tiers),
            md_escape(&row.framework),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            row.status,
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "**Aggregate:** {:.2}% described across {} tools, {} no-tier, {} suspicious, {} verbatim.\n\n",
        aggregate.pct_described,
        aggregate.total,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
    ));
    out.push_str(&format!(
        "**Framework detection:** {}/{} tools ({:.1}%).\n",
        aggregate.framework_detected_count,
        aggregate.total,
        detection_rate_pct(aggregate),
    ));
    if !aggregate.framework_counts.is_empty() {
        out.push_str("\n**Per-framework counts:**\n\n");
        for (name, count) in &aggregate.framework_counts {
            out.push_str(&format!("- {}: {count}\n", md_escape(name)));
        }
    }
    out.push_str(&worst_parsed_section_markdown(&worst_parsed(rows)));
    // The same machine-readable footer the text format carries, wrapped in
    // an HTML comment so it stays invisible when rendered but parseable by
    // whatever recombines shards. Without it a sharded markdown run could
    // only be merged by re-deriving totals from the rounded per-row
    // %described column, which is exactly the approximation
    // `described_flags`/`total_flags` exist to avoid.
    out.push_str("\n<!-- ");
    out.push_str(aggregate_footer_line(aggregate).trim_end());
    out.push_str(" -->\n");
    out
}

/// Escape the one character (`|`) that would otherwise break a GFM table
/// cell. Tool names and framework labels are the only free-form content
/// here; a `|` in either is exotic but not impossible on a real `PATH`.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

fn detection_rate_pct(aggregate: &Aggregate) -> f64 {
    if aggregate.total == 0 {
        0.0
    } else {
        aggregate.framework_detected_count as f64 / aggregate.total as f64 * 100.0
    }
}

/// The single `# aggregate: ...` line every format carries — this is the
/// only line `parse_aggregate_footer` needs to understand, so it's kept
/// identical (modulo the new `verbatim_count` field) across text and
/// markdown output on purpose, even though markdown output isn't itself
/// meant to be re-parsed by `--check` (that always reads the plain-text
/// `coverage-scoreboard.txt`).
fn aggregate_footer_line(aggregate: &Aggregate) -> String {
    format!(
        "# aggregate: pct_described={:.2} no_tier_count={} suspicious_count={} verbatim_count={} total={} described_flags={:.4} total_flags={}\n",
        aggregate.pct_described,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
        aggregate.total,
        aggregate.described_flags,
        aggregate.total_flags,
    )
}

/// Human-readable (not re-parsed) framework-detection summary: total
/// detection rate plus per-framework counts, sorted by name for a stable
/// diff.
fn framework_summary_lines(aggregate: &Aggregate) -> String {
    let mut out = format!(
        "# framework-detection: {}/{} tools ({:.1}%)\n",
        aggregate.framework_detected_count,
        aggregate.total,
        detection_rate_pct(aggregate),
    );
    if !aggregate.framework_counts.is_empty() {
        let counts: Vec<String> = aggregate
            .framework_counts
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        out.push_str(&format!("# framework-counts: {}\n", counts.join(", ")));
    }
    out
}

/// Parse the `# aggregate: ...` footer line this module writes, so
/// `--check` can compare against a prior run without re-parsing the whole
/// table. Only reads the single-line `key=value` aggregate footer —
/// `framework-detection`/`framework-counts` are informational only (see
/// [`framework_summary_lines`]) and never gated, so they don't need to
/// round-trip through this parser.
pub fn parse_aggregate_footer(scoreboard: &str) -> Option<Aggregate> {
    let line = scoreboard.lines().find(|l| l.starts_with("# aggregate:"))?;
    let mut pct_described = None;
    let mut no_tier_count = None;
    // Older scoreboards (pre structure-sanity / pre-framework columns)
    // are missing `suspicious_count`/`verbatim_count` entirely; default
    // both to 0 rather than failing to parse, so `--check` against a
    // not-yet-regenerated baseline still works for the fields that did
    // exist.
    let mut suspicious_count = 0usize;
    let mut verbatim_count = 0usize;
    let mut described_flags = 0.0f64;
    let mut total_flags = 0usize;
    let mut total = None;
    for field in line.trim_start_matches("# aggregate:").split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "pct_described" => pct_described = value.parse::<f64>().ok(),
            "no_tier_count" => no_tier_count = value.parse::<usize>().ok(),
            "suspicious_count" => suspicious_count = value.parse::<usize>().ok()?,
            "verbatim_count" => verbatim_count = value.parse::<usize>().ok()?,
            "described_flags" => described_flags = value.parse::<f64>().ok()?,
            "total_flags" => total_flags = value.parse::<usize>().ok()?,
            "total" => total = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(Aggregate {
        pct_described: pct_described?,
        no_tier_count: no_tier_count?,
        suspicious_count,
        verbatim_count,
        framework_detected_count: 0,
        framework_counts: BTreeMap::new(),
        total: total?,
        described_flags,
        total_flags,
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
        let table = "tool  tier(s)\nfoo   carapace\n\n# aggregate: pct_described=42.50 no_tier_count=3 suspicious_count=2 verbatim_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_described, 42.5);
        assert_eq!(agg.no_tier_count, 3);
        assert_eq!(agg.suspicious_count, 2);
        assert_eq!(agg.verbatim_count, 1);
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

    /// Same for `verbatim_count`, added in batch 6 part 5: a scoreboard
    /// from before this batch has no such field.
    #[test]
    fn footer_without_verbatim_count_defaults_to_zero() {
        let table =
            "# aggregate: pct_described=42.50 no_tier_count=3 suspicious_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.verbatim_count, 0);
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
            framework: "—".to_string(),
            nodes: 1,
            flags,
            pct_described,
            ms: 1,
            suspicious_nodes: 0,
            verbatim: false,
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
    fn aggregate_counts_verbatim_separately_and_it_is_not_gated_by_construction() {
        let mut verbatim_row = row("mystery", 0, None, "verbatim");
        verbatim_row.verbatim = true;
        let rows = vec![row("clean", 10, Some(100.0), "ok"), verbatim_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.verbatim_count, 1);
        // `Aggregate` simply has no field a gate could accidentally key
        // on beyond the three documented ones; this test exists so a
        // future reader sees `verbatim_count` is computed and populated,
        // not forgotten — the *not gated* half is enforced by
        // `xtask/src/main.rs` never comparing it, covered by reading that
        // function, not a unit test over a private struct.
    }

    #[test]
    fn framework_counts_aggregate_by_name_ignoring_method() {
        let mut a = row("gh", 10, Some(90.0), "ok");
        a.framework = "cobra (artifact)".to_string();
        let mut b = row("docker", 20, Some(80.0), "ok");
        b.framework = "cobra (artifact)".to_string();
        let mut c = row("tar", 5, Some(70.0), "ok");
        c.framework = "GNU argp/getopt_long (help-text)".to_string();
        let mut d = row("weird", 0, None, "no-tier");
        d.framework = "—".to_string();
        let agg = compute_aggregate(&[a, b, c, d]);
        assert_eq!(agg.framework_counts.get("cobra"), Some(&2));
        assert_eq!(agg.framework_counts.get("GNU argp/getopt_long"), Some(&1));
        assert_eq!(agg.framework_detected_count, 3);
    }

    #[test]
    fn framework_name_only_strips_the_method_suffix() {
        assert_eq!(
            framework_name_only("clap (v3/v4) (artifact)"),
            Some("clap (v3/v4)")
        );
        assert_eq!(framework_name_only("—"), None);
    }

    #[test]
    fn truncate_col_pads_nothing_leaves_short_strings_alone() {
        assert_eq!(truncate_col("git", 24), "git");
    }

    #[test]
    fn truncate_col_shortens_long_names_with_an_ellipsis_marker() {
        let long = "UnicodeNameMappingGenerator-18";
        let truncated = truncate_col(long, 24);
        assert_eq!(truncated.chars().count(), 24);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn text_table_columns_stay_aligned_despite_a_very_long_tool_name() {
        let rows = vec![
            row(
                "aarch64-linux-gnu-cpp-13-extremely-long-name",
                5,
                Some(100.0),
                "ok",
            ),
            row("git", 5, Some(100.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        let table = render_text(&rows, &agg);
        let lines: Vec<&str> = table
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .collect();
        // Every data/header row must be exactly the same length up to the
        // status column's start — i.e. every fixed-width column lines up
        // regardless of how long any one tool's name was. Measured in
        // *characters*, not bytes: the framework column's `—` fallback and
        // a truncated tool name's `…` marker are both multi-byte UTF-8, so
        // a byte offset would (and, before this fix, did) disagree between
        // rows with different multi-byte-character counts even though the
        // actual rendered alignment is fine.
        let status_col_start = |line: &str| -> usize {
            // Two spaces precede the status column in the format string.
            match line.rfind("  ") {
                Some(byte_idx) => line[..byte_idx].chars().count() + 2,
                None => line.chars().count(),
            }
        };
        let widths: Vec<usize> = lines.iter().map(|l| status_col_start(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "column widths were not aligned: {widths:?}\n{table}"
        );
    }

    #[test]
    fn markdown_format_produces_a_gfm_table_and_footer() {
        let rows = vec![row("git", 10, Some(90.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.starts_with("| tool |"));
        assert!(md.contains("|---|"));
        assert!(md.contains("| git |"));
        assert!(md.contains("**Aggregate:**"));
        assert!(md.contains("**Framework detection:**"));
    }

    /// "Surfacing unidentified tools for audit": the top-unidentified list
    /// Ranked by how many flag *descriptions* are missing, not by flag
    /// count and not by percentage alone: a tool with 150 flags at 80% has
    /// more missing documentation behind it than one with 3 flags at 0%.
    /// Tools that parsed cleanly are excluded entirely, since a work queue
    /// of finished work is not a work queue.
    #[test]
    fn worst_parsed_ranks_by_missing_descriptions() {
        let rows = vec![
            row("perfect", 500, Some(100.0), "ok"),    // nothing missing
            row("tiny-but-awful", 3, Some(0.0), "ok"), // 3 missing
            row("big-and-ok", 150, Some(80.0), "ok"),  // 30 missing
            row("mid", 40, Some(50.0), "ok"),          // 20 missing
        ];
        let worst = worst_parsed(&rows);
        let names: Vec<&str> = worst.iter().map(|r| r.tool.as_str()).collect();
        assert_eq!(
            names,
            vec!["big-and-ok", "mid", "tiny-but-awful"],
            "expected ranking by missing descriptions, cleanly-parsed excluded: {names:?}"
        );
    }

    #[test]
    fn worst_parsed_is_capped() {
        let rows: Vec<Row> = (0..(WORST_PARSED_LIMIT + 10))
            .map(|i| row(&format!("tool{i}"), i + 10, Some(10.0), "ok"))
            .collect();
        assert_eq!(worst_parsed(&rows).len(), WORST_PARSED_LIMIT);
    }

    /// Nothing to report when every tool parsed cleanly. The section
    /// disappears rather than printing an empty heading.
    #[test]
    fn worst_parsed_lines_text_is_empty_when_everything_parsed_cleanly() {
        let rows = vec![row("git", 10, Some(100.0), "ok")];
        assert!(worst_parsed_lines_text(&worst_parsed(&rows)).is_empty());
    }

    #[test]
    fn render_text_includes_the_worst_parsed_audit_section() {
        let rows = vec![row("half-parsed", 42, Some(50.0), "ok")];
        let agg = compute_aggregate(&rows);
        let table = render_text(&rows, &agg);
        assert!(table.contains("# worst-parsed"));
        assert!(table.contains("half-parsed"));
    }

    #[test]
    fn render_markdown_includes_the_worst_parsed_audit_section() {
        let rows = vec![row("half-parsed", 42, Some(50.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.contains("**Worst-parsed tools**"));
        assert!(md.contains("| half-parsed |"));
    }

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

    // `structure_sanity`'s own unit tests (fabricated names, empty nodes,
    // the root-name exclusion, `heading_attested` provenance, a clean
    // tree) now live in `status.rs`'s test module, alongside the function
    // itself — see that module's doc comment for why it moved.
}
