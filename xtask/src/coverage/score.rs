//! Per-tool scoring: runs the extraction pipeline on one tool and turns the
//! result into a [`super::Row`], including the four defect detectors'
//! per-row samples, framework/tier labeling, and the man-page-shaped check.

use super::fingerprint::build_fingerprint;
use super::render_text::truncate_col;
use super::Row;
use crate::alternation;
use crate::bundling;
use crate::existence;
use crate::misattribution::{self, RecordingProbe};
use crate::repeated_char;
use crate::single_dash_long;
use crate::tail_operand;
use crate::wrapped_prose;
use mandible_core::CommandNode;
use mandible_extract::{default_tiers_with_probe, resolve_tool, ExtractionResult, Runner};
use std::sync::Arc;
use std::time::Instant;

/// Cap on how many of one tool's own suspect descriptions feed the
/// fleet-wide sample section — see [`Row::misattribution_samples`].
const MISATTRIBUTION_SAMPLES_PER_ROW: usize = 3;

/// Truncate a suspect's description to a length that keeps one sample line
/// readable — the full text is still in the tree the sweep already wrote,
/// this is a display concern only.
const MISATTRIBUTION_DESC_DISPLAY_LEN: usize = 70;

/// Cap on how many of one tool's own [`crate::existence`] fabrications feed
/// the fleet-wide sample section — mirrors
/// [`MISATTRIBUTION_SAMPLES_PER_ROW`]'s reasoning exactly.
const EXISTENCE_SAMPLES_PER_ROW: usize = 3;

/// Cap on how many of one tool's own [`crate::bundling`] collapses feed the
/// fleet-wide sample section — mirrors [`EXISTENCE_SAMPLES_PER_ROW`].
const BUNDLE_SAMPLES_PER_ROW: usize = 3;

/// Cap on how many of one tool's own [`crate::alternation`] findings feed
/// the fleet-wide sample section — mirrors [`BUNDLE_SAMPLES_PER_ROW`].
const ALTERNATION_SAMPLES_PER_ROW: usize = 3;

/// Cap on how many of one tool's own [`crate::single_dash_long`] splits or
/// [`crate::repeated_char`] misreads feed their fleet-wide sample sections —
/// mirrors [`BUNDLE_SAMPLES_PER_ROW`]. Shared by both families; read from
/// the same capture pass.
const SPLIT_SAMPLES_PER_ROW: usize = 3;

/// Cap on how many of one tool's own [`crate::wrapped_prose`] fabrications
/// or [`crate::tail_operand`] findings feed their fleet-wide sample
/// sections — mirrors [`SPLIT_SAMPLES_PER_ROW`].
const FAMILY_DETECTOR_SAMPLES_PER_ROW: usize = 3;

pub(super) fn score_one(tool: &str) -> Row {
    let start = Instant::now();
    // A fresh [`RecordingProbe`] per tool (never shared across the sweep):
    // it's a transparent passthrough to [`mandible_extract::exec::LiveProbe`]
    // that also remembers the bytes each call returned, so
    // [`crate::misattribution`] can read the tool's own raw `--help` text
    // after extraction without a second probe — see that module's doc
    // comment. This spawns nothing a plain `default_tiers()` runner
    // wouldn't already have spawned.
    let probe = Arc::new(RecordingProbe::new());
    let runner = Runner::new(default_tiers_with_probe(probe.clone()));
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
    let describable = result.describable_flag_count();

    // Status derivation (structure-sanity count, verbatim flag,
    // %described, and the final label) is computed once in `status.rs`
    // and shared verbatim with the corpus runner — see that module's doc
    // comment for why an independent second definition here would be a
    // drift risk, not a convenience.
    let status = crate::status::compute(&result);
    let man_shaped = root_is_man_shaped(&result);

    // Zero additional probes: `probe.root_help_text()` reads bytes the
    // `runner.extract_full` call above already fetched (see the doc
    // comment on `probe` above and `crate::misattribution`'s own doc
    // comment). Both the raw text and a root node are required — a tool
    // with no root, or whose only text came back empty, has nothing to
    // check.
    let (misattribution_suspect_count, misattribution_column_aligned, misattribution_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = misattribution::detect(&raw, root);
                let samples = report
                    .suspects
                    .iter()
                    .take(MISATTRIBUTION_SAMPLES_PER_ROW)
                    .map(format_misattribution_sample)
                    .collect();
                (report.suspect_count(), report.column_aligned, samples)
            }
            _ => (0, false, Vec::new()),
        };

    // Same captured text, zero additional probes — [`crate::existence`]'s
    // own doc comment on why re-reading `probe.root_help_text()` a second
    // time here (rather than sharing one `raw` binding with the
    // misattribution block above) costs nothing: both are cheap `Option<String>`
    // clones of bytes already in memory, not a second fetch.
    let (existence_fabrication_count, existence_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = existence::detect(&raw, root);
                let samples = report
                    .fabrications
                    .iter()
                    .take(EXISTENCE_SAMPLES_PER_ROW)
                    .map(format_existence_sample)
                    .collect();
                (report.fabrication_count(), samples)
            }
            _ => (0, Vec::new()),
        };

    // Third read of the same already-fetched capture, still zero probes —
    // see [`crate::bundling`]'s doc comment on why this detector needs no
    // new argv at all: the collapse is visible in the text the sweep
    // already has.
    let (bundle_collapse_count, bundle_destroyed_flags, bundle_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = bundling::detect(&raw, root);
                let samples = report
                    .collapses
                    .iter()
                    .take(BUNDLE_SAMPLES_PER_ROW)
                    .map(format_bundle_sample)
                    .collect();
                (
                    report.collapse_count(),
                    report.destroyed_flag_count(),
                    samples,
                )
            }
            _ => (0, 0, Vec::new()),
        };

    // Fourth read of the same already-fetched capture, still zero probes.
    let (alternation_defect_count, alternation_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = alternation::detect(&raw, root);
                let samples = report
                    .findings
                    .iter()
                    .take(ALTERNATION_SAMPLES_PER_ROW)
                    .map(format_alternation_sample)
                    .collect();
                (report.finding_count(), samples)
            }
            _ => (0, Vec::new()),
        };
    // Fourth read of the same already-fetched capture, still zero probes
    // — `crate::commandtable`'s shape is visible in the text the sweep
    // already has, exactly like the three detectors above.
    let command_table_count = match (probe.root_help_text(), result.root.as_ref()) {
        (Some(raw), Some(root)) if !raw.trim().is_empty() => {
            crate::commandtable::detect(&raw, root).missing.len()
        }
        _ => 0,
    };
    // The two remaining families of the three that share the `short &&
    // !long && value_name` fingerprint, read off the same capture on the
    // same pass and costing the same zero additional subprocess spawns —
    // see `split_family_counts`.
    let (
        single_dash_split_count,
        single_dash_samples,
        repeated_char_misread_count,
        repeated_char_samples,
    ) = split_family_counts(probe.root_help_text(), result.root.as_ref());
    let (wrapped_prose_count, wrapped_prose_samples, tail_operand_count, tail_operand_samples) =
        family_detector_counts(probe.root_help_text(), result.root.as_ref());
    let vim_family = vim_family_counts(probe.root_help_text(), result.root.as_ref());
    Row {
        tool: tool.to_string(),
        tiers: tiers_label,
        framework,
        nodes,
        flags,
        describable,
        pct_flags_with_text: status.pct_flags_with_text,
        ms,
        suspicious_nodes: status.suspicious_nodes,
        verbatim: status.verbatim,
        man_shaped,
        misattribution_suspect_count,
        misattribution_column_aligned,
        misattribution_samples,
        existence_fabrication_count,
        existence_samples,
        bundle_collapse_count,
        bundle_destroyed_flags,
        bundle_samples,
        alternation_defect_count,
        alternation_samples,
        command_table_count,
        single_dash_split_count,
        single_dash_samples,
        repeated_char_misread_count,
        repeated_char_samples,
        wrapped_prose_count,
        wrapped_prose_samples,
        tail_operand_count,
        tail_operand_samples,
        vim_family,
        status: status.label,
        fingerprint: build_fingerprint(result.root.as_ref()),
    }
}

/// One [`crate::alternation`] finding, rendered as a single audit-section
/// line: the group as the tool wrote it, and what went wrong with it.
fn format_alternation_sample(finding: &alternation::Finding) -> String {
    format!("{}: {} — {}", finding.path, finding.group, finding.detail)
}

/// One suspect, rendered as a single audit-section line: the tool/path, the
/// flag, its (length-capped) description, and which tokens triggered it.
fn format_misattribution_sample(suspect: &misattribution::Suspect) -> String {
    let mut desc = suspect.description.clone();
    if desc.chars().count() > MISATTRIBUTION_DESC_DISPLAY_LEN {
        desc = truncate_col(&desc, MISATTRIBUTION_DESC_DISPLAY_LEN);
    }
    format!(
        "{}: {} {:?} contains {}",
        suspect.path,
        suspect.flag,
        desc,
        suspect.offending_tokens.join(", "),
    )
}

/// One fabrication, rendered as a single audit-section line: which node
/// path carries it, whether it's a subcommand, a flag or an operand, and the specific
/// offending spelling — mirrors [`format_misattribution_sample`]'s shape.
fn format_existence_sample(fabrication: &existence::Fabrication) -> String {
    let kind = match fabrication.kind {
        existence::FabricationKind::Subcommand => "subcommand",
        existence::FabricationKind::Flag => "flag",
        existence::FabricationKind::Positional => "positional",
    };
    format!(
        "{}: invented {kind} {:?} not found in raw text",
        fabrication.path, fabrication.name,
    )
}

/// One collapse, rendered as a single audit-section line: which node path
/// carries it, the raw cluster, and how many real flags it destroyed —
/// mirrors [`format_existence_sample`]'s shape.
fn format_bundle_sample(collapse: &bundling::Collapse) -> String {
    format!(
        "{}: {:?} read as {} with a required value — {} real flag(s) destroyed",
        collapse.path, collapse.cluster, collapse.spelling, collapse.destroyed,
    )
}

/// One split, rendered as a single audit-section line — mirrors
/// [`format_bundle_sample`]'s shape.
fn format_single_dash_sample(split: &single_dash_long::Split) -> String {
    format!(
        "{}: {:?} split into {} plus a required value",
        split.path, split.token, split.spelling,
    )
}

/// One repeated-character misread, rendered as a single audit-section line —
/// mirrors [`format_single_dash_sample`]'s shape.
fn format_repeated_char_sample(misread: &repeated_char::Misread) -> String {
    format!(
        "{}: {:?} read as {} carrying its own letter as a value",
        misread.path, misread.token, misread.spelling,
    )
}

/// One wrapped-prose-row-boundary fabrication, rendered as a single audit-
/// section line.
fn format_wrapped_prose_sample(finding: &wrapped_prose::Finding) -> String {
    format!(
        "{:?} fabricated from the line {:?}",
        finding.flag, finding.line
    )
}

/// One unparsed-tail-operand finding, rendered as a single audit-section
/// line.
fn format_tail_operand_sample(finding: &tail_operand::Finding) -> String {
    format!(
        "{:?} never became a positional, from {:?}",
        finding.operand, finding.usage_line
    )
}

/// [`wrapped_prose::detect`] and [`tail_operand::detect`], run over one
/// tool's already-captured text and tree — split out of [`score_one`]
/// purely to keep that function under clippy's line-count lint.
/// [`single_dash_long::detect`] and [`repeated_char::detect`], the other
/// two families sharing the `short && !long && value_name` fingerprint —
/// split out of [`score_one`] for the same line-count reason as
/// [`family_detector_counts`].
fn split_family_counts(
    raw: Option<String>,
    root: Option<&CommandNode>,
) -> (usize, Vec<String>, usize, Vec<String>) {
    let (Some(raw), Some(root)) = (raw, root) else {
        return (0, Vec::new(), 0, Vec::new());
    };
    if raw.trim().is_empty() {
        return (0, Vec::new(), 0, Vec::new());
    }
    let sd = single_dash_long::detect(&raw, root);
    let sd_samples = sd
        .splits
        .iter()
        .take(SPLIT_SAMPLES_PER_ROW)
        .map(format_single_dash_sample)
        .collect();
    let rc = repeated_char::detect(&raw, root);
    let rc_samples = rc
        .misreads
        .iter()
        .take(SPLIT_SAMPLES_PER_ROW)
        .map(format_repeated_char_sample)
        .collect();
    (sd.split_count(), sd_samples, rc.misread_count(), rc_samples)
}

fn family_detector_counts(
    raw: Option<String>,
    root: Option<&CommandNode>,
) -> (usize, Vec<String>, usize, Vec<String>) {
    let (Some(raw), Some(root)) = (raw, root) else {
        return (0, Vec::new(), 0, Vec::new());
    };
    if raw.trim().is_empty() {
        return (0, Vec::new(), 0, Vec::new());
    }
    let wp = wrapped_prose::detect(&raw, root);
    let wp_samples = wp
        .findings
        .iter()
        .take(FAMILY_DETECTOR_SAMPLES_PER_ROW)
        .map(format_wrapped_prose_sample)
        .collect();
    let to = tail_operand::detect(&raw, root);
    let to_samples = to
        .findings
        .iter()
        .take(FAMILY_DETECTOR_SAMPLES_PER_ROW)
        .map(format_tail_operand_sample)
        .collect();
    (
        wp.finding_count(),
        wp_samples,
        to.finding_count(),
        to_samples,
    )
}

/// The seven vim-family detectors (atlas S-105..S-111), read off the same
/// already-captured raw text and tree. One function rather than seven
/// named locals: every family shares the exact same (name, finding count,
/// capped samples) shape, and `Row::vim_family`/`Aggregate::vim_family`
/// read that list generically rather than by seven repeated field names.
fn vim_family_counts(
    raw: Option<String>,
    root: Option<&CommandNode>,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let (Some(raw), Some(root)) = (raw, root) else {
        return Vec::new();
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;

    let pp = crate::plus_prefixed_option::detect(&raw, root);
    let eo = crate::end_of_options_marker::detect(&raw, root);
    let ss = crate::single_space_description_column::detect(&raw, root);
    let uv = crate::usage_only_value_name::detect(&raw, root);
    let sv = crate::second_optional_value_dropped::detect(&raw, root);
    let pq = crate::parenthetical_qualifier_as_value::detect(&raw, root);
    let oj = crate::or_joined_alias::detect(&raw, root);

    vec![
        (
            "plus-prefixed-option",
            pp.finding_count(),
            pp.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?} never became a flag, from {:?}", f.token, f.line))
                .collect(),
        ),
        (
            "end-of-options-marker",
            eo.finding_count(),
            eo.findings
                .iter()
                .take(cap)
                .map(|f| format!("`--` never became a flag, from {:?}", f.line))
                .collect(),
        ),
        (
            "single-space-description-column",
            ss.finding_count(),
            ss.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?} never attached to {:?}", f.description, f.spellings))
                .collect(),
        ),
        (
            "usage-only-value-name",
            uv.finding_count(),
            uv.findings
                .iter()
                .take(cap)
                .map(|f| format!("-{} never carried value name {:?}", f.flag, f.value_name))
                .collect(),
        ),
        (
            "second-optional-value-dropped",
            sv.finding_count(),
            sv.findings
                .iter()
                .take(cap)
                .map(|f| format!("-{} lost {:?}", f.flag, f.second_value))
                .collect(),
        ),
        (
            "parenthetical-qualifier-as-value",
            pq.finding_count(),
            pq.findings
                .iter()
                .take(cap)
                .map(|f| format!("-{} carries value name {:?}", f.flag, f.value_name))
                .collect(),
        ),
        (
            "or-joined-alias",
            oj.finding_count(),
            oj.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?} never became an alias of {:?}", f.second, f.first))
                .collect(),
        ),
    ]
}

/// True when the root's captured `--help` output was detected as a
/// rendered man page (spec [M-16]).
///
/// Sends no new probe: reads `CommandNode::unparsed`, which `help_text::
/// build_node` sets to the raw captured lines whenever nothing parsed as
/// structure (spec §7 Tier B step 3), and re-runs
/// [`mandible_extract::help_text::is_man_page_banner`] over the first line
/// to tell a man-page banner apart from ordinary unparseable output.
fn root_is_man_shaped(result: &ExtractionResult) -> bool {
    let Some(root) = result.root.as_ref() else {
        return false;
    };
    let Some(first_line) = root.unparsed.iter().find(|t| !t.as_str().trim().is_empty()) else {
        return false;
    };
    mandible_extract::help_text::is_man_page_banner(first_line.as_str())
}

/// Compact `"<framework name> (<method>)"` label for the scoreboard's
/// `framework` column, or `"—"` when Tier A′ didn't identify one (spec §7
/// Tier A′ step 3). Framework name comes from `CommandNode::detected_framework`
/// (set only by Tier B, per-field authority resolution never lets `None`
/// displace `Some`). Method is re-derived via `framework::identify_from_artifact`
/// (memoized per binary path, spawns no process), since `Source`/`Provenance`
/// stay framework-agnostic (spec §4.2).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_tier_name_maps_known_names() {
        assert_eq!(short_tier_name("known_specs::carapace"), "carapace");
        assert_eq!(short_tier_name("help_text"), "help");
        assert_eq!(short_tier_name("something_else"), "something_else");
    }

    fn extraction_result_with_unparsed(tool: &str, first_line: &str) -> ExtractionResult {
        use mandible_core::{CommandNode, Provenance, Source, Text};
        let mut root = CommandNode::new(tool, Provenance::with_confidence(Source::HelpText, 0.0));
        root.unparsed = vec![Text::sanitize(first_line)];
        ExtractionResult {
            tool: tool.to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        }
    }

    /// True positive: a captured `unparsed` first line carrying the man
    /// banner shape is detected, via the exact same rule
    /// `sections::is_man_page_banner` exposes — this is `root_is_man_shaped`
    /// reading data the pipeline already captured, not a second probe.
    #[test]
    fn root_is_man_shaped_true_positive_on_a_captured_man_banner() {
        let result = extraction_result_with_unparsed(
            "git-bisect",
            "GIT-BISECT(1)     Git Manual     GIT-BISECT(1)",
        );
        assert!(root_is_man_shaped(&result));
    }

    /// A root that degraded to verbatim for an *ordinary* reason (the
    /// grammar just found nothing usable, no man banner involved) must not
    /// be counted as man-shaped — the two "gave up" reasons are distinct,
    /// which is the entire reason this function re-checks the captured
    /// text instead of trusting `verbatim` alone.
    #[test]
    fn root_is_man_shaped_false_when_verbatim_for_a_non_man_reason() {
        let result = extraction_result_with_unparsed(
            "mystery",
            "This tool prints only a friendly banner and nothing else.",
        );
        assert!(!root_is_man_shaped(&result));
    }

    /// git's own *root* must never register as man-shaped ([M-16]'s
    /// central subtlety: only its subcommands render man pages). A root
    /// that parsed real structure never carries `unparsed` at all, so
    /// there is no captured text to test the banner against.
    #[test]
    fn root_is_man_shaped_false_when_root_parsed_structurally() {
        use mandible_core::{CommandNode, Provenance, Source};
        let root = CommandNode::new("git", Provenance::with_confidence(Source::HelpText, 0.8));
        let result = ExtractionResult {
            tool: "git".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(!root_is_man_shaped(&result));
    }

    #[test]
    fn root_is_man_shaped_false_when_no_tier_produced_a_root() {
        let result = ExtractionResult {
            tool: "nothing".to_string(),
            root: None,
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(!root_is_man_shaped(&result));
    }
}
