//! Aggregate stats: [`Aggregate`] itself, computing it from a sweep's
//! [`super::Row`]s, and the `# aggregate: ...` footer line both rendered
//! formats carry and [`parse_aggregate_footer`] reads back for `--check`.

use super::Row;
use std::collections::BTreeMap;

/// Aggregate stats. `pct_flags_with_text`, `no_tier_count`, and
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
    pub pct_flags_with_text: f64,
    /// Tools for which no tier produced a root node at all.
    pub no_tier_count: usize,
    /// Tools with at least one structurally-suspicious node (spec §13.1):
    /// a name failing [`mandible_core::is_command_name_shaped`], or a node
    /// with no flags, children, or summary. Gated like `no_tier_count` —
    /// [M-10] shipped `ok` at 100% described because `%described` can't
    /// see fabricated structure; this column can.
    pub suspicious_count: usize,
    /// Tools whose root degraded to verbatim (spec §7 Tier B step 3).
    /// **Not gated** — see [`compute_aggregate`].
    pub verbatim_count: usize,
    /// Tools at status `incomplete` (spec §6 rule 2b): a truncation
    /// confession was detected but not followed. **Not gated** — no
    /// baseline exists yet for this measurement.
    pub incomplete_count: usize,
    /// Tools whose root `--help` output was detected as a rendered man
    /// page ([M-16]). A subset of `verbatim_count`. **Not gated** — no
    /// baseline exists yet for this measurement.
    pub man_shaped_count: usize,
    /// Tools at status `ok` with zero flags at all ([M-15]). A synopsis
    /// flag is excluded from `pct_flags_with_text`'s denominator entirely
    /// (see [`mandible_extract::ExtractionResult::describable_flag_count`]),
    /// so this count and `pct_flags_with_text` move independently. **Not
    /// gated** — no baseline exists yet for this measurement.
    pub zero_flag_ok_count: usize,
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
    /// Raw numerator behind `pct_flags_with_text`, carried in the footer so a
    /// scoreboard produced in *shards* can be merged exactly. Recomputing
    /// the aggregate from the per-row `%flags_text` column cannot be exact
    /// — that column is rounded to whole percent — and a gated regression
    /// baseline must not be approximate. A full-PATH sweep is long enough
    /// to be worth running in shards, and CI's PATH sweep will want the
    /// same.
    pub described_flags: f64,
    /// **The** denominator behind `pct_flags_with_text` (spec §13's metric
    /// design rules) — the sum, across every tool, of flags whose source
    /// could have supplied a description. Excludes usage-synopsis-only
    /// flags; see
    /// [`mandible_extract::ExtractionResult::describable_flag_count`].
    pub describable_flags: f64,
    /// Raw flag total across every tool, including usage-synopsis-only
    /// ones — **not** `pct_flags_with_text`'s denominator (that's
    /// [`Self::describable_flags`]). Kept as its own number precisely so a
    /// fix that recovers real, honestly-undescribable flags is visible as
    /// recall gained rather than silently absent from every footer field,
    /// per spec §13's "keep the raw flag count visible" rule.
    pub total_flags: usize,
    /// Tools with at least one [`crate::misattribution`] suspect — the
    /// answer to "is `lsof` isolated, or is misattribution widespread?"
    /// **Not gated**: a brand-new detector with a measured, nonzero false-
    /// positive rate (see that module's doc comment) must not fail a build
    /// the first time it runs. Reported every run, compared against the
    /// previous one for visibility only (`xtask/src/main.rs`).
    pub misattribution_suspect_tools: usize,
    /// Tools whose raw captured text had at least one column-aligned
    /// secondary definition position at all — see
    /// [`Row::misattribution_column_aligned`]. Always `>=
    /// misattribution_suspect_tools`, and reported alongside it so a reader
    /// can see how often the strengthening signal fires versus how often it
    /// actually turns up a suspect. **Not gated**, same reasoning as
    /// `misattribution_suspect_tools`.
    pub misattribution_column_aligned_tools: usize,
    /// Tools with at least one [`crate::existence`] fabrication — a help-
    /// text-sourced subcommand name or flag spelling that does not occur
    /// literally in that tool's own raw captured text. This is the *other*
    /// half of what docs/design.md's WS4 originally called one "anti-fabrication
    /// oracle" — [`Self::misattribution_suspect_tools`]'s twin, with a
    /// different victim: [M-10]'s invented `tar`/`dd`/`less`/`apt-get`
    /// nodes, not `lsof`'s column-bled descriptions. **Not gated**, same
    /// reasoning as `misattribution_suspect_tools`: a brand-new detector
    /// with no fleet-wide baseline must not fail a build the first time it
    /// runs (spec §13.1b).
    pub existence_fabrication_tools: usize,
    /// Tools with at least one [`crate::bundling`] collapse — a synopsis
    /// cluster of bundled single-character switches (`[-2CDlNuVv]`) parsed
    /// as one flag carrying the rest as a required value. The third oracle,
    /// and the one the other two are structurally blind to: a collapsed
    /// `-2` *is* attested by [`Self::existence_fabrication_tools`]'s check
    /// (it occurs, literally, in the raw text) and carries no description
    /// for [`Self::misattribution_suspect_tools`]'s to misjudge, while the
    /// parse is badly wrong. **Not gated**, same reasoning as both:
    /// a brand-new detector with no fleet-wide baseline must not fail a
    /// build the first time it runs (spec §13.1b).
    pub bundle_collapse_tools: usize,
    /// Real flags destroyed by those collapses, fleet-wide — every cluster
    /// member after the first. This is the recall number;
    /// `bundle_collapse_tools` is only the blast radius.
    pub bundle_destroyed_flags: usize,
    /// Tools with at least one [`crate::alternation`] finding — a flag
    /// spelling written inside a delimited alternation group that reaches no
    /// flag in the tree, or one that reaches a flag still carrying the
    /// group's own punctuation as its value. The fourth oracle, and the one
    /// the three before it are blind to for three different reasons:
    /// `eqn`'s `--version` occurs literally in its raw text (so
    /// [`Self::existence_fabrication_tools`]'s check attests it), it carries
    /// no description for [`Self::misattribution_suspect_tools`]'s to
    /// misjudge, and its members are separated by `|` rather than glued, so
    /// the cluster grammar behind [`Self::bundle_collapse_tools`] neither
    /// helps nor hinders it.
    pub alternation_defect_tools: usize,
    /// Flag spellings those tools lost or mangled, fleet-wide. The recall
    /// number; `alternation_defect_tools` is only the blast radius.
    pub alternation_defect_flags: usize,
    /// Tools with at least one wholly-unparsed `commands:` table, fleet-
    /// wide. **Ratcheted at zero** (`detector::ratchet_at_zero`) rather
    /// than merely reported: the shape is fixed, and the gate is paired
    /// with the detector's own self-checks so a zero cannot be earned by
    /// deleting the rule.
    pub command_table_tools: usize,
    /// Tools with at least one [`crate::single_dash_long`] split — an
    /// option-table row naming a single-dash long option (`-help`) read as a
    /// one-character short flag plus a required value. The second of the
    /// three families sharing `bundle_collapse_tools`'s structural
    /// fingerprint, and blind to the same two oracles for the same reason:
    /// `-h` occurs in `qemu`'s raw text and carries a description, so
    /// nothing before this counted it. **Ratcheted at zero**
    /// (`detector::ratchet_at_zero`) since
    /// `help_text::sections::repair_single_dash_long_options` landed, on the
    /// same paired terms as `command_table_tools`: the count and the
    /// detector's own self-checks together, so a zero cannot be earned by
    /// deleting the rule.
    pub single_dash_split_tools: usize,
    /// Real flags lost to those splits, fleet-wide — one per split (the long
    /// spelling itself). Carried beside the tool count for the same reason
    /// `bundle_destroyed_flags` is, even though the ratio is milder here:
    /// the tool count is the blast radius, this is the recall cost.
    pub single_dash_split_flags: usize,
    /// Tools with at least one [`crate::repeated_char`] misread — `-vv` read
    /// as `-v` carrying its own letter as a value. The third family.
    pub repeated_char_tools: usize,
    /// Real flags lost to those misreads, fleet-wide — one per misread.
    pub repeated_char_flags: usize,
    /// Tools with at least one [`crate::wrapped_prose`] fabrication — a
    /// dash-led continuation line, at the same indent as an unfinished
    /// sentence above it, whose own leading spelling reached the tree as a
    /// flag (atlas S-027). **Not gated**, same reasoning as every brand-new
    /// detector count above: no fleet-wide baseline exists yet, and neither
    /// ground-truth tool (`zgrep`, `resolvconf`) has a reviewed audit
    /// verdict yet either (spec §13.1b, §13.1e).
    pub wrapped_prose_tools: usize,
    /// Real flags fabricated by that shape, fleet-wide — one per
    /// fabrication line.
    pub wrapped_prose_flags: usize,
    /// Tools with at least one [`crate::tail_operand`] finding — a usage
    /// line's own trailing operand token that never became a positional
    /// (atlas S-041). **Not gated**, same reasoning as above.
    pub tail_operand_tools: usize,
    /// Real operands lost to that shape, fleet-wide — one per finding.
    pub tail_operand_flags: usize,
    /// The seven vim-family detectors (atlas S-095 to S-100 and S-105,
    /// `xtask/src/plus_prefixed_option.rs` and its six siblings):
    /// `(family name, tools with at least one finding, findings
    /// fleet-wide)`, in registration order. **Not gated** — the calibration
    /// precondition (spec §13.1e) has not passed yet for any of the seven.
    pub vim_family: Vec<(&'static str, usize, usize)>,
}

/// Compute aggregate stats over `rows`.
///
/// `verbatim_count` is reported but not part of the regression gate (spec
/// §13.1's `--check`): a growing count can be a correct degrade-rather-
/// than-fabricate move (spec §7 Tier B step 3), not a regression.
/// `framework_detected_count`/`framework_counts` are unlisted for the same
/// reason — identifying more frameworks is progress, not a regression.
pub(super) fn compute_aggregate(rows: &[Row]) -> Aggregate {
    let total_flags: usize = rows.iter().map(|r| r.flags).sum();
    let describable_flags: f64 = rows.iter().map(|r| r.describable as f64).sum();
    // Weighted by each row's *describable* count, not its raw flag count
    // (spec §13's metric design rules) — a row's `pct_flags_with_text` is
    // already described/describable, so multiplying it back by
    // `r.flags` here would silently reintroduce [M-15]'s defect by
    // crediting synopsis-only flags into a denominator they were just
    // excluded from.
    let described_flags: f64 = rows
        .iter()
        .map(|r| {
            r.pct_flags_with_text
                .map(|p| p / 100.0 * r.describable as f64)
                .unwrap_or(0.0)
        })
        .sum();
    let pct_flags_with_text = if describable_flags == 0.0 {
        0.0
    } else {
        described_flags / describable_flags * 100.0
    };
    let no_tier_count = rows.iter().filter(|r| r.status == "no-tier").count();
    let suspicious_count = rows.iter().filter(|r| r.status == "suspicious").count();
    let verbatim_count = rows.iter().filter(|r| r.verbatim).count();
    let incomplete_count = rows.iter().filter(|r| r.status == "incomplete").count();
    let man_shaped_count = rows.iter().filter(|r| r.man_shaped).count();
    let zero_flag_ok_count = rows
        .iter()
        .filter(|r| r.status == "ok" && r.flags == 0)
        .count();
    let misattribution_suspect_tools = rows
        .iter()
        .filter(|r| r.misattribution_suspect_count > 0)
        .count();
    let misattribution_column_aligned_tools = rows
        .iter()
        .filter(|r| r.misattribution_column_aligned)
        .count();
    let existence_fabrication_tools = rows
        .iter()
        .filter(|r| r.existence_fabrication_count > 0)
        .count();
    let bundle_collapse_tools = rows.iter().filter(|r| r.bundle_collapse_count > 0).count();
    let bundle_destroyed_flags: usize = rows.iter().map(|r| r.bundle_destroyed_flags).sum();
    let alternation_defect_tools = rows
        .iter()
        .filter(|r| r.alternation_defect_count > 0)
        .count();
    let alternation_defect_flags: usize = rows.iter().map(|r| r.alternation_defect_count).sum();
    let command_table_tools = rows.iter().filter(|r| r.command_table_count > 0).count();
    let single_dash_split_tools = rows
        .iter()
        .filter(|r| r.single_dash_split_count > 0)
        .count();
    let single_dash_split_flags: usize = rows.iter().map(|r| r.single_dash_split_count).sum();
    let repeated_char_tools = rows
        .iter()
        .filter(|r| r.repeated_char_misread_count > 0)
        .count();
    let repeated_char_flags: usize = rows.iter().map(|r| r.repeated_char_misread_count).sum();
    let wrapped_prose_tools = rows.iter().filter(|r| r.wrapped_prose_count > 0).count();
    let wrapped_prose_flags: usize = rows.iter().map(|r| r.wrapped_prose_count).sum();
    let tail_operand_tools = rows.iter().filter(|r| r.tail_operand_count > 0).count();
    let tail_operand_flags: usize = rows.iter().map(|r| r.tail_operand_count).sum();
    let vim_family = compute_vim_family(rows);

    let mut framework_counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        if let Some(name) = framework_name_only(&row.framework) {
            *framework_counts.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    let framework_detected_count: usize = framework_counts.values().sum();

    Aggregate {
        pct_flags_with_text,
        no_tier_count,
        suspicious_count,
        verbatim_count,
        incomplete_count,
        man_shaped_count,
        zero_flag_ok_count,
        framework_detected_count,
        framework_counts,
        total: rows.len(),
        described_flags,
        describable_flags,
        total_flags,
        misattribution_suspect_tools,
        misattribution_column_aligned_tools,
        existence_fabrication_tools,
        bundle_collapse_tools,
        bundle_destroyed_flags,
        alternation_defect_tools,
        alternation_defect_flags,
        command_table_tools,
        single_dash_split_tools,
        single_dash_split_flags,
        repeated_char_tools,
        repeated_char_flags,
        wrapped_prose_tools,
        wrapped_prose_flags,
        tail_operand_tools,
        tail_operand_flags,
        vim_family,
    }
}

/// The seven vim-family detectors, aggregated in the same order
/// [`super::score::vim_family_counts`] returns them: for each family, how
/// many tools carried at least one finding, and how many findings
/// fleet-wide.
fn compute_vim_family(rows: &[Row]) -> Vec<(&'static str, usize, usize)> {
    let Some(first) = rows.iter().find(|r| !r.vim_family.is_empty()) else {
        return Vec::new();
    };
    first
        .vim_family
        .iter()
        .map(|(name, _, _)| {
            let tools = rows
                .iter()
                .filter(|r| {
                    r.vim_family
                        .iter()
                        .any(|(n, count, _)| n == name && *count > 0)
                })
                .count();
            let flags: usize = rows
                .iter()
                .filter_map(|r| {
                    r.vim_family
                        .iter()
                        .find(|(n, ..)| n == name)
                        .map(|(_, c, _)| *c)
                })
                .sum();
            (*name, tools, flags)
        })
        .collect()
}

/// Strip a row's `"<name> (<method>)"` framework label back down to just
/// the name, for aggregation; `None` for the unidentified sentinel `"—"`.
fn framework_name_only(label: &str) -> Option<&str> {
    if label == "—" {
        return None;
    }
    label.rsplit_once(" (").map(|(name, _)| name)
}

pub(super) fn detection_rate_pct(aggregate: &Aggregate) -> f64 {
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
pub(super) fn aggregate_footer_line(aggregate: &Aggregate) -> String {
    format!(
        "# aggregate: pct_flags_with_text={:.2} no_tier_count={} suspicious_count={} verbatim_count={} incomplete_count={} man_shaped_count={} zero_flag_ok_count={} misattribution_suspect_tools={} misattribution_column_aligned_tools={} existence_fabrication_tools={} bundle_collapse_tools={} bundle_destroyed_flags={} alternation_defect_tools={} alternation_defect_flags={} command_table_tools={} single_dash_split_tools={} single_dash_split_flags={} repeated_char_tools={} repeated_char_flags={} wrapped_prose_tools={} wrapped_prose_flags={} tail_operand_tools={} tail_operand_flags={} vim_family={} total={} described_flags={:.4} describable_flags={:.4} total_flags={}\n",
        aggregate.pct_flags_with_text,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
        aggregate.incomplete_count,
        aggregate.man_shaped_count,
        aggregate.zero_flag_ok_count,
        aggregate.misattribution_suspect_tools,
        aggregate.misattribution_column_aligned_tools,
        aggregate.existence_fabrication_tools,
        aggregate.bundle_collapse_tools,
        aggregate.bundle_destroyed_flags,
        aggregate.alternation_defect_tools,
        aggregate.alternation_defect_flags,
        aggregate.command_table_tools,
        aggregate.single_dash_split_tools,
        aggregate.single_dash_split_flags,
        aggregate.repeated_char_tools,
        aggregate.repeated_char_flags,
        aggregate.wrapped_prose_tools,
        aggregate.wrapped_prose_flags,
        aggregate.tail_operand_tools,
        aggregate.tail_operand_flags,
        encode_vim_family(&aggregate.vim_family),
        aggregate.total,
        aggregate.described_flags,
        aggregate.describable_flags,
        aggregate.total_flags,
    )
}

/// The seven `(name, tools, flags)` triples packed into one whitespace-free
/// `aggregate_footer_line` field: `"name:tools:flags,name:tools:flags,..."`,
/// `"-"` when empty (a footer written before these detectors existed).
fn encode_vim_family(entries: &[(&'static str, usize, usize)]) -> String {
    if entries.is_empty() {
        return "-".to_string();
    }
    entries
        .iter()
        .map(|(name, tools, flags)| format!("{name}:{tools}:{flags}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The inverse of [`encode_vim_family`]. Malformed entries are skipped
/// rather than failing the whole parse — a forward-compatible footer field
/// should degrade, not break `--check`.
fn decode_vim_family(field: &str) -> Vec<(&'static str, usize, usize)> {
    if field == "-" || field.is_empty() {
        return Vec::new();
    }
    field
        .split(',')
        .filter_map(|entry| {
            let mut parts = entry.split(':');
            let name = family_static_name(parts.next()?)?;
            let tools = parts.next()?.parse::<usize>().ok()?;
            let flags = parts.next()?.parse::<usize>().ok()?;
            Some((name, tools, flags))
        })
        .collect()
}

/// The closed set of vim-family names, resolved to their `'static`
/// spelling — mirrors `xtask/src/detector/commands.rs`'s `family_static`,
/// scoped to just these seven so a footer field never manufactures an
/// unregistered family name.
fn family_static_name(word: &str) -> Option<&'static str> {
    [
        "plus-prefixed-option",
        "end-of-options-marker",
        "single-space-description-column",
        "usage-only-value-name",
        "second-optional-value-dropped",
        "parenthetical-qualifier-as-value",
        "or-joined-alias",
    ]
    .into_iter()
    .find(|&n| n == word)
}

/// Human-readable (not re-parsed) framework-detection summary: total
/// detection rate plus per-framework counts, sorted by name for a stable
/// diff.
pub(super) fn framework_summary_lines(aggregate: &Aggregate) -> String {
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
    let mut pct_flags_with_text = None;
    let mut no_tier_count = None;
    // Older scoreboards (pre structure-sanity / pre-framework / pre-man-
    // shaped / pre-zero-flag columns) are missing `suspicious_count`/
    // `verbatim_count`/`man_shaped_count`/`zero_flag_ok_count` entirely;
    // default all four to 0 rather than failing to parse, so `--check`
    // against a not-yet-regenerated baseline still works for the fields
    // that did exist.
    let mut suspicious_count = 0usize;
    let mut verbatim_count = 0usize;
    // Brand-new field (spec §6 rule 2b, this batch): a scoreboard from
    // before the `incomplete` status existed has no such key at all, so
    // `--check` against one must still work.
    let mut incomplete_count = 0usize;
    let mut man_shaped_count = 0usize;
    let mut zero_flag_ok_count = 0usize;
    let mut described_flags = 0.0f64;
    // A scoreboard from before spec §13's metric redefinition has no
    // `describable_flags` field at all — its `pct_flags_with_text` was computed
    // over raw `total_flags` instead. Defaulting to 0.0 here (same pattern
    // as every other new-field default above) only affects reconstructing
    // an *exact* numerator/denominator for shard merging; `--check`
    // compares `pct_flags_with_text` values directly and never recomputes them
    // from this pair, so an old baseline still round-trips.
    let mut describable_flags = 0.0f64;
    let mut total_flags = 0usize;
    let mut total = None;
    // Brand-new field (spec §13.1's rename note, this task): a scoreboard
    // from before the misattribution detector existed has no such key at
    // all, so `--check` against one must still work.
    let mut misattribution_suspect_tools = 0usize;
    let mut misattribution_column_aligned_tools = 0usize;
    // Same reasoning, same pattern, brand new field (this task): a
    // scoreboard from before the existence detector existed has no such
    // key at all, so `--check` against one must still work.
    let mut existence_fabrication_tools = 0usize;
    // Same reasoning, same pattern, brand new field (this task): a
    // scoreboard from before the bundled-short-flag detector existed has no
    // such key at all, so `--check` against one must still work.
    let mut bundle_collapse_tools = 0usize;
    let mut bundle_destroyed_flags = 0usize;
    // Same reasoning again, brand new field (this task): a scoreboard
    // written before the brace-alternation detector existed carries no such
    // key, so `--check` against one must still work.
    let mut alternation_defect_tools = 0usize;
    let mut alternation_defect_flags = 0usize;
    let mut command_table_tools = 0usize;
    let mut single_dash_split_tools = 0usize;
    let mut single_dash_split_flags = 0usize;
    let mut repeated_char_tools = 0usize;
    let mut repeated_char_flags = 0usize;
    // Same reasoning again, brand new field (this task): a scoreboard
    // written before the wrapped-prose-row-boundary / unparsed-tail-operand
    // detectors existed carries neither key.
    let mut wrapped_prose_tools = 0usize;
    let mut wrapped_prose_flags = 0usize;
    let mut tail_operand_tools = 0usize;
    let mut tail_operand_flags = 0usize;
    // Same reasoning again, brand new field (this task): a scoreboard
    // written before the seven vim-family detectors existed carries no such
    // key.
    let mut vim_family: Vec<(&'static str, usize, usize)> = Vec::new();
    for field in line.trim_start_matches("# aggregate:").split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "pct_flags_with_text" => pct_flags_with_text = value.parse::<f64>().ok(),
            // Backward compatibility with every scoreboard written before
            // this rename (spec §13.1/§13.1b, Appendix B): the field is the
            // same ratio under its old, accuracy-implying name —
            // `pct_described`. Never written by this module anymore (see
            // `aggregate_footer_line`), only read.
            "pct_described" => pct_flags_with_text = value.parse::<f64>().ok(),
            "no_tier_count" => no_tier_count = value.parse::<usize>().ok(),
            "suspicious_count" => suspicious_count = value.parse::<usize>().ok()?,
            "verbatim_count" => verbatim_count = value.parse::<usize>().ok()?,
            "incomplete_count" => incomplete_count = value.parse::<usize>().ok()?,
            "man_shaped_count" => man_shaped_count = value.parse::<usize>().ok()?,
            "zero_flag_ok_count" => zero_flag_ok_count = value.parse::<usize>().ok()?,
            "misattribution_suspect_tools" => {
                misattribution_suspect_tools = value.parse::<usize>().ok()?
            }
            "misattribution_column_aligned_tools" => {
                misattribution_column_aligned_tools = value.parse::<usize>().ok()?
            }
            "existence_fabrication_tools" => {
                existence_fabrication_tools = value.parse::<usize>().ok()?
            }
            "bundle_collapse_tools" => bundle_collapse_tools = value.parse::<usize>().ok()?,
            "bundle_destroyed_flags" => bundle_destroyed_flags = value.parse::<usize>().ok()?,
            "alternation_defect_tools" => alternation_defect_tools = value.parse::<usize>().ok()?,
            "alternation_defect_flags" => alternation_defect_flags = value.parse::<usize>().ok()?,
            // Absent from a scoreboard written before this key existed,
            // which parses as 0 — the same value a healthy fleet produces,
            // so an older baseline stays comparable instead of failing.
            "command_table_tools" => command_table_tools = value.parse::<usize>().ok()?,
            "single_dash_split_tools" => single_dash_split_tools = value.parse::<usize>().ok()?,
            "single_dash_split_flags" => single_dash_split_flags = value.parse::<usize>().ok()?,
            "repeated_char_tools" => repeated_char_tools = value.parse::<usize>().ok()?,
            "repeated_char_flags" => repeated_char_flags = value.parse::<usize>().ok()?,
            "wrapped_prose_tools" => wrapped_prose_tools = value.parse::<usize>().ok()?,
            "wrapped_prose_flags" => wrapped_prose_flags = value.parse::<usize>().ok()?,
            "tail_operand_tools" => tail_operand_tools = value.parse::<usize>().ok()?,
            "tail_operand_flags" => tail_operand_flags = value.parse::<usize>().ok()?,
            "vim_family" => vim_family = decode_vim_family(value),
            "described_flags" => described_flags = value.parse::<f64>().ok()?,
            "describable_flags" => describable_flags = value.parse::<f64>().ok()?,
            "total_flags" => total_flags = value.parse::<usize>().ok()?,
            "total" => total = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(Aggregate {
        pct_flags_with_text: pct_flags_with_text?,
        no_tier_count: no_tier_count?,
        suspicious_count,
        verbatim_count,
        incomplete_count,
        man_shaped_count,
        zero_flag_ok_count,
        framework_detected_count: 0,
        framework_counts: BTreeMap::new(),
        total: total?,
        described_flags,
        describable_flags,
        total_flags,
        misattribution_suspect_tools,
        misattribution_column_aligned_tools,
        existence_fabrication_tools,
        bundle_collapse_tools,
        bundle_destroyed_flags,
        alternation_defect_tools,
        alternation_defect_flags,
        command_table_tools,
        single_dash_split_tools,
        single_dash_split_flags,
        repeated_char_tools,
        repeated_char_flags,
        wrapped_prose_tools,
        wrapped_prose_flags,
        tail_operand_tools,
        tail_operand_flags,
        vim_family,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::row;

    #[test]
    fn parses_its_own_footer_format() {
        let table = "tool  tier(s)\nfoo   carapace\n\n# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=2 verbatim_count=1 man_shaped_count=1 zero_flag_ok_count=4 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_flags_with_text, 42.5);
        assert_eq!(agg.no_tier_count, 3);
        assert_eq!(agg.suspicious_count, 2);
        assert_eq!(agg.verbatim_count, 1);
        assert_eq!(agg.incomplete_count, 0);
        assert_eq!(agg.man_shaped_count, 1);
        assert_eq!(agg.zero_flag_ok_count, 4);
        assert_eq!(agg.total, 10);
    }

    /// A scoreboard written before the structure-sanity column existed has
    /// no `suspicious_count` field at all — `--check` against it must
    /// still work (defaulting to 0) rather than treating the whole footer
    /// as unparseable.
    #[test]
    fn footer_without_suspicious_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.suspicious_count, 0);
    }

    /// Same for `verbatim_count`, added in batch 6 part 5: a scoreboard
    /// from before this batch has no such field.
    #[test]
    fn footer_without_verbatim_count_defaults_to_zero() {
        let table =
            "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.verbatim_count, 0);
    }

    /// Same for `incomplete_count` (spec §6 rule 2b, this batch): a
    /// scoreboard from before the `incomplete` status existed has no such
    /// field.
    #[test]
    fn footer_without_incomplete_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.incomplete_count, 0);
    }

    /// Round-trips through a freshly-written footer, unlike the
    /// backward-compatibility tests above which parse a hand-written one.
    #[test]
    fn incomplete_count_round_trips_through_a_freshly_written_footer() {
        let rows = vec![
            row("curl", 12, Some(100.0), "incomplete"),
            row("git", 34, Some(100.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.incomplete_count, 1);
        let line = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&line).unwrap();
        assert_eq!(parsed.incomplete_count, 1);
    }

    /// Same for `man_shaped_count`, added by this batch ([M-16]'s
    /// exposure enumeration): a scoreboard from before it exists has no
    /// such field, and `--check` against it must still work.
    #[test]
    fn footer_without_man_shaped_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.man_shaped_count, 0);
    }

    /// Same for `zero_flag_ok_count` ([M-15]): a scoreboard from before
    /// this metric existed has no such field, and `--check` against it
    /// must still work.
    #[test]
    fn footer_without_zero_flag_ok_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 man_shaped_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.zero_flag_ok_count, 0);
    }

    /// Same for `describable_flags` (spec §13's metric redefinition): a
    /// scoreboard from before it exists has no such field, and `--check`
    /// against it must still work — `--check` compares `pct_flags_with_text`
    /// values directly and never reconstructs them from this pair, so a
    /// pre-redefinition baseline still round-trips (see
    /// `parse_aggregate_footer`'s doc comment on this field).
    #[test]
    fn footer_without_describable_flags_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 man_shaped_count=1 zero_flag_ok_count=1 total=10 described_flags=4.2000 total_flags=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.describable_flags, 0.0);
    }

    /// A freshly-written footer round-trips `describable_flags` exactly —
    /// this is the field a sharded `--check` run needs to merge partial
    /// scoreboards without re-deriving `pct_flags_with_text` from the rounded
    /// per-row percentage column.
    #[test]
    fn footer_round_trips_describable_flags() {
        let rows = vec![row("git", 34, Some(100.0), "ok")];
        let mut only_row = rows;
        only_row[0].describable = 16;
        let agg = compute_aggregate(&only_row);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.describable_flags, 16.0);
    }

    /// spec §13.1/§13.1b's rename (this task): a scoreboard written under
    /// the old, accuracy-implying `pct_described` key must still parse —
    /// `--check` against a not-yet-regenerated baseline must not suddenly
    /// start failing to parse the footer at all just because the field
    /// changed names. See `aggregate_footer_line`: nothing written by this
    /// module ever emits the old key again, this is read-only compatibility.
    #[test]
    fn footer_reads_the_legacy_pct_described_key_name() {
        let table = "# aggregate: pct_described=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_flags_with_text, 42.5);
    }

    /// Same pattern as every other new-column default: a scoreboard from
    /// before the misattribution detector existed has no such field.
    #[test]
    fn footer_without_misattribution_suspect_tools_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.misattribution_suspect_tools, 0);
    }

    #[test]
    fn footer_round_trips_misattribution_suspect_tools() {
        let mut suspect_row = row("lsof", 42, Some(79.0), "ok");
        suspect_row.misattribution_suspect_count = 1;
        let rows = vec![row("git", 34, Some(100.0), "ok"), suspect_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.misattribution_suspect_tools, 1);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.misattribution_suspect_tools, 1);
    }

    /// Same pattern as every other new-column default: a scoreboard from
    /// before the existence detector existed has no such field.
    #[test]
    fn footer_without_existence_fabrication_tools_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.existence_fabrication_tools, 0);
    }

    #[test]
    fn footer_round_trips_existence_fabrication_tools() {
        let mut fabricated_row = row("tar", 42, Some(79.0), "ok");
        fabricated_row.existence_fabrication_count = 1;
        let rows = vec![row("git", 34, Some(100.0), "ok"), fabricated_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.existence_fabrication_tools, 1);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.existence_fabrication_tools, 1);
    }

    #[test]
    fn missing_footer_returns_none() {
        assert!(parse_aggregate_footer("no footer here\n").is_none());
    }

    #[test]
    fn aggregate_weights_by_flag_count_not_per_tool_average() {
        let rows = vec![
            row("big", 100, Some(100.0), "ok"),
            row("small", 1, Some(0.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        // 100 described out of 101 total, not (100% + 0%)/2 = 50%.
        assert!((agg.pct_flags_with_text - (100.0 / 101.0 * 100.0)).abs() < 0.01);
    }

    /// spec §13's metric redefinition, at aggregate granularity: a tool
    /// whose flags are mostly undescribable-by-construction (synopsis-only)
    /// must not drag the fleet-wide ratio down for that reason — the
    /// aggregate is weighted by each row's *describable* count, not its
    /// raw flag count. Models the git shape directly: 34 raw flags, only
    /// 16 describable, all 16 described (spec's git fixture, post-fix).
    #[test]
    fn aggregate_weights_by_describable_count_not_raw_flag_count() {
        let mut git_like = row("git", 34, Some(100.0), "ok");
        git_like.describable = 16;
        let rows = vec![git_like];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.pct_flags_with_text, 100.0);
        assert_eq!(agg.describable_flags, 16.0);
        assert_eq!(agg.described_flags, 16.0);
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

    /// [M-16]'s enumeration column: a man-shaped root is a *subset* of
    /// verbatim (git's subcommands are both), but not every verbatim root
    /// is man-shaped (some tools produce output the grammar just can't
    /// use, with no man banner in sight) — so the two counts must move
    /// independently, and `man_shaped_count` must never be gated (this is
    /// a brand-new measurement with no baseline, per the task).
    #[test]
    fn aggregate_counts_man_shaped_separately_from_plain_verbatim() {
        let mut man_shaped_row = row("git-bisect", 0, None, "verbatim");
        man_shaped_row.verbatim = true;
        man_shaped_row.man_shaped = true;
        let mut plain_verbatim_row = row("mystery", 0, None, "verbatim");
        plain_verbatim_row.verbatim = true;
        let rows = vec![
            row("clean", 10, Some(100.0), "ok"),
            man_shaped_row,
            plain_verbatim_row,
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.verbatim_count, 2);
        assert_eq!(agg.man_shaped_count, 1);
    }

    /// [M-15]'s own measure: a tool at status `ok` with zero flags at all
    /// (the shape 378 of 1,895 `ok` tools had fleet-wide before the usage-
    /// synopsis flag grammar). A `low-confidence` or `no-tier` tool with
    /// zero flags must not count — only `ok` ones do, since those are the
    /// ones a reader would otherwise trust as "nothing more to find here."
    #[test]
    fn aggregate_counts_ok_tools_with_zero_flags() {
        let rows = vec![
            row("git-like", 0, None, "ok"),
            row("has-flags", 10, Some(90.0), "ok"),
            row("weak", 0, None, "low-confidence"),
            row("nothing", 0, None, "no-tier"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.zero_flag_ok_count, 1);
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
}
