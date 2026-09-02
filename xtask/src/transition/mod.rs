//! The per-tool sweep transition report (WS2 part 1): a semantic diff
//! between two independently-generated [`crate::coverage::ScoreFormat::Text`]
//! scoreboards. Automates the manual per-tool-diff step that has actually
//! caught real fleet-wide regressions the aggregate `%flags_text` gate and
//! the 4-fixture corpus both stayed green through — a flag-weighted
//! average moves by hundredths of a percent on a handful of regressed
//! tools out of a couple thousand.
//!
//! Reports gains and losses as two separate totals, never netted
//! ([`FlagDelta`], [`render_markdown`]): summing them into one signed
//! number would hide a real regression on a few tools behind large gains
//! elsewhere.
//!
//! Non-blocking by design (maintainer decision D4): `cargo xtask
//! sweep-diff` always exits `0`; no `--check`/`--gate` flag exists here.
//!
//! Truncated tool names are a real hazard: `coverage::truncate_col` elides
//! long names with `…`, so two different names can render identically. A
//! truncated row (detected in [`parse_scoreboard`]) is dropped from the
//! comparison entirely rather than joined, to avoid diffing one tool's
//! before against a different tool's after — the drop count is reported
//! ([`ParsedScoreboard::truncated_dropped`]).

mod diff;
mod fingerprint;
mod parse;
mod render_markdown;
mod render_text;

pub use diff::diff;
pub use parse::parse_scoreboard;
pub use render_markdown::render_markdown;
pub use render_text::render_text;

use fingerprint::ParsedFingerprint;
use std::collections::BTreeMap;

/// The single-probe extraction timeout (`mandible_extract::help_text::mod`'s
/// and `native::mod`'s own private `EXTRACT_TIMEOUT`, `Duration::from_secs(10)`).
/// Duplicated here, not imported: `xtask` has no path to that private
/// constant. If it ever changes, re-measure and update both places in the
/// same commit (AGENTS.md's environment-facts discipline).
pub(super) const EXTRACT_TIMEOUT_MS: u128 = 10_000;

/// True when `ms` is close enough to [`EXTRACT_TIMEOUT_MS`] that a status
/// derived from it is a statement about the machine, not the parser (spec
/// §13.1b rule 3; maintainer decision D4).
///
/// Lower bound only, no upper: `score_one` recurses into every discovered
/// subcommand, each under its own `EXTRACT_TIMEOUT`, so a tool's total
/// `ms` legitimately exceeds one cap's worth of wall time by design
/// (`waagent2.0` measured both 41.9s and 21.4s across identical code, both
/// multiples of the 10s cap). A symmetric "within 2x" band would miss
/// that case; "at least half the cap", open-ended above, does not.
fn near_timeout_cap(ms: u128) -> bool {
    ms.saturating_mul(2) >= EXTRACT_TIMEOUT_MS
}

/// One data row parsed back out of a rendered
/// [`crate::coverage::ScoreFormat::Text`] scoreboard.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRow {
    pub tool: String,
    pub tiers: String,
    pub framework: String,
    pub nodes: usize,
    pub flags: usize,
    pub pct_flags_with_text: Option<f64>,
    pub ms: u128,
    pub suspicious_nodes: usize,
    pub man_shaped: bool,
    /// `None` on a scoreboard rendered before the misattribution detector
    /// existed (no `misattr` column at all) — see [`has_misattr_column`].
    /// Distinct from `Some(0)` ("column present, zero suspects"), so a
    /// reader can tell "not measured yet" from "measured, clean".
    pub misattribution_suspect_count: Option<usize>,
    /// `None` on a scoreboard rendered before the existence detector
    /// existed (no `exist` column at all) — see [`has_existence_column`].
    /// Same `None`-vs-`Some(0)` distinction as
    /// `misattribution_suspect_count` above.
    pub existence_fabrication_count: Option<usize>,
    /// `None` on a scoreboard rendered before the bundled-short-flag
    /// detector existed (no `bundle` column at all) — see
    /// [`has_bundle_column`]. Same `None`-vs-`Some(0)` distinction as the
    /// two counts above.
    pub bundle_collapse_count: Option<usize>,
    pub status: String,
}

impl ParsedRow {
    fn near_cap(&self) -> bool {
        near_timeout_cap(self.ms)
    }
}

/// The result of parsing one scoreboard: every clean data row, keyed by
/// tool name for the join in [`diff`], plus counts of what had to be
/// dropped and why — both surfaced in the report rather than silently
/// swallowed (this module's own doc comment on the truncation hazard).
#[derive(Debug, Default)]
pub struct ParsedScoreboard {
    pub rows: BTreeMap<String, ParsedRow>,
    /// Rows dropped because the tool-name column was truncated
    /// (`coverage::truncate_col`'s `…` marker) — never joined, since a
    /// truncated name can collide with a different tool's truncated name.
    pub truncated_dropped: usize,
    /// Rows dropped because a numeric field didn't parse (a hand-edited or
    /// corrupted scoreboard, or a row whose content overflowed its nominal
    /// column width and desynced every fixed-offset field after it — see
    /// this module's doc comment). Never expected on a scoreboard this
    /// binary itself produced; tracked so a malformed input fails visibly
    /// small rather than silently large.
    pub unparseable_dropped: usize,
    /// Every tool's field-level fingerprint, parsed from the scoreboard's
    /// `#fp`/`#fp2` footer lines (`coverage::fingerprint_lines`'s own doc
    /// comment has the line shape). **Absent for a scoreboard rendered
    /// before this footer existed** — a tool missing from this map (as
    /// opposed to present with an empty [`ParsedFingerprint`]) means "not
    /// measured," mirrored in [`diff`] by skipping field-level comparison
    /// for that tool entirely rather than reporting a false wholesale
    /// removal of every flag it has.
    pub fingerprints: BTreeMap<String, ParsedFingerprint>,
    /// Which fingerprint wire-format version this scoreboard's footer lines
    /// were written in, detected from the line prefix actually present
    /// (`None` when the scoreboard carries no fingerprint footer at all —
    /// see [`FingerprintFormat`] and [`fingerprint_format_mismatch`]).
    pub fingerprint_format: Option<FingerprintFormat>,
}

/// Which `#fp`-family wire-format version a scoreboard's fingerprint footer
/// was written in. Exists because the entity-identity strings the two
/// versions embed are shaped differently and must never be joined as if
/// they were the same key — see [`fingerprint_format_mismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintFormat {
    /// The pre-generalization format: `#fp <tool>\t<subs>\t<flags>` lines,
    /// flags only, entity ids with no `EntityKind` tag
    /// (`(root)::--flag`).
    V1,
    /// The current format (`coverage::FP_LINE_PREFIX_V2`'s doc comment):
    /// `#fp2 <tool>\t<subs>\t<entities>` lines, every `EntityKind`, entity
    /// ids carrying their kind (`(root)::Flag::--flag`,
    /// `(root)::Modifier::d`, `(root)::EnvVar::BPFTRACE_BTF`).
    V2,
}

/// Refuse to treat a V1-footer scoreboard and a V2-footer scoreboard as
/// comparable: the entity `id` strings differ in shape
/// (`(root)::--flag` vs `(root)::Flag::--flag`), so [`field_diff`] would
/// report every entity as removed on one side and added on the other — a
/// false wholesale loss/gain indistinguishable from a real regression.
/// Callers (`run_sweep_diff`) call this before [`diff`] and bail on the
/// returned message.
///
/// Returns `None` when either side carries no fingerprint footer at all,
/// or both sides agree on a format.
pub fn fingerprint_format_mismatch(
    before: &ParsedScoreboard,
    after: &ParsedScoreboard,
) -> Option<String> {
    match (before.fingerprint_format, after.fingerprint_format) {
        (Some(b), Some(a)) if b != a => Some(format!(
            "fingerprint format mismatch: --before scoreboard carries {b:?} fingerprints, \
             --after carries {a:?} — these use differently-shaped entity identities and cannot \
             be field-diffed against each other (a V1/V2 join would misreport every entity as \
             removed on one side and added on the other). Re-run the sweep that produced the \
             {v1_side} scoreboard with the current xtask to get a matching pair.",
            v1_side = if b == FingerprintFormat::V1 {
                "--before"
            } else {
                "--after"
            },
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::fingerprint::*;
    use super::*;
    use crate::coverage::{run_over, ScoreFormat, TOOL_COL_WIDTH};

    /// Round-trips a scoreboard this binary just rendered itself — the
    /// sanity floor: if the renderer and parser ever drift, this is the
    /// test that catches it before a real sweep does.
    ///
    /// Checks `status`/`existence_fabrication_count`/
    /// `misattribution_suspect_count` explicitly, not just row presence:
    /// a fixed-offset desync (e.g. adding `exist` — [`crate::existence`],
    /// this task — without teaching this module its width) still leaves
    /// the `tool` column, and therefore the row key, intact, so a presence-
    /// only check would have stayed green through exactly that bug. This
    /// version would not have.
    #[test]
    fn parses_a_freshly_rendered_scoreboard_back_out() {
        let (table, _agg) = run_over(
            vec!["sh".to_string(), "true".to_string()],
            None,
            false,
            ScoreFormat::Text,
        );
        let parsed = parse_scoreboard(&table);
        assert_eq!(parsed.truncated_dropped, 0);
        assert_eq!(parsed.unparseable_dropped, 0);
        for tool in ["sh", "true"] {
            let row = parsed
                .rows
                .get(tool)
                .unwrap_or_else(|| panic!("{tool} row parsed"));
            assert_eq!(row.misattribution_suspect_count, Some(0));
            assert_eq!(row.existence_fabrication_count, Some(0));
            assert!(
                row.status
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase()),
                "status field looks corrupted (fixed-offset desync?): {:?}",
                row.status
            );
        }
    }

    fn sample_text(rows: &[&str]) -> String {
        let mut out = String::from(
            "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr  status\n",
        );
        for r in rows {
            out.push_str(r);
            out.push('\n');
        }
        out.push_str("# aggregate: pct_flags_with_text=90.00 no_tier_count=0 total=1\n");
        out
    }

    /// A hand-built row at the current (with `misattr`) column widths,
    /// matching exactly what `render_text` would produce for these values.
    fn row_line(tool: &str, status: &str, flags: usize, ms: u128) -> String {
        format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}",
            tool, "help", "—", 1, flags, "100%", ms, 0, "-", 0, status,
        )
    }

    #[test]
    fn parses_a_hand_built_row_at_current_widths() {
        let text = sample_text(&[&row_line("git", "ok", 34, 120)]);
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.ms, 120);
        assert_eq!(row.status, "ok");
        assert_eq!(row.misattribution_suspect_count, Some(0));
    }

    /// A hand-built row at the current (`misattr` + `exist`) column widths —
    /// the shape every scoreboard this task's own `cargo xtask coverage`
    /// run produces.
    fn row_line_with_existence(
        tool: &str,
        status: &str,
        flags: usize,
        ms: u128,
        existence_fabrication_count: usize,
    ) -> String {
        format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}{:>6}  {}",
            tool, "help", "—", 1, flags, "100%", ms, 0, "-", 0, existence_fabrication_count, status,
        )
    }

    #[test]
    fn parses_a_hand_built_row_with_the_existence_column() {
        let header = "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr exist  status\n";
        let row = row_line_with_existence("git", "ok", 34, 120, 2);
        let text = format!(
            "{header}{row}\n# aggregate: pct_flags_with_text=90.00 no_tier_count=0 total=1\n"
        );
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.status, "ok");
        assert_eq!(row.misattribution_suspect_count, Some(0));
        assert_eq!(row.existence_fabrication_count, Some(2));
    }

    /// A scoreboard from before the misattribution detector existed has no
    /// `misattr` column at all (and therefore no `exist` column either,
    /// since `exist` was only ever appended after `misattr`) — the four
    /// real scratch scoreboards used to verify this module during
    /// development are exactly this shape.
    #[test]
    fn parses_a_legacy_row_with_no_misattr_column() {
        let header = "tool                     tier(s)            framework                    nodes   flags   %described     ms suspect   man  status\n";
        let row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}  {}\n",
            "git", "help", "—", 1, 34, "100%", 120, 0, "-", "ok",
        );
        let text =
            format!("{header}{row}# aggregate: pct_described=90.00 no_tier_count=0 total=1\n");
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.misattribution_suspect_count, None);
        assert_eq!(row.existence_fabrication_count, None);
    }

    /// The exact hazard this module's doc comment describes: a truncated
    /// tool name must never be joined, because two different real names can
    /// truncate to the same string.
    #[test]
    fn truncated_tool_names_are_dropped_not_joined() {
        let long_name = "a".repeat(TOOL_COL_WIDTH + 5);
        let truncated = format!(
            "{}…",
            long_name
                .chars()
                .take(TOOL_COL_WIDTH - 1)
                .collect::<String>()
        );
        let text = sample_text(&[&row_line(&truncated, "ok", 5, 50)]);
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.truncated_dropped, 1);
        assert!(parsed.rows.is_empty());
    }

    /// A short tool name that merely happens to be padded to the column
    /// width is not truncated and must parse normally.
    #[test]
    fn short_tool_names_are_not_mistaken_for_truncated() {
        let text = sample_text(&[&row_line("git", "ok", 5, 50)]);
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.truncated_dropped, 0);
        assert!(parsed.rows.contains_key("git"));
    }

    fn scoreboard(rows: Vec<(&str, &str, usize, u128)>) -> ParsedScoreboard {
        let lines: Vec<String> = rows
            .iter()
            .map(|(tool, status, flags, ms)| row_line(tool, status, *flags, *ms))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        parse_scoreboard(&sample_text(&refs))
    }

    #[test]
    fn diff_reports_status_transitions() {
        let before = scoreboard(vec![("foo", "low-confidence", 5, 50)]);
        let after = scoreboard(vec![("foo", "ok", 5, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.status_transitions.len(), 1);
        assert_eq!(t.status_transitions[0].before, "low-confidence");
        assert_eq!(t.status_transitions[0].after, "ok");
    }

    /// The central rule this module exists to enforce: a tool that gains
    /// 2,000 flags elsewhere never cancels out a tool that lost 6 — losses
    /// and gains are reported as two independent totals.
    #[test]
    fn gains_and_losses_are_never_netted() {
        let before = scoreboard(vec![("big", "ok", 100, 50), ("lsof", "ok", 100, 50)]);
        let after = scoreboard(vec![("big", "ok", 2100, 50), ("lsof", "ok", 94, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.flag_gains.len(), 1);
        assert_eq!(t.flag_gains[0].delta(), 2000);
        assert_eq!(t.flag_losses.len(), 1);
        assert_eq!(t.flag_losses[0].delta(), -6);
        let md = render_markdown(&t);
        assert!(md.contains("6 flag(s) lost across 1 tool(s)"));
        assert!(md.contains("2000 flag(s) gained across 1 tool(s)"));
    }

    #[test]
    fn appeared_and_disappeared_tools_are_reported() {
        let before = scoreboard(vec![("old", "ok", 5, 50)]);
        let after = scoreboard(vec![("new", "ok", 5, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.appeared, vec!["new"]);
        assert_eq!(t.disappeared, vec!["old"]);
    }

    /// The timeout-bucketing proof: a tool whose elapsed time crosses half
    /// the single-probe cap must be excluded from status transitions and
    /// flag deltas, landing only in its own section, exactly as `waagent2.0`
    /// (AGENTS.md) should have been treated instead of read as a real
    /// regression.
    #[test]
    fn near_cap_tools_are_excluded_from_every_gated_dimension() {
        let before = scoreboard(vec![("waagent2.0", "ok", 40, 8_000)]);
        let after = scoreboard(vec![("waagent2.0", "verbatim", 0, 6_000)]);
        let t = diff(&before, &after);
        assert!(t.status_transitions.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.flag_gains.is_empty());
        assert_eq!(t.near_cap, vec!["waagent2.0"]);
        let md = render_markdown(&t);
        assert!(md.contains("Excluded — near the 10s timeout cap"));
        assert!(md.contains("waagent2.0"));
    }

    #[test]
    fn ms_well_under_the_cap_is_not_bucketed_as_near_cap() {
        assert!(!near_timeout_cap(500));
        assert!(!near_timeout_cap(4_999));
    }

    #[test]
    fn ms_at_or_past_half_the_cap_is_near_cap() {
        assert!(near_timeout_cap(5_000));
        assert!(near_timeout_cap(9_999));
        assert!(near_timeout_cap(10_000));
        // Well past the cap — the multi-probe `waagent2.0` shape — still
        // counts, per `near_timeout_cap`'s own doc comment on why there's
        // no upper bound.
        assert!(near_timeout_cap(41_900));
    }

    #[test]
    fn render_text_produces_a_structured_non_diff_report() {
        let before = scoreboard(vec![("foo", "ok", 5, 50)]);
        let after = scoreboard(vec![("foo", "suspicious", 5, 50)]);
        let t = diff(&before, &after);
        let text = render_text(&t);
        assert!(text.contains("foo: ok -> suspicious"));
    }

    /// Attach `#fp` fingerprints to a scoreboard already built by
    /// [`scoreboard`] — the counts/status/tiers/framework columns and the
    /// field-level fingerprint are independent inputs to [`diff`], and a
    /// test that wants to hold the former fixed while varying only the
    /// latter (exactly PR #14's shape: flag counts and status untouched,
    /// only field content changed) needs to set both.
    fn scoreboard_with_fp(
        rows: Vec<(&str, &str, usize, u128)>,
        fps: Vec<(&str, ParsedFingerprint)>,
    ) -> ParsedScoreboard {
        let mut sb = scoreboard(rows);
        for (tool, fp) in fps {
            sb.fingerprints.insert(tool.to_string(), fp);
        }
        sb
    }

    fn flag_fp(
        has_description: bool,
        description_hash: Option<u64>,
        choices_hash: Option<u64>,
        value_name: Option<&str>,
    ) -> ParsedFlagFingerprint {
        ParsedFlagFingerprint {
            has_description,
            description_hash,
            choices_hash,
            value_name: value_name.map(str::to_string),
        }
    }

    /// **The exact PR #14 shape, description half**: `--strip`'s
    /// description was deleted while every count-based column (flags,
    /// status, tiers, framework) stayed put. Proves the new field-level
    /// dimension catches it *and*, by asserting every pre-existing
    /// dimension is empty, documents precisely what the old
    /// count/status-only comparison had to work with — nothing. See this
    /// module's own doc comment and the CHANGELOG entry on the detector
    /// that first shipped this exact regression.
    #[test]
    fn field_diff_catches_a_description_only_change() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--strip".to_string(),
            flag_fp(true, Some(111), None, None),
        );
        let after_fp = ParsedFingerprint {
            flags: {
                let mut m = BTreeMap::new();
                m.insert(
                    "(root)::--strip".to_string(),
                    flag_fp(false, None, None, None),
                );
                m
            },
            subcommands: Default::default(),
        };

        let before = scoreboard_with_fp(vec![("pngfix", "ok", 3, 20)], vec![("pngfix", before_fp)]);
        let after = scoreboard_with_fp(vec![("pngfix", "ok", 3, 20)], vec![("pngfix", after_fp)]);

        let t = diff(&before, &after);

        // Every dimension the pre-existing comparison had: all quiet.
        assert!(t.status_transitions.is_empty());
        assert!(t.flag_gains.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.appeared.is_empty() && t.disappeared.is_empty());

        // The new field-level dimension: caught.
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pngfix");
        assert_eq!(
            t.field_diffs[0].description_changed,
            vec!["(root)::--strip"]
        );
        assert!(t.field_diffs[0].choices_changed.is_empty());
        assert!(
            !t.is_identical(),
            "a deleted description must not report as an identical run"
        );
    }

    /// **The exact PR #14 shape, choices half**: `--guesswork` had a
    /// fabricated choices list attached while flag counts and status stayed
    /// put — the other half of the same regression.
    #[test]
    fn field_diff_catches_a_choices_only_change() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        let mut after_fp = ParsedFingerprint::default();
        after_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(1), Some(999), None),
        );

        let before =
            scoreboard_with_fp(vec![("pod2man", "ok", 3, 20)], vec![("pod2man", before_fp)]);
        let after = scoreboard_with_fp(vec![("pod2man", "ok", 3, 20)], vec![("pod2man", after_fp)]);

        let t = diff(&before, &after);

        assert!(t.status_transitions.is_empty());
        assert!(t.flag_gains.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.appeared.is_empty() && t.disappeared.is_empty());

        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pod2man");
        assert_eq!(
            t.field_diffs[0].choices_changed,
            vec!["(root)::--guesswork"]
        );
        assert!(t.field_diffs[0].description_changed.is_empty());
        assert!(
            !t.is_identical(),
            "a fabricated choices list must not report as an identical run"
        );
    }

    /// A scoreboard from before this task (no `#fp` footer at all) must
    /// still load — `ParsedScoreboard::fingerprints` stays empty, and
    /// [`diff`] reports the affected tools as field-diff-unmeasured rather
    /// than silently claiming "no field-level changes" for data it never
    /// saw.
    #[test]
    fn legacy_scoreboards_with_no_fp_footer_report_unmeasured_not_identical_fields() {
        let before = scoreboard(vec![("git", "ok", 34, 120)]);
        let after = scoreboard(vec![("git", "ok", 34, 120)]);
        assert!(before.fingerprints.is_empty());
        let t = diff(&before, &after);
        assert!(t.field_diffs.is_empty());
        assert_eq!(t.field_diff_unmeasured, 1);
        // Every other dimension is genuinely unchanged here, so the overall
        // determination still reads identical — this test is only about
        // the unmeasured counter, not about forcing non-identical when
        // nothing else moved either.
        assert!(t.is_identical());
    }

    /// **The follow-up defect, direction 1**: a tool that had flags on the
    /// "before" side and loses every one of them must be reported as every
    /// flag removed, not as field-diff-unmeasured. `coverage::fingerprint_lines`
    /// used to skip emitting a `#fp` line for a row with no flags and no
    /// subcommands, so the "after" side (now empty) had no line at all and
    /// this fell into the unmeasured bucket instead — the field-level
    /// section going silent on exactly the case it exists to catch. See
    /// this test's sibling below (`without the fix...`) for the
    /// commit-then-attack proof this test was written to fail against.
    #[test]
    fn a_tool_that_loses_every_flag_is_reported_removed_not_unmeasured() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--strip".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        before_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(2), None, None),
        );

        let before = scoreboard_with_fp(vec![("pngfix", "ok", 2, 20)], vec![("pngfix", before_fp)]);
        // The "after" side carries *no* `#fp` entry for this tool at all —
        // exactly the shape `coverage::fingerprint_lines`'s pre-fix
        // skip-if-empty bug produced for a tool that lost every flag: the
        // row has no flags and no subcommands left, so the line was
        // dropped entirely rather than written as an empty one. Built with
        // plain `scoreboard` (no `#fp` population), not `scoreboard_with_fp`
        // with an explicit empty entry — the whole point of this test is
        // the *absent* entry, not a present-but-empty one.
        let after = scoreboard(vec![("pngfix", "ok", 0, 20)]);

        let t = diff(&before, &after);

        assert_eq!(
            t.field_diff_unmeasured, 0,
            "a missing entry on only one side must be read as empty, never as unmeasured"
        );
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pngfix");
        assert_eq!(
            t.field_diffs[0].flags_removed,
            vec!["(root)::--guesswork", "(root)::--strip"]
        );
        assert!(t.field_diffs[0].flags_added.is_empty());
        assert!(!t.is_identical());
    }

    /// **The follow-up defect, direction 2**: a flagless, subcommandless
    /// tool present (with an empty fingerprint) on both sides must be
    /// measured-with-no-changes — absent from `field_diffs` entirely — not
    /// counted as unmeasured. This is the common case on a real sweep
    /// (verbatim tools, zero-flag `ok` tools), and conflating "measured
    /// clean" with "not measured" was the other half of the same defect.
    #[test]
    fn a_flagless_tool_present_on_both_sides_is_measured_clean_not_unmeasured() {
        let before = scoreboard_with_fp(
            vec![("true", "ok", 0, 5)],
            vec![("true", ParsedFingerprint::default())],
        );
        let after = scoreboard_with_fp(
            vec![("true", "ok", 0, 5)],
            vec![("true", ParsedFingerprint::default())],
        );

        let t = diff(&before, &after);

        assert_eq!(
            t.field_diff_unmeasured, 0,
            "a present, empty fingerprint on both sides is measured, not unmeasured"
        );
        assert!(
            t.field_diffs.is_empty(),
            "no change to report for a flagless tool whose fingerprint didn't move"
        );
        assert!(t.is_identical());
    }

    /// Adds/removes/changes are reported as the actual flag identities and
    /// subcommand paths, never folded into a bare count — the requirement
    /// this whole diff exists to satisfy.
    #[test]
    fn field_diff_reports_flag_and_subcommand_adds_and_removes_by_name() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--old".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        before_fp.subcommands.insert("old-sub".to_string());

        let mut after_fp = ParsedFingerprint::default();
        after_fp.flags.insert(
            "(root)::--new".to_string(),
            flag_fp(true, Some(2), None, None),
        );
        after_fp.subcommands.insert("new-sub".to_string());

        let before = scoreboard_with_fp(vec![("t", "ok", 1, 10)], vec![("t", before_fp)]);
        let after = scoreboard_with_fp(vec![("t", "ok", 1, 10)], vec![("t", after_fp)]);

        let t = diff(&before, &after);
        assert_eq!(t.field_diffs.len(), 1);
        let fd = &t.field_diffs[0];
        assert_eq!(fd.flags_added, vec!["(root)::--new"]);
        assert_eq!(fd.flags_removed, vec!["(root)::--old"]);
        assert_eq!(fd.subcommands_added, vec!["new-sub"]);
        assert_eq!(fd.subcommands_removed, vec!["old-sub"]);
    }

    /// **The awk regression, at `sweep-diff` level.** `coverage.rs`'s own
    /// test module proves the wire format round-trips a `|`-containing
    /// value_name through the real rendering pipeline; this test proves the
    /// *consumer* of that format — `sweep-diff`'s `diff` — actually uses the
    /// recovered data instead of quietly losing the tool to the
    /// "unmeasured" bucket. The `#fp` line below is hand-written in the
    /// fixed wire format (`\p` standing in for the escaped `|` inside
    /// `awk`'s real `fatal|invalid|no-ext` value_name — `coverage::fp_escape`
    /// is private to that module, so the format's own fixed, documented
    /// shape is pinned here directly rather than called into). Pre-fix,
    /// `parse_fingerprint_line` has no unescaping step at all: it takes the
    /// literal two characters `\` and `p` at face value, so nothing here
    /// trips the *old* bug (splitting on a raw `|`) — instead it proves the
    /// value_name comes back as literal `fatal\pinvalid\pno-ext` instead of
    /// `fatal|invalid|no-ext`, and separately, `field_diff_unmeasured` and
    /// `field_diffs` should have been produced correctly since no raw `|`
    /// entered the line at all. Only the value_name assertion at the bottom
    /// is expected to fail before `fp_unescape` exists; kept as one test
    /// (rather than splitting the unescape check out) because the whole
    /// point is that `sweep-diff` must report the change *and* recover the
    /// right value_name, not one or the other.
    #[test]
    fn sweep_diff_reports_field_change_for_a_tool_whose_value_name_contains_a_flag_separator() {
        let row = row_line("awk", "ok", 1, 20);
        let before_text = format!(
            "{}#fp awk\t\t(root)::-L=0:-:-:fatal\\pinvalid\\pno-ext\n",
            sample_text(&[&row])
        );
        let after_text = format!(
            "{}#fp awk\t\t(root)::-L=1:abc:-:fatal\\pinvalid\\pno-ext\n",
            sample_text(&[&row])
        );
        let before = parse_scoreboard(&before_text);
        let after = parse_scoreboard(&after_text);

        let t = diff(&before, &after);
        assert_eq!(
            t.field_diff_unmeasured, 0,
            "the #fp line must parse on both sides, not fall into the unmeasured bucket"
        );
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "awk");
        assert_eq!(
            t.field_diffs[0].description_changed,
            vec!["(root)::-L"],
            "the description change on awk's -L flag must be reported, not swallowed"
        );

        let fp = before
            .fingerprints
            .get("awk")
            .expect("awk fingerprint present");
        assert_eq!(
            fp.flags
                .get("(root)::-L")
                .and_then(|f| f.value_name.clone()),
            Some("fatal|invalid|no-ext".to_string()),
            "value_name must unescape back to awk's real text, not stay literal escape codes"
        );
    }

    /// **Backward compatibility, explicitly.** An OLD-format `#fp` line —
    /// written before this task, with no backslash escaping at all, and a
    /// `value_name` carrying a raw `:` the way `coverage::fp_escape` never
    /// touched before now — must parse to exactly the same
    /// [`ParsedFingerprint`] it always has. This is the measured claim: 0
    /// backslashes appear across all 2,308 `#fp` lines in a full-PATH sweep
    /// capture and 0 in `coverage-scoreboard.ci.txt`, so `fp_unescape` is
    /// the identity function on every existing scoreboard, and a raw `:`
    /// inside `value_name` (the pre-existing case `splitn(4, ':')` exists
    /// for) still lands whole in the final field exactly as before. The
    /// only theoretical incompatibility — an old scoreboard whose
    /// `value_name` held a literal backslash immediately followed by one of
    /// the seven escape letters (`\`, `t`, `n`, `p`, `c`, `e`, `s`) — is a
    /// known, measured-zero caveat: `fp_unescape` would consume that
    /// backslash-letter pair as an escape sequence instead of passing it
    /// through as two literal characters. No such value_name exists in
    /// either measured corpus.
    #[test]
    fn old_format_fp_line_without_backslashes_parses_unchanged() {
        let line = "t\tsub-a,sub-b\t(root)::--time=1:1a2b:-:10:30|(root)::--verbose=0:-:-:-";
        let (tool, fp) = parse_fingerprint_line(line).expect("old-format line parses");
        assert_eq!(tool, "t");
        assert_eq!(fp.subcommands.len(), 2);
        assert!(fp.subcommands.contains("sub-a"));
        assert!(fp.subcommands.contains("sub-b"));

        let time = fp.flags.get("(root)::--time").expect("--time flag present");
        assert!(time.has_description);
        assert_eq!(time.description_hash, Some(0x1a2b));
        assert_eq!(time.choices_hash, None);
        assert_eq!(
            time.value_name.as_deref(),
            Some("10:30"),
            "a raw colon inside value_name must still land whole via splitn(4, ':')"
        );

        let verbose = fp
            .flags
            .get("(root)::--verbose")
            .expect("--verbose flag present");
        assert!(!verbose.has_description);
        assert_eq!(verbose.description_hash, None);
        assert_eq!(verbose.choices_hash, None);
        assert_eq!(verbose.value_name, None);
    }

    /// A tier or framework change on an otherwise field-identical tool is
    /// still surfaced — the field-level diff isn't only about flags.
    #[test]
    fn tier_and_framework_changes_are_reported_per_tool() {
        let text = "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr  status\n";
        let before_row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}\n",
            "t", "help", "clap (v3/v4) (artifact)", 1, 1, "100%", 10, 0, "-", 0, "ok",
        );
        let after_row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}\n",
            "t", "help+native", "cobra (artifact)", 1, 1, "100%", 10, 0, "-", 0, "ok",
        );
        let before = parse_scoreboard(&format!(
            "{text}{before_row}# aggregate: pct_flags_with_text=100.00 no_tier_count=0 total=1\n"
        ));
        let after = parse_scoreboard(&format!(
            "{text}{after_row}# aggregate: pct_flags_with_text=100.00 no_tier_count=0 total=1\n"
        ));
        let t = diff(&before, &after);
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tier_changed, Some(("help", "help+native")));
        assert_eq!(
            t.field_diffs[0].framework_changed,
            Some(("clap (v3/v4) (artifact)", "cobra (artifact)"))
        );
        assert!(!t.is_identical());
    }

    /// A scoreboard whose footer carries only pre-generalization `#fp` lines
    /// (no `EntityKind` tag in the entity id) is detected as
    /// [`FingerprintFormat::V1`].
    #[test]
    fn a_scoreboard_with_only_v1_fp_lines_is_detected_as_v1() {
        let text = format!(
            "{}#fp t\t\t(root)::--flag=0:-:-:-\n",
            sample_text(&[&row_line("t", "ok", 1, 10)])
        );
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.fingerprint_format, Some(FingerprintFormat::V1));
    }

    /// A scoreboard whose footer carries `#fp2` lines (the current,
    /// `EntityKind`-tagged format) is detected as [`FingerprintFormat::V2`].
    #[test]
    fn a_scoreboard_with_fp2_lines_is_detected_as_v2() {
        let text = format!(
            "{}#fp2 t\t\t(root)::Flag::--flag=0:-:-:-\n",
            sample_text(&[&row_line("t", "ok", 1, 10)])
        );
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.fingerprint_format, Some(FingerprintFormat::V2));
    }

    /// A scoreboard with no fingerprint footer at all — predates the
    /// feature entirely — carries no format, distinct from either version.
    #[test]
    fn a_scoreboard_with_no_fp_footer_has_no_fingerprint_format() {
        let before = scoreboard(vec![("t", "ok", 1, 10)]);
        assert_eq!(before.fingerprint_format, None);
    }

    /// **The migration-story guard this task exists for.** A V1-footer
    /// scoreboard and a V2-footer scoreboard must never be silently joined:
    /// [`fingerprint_format_mismatch`] must name the mismatch so a caller
    /// (`xtask/src/main.rs`'s `run_sweep_diff`) can refuse to proceed,
    /// rather than let [`field_diff`] misread every V1 entity id as
    /// "removed" (it doesn't carry the `EntityKind` tag a V2 id does) and
    /// every V2 entity id as "added" — a false wholesale loss-and-gain, not
    /// a real one.
    #[test]
    fn fingerprint_format_mismatch_names_a_v1_v2_pair() {
        let v1_text = format!(
            "{}#fp t\t\t(root)::--flag=0:-:-:-\n",
            sample_text(&[&row_line("t", "ok", 1, 10)])
        );
        let v2_text = format!(
            "{}#fp2 t\t\t(root)::Flag::--flag=0:-:-:-\n",
            sample_text(&[&row_line("t", "ok", 1, 10)])
        );
        let v1 = parse_scoreboard(&v1_text);
        let v2 = parse_scoreboard(&v2_text);
        assert_eq!(v1.fingerprint_format, Some(FingerprintFormat::V1));
        assert_eq!(v2.fingerprint_format, Some(FingerprintFormat::V2));

        let msg = fingerprint_format_mismatch(&v1, &v2)
            .expect("a V1/V2 pair must be reported as a mismatch, never silently diffed");
        assert!(msg.contains("V1"), "message must name the V1 side: {msg}");
        assert!(msg.contains("V2"), "message must name the V2 side: {msg}");

        // Reversed order — same mismatch, must still be caught.
        let msg_rev = fingerprint_format_mismatch(&v2, &v1)
            .expect("the mismatch must be caught regardless of which side is --before");
        assert!(msg_rev.contains("V1") && msg_rev.contains("V2"));
    }

    /// Two scoreboards agreeing on format — both V1, both V2, or both
    /// carrying no footer at all (the pre-existing legacy-pair case, already
    /// handled by `field_diff_unmeasured`) — are never reported as a
    /// mismatch.
    #[test]
    fn fingerprint_format_mismatch_is_none_when_both_sides_agree() {
        let v2_text = format!(
            "{}#fp2 t\t\t(root)::Flag::--flag=0:-:-:-\n",
            sample_text(&[&row_line("t", "ok", 1, 10)])
        );
        let v2_a = parse_scoreboard(&v2_text);
        let v2_b = parse_scoreboard(&v2_text);
        assert_eq!(fingerprint_format_mismatch(&v2_a, &v2_b), None);

        let no_footer_a = scoreboard(vec![("t", "ok", 1, 10)]);
        let no_footer_b = scoreboard(vec![("t", "ok", 1, 10)]);
        assert_eq!(
            fingerprint_format_mismatch(&no_footer_a, &no_footer_b),
            None
        );
    }

    // The end-to-end render→parse round trip used to live here, driven by a
    // real `grep --help` probe, and asserted "at least one flag carries a
    // description" — a fact about the *host's* grep (GNU grep documents its
    // options; BSD grep, on macOS, prints a bare usage synopsis with none),
    // which is exactly the class of failure AGENTS.md §4 warns about
    // ("macOS breaks in ways Linux CI cannot see") and turned
    // `test (macos-latest)` red on this branch. It's now two tests in
    // `coverage::tests`, where `Row`/`build_fingerprint`/`render_text` are
    // already reachable without a second cross-module exposure:
    // `fingerprint_footer_round_trips_a_synthetic_tree` (a hand-built
    // `CommandNode`, so the description/choices/value_name-carrying case is
    // true by construction on every platform) and
    // `fingerprint_footer_round_trips_whatever_a_real_grep_produced` (keeps
    // the real-binary smoke check spec §3.1 asks for, but only asserts that
    // whatever this host's grep produced survives the round trip losslessly
    // — never a claim about grep's own content).
}
