//! Plain-text scoreboard rendering (the format checked into
//! `coverage-scoreboard.txt`): the fixed-width table, the aggregate/worst-
//! parsed/per-detector-sample sections, and the shared helpers
//! ([`truncate_col`], [`worst_parsed`]) [`super::render_markdown`] reuses.

use super::aggregate::{aggregate_footer_line, framework_summary_lines};
use super::fingerprint::{fingerprint_lines, ToolFingerprint};
use super::Aggregate;

/// Fixed display width for the `tool` column in [`ScoreFormat::Text`]
/// output. Truncated (never just left unbounded) because real tool names
/// on a real `PATH` blow past any reasonable assumption —
/// `aarch64-linux-gnu-cpp-13` (24 chars), `UnicodeNameMappingGenerator-18`
/// (31 chars) — and an untruncated long name shoves every column after it
/// out of alignment for that one row, which is exactly the bug this
/// constant (and [`truncate_col`]) exists to fix.
pub(crate) const TOOL_COL_WIDTH: usize = 24;
/// Fixed display width for the `tier(s)` column, same reasoning.
pub(crate) const TIER_COL_WIDTH: usize = 18;
/// Fixed display width for the new `framework` column, same reasoning.
pub(crate) const FRAMEWORK_COL_WIDTH: usize = 26;
/// Fixed display width for the right-aligned `nodes` column.
pub(crate) const NODES_COL_WIDTH: usize = 7;
/// Fixed display width for the right-aligned `flags` column.
pub(crate) const FLAGS_COL_WIDTH: usize = 8;
/// Fixed display width for the right-aligned `%flags_text` column.
pub(crate) const PCT_COL_WIDTH: usize = 13;
/// Fixed display width for the right-aligned `ms` column.
pub(crate) const MS_COL_WIDTH: usize = 7;
/// Fixed display width for the right-aligned `suspect` column.
pub(crate) const SUSPECT_COL_WIDTH: usize = 8;
/// Fixed display width for the right-aligned `man` column.
pub(crate) const MAN_COL_WIDTH: usize = 6;
/// Fixed display width for the right-aligned `misattr` column.
///
/// All eight widths above are `pub(crate)`, not local to [`render_text`]:
/// [`crate::transition`] parses a rendered `ScoreFormat::Text` scoreboard
/// back into rows by slicing at these exact offsets. A single source of
/// truth avoids the two-copy drift risk `status.rs`'s doc comment warns
/// about.
pub(crate) const MISATTR_COL_WIDTH: usize = 9;
/// Fixed display width for the right-aligned `exist` column
/// ([`crate::existence`]'s fabrication count).
pub(crate) const EXISTENCE_COL_WIDTH: usize = 6;
/// Fixed display width for the right-aligned `bundle` column
/// ([`crate::bundling`]'s collapse count).
pub(crate) const BUNDLE_COL_WIDTH: usize = 7;

/// One tool's row in the scoreboard.
pub(super) struct Row {
    pub(super) tool: String,
    pub(super) tiers: String,
    /// The detected framework (spec §7 Tier A′) plus how it was detected,
    /// e.g. `"clap (v3/v4) (artifact)"`, or `"—"` when unidentified. See
    /// [`framework_label`].
    pub(super) framework: String,
    pub(super) nodes: usize,
    /// Raw flag count, including usage-synopsis-only flags that can never
    /// carry a description ([M-15]). Kept separate from [`Self::describable`]
    /// per spec §13's metric design rules.
    pub(super) flags: usize,
    /// Flags whose source could, in principle, carry a description — the
    /// denominator [`Self::pct_flags_with_text`] is computed over. See
    /// [`mandible_extract::ExtractionResult::describable_flag_count`].
    pub(super) describable: usize,
    /// `None` when there are no describable flags to compute a percentage
    /// over.
    ///
    /// **Presence, not correctness** — never checks whether the attached
    /// text is the *right* text (`corpus/lsof/4.95.0`, `[xfail]`; see
    /// [`crate::misattribution`] for the accuracy instrument). Every
    /// scoreboard also carries `accuracy: unmeasured` —
    /// [`accuracy_unmeasured_line`].
    pub(super) pct_flags_with_text: Option<f64>,
    pub(super) ms: u128,
    /// Structure-sanity count (spec §13.1): descendant nodes failing
    /// [`mandible_core::is_command_name_shaped`], or with no flags,
    /// children, or summary. Non-zero forces `status` to `"suspicious"`
    /// regardless of `%described`, since fabricated structure *inflates*
    /// that number ([M-10]).
    pub(super) suspicious_nodes: usize,
    /// True when the root node degraded to spec §7 Tier B step 3's
    /// verbatim rendering (`CommandNode::unparsed` non-empty) rather than
    /// producing any structure at all.
    pub(super) verbatim: bool,
    /// True when the root `--help` probe's captured output was detected as
    /// a rendered man page (spec [M-16]) rather than ordinary help text —
    /// see [`root_is_man_shaped`]. A measurement column only: this is the
    /// exposure enumeration for a pending, not-yet-implemented safety
    /// decision (falling back to `-h` when this fires), so it is reported
    /// but never gated (spec [M-16], [`compute_aggregate`]'s doc comment
    /// on why `verbatim_count` gets the same treatment).
    pub(super) man_shaped: bool,
    /// [`crate::misattribution`]'s own measurement: count of this tool's
    /// flag descriptions that contain a flag-shaped token attested at a
    /// column-aligned definition position elsewhere in the tool's raw
    /// captured `--help` text — `lsof`'s bug, generalized. **Not gated**
    /// (see that module's doc comment): a brand-new detector with a
    /// measured, nonzero false-positive rate must not fail a build the
    /// first time it runs.
    pub(super) misattribution_suspect_count: usize,
    /// [`misattribution::MisattributionReport::column_aligned`]'s own
    /// report: whether this tool's raw text had at least one column offset
    /// that met the column-alignment recurrence bar at all — reported
    /// separately from `misattribution_suspect_count` because a tool can
    /// have a real multi-column table (`column_aligned: true`) whose
    /// descriptions just never happen to mention a neighbouring column, and
    /// that's a materially different "nothing to find here" than a tool
    /// whose text never had a second column in the first place.
    pub(super) misattribution_column_aligned: bool,
    /// A few of this row's own suspects, pre-formatted for the sweep's
    /// `# misattribution-suspects (sample)` section — capped per row
    /// ([`MISATTRIBUTION_SAMPLES_PER_ROW`]) so one pathological tool with
    /// hundreds of suspect flags can't crowd out every other tool's sample
    /// from a fleet-wide report.
    pub(super) misattribution_samples: Vec<String>,
    /// [`crate::existence`]'s own measurement: count of this tool's help-
    /// text-sourced subcommand names and flag spellings that do not occur
    /// literally in the tool's own raw captured `--help` text — [M-10]'s
    /// shape, generalized, and this task's own instrument. **Not gated**,
    /// same reasoning as `misattribution_suspect_count`: a brand-new
    /// detector with no fleet-wide baseline must not fail a build the
    /// first time it runs (spec §13.1b).
    pub(super) existence_fabrication_count: usize,
    /// A few of this row's own fabrications, pre-formatted, mirroring
    /// [`Self::misattribution_samples`] — capped per row
    /// ([`EXISTENCE_SAMPLES_PER_ROW`]).
    pub(super) existence_samples: Vec<String>,
    /// [`crate::bundling`]'s own measurement: count of this tool's synopsis
    /// flag clusters (`[-2CDlNuVv]`) read as one value-taking flag instead
    /// of the several boolean flags they name. **Not gated**, same
    /// reasoning as the two counts above: a brand-new detector with no
    /// fleet-wide baseline must not fail a build the first time it runs
    /// (spec §13.1b).
    pub(super) bundle_collapse_count: usize,
    /// How many real flags this row's collapses destroyed — every cluster
    /// member after the first. Carried separately from
    /// `bundle_collapse_count` because the two answer different questions
    /// and differ by more than an order of magnitude on a single tool:
    /// `tcpdump` is *one* collapse and *25* destroyed flags, so a count of
    /// collapses alone says nothing about how much recall the defect costs.
    pub(super) bundle_destroyed_flags: usize,
    /// A few of this row's own collapses, pre-formatted, mirroring
    /// [`Self::existence_samples`] — capped per row
    /// ([`BUNDLE_SAMPLES_PER_ROW`]).
    pub(super) bundle_samples: Vec<String>,
    /// [`crate::alternation`]'s own measurement: flag spellings this tool
    /// writes inside a delimited alternation group (`{-i|--input}`,
    /// `[[-c|-C] cmd]`) that reach no flag in its tree, plus any that reach
    /// one still carrying the group's punctuation as a value.
    pub(super) alternation_defect_count: usize,
    /// A few of this row's own, pre-formatted, mirroring
    /// [`Self::bundle_samples`] — capped per row
    /// ([`ALTERNATION_SAMPLES_PER_ROW`]).
    pub(super) alternation_samples: Vec<String>,
    /// How many `commands:` tables this row's help text offers whose every
    /// name is missing from the tree (`crate::commandtable`). Shape A of
    /// the four-grammar `unparsed-subcommand` split; the other three
    /// shapes are deliberately not counted here — see that module.
    pub(super) command_table_count: usize,
    /// [`crate::single_dash_long`]'s own measurement: count of this tool's
    /// option-table rows naming a single-dash long option (`-help`) that
    /// split into a one-character short flag plus a required value. The
    /// second of the three families sharing the `short && !long &&
    /// value_name` fingerprint. **Not gated until the family is repaired**,
    /// same reasoning as every count above: a brand-new detector with no
    /// fleet-wide baseline must not fail a build the first time it runs
    /// (spec §13.1b).
    pub(super) single_dash_split_count: usize,
    /// A few of this row's own splits, pre-formatted, capped per row
    /// ([`SPLIT_SAMPLES_PER_ROW`]).
    pub(super) single_dash_samples: Vec<String>,
    /// [`crate::repeated_char`]'s own measurement: count of this tool's
    /// repeated-character flags (`-vv`) read as the bare short flag carrying
    /// its own letter as a required value. The third family. Same gating
    /// note as above.
    pub(super) repeated_char_misread_count: usize,
    /// A few of this row's own misreads, pre-formatted, capped per row
    /// ([`SPLIT_SAMPLES_PER_ROW`]).
    pub(super) repeated_char_samples: Vec<String>,
    /// [`crate::wrapped_prose`]'s own measurement: count of this tool's
    /// physical lines whose own leading spelling was fabricated into a flag
    /// because a description wrapped mid-sentence onto a dash-led
    /// continuation line (atlas S-027). **Not gated**, same reasoning as
    /// every brand-new detector count above: no fleet-wide baseline exists
    /// yet (spec §13.1b).
    pub(super) wrapped_prose_count: usize,
    /// A few of this row's own fabrications, pre-formatted, mirroring
    /// [`Self::repeated_char_samples`].
    pub(super) wrapped_prose_samples: Vec<String>,
    /// [`crate::tail_operand`]'s own measurement: count of this tool's
    /// usage-line trailing operand tokens that never became a positional
    /// (atlas S-041). **Not gated**, same reasoning as above.
    pub(super) tail_operand_count: usize,
    /// A few of this row's own findings, pre-formatted, mirroring
    /// [`Self::wrapped_prose_samples`].
    pub(super) tail_operand_samples: Vec<String>,
    /// The seven vim-family detectors (atlas S-095 to S-100 and S-105): `(family
    /// name, this tool's finding count, capped samples)`, one entry per
    /// family, in registration order. One field rather than seven
    /// repeated ones — see `crate::coverage::score::vim_family_counts`.
    pub(super) vim_family: Vec<(&'static str, usize, Vec<String>)>,
    pub(super) status: &'static str,
    /// This tool's field-level fingerprint (WS2 part 2,
    /// [`crate::transition`]'s per-tool diff): enough for `sweep-diff` to
    /// tell a per-flag description/choices/value_name *change* apart from a
    /// count that merely stayed the same. See [`build_fingerprint`]'s doc
    /// comment for why the scoreboard's existing columns (flag counts,
    /// `%flags_text`) cannot see this — that gap is exactly what let PR #14
    /// delete `pngfix`'s and `pod2man`'s descriptions and fabricate a
    /// choices list while `sweep-diff` reported the run unchanged.
    pub(super) fingerprint: ToolFingerprint,
}

/// Cap on the total number of sample lines the fleet-wide
/// `# misattribution-suspects (sample)` section prints, mirroring
/// [`WORST_PARSED_LIMIT`]'s reasoning: a work-queue/audit aid needs to stay
/// scannable, not exhaustive — a human judging the false-positive rate
/// needs "enough to see the shape," not every hit on a full sweep.
pub(super) const MISATTRIBUTION_SAMPLE_LIMIT: usize = 20;

/// Cap on the total number of sample lines the fleet-wide
/// `# existence-fabrications (sample)` section prints — mirrors
/// [`MISATTRIBUTION_SAMPLE_LIMIT`]'s reasoning exactly.
pub(super) const EXISTENCE_SAMPLE_LIMIT: usize = 20;

/// Cap on the total number of sample lines the fleet-wide
/// `# bundled-short-flag collapses (sample)` section prints — mirrors
/// [`EXISTENCE_SAMPLE_LIMIT`].
pub(super) const BUNDLE_SAMPLE_LIMIT: usize = 20;

/// Cap on the total number of sample lines the fleet-wide
/// `# brace-alternation-flag defects (sample)` section prints — mirrors
/// [`BUNDLE_SAMPLE_LIMIT`].
pub(super) const ALTERNATION_SAMPLE_LIMIT: usize = 20;

/// Cap on the total number of sample lines each of the two
/// fingerprint-sibling sections prints — mirrors [`BUNDLE_SAMPLE_LIMIT`].
pub(super) const SPLIT_SAMPLE_LIMIT: usize = 20;

/// Truncate `s` to at most `width` characters, replacing the tail with a
/// single `…` marker when it doesn't fit. Character count, not
/// `unicode-width` — unlike `mandible-tui`'s rendering (which the
/// project's own invariants require display-width-safe truncation for,
/// since it draws into fixed terminal cells the user is actually looking
/// at), this is a plain-text developer report over tool names that are
/// overwhelmingly ASCII, so the extra dependency isn't justified here.
pub(super) fn truncate_col(s: &str, width: usize) -> String {
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

pub(super) fn render_text(rows: &[Row], aggregate: &Aggregate) -> String {
    // Note the fixed `{:<N}` widths on the *truncated* string, not the
    // raw one: `{:<N}` only ever pads to a minimum width, it never
    // truncates — feeding it an untruncated long tool/tier/framework name
    // is exactly what let one long name shove every later column out of
    // alignment for that row.
    let mut out = String::new();
    out.push_str(&format!(
        "{:<tw$} {:<iw$} {:<fw$} {:>nw$}{:>flw$}{:>pw$}{:>msw$}{:>sw$}{:>manw$}{:>miw$}{:>ew$}{:>bw$}  {}\n",
        "tool",
        "tier(s)",
        "framework",
        "nodes",
        "flags",
        // Renamed from "%described" (spec §13.1/§13.1b): this column has
        // only ever measured whether a flag has text attached, never
        // whether the text is right — see `Row::pct_flags_with_text`'s doc
        // comment and the `accuracy: unmeasured` line below.
        "%flags_text",
        "ms",
        "suspect",
        "man",
        "misattr",
        "exist",
        "bundle",
        "status",
        tw = TOOL_COL_WIDTH,
        iw = TIER_COL_WIDTH,
        fw = FRAMEWORK_COL_WIDTH,
        nw = NODES_COL_WIDTH,
        flw = FLAGS_COL_WIDTH,
        pw = PCT_COL_WIDTH,
        msw = MS_COL_WIDTH,
        sw = SUSPECT_COL_WIDTH,
        manw = MAN_COL_WIDTH,
        miw = MISATTR_COL_WIDTH,
        ew = EXISTENCE_COL_WIDTH,
        bw = BUNDLE_COL_WIDTH,
    ));
    for row in rows {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{:<tw$} {:<iw$} {:<fw$} {:>nw$}{:>flw$}{:>pw$}{:>msw$}{:>sw$}{:>manw$}{:>miw$}{:>ew$}{:>bw$}  {}\n",
            truncate_col(&row.tool, TOOL_COL_WIDTH),
            truncate_col(&row.tiers, TIER_COL_WIDTH),
            truncate_col(&row.framework, FRAMEWORK_COL_WIDTH),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            if row.man_shaped { "yes" } else { "-" },
            row.misattribution_suspect_count,
            row.existence_fabrication_count,
            row.bundle_collapse_count,
            row.status,
            tw = TOOL_COL_WIDTH,
            iw = TIER_COL_WIDTH,
            fw = FRAMEWORK_COL_WIDTH,
            nw = NODES_COL_WIDTH,
            flw = FLAGS_COL_WIDTH,
            pw = PCT_COL_WIDTH,
            msw = MS_COL_WIDTH,
            sw = SUSPECT_COL_WIDTH,
            manw = MAN_COL_WIDTH,
            miw = MISATTR_COL_WIDTH,
            ew = EXISTENCE_COL_WIDTH,
            bw = BUNDLE_COL_WIDTH,
        ));
    }
    out.push_str(&aggregate_footer_line(aggregate));
    out.push('\n');
    out.push_str(&accuracy_unmeasured_line());
    out.push_str(&framework_summary_lines(aggregate));
    out.push_str(&worst_parsed_lines_text(&worst_parsed(rows)));
    out.push_str(&misattribution_sample_lines_text(rows));
    out.push_str(&existence_sample_lines_text(rows));
    out.push_str(&bundle_sample_lines_text(rows));
    out.push_str(&alternation_sample_lines_text(rows));
    out.push_str(&single_dash_sample_lines_text(rows));
    out.push_str(&repeated_char_sample_lines_text(rows));
    out.push_str(&wrapped_prose_sample_lines_text(rows));
    out.push_str(&tail_operand_sample_lines_text(rows));
    out.push_str(&vim_family_sample_lines_text(rows));
    out.push_str(&fingerprint_lines(rows));
    out
}

/// The literal line every scoreboard carries until an instrument actually
/// measures whether a flag's attached text is correct, not just present
/// (spec §13.1's rename note). `pct_flags_with_text` answers presence
/// only — `corpus/lsof/4.95.0` (`[xfail]`) scored 79% while a quarter of
/// its flags were wrong. [`crate::misattribution`] is a first step but not
/// a general accuracy oracle.
fn accuracy_unmeasured_line() -> String {
    "# accuracy: unmeasured\n".to_string()
}

/// Cap on the worst-parsed audit section. Not load-bearing (this is a
/// work-queue aid, not a gated metric); 25 keeps the footer scannable
/// rather than dumping every imperfect tool on a full-`PATH` sweep.
const WORST_PARSED_LIMIT: usize = 25;

/// How many of a tool's *describable* flags the grammar failed to find a
/// description for. The ranking key below.
///
/// Measured against `row.describable`, not `row.flags` (spec §13's metric
/// design rules): a usage-synopsis-only flag was never a candidate for a
/// description in the first place, so counting it as "missing" here would
/// reopen exactly the trap this whole redefinition closes — a tool that
/// gains recall in flags nothing could ever describe would rank as having
/// gotten *worse*.
pub(super) fn undescribed_flags(row: &Row) -> usize {
    match row.pct_flags_with_text {
        Some(pct) => {
            let described = (row.describable as f64) * (pct / 100.0);
            row.describable.saturating_sub(described.round() as usize)
        }
        // No describable flags at all, so nothing was missed.
        None => 0,
    }
}

/// The tools this harness parsed worst, ranked by how many flag
/// descriptions went missing, capped to [`WORST_PARSED_LIMIT`].
///
/// Ranked by undescribed-flag count, not percentage alone: a 150-flag
/// tool at 60% has more missing documentation than a 3-flag tool at 0%.
/// Not ranked by detection status — measurement showed unidentified and
/// identified tools score about the same (~90-92% described), so
/// detection isn't what separates a good result from a bad one. Ties
/// broken by tool name.
pub(super) fn worst_parsed(rows: &[Row]) -> Vec<&Row> {
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
            .pct_flags_with_text
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

/// Plain-text rendering of every row's [`Row::misattribution_samples`],
/// flattened and capped at [`MISATTRIBUTION_SAMPLE_LIMIT`] — a human-
/// readable sample of what [`crate::misattribution`] flagged, for judging
/// the false-positive rate (spec's own instruction: "report the rate, show
/// a sample of what it flags"). Mirrors [`worst_parsed_lines_text`]'s
/// "nothing to report → no section" convention.
fn misattribution_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.misattribution_samples.iter())
        .take(MISATTRIBUTION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# misattribution-suspects (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::existence_samples`], flattened
/// and capped at [`EXISTENCE_SAMPLE_LIMIT`] — mirrors
/// [`misattribution_sample_lines_text`]'s shape and "nothing to report → no
/// section" convention exactly.
fn existence_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.existence_samples.iter())
        .take(EXISTENCE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# existence-fabrications (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::bundle_samples`], flattened
/// and capped at [`BUNDLE_SAMPLE_LIMIT`] — mirrors
/// [`existence_sample_lines_text`]'s shape and "nothing to report → no
/// section" convention exactly.
fn bundle_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.bundle_samples.iter())
        .take(BUNDLE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# bundled-short-flag collapses (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::alternation_samples`],
/// flattened and capped at [`ALTERNATION_SAMPLE_LIMIT`] — mirrors
/// [`bundle_sample_lines_text`] exactly, including the "nothing to report →
/// no section" convention.
fn alternation_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.alternation_samples.iter())
        .take(ALTERNATION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# brace-alternation-flag defects (sample — reported, NOT gated; the residual is a subcommand-scoped flag this family cannot place, see xtask/src/main.rs):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::single_dash_samples`] — the
/// second of the three families that share the `short && !long &&
/// value_name` fingerprint, printed as its own section for the same reason
/// [`bundle_sample_lines_text`] prints one: a count with no sample beside it
/// cannot be judged for false positives by anyone but its author. Mirrors
/// that function's "nothing to report -> no section" convention exactly.
fn single_dash_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.single_dash_samples.iter()),
        "# single-dash-long splits (sample — judge the false-positive rate yourself):\n",
    )
}

/// Twin of [`single_dash_sample_lines_text`] for [`crate::repeated_char`].
fn repeated_char_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.repeated_char_samples.iter()),
        "# repeated-char-flag misreads (sample — judge the false-positive rate yourself):\n",
    )
}

/// Twin of [`single_dash_sample_lines_text`] for [`crate::wrapped_prose`].
fn wrapped_prose_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.wrapped_prose_samples.iter()),
        "# wrapped-prose-row-boundary fabrications (sample — judge the false-positive rate \
         yourself):\n",
    )
}

/// Twin of [`single_dash_sample_lines_text`] for [`crate::tail_operand`].
fn tail_operand_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.tail_operand_samples.iter()),
        "# unparsed-tail-operand findings (sample — judge the false-positive rate yourself):\n",
    )
}

/// Twin of [`tail_operand_sample_lines_text`] for the seven vim-family
/// detectors: one heading and sample block per family, in registration
/// order.
fn vim_family_sample_lines_text(rows: &[Row]) -> String {
    let Some(names): Option<Vec<&'static str>> = rows
        .iter()
        .find(|r| !r.vim_family.is_empty())
        .map(|r| r.vim_family.iter().map(|(n, ..)| *n).collect())
    else {
        return String::new();
    };
    let mut out = String::new();
    for name in names {
        let heading =
            format!("# {name} findings (sample — judge the false-positive rate yourself):\n");
        out.push_str(&sample_lines_text(
            rows.iter().flat_map(|r| {
                r.vim_family
                    .iter()
                    .find(|(n, ..)| *n == name)
                    .into_iter()
                    .flat_map(|(_, _, s)| s.iter())
            }),
            &heading,
        ));
    }
    out
}

/// The shared body of the two functions above: up to [`SPLIT_SAMPLE_LIMIT`]
/// samples under `heading`, or nothing at all when there are none. Factored
/// out rather than copied a fourth time — the three older sections predate
/// it and are left alone, but a fifth hand-written copy of the same ten
/// lines is exactly the drift risk `status.rs`'s own doc comment names.
fn sample_lines_text<'a>(samples: impl Iterator<Item = &'a String>, heading: &str) -> String {
    let samples: Vec<&String> = samples.take(SPLIT_SAMPLE_LIMIT).collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(heading);
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::aggregate::compute_aggregate;
    use crate::coverage::row;

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
}
