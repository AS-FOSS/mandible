//! `cargo run -p xtask -- audit`: a bounded, random, human-reviewed sample
//! of real tools, comparing raw captured `--help` text against the parsed
//! tree — this project's only instrument that compares output to truth
//! rather than to itself (every other instrument measures agreement with
//! the parser). Sample size is `n=80` (~±8-10 points at 95% confidence,
//! [`wilson_interval`]).
//!
//! Each reviewed tool can become a `corpus/` fixture (spec §13.2), turning
//! review effort into a permanent regression-net entry.
//!
//! # Shape
//!
//! - `xtask audit freeze` (`crate::queue::cmd_freeze`) sweeps `PATH`,
//!   classifies by parse status, shuffle-stratifies with a recorded seed,
//!   writes `<dir>/queue.toml` / `<dir>/queue-captures/`. `xtask audit
//!   sample` (`crate::queue::cmd_sample`) advances a cursor through that
//!   frozen queue into a resumable verdict file.
//! - [`cmd_review`] is the interactive loop: raw text and parsed tree side
//!   by side, a one-word verdict, persisted after every tool.
//! - [`cmd_emit`]/[`cmd_ingest`] are the non-interactive twin (this machine
//!   has no tty, AGENTS.md §3.2): `emit` writes pending pairs to a file,
//!   `ingest` reads verdicts back in.
//! - [`cmd_report`] renders per-stratum and overall accuracy with an
//!   explicit sample size and confidence interval, never a bare percentage
//!   (spec §13.1b).
//! - [`cmd_fixtures`] turns a reviewed tool into a `corpus/`-shaped fixture:
//!   `correct` gets a real `expected.snap`; `incomplete`/`wrong` get
//!   `[xfail]` with the reviewer's note as `reason`.
//!
//! # No cherry-picking, structurally
//!
//! [`cmd_review`]'s only responses are `correct`/`incomplete`/`wrong`/
//! `skip`, and `skip` is recorded, not omitted — visible in
//! [`cmd_report`]'s output, excluded only from the accuracy ratio. The
//! draw (`crate::queue::shuffle_stratify`) never consults a tool's status
//! or name, only `(tool, stratum)` pairs and a seeded shuffle.

mod batch;
mod fixtures;
mod interactive;
mod report;
mod signatures;

pub(crate) use batch::sanitize_filename;
pub use batch::{cmd_amend, cmd_emit, cmd_ingest};
pub use fixtures::cmd_fixtures;
pub use interactive::{cmd_review, cmd_spot_audit};
pub use report::cmd_report;
pub(crate) use report::render_report;
use signatures::{k1_signature, k2_signature, k3_signature};

use crate::existence;
use crate::misattribution::RecordingProbe;
use crate::status;
use mandible_core::audit::Entry;
use mandible_extract::exec::ExecOutput;
use mandible_extract::{default_tiers_with_probe, ExtractionResult, Runner};
use rayon::prelude::*;
use std::path::Path;
use std::sync::Arc;

/// The manifest schema ([`Entry`], [`AuditFile`], [`AuditMeta`], load/save,
/// verdict-word/tag-override parsers) lives in [`mandible_core::audit`],
/// not here, so `mandible --review` reads/writes the same `audit/<seed>.toml`
/// without a drifting second copy. This module draws the sample and
/// computes K1/K2/K3 pre-tag suggestions, needing a live extraction pass;
/// `mandible --review` only displays what's already in the file.
///
/// The synthetic stratum label [`cmd_report`] uses for every entry with an
/// [`Entry::include_reason`], tallied separately from the ordinary
/// stratified draw — see [`Entry::stratum`]'s doc comment.
const FORCED_INCLUSION_STRATUM: &str = "forced-inclusion";

/// One tool's classification: its drawn/measured stratum, the extracted
/// tree, and (when available) the raw captured text and the exact capture
/// needed to write a corpus fixture — all obtained from **one** extraction
/// pass, via [`RecordingProbe`], never a second probe of the tool (same "no
/// new probes" property [`crate::misattribution`] documents). `pub(crate)`
/// so `crate::queue` (the freeze/cursor-draw implementation) can read a
/// tool's stratum the same way [`entry_from_classified`] already does,
/// without a second copy of this shape.
pub(crate) struct Classified {
    pub(crate) stratum: &'static str,
    pub(crate) result: ExtractionResult,
    pub(crate) raw_text: Option<String>,
    pub(crate) raw_capture: Option<(Vec<String>, ExecOutput)>,
}

pub(crate) fn classify_one(tool: &str) -> Classified {
    let probe = Arc::new(RecordingProbe::new());
    let runner = Runner::new(default_tiers_with_probe(probe.clone()));
    let result = runner.extract_full(tool);
    let stratum = status::compute(&result).label;
    Classified {
        stratum,
        raw_text: probe.root_help_text(),
        raw_capture: probe.root_help_capture(),
        result,
    }
}

/// [`classify_one`], plus every `(argv, output)` pair the extraction pass
/// actually recorded — not just the root `--help` capture
/// [`RecordingProbe::root_help_capture`] singles out, but everything the
/// pipeline sent, so `crate::queue::cmd_freeze` can persist enough bytes for
/// `crate::queue::cmd_reclassify` to replay the *exact same* extraction via
/// [`mandible_extract::exec::Transcript`] later, with zero subprocess
/// spawns, regardless of how many probes a given tool's framework needed
/// (cobra's two-probe protocol included).
pub(crate) fn classify_one_with_recordings(
    tool: &str,
) -> (
    Classified,
    std::collections::HashMap<Vec<String>, ExecOutput>,
) {
    let probe = Arc::new(RecordingProbe::new());
    let runner = Runner::new(default_tiers_with_probe(probe.clone()));
    let result = runner.extract_full(tool);
    let stratum = status::compute(&result).label;
    let classified = Classified {
        stratum,
        raw_text: probe.root_help_text(),
        raw_capture: probe.root_help_capture(),
        result,
    };
    (classified, probe.all_recordings())
}

/// [`classify_one_with_recordings`], run in parallel across `tools` — same
/// reasoning as [`classify_all`].
pub(crate) fn classify_all_with_recordings(
    tools: &[String],
) -> Vec<(
    String,
    Classified,
    std::collections::HashMap<Vec<String>, ExecOutput>,
)> {
    tools
        .par_iter()
        .map(|t| {
            let (classified, recordings) = classify_one_with_recordings(t);
            (t.clone(), classified, recordings)
        })
        .collect()
}

/// Build one [`Entry`] from a classified tool, computing every pre-tag
/// suggestion from the same single extraction pass — no second probe, same
/// property [`Classified`]'s own doc comment describes. Shared by
/// `crate::queue::cmd_sample`'s drawn-tool and force-include paths, so the
/// two can never compute a K1/K2/K3 suggestion differently. `pub(crate)`
/// for exactly that cross-module reuse.
pub(crate) fn entry_from_classified(
    tool: String,
    classified: &Classified,
    include_reason: Option<String>,
) -> Entry {
    let k1 = classified.result.root.as_ref().and_then(k1_signature);
    let k2 = match (&classified.result.root, &classified.raw_text) {
        (Some(root), Some(raw)) => k2_signature(&existence::detect(raw, root), raw),
        _ => None,
    };
    let k3 = classified.result.root.as_ref().and_then(k3_signature);
    Entry {
        tool,
        stratum: classified.stratum.to_string(),
        verdict: None,
        note: String::new(),
        k1,
        k2,
        k3,
        include_reason,
        // Set by `cmd_spot_audit` after this call returns — this
        // constructor is also used by the ordinary queue draw and
        // force-include paths, neither of which is a spot-audit.
        spot_audit_event: None,
        // A freshly drawn entry has no verdict yet, so it can carry no
        // defect family either — a family names what is wrong, and nothing
        // has been judged wrong at draw time. Labels arrive later, either
        // from a reviewer or (marked as such) derived from their note.
        families: Vec::new(),
        families_derived: None,
        amendments: Vec::new(),
    }
}

/// Read a force-include file: `<tool> <reason...>` per line (`#` comments
/// and blank lines ignored — the same convention [`cmd_ingest`]'s verdicts
/// file uses), for `crate::queue::cmd_sample`'s `force_include` parameter. A
/// reason is required, not optional: an unconditional inclusion with no
/// stated reason is exactly the kind of unauditable claim docs/design.md Appendix A
/// exists to rule out (see `Entry::include_reason`'s doc comment).
pub fn load_force_include(path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (lineno, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let tool = parts.next().unwrap_or("").to_string();
        let reason = parts.next().unwrap_or("").trim().to_string();
        if reason.is_empty() {
            anyhow::bail!(
                "{}:{}: force-include line for {tool:?} has no reason",
                path.display(),
                lineno + 1
            );
        }
        out.push((tool, reason));
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::report::*;
    use super::signatures::*;
    use super::*;
    use mandible_core::audit::{
        extract_tag_override, load, parse_verdict_word, save, verdict_path, AuditFile, AuditMeta,
    };
    use mandible_core::{CommandNode, Entity, Provenance, Source};
    use std::io::Cursor;
    use std::path::PathBuf;

    #[test]
    fn wilson_interval_is_wide_for_small_perfect_samples() {
        // n=5, k=5 ("100% correct so far") must not report [100%, 100%] —
        // that would misrepresent five tools as certainty.
        let (lo, hi) = wilson_interval(5, 5);
        assert!(
            lo > 0.0 && lo < 0.6,
            "lower bound should be well below 100%: {lo}"
        );
        assert!(hi <= 1.0);
    }

    #[test]
    fn wilson_interval_narrows_as_n_grows() {
        let (lo_small, hi_small) = wilson_interval(40, 50);
        let (lo_big, hi_big) = wilson_interval(400, 500);
        assert!(
            hi_big - lo_big < hi_small - lo_small,
            "more data should narrow the interval"
        );
    }

    #[test]
    fn parse_verdict_word_accepts_short_and_long_forms() {
        assert_eq!(parse_verdict_word("c").unwrap(), "correct");
        assert_eq!(parse_verdict_word("correct").unwrap(), "correct");
        assert_eq!(parse_verdict_word("i").unwrap(), "incomplete");
        assert_eq!(parse_verdict_word("w").unwrap(), "wrong");
        assert_eq!(parse_verdict_word("s").unwrap(), "skip");
        assert!(parse_verdict_word("maybe").is_err());
    }

    fn write_sample_file(dir: &Path, seed: u64, tools: &[(&str, &str)]) -> PathBuf {
        let path = verdict_path(dir, seed);
        let file = AuditFile {
            meta: AuditMeta {
                seed,
                sample_size: tools.len(),
            },
            entries: tools
                .iter()
                .map(|(tool, stratum)| Entry {
                    tool: tool.to_string(),
                    stratum: stratum.to_string(),
                    verdict: None,
                    note: String::new(),
                    k1: None,
                    k2: None,
                    k3: None,
                    include_reason: None,
                    spot_audit_event: None,
                    families: Vec::new(),
                    families_derived: None,
                    amendments: Vec::new(),
                })
                .collect(),
        };
        save(&path, &file).unwrap();
        path
    }

    /// Resumption, end to end: a "review" that answers only the first
    /// entry before its input runs out (simulating an interrupted session
    /// — a killed process leaves exactly this shape on disk, one verdict
    /// written, the rest untouched) must leave the remaining entries
    /// pending, and a second call over the *same* file with fresh input
    /// must pick up exactly where the first left off rather than re-asking
    /// the already-answered tool.
    ///
    /// Uses `sh` as both sample tools — a real, always-present binary — so
    /// this test exercises the real extraction pipeline (`classify_one`)
    /// end to end rather than a synthetic stand-in, per AGENTS.md's own
    /// rule about exercising real argv construction.
    #[test]
    fn review_resumes_after_simulated_interruption() {
        let tmp = tempfile::tempdir().unwrap();
        write_sample_file(tmp.path(), 12345, &[("sh", "ok"), ("cat", "ok")]);

        // First "session": only one line of input, so the loop stops after
        // the first tool (EOF on the second `read_line`) — modeling a
        // process that was killed mid-review.
        let mut input = Cursor::new(b"correct first tool looked right\n".to_vec());
        let mut out = Vec::new();
        cmd_review(tmp.path(), 12345, &mut input, &mut out).unwrap();

        let after_first = load(&verdict_path(tmp.path(), 12345)).unwrap();
        let reviewed: Vec<&Entry> = after_first
            .entries
            .iter()
            .filter(|e| e.verdict.is_some())
            .collect();
        assert_eq!(
            reviewed.len(),
            1,
            "exactly one entry should be answered after the interruption"
        );
        let pending_after_first: Vec<&Entry> = after_first
            .entries
            .iter()
            .filter(|e| e.verdict.is_none())
            .collect();
        assert_eq!(
            pending_after_first.len(),
            1,
            "the other entry must remain pending, not re-drawn or lost"
        );

        // Second "session", fresh process (a fresh call), answering the
        // rest — must not re-present the already-answered tool.
        let mut input2 = Cursor::new(b"wrong parsed tree looked empty\n".to_vec());
        let mut out2 = Vec::new();
        cmd_review(tmp.path(), 12345, &mut input2, &mut out2).unwrap();
        let transcript = String::from_utf8(out2).unwrap();
        assert_eq!(
            transcript.matches("=== ").count(),
            1,
            "resumed review must present exactly the one still-pending tool, not restart from the top"
        );

        let after_second = load(&verdict_path(tmp.path(), 12345)).unwrap();
        assert!(after_second.entries.iter().all(|e| e.verdict.is_some()));
        assert_eq!(after_second.pending().count(), 0);
    }

    #[test]
    fn ingest_does_not_overwrite_without_the_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 7, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            save(&path, &f).unwrap();
        }
        let verdicts_path = tmp.path().join("verdicts.txt");
        std::fs::write(&verdicts_path, "sh wrong should not apply\n").unwrap();
        cmd_ingest(tmp.path(), 7, &verdicts_path, false).unwrap();
        let after = load(&path).unwrap();
        assert_eq!(
            after.entries[0].verdict.as_deref(),
            Some("correct"),
            "must not silently overwrite"
        );

        cmd_ingest(tmp.path(), 7, &verdicts_path, true).unwrap();
        let after_overwrite = load(&path).unwrap();
        assert_eq!(after_overwrite.entries[0].verdict.as_deref(), Some("wrong"));
    }

    #[test]
    fn ingest_reports_unknown_tools_instead_of_silently_dropping_them() {
        let tmp = tempfile::tempdir().unwrap();
        write_sample_file(tmp.path(), 3, &[("sh", "ok")]);
        let verdicts_path = tmp.path().join("verdicts.txt");
        std::fs::write(&verdicts_path, "not-in-sample correct\n").unwrap();
        // Doesn't error — an unknown line is reported, not fatal, since a
        // verdicts file may legitimately be hand-edited or come from a
        // stale sample.
        cmd_ingest(tmp.path(), 3, &verdicts_path, false).unwrap();
    }

    #[test]
    fn skip_is_recorded_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 9, &[("sh", "ok"), ("cat", "ok")]);
        let verdicts_path = tmp.path().join("verdicts.txt");
        std::fs::write(&verdicts_path, "sh skip couldn't judge\ncat correct\n").unwrap();
        cmd_ingest(tmp.path(), 9, &verdicts_path, false).unwrap();
        let after = load(&path).unwrap();
        assert_eq!(after.entries.len(), 2, "a skip must still occupy its slot");
        let sh = after.entries.iter().find(|e| e.tool == "sh").unwrap();
        assert_eq!(sh.verdict.as_deref(), Some("skip"));
    }

    // -------------------------------------------------------------
    // K1 pre-tag
    // -------------------------------------------------------------

    fn k1_flag() -> Entity {
        let mut f = Entity::flag_short('f', Provenance::single(Source::HelpText));
        f.value_name = Some("dump-scos".to_string());
        f
    }

    fn ordinary_flag(short: char, long: &str) -> Entity {
        Entity::flag_spelled(
            Some(short),
            Some(long.to_string()),
            false,
            false,
            Provenance::single(Source::HelpText),
        )
    }

    #[test]
    fn k1_signature_flags_the_gcc_single_dash_long_shape() {
        let mut root = CommandNode::new("clang", Provenance::single(Source::HelpText));
        root.entities.push(k1_flag());
        root.entities.push(ordinary_flag('v', "verbose"));
        assert_eq!(k1_signature(&root), Some(true));
    }

    #[test]
    fn k1_signature_is_none_when_no_flag_matches() {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.entities.push(ordinary_flag('v', "verbose"));
        assert_eq!(
            k1_signature(&root),
            None,
            "a tool with no K1-shaped flag anywhere gets no suggestion, not Some(false)"
        );
    }

    #[test]
    fn k1_signature_recurses_into_subcommands() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut child = CommandNode::new("sub", Provenance::single(Source::HelpText));
        child.entities.push(k1_flag());
        root.subcommands.push(child);
        assert_eq!(
            k1_signature(&root),
            Some(true),
            "the defect can appear on any subcommand's flags, not just the root's"
        );
    }

    #[test]
    fn k1_signature_stats_counts_matching_and_total_across_the_tree() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(k1_flag());
        root.entities.push(ordinary_flag('v', "verbose"));
        let mut child = CommandNode::new("sub", Provenance::single(Source::HelpText));
        child.entities.push(k1_flag());
        root.subcommands.push(child);
        assert_eq!(k1_signature_stats(&root), (2, 3));
    }

    // -------------------------------------------------------------
    // K2 pre-tag
    // -------------------------------------------------------------

    /// The multi-column case this pre-tag was built to explain is now
    /// **fixed at the source**, so there is nothing left for it to explain.
    ///
    /// `existence::list_row_words` reads a column-aligned or comma-joined
    /// index as a list row and attests every item on it, not just the
    /// line's first token. This test used to assert three fabrications on
    /// exactly this input, with `k2_signature` waving all three through;
    /// the detector now emits none, so the suggestion is `None` — the same
    /// answer it gives for any tool with no subcommand fabrications to
    /// judge. Kept as a regression test in the new direction: if the
    /// list-row rule ever regresses, this fails.
    #[test]
    fn a_multi_column_index_no_longer_produces_a_fabrication_to_pre_tag() {
        // Real busybox/openssl shape: several names on one line, only the
        // first of which is a "line start word".
        let raw = "asn1parse         ca                ciphers           cmp\n";
        let mut root = CommandNode::new("openssl", Provenance::single(Source::HelpText));
        for name in ["asn1parse", "ca", "ciphers", "cmp"] {
            root.subcommands
                .push(CommandNode::new(name, Provenance::single(Source::HelpText)));
        }
        let report = existence::detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "every column of a real command grid is attested: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(k2_signature(&report, raw), None);
    }

    #[test]
    fn k2_signature_is_false_when_a_fabrication_is_not_explained() {
        let raw = "asn1parse         ca\n";
        let mut root = CommandNode::new("openssl", Provenance::single(Source::HelpText));
        root.subcommands.push(CommandNode::new(
            "asn1parse",
            Provenance::single(Source::HelpText),
        ));
        root.subcommands
            .push(CommandNode::new("ca", Provenance::single(Source::HelpText)));
        // A third, wholly invented name that occurs nowhere in the raw
        // text at all — a genuine [M-10]-shaped fabrication, not a
        // tokenizer artifact.
        root.subcommands.push(CommandNode::new(
            "totally-invented",
            Provenance::single(Source::HelpText),
        ));
        let report = existence::detect(raw, &root);
        assert_eq!(
            k2_signature(&report, raw),
            Some(false),
            "a fabrication with no raw-text occurrence at all must not be waved through as K2"
        );
    }

    #[test]
    fn k2_signature_is_none_with_no_subcommand_fabrications() {
        let raw = "clone     Clone a repository\n";
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.subcommands.push(CommandNode::new(
            "clone",
            Provenance::single(Source::HelpText),
        ));
        let report = existence::detect(raw, &root);
        assert_eq!(k2_signature(&report, raw), None);
    }

    // -------------------------------------------------------------
    // K3 pre-tag
    // -------------------------------------------------------------

    #[test]
    fn k3_signature_flags_an_attestation_gated_stub() {
        // git-lfs's shape: a real, non-empty root (so the whole-tree-zero-
        // flags cause can't be what's firing) with at least one subcommand
        // whose name came from a native/cobra artifact, never a recognized
        // heading — `CommandNode::new` defaults `heading_attested` to
        // `false`, which is the honest state for exactly this case.
        let mut root = CommandNode::new("git-lfs", Provenance::single(Source::HelpText));
        root.entities.push(Entity::flag_long(
            "version",
            Provenance::single(Source::HelpText),
        ));
        root.subcommands.push(CommandNode::new(
            "install",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(count_attestation_gated_stubs(&root.subcommands[0]), 1);
        assert_eq!(k3_signature(&root), Some(true));
    }

    #[test]
    fn k3_signature_flags_unfetched_subcommand_help_when_the_whole_tree_has_zero_flags() {
        // openssl's shape: a bare command grid at the root (no options
        // section at all, so zero flags anywhere) with subcommands that
        // *are* heading_attested — real names, just never individually
        // probed by the single root-only extraction pass this signature
        // is computed from.
        let mut root = CommandNode::new("openssl", Provenance::single(Source::HelpText));
        for name in ["asn1parse", "ca", "ciphers"] {
            let mut child = CommandNode::new(name, Provenance::single(Source::HelpText));
            child.heading_attested = true;
            root.subcommands.push(child);
        }
        assert!(
            has_unfetched_subcommand_help(&root),
            "root has subcommands but zero flags anywhere"
        );
        let gated: usize = root
            .subcommands
            .iter()
            .map(count_attestation_gated_stubs)
            .sum();
        assert_eq!(
            gated, 0,
            "these subcommands are heading_attested, so cause (a) must not also fire"
        );
        assert_eq!(k3_signature(&root), Some(true));
    }

    #[test]
    fn k3_signature_is_none_for_an_ordinary_tool() {
        // git's shape: the root itself documents flags, and its
        // subcommands are heading_attested (real, recognized-heading
        // names) even though their own flags haven't been fetched yet by
        // this single pass — the ordinary, unremarkable lazy-fill state
        // every multi-level tool is in at sample time.
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.entities.push(Entity::flag_long(
            "version",
            Provenance::single(Source::HelpText),
        ));
        let mut child = CommandNode::new("clone", Provenance::single(Source::HelpText));
        child.heading_attested = true;
        root.subcommands.push(child);
        assert_eq!(
            k3_signature(&root),
            None,
            "an ordinary un-recursed subcommand must not be tagged K3"
        );
    }

    #[test]
    fn count_attestation_gated_stubs_excludes_the_root_itself() {
        // The root is definitionally real (the literal name resolved from
        // PATH), never something a tier guessed at from a heading — same
        // exclusion `status::structure_sanity` already makes. A childless,
        // flagless, unattested root must not tag K3 on that basis alone.
        let root = CommandNode::new("sh", Provenance::single(Source::HelpText));
        assert!(!root.heading_attested);
        assert_eq!(k3_signature(&root), None);
    }

    // -------------------------------------------------------------
    // Tag-override parsing
    // -------------------------------------------------------------

    #[test]
    fn extract_tag_override_pulls_the_token_out_of_the_note() {
        let mut note =
            "the extra flags were genuinely wrong k1=false not the gcc defect".to_string();
        let k1 = extract_tag_override(&mut note, "k1");
        assert_eq!(k1, Some(false));
        assert_eq!(
            note, "the extra flags were genuinely wrong not the gcc defect",
            "the token is removed, the rest of the note survives untouched"
        );
    }

    #[test]
    fn extract_tag_override_is_case_insensitive_and_absent_returns_none() {
        let mut note = "K1=TRUE looks like the known defect".to_string();
        assert_eq!(extract_tag_override(&mut note, "k1"), Some(true));
        assert_eq!(extract_tag_override(&mut note, "k2"), None);
    }

    #[test]
    fn extract_tag_override_handles_both_keys_in_one_note() {
        let mut note = "k1=true k2=false mixed causes".to_string();
        assert_eq!(extract_tag_override(&mut note, "k1"), Some(true));
        assert_eq!(extract_tag_override(&mut note, "k2"), Some(false));
        assert_eq!(note, "mixed causes");
    }

    #[test]
    fn review_verdict_line_applies_a_k1_override() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 21, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].k1 = Some(true);
            save(&path, &f).unwrap();
        }
        let mut input =
            Cursor::new(b"w k1=false actually a real bug, not the gcc defect\n".to_vec());
        let mut out = Vec::new();
        cmd_review(tmp.path(), 21, &mut input, &mut out).unwrap();
        let after = load(&path).unwrap();
        assert_eq!(after.entries[0].k1, Some(false), "override must persist");
        assert_eq!(after.entries[0].verdict.as_deref(), Some("wrong"));
        assert_eq!(
            after.entries[0].note,
            "actually a real bug, not the gcc defect"
        );
    }

    #[test]
    fn review_verdict_line_without_override_leaves_the_suggestion_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 22, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].k1 = Some(true);
            save(&path, &f).unwrap();
        }
        let mut input = Cursor::new(b"c known defect, confirmed\n".to_vec());
        let mut out = Vec::new();
        cmd_review(tmp.path(), 22, &mut input, &mut out).unwrap();
        let after = load(&path).unwrap();
        assert_eq!(
            after.entries[0].k1,
            Some(true),
            "leaving the tag out of the verdict line confirms the pre-tagged suggestion"
        );
    }

    // -------------------------------------------------------------
    // Force-include (Task C: unaudited-promotion tools)
    // -------------------------------------------------------------

    #[test]
    fn load_force_include_parses_tool_and_reason_and_skips_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("force.txt");
        std::fs::write(
            &path,
            "# unaudited promotions\nzoxide unaudited promotion, low-confidence -> ok\n\ncurl another reason\n",
        )
        .unwrap();
        let parsed = load_force_include(&path).unwrap();
        assert_eq!(
            parsed,
            vec![
                (
                    "zoxide".to_string(),
                    "unaudited promotion, low-confidence -> ok".to_string()
                ),
                ("curl".to_string(), "another reason".to_string()),
            ]
        );
    }

    #[test]
    fn load_force_include_rejects_a_line_with_no_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("force.txt");
        std::fs::write(&path, "zoxide\n").unwrap();
        assert!(load_force_include(&path).is_err());
    }

    // `cmd_sample`'s force-include behavior (independent of the queue draw
    // itself, unconditional inclusion, idempotent re-run) is now exercised
    // in `crate::queue`'s own tests, alongside the queue it now requires.

    // -------------------------------------------------------------
    // Report: effective stratum and accuracy views
    // -------------------------------------------------------------

    #[test]
    fn effective_stratum_buckets_forced_entries_separately() {
        let mut e = Entry {
            tool: "zoxide".to_string(),
            stratum: "ok".to_string(),
            verdict: None,
            note: String::new(),
            k1: None,
            k2: None,
            k3: None,
            include_reason: None,
            spot_audit_event: None,
            families: Vec::new(),
            families_derived: None,
            amendments: Vec::new(),
        };
        assert_eq!(effective_stratum(&e), "ok");
        e.include_reason = Some("unaudited promotion".to_string());
        assert_eq!(effective_stratum(&e), FORCED_INCLUSION_STRATUM);
    }

    /// A spot-audit entry is bucketed under its own `spot-audit:<event>`
    /// row, per promotion event — never blended into the single
    /// `forced-inclusion` catch-all, even though it also carries an
    /// `include_reason` documenting the draw itself.
    #[test]
    fn effective_stratum_gives_spot_audit_its_own_row_per_event() {
        let mut e = Entry {
            tool: "tcpdump".to_string(),
            stratum: "ok".to_string(),
            verdict: None,
            note: String::new(),
            k1: None,
            k2: None,
            k3: None,
            include_reason: Some("spot-audit of promotion event \"x\": 5 of 5 drawn".to_string()),
            spot_audit_event: Some("bundled-short-flag-942890d".to_string()),
            families: Vec::new(),
            families_derived: None,
            amendments: Vec::new(),
        };
        assert_eq!(
            effective_stratum(&e),
            "spot-audit:bundled-short-flag-942890d"
        );
        // A different event never collides with this one's row.
        e.spot_audit_event = Some("other-promotion".to_string());
        assert_eq!(effective_stratum(&e), "spot-audit:other-promotion");
    }

    fn entry(tool: &str, verdict: Option<&str>, k1: Option<bool>, k2: Option<bool>) -> Entry {
        Entry {
            tool: tool.to_string(),
            stratum: "ok".to_string(),
            verdict: verdict.map(str::to_string),
            note: String::new(),
            k1,
            k2,
            k3: None,
            include_reason: None,
            spot_audit_event: None,
            families: Vec::new(),
            families_derived: None,
            amendments: Vec::new(),
        }
    }

    /// The accuracy figures exclude every `skip`, so the report has to
    /// name them: a bare per-stratum count says how many tools left the
    /// denominator and never which, which is not a checkable claim. A
    /// skipped entry with no note prints an explicit placeholder rather
    /// than a fabricated reason — `skip` is the one verdict that does not
    /// require a note.
    #[test]
    fn skipped_lines_names_every_skipped_tool_and_says_when_no_reason_was_given() {
        let mut with_reason = entry("jconsole", Some("skip"), None, None);
        with_reason.note = "it hangs the application".to_string();
        let file = AuditFile {
            meta: AuditMeta {
                seed: 4,
                sample_size: 3,
            },
            entries: vec![
                entry("zzz-editres", Some("skip"), None, None),
                entry("kept", Some("correct"), None, None),
                with_reason,
            ],
        };
        let lines = skipped_lines(&file);
        assert_eq!(lines[0], "");
        assert!(lines[1].starts_with("tools skipped (2 —"), "{:?}", lines[1]);
        assert_eq!(lines.len(), 4);
        assert!(lines[2].contains("jconsole"), "{:?}", lines[2]);
        assert!(lines[2].contains("it hangs the application"));
        assert!(lines[3].contains("zzz-editres"), "{:?}", lines[3]);
        assert!(lines[3].contains("(no reason recorded)"));
        assert!(lines.iter().all(|l| !l.contains("kept")));
    }

    #[test]
    fn accuracy_over_excludes_pending_and_skip_from_the_denominator() {
        let entries = [
            entry("a", Some("correct"), None, None),
            entry("b", Some("wrong"), None, None),
            entry("c", None, None, None),
            entry("d", Some("skip"), None, None),
        ];
        let (correct, judged) = accuracy_over(entries.iter());
        assert_eq!((correct, judged), (1, 2));
    }

    /// task #28: a judged defect whose *only* family is `display-only` is
    /// a real finding (still `wrong`/`incomplete` on disk) that must not
    /// count toward the accuracy denominator at all — not as judged, and
    /// certainly not as correct.
    #[test]
    fn accuracy_over_excludes_pure_display_only_findings() {
        let mut display_only = entry("bashbug", Some("incomplete"), None, None);
        display_only.families = vec!["display-only".to_string()];
        display_only.families_derived = Some(true);
        let entries = [
            entry("a", Some("correct"), None, None),
            entry("b", Some("wrong"), None, None),
            display_only,
        ];
        let (correct, judged) = accuracy_over(entries.iter());
        assert_eq!(
            (correct, judged),
            (1, 2),
            "the display-only entry must not appear in either count"
        );
    }

    /// The mixed-family case `Entry::is_display_only`'s doc comment warns
    /// about: a real parse-shape family riding alongside `display-only`
    /// must NOT get the exclusion. Two true labels do not launder a
    /// genuine defect out of the denominator — this is the whole reason
    /// the check is "family set == {display-only}", not "contains
    /// display-only".
    #[test]
    fn accuracy_over_keeps_mixed_family_findings_in_the_denominator() {
        let mut mixed = entry("tcpdump", Some("wrong"), None, None);
        mixed.families = vec!["bundled-short-flag".to_string(), "display-only".to_string()];
        mixed.families_derived = Some(true);
        assert!(
            !mixed.is_display_only(),
            "a second, genuine family must block the exclusion"
        );
        let (correct, judged) = accuracy_over(std::iter::once(&mixed));
        assert_eq!((correct, judged), (0, 1));
    }

    #[test]
    fn k1_excluded_view_drops_only_k1_true_entries() {
        let entries = [
            entry("a", Some("correct"), Some(true), None),
            entry("b", Some("wrong"), Some(false), None),
            entry("c", Some("wrong"), None, None),
        ];
        let (correct, judged) = accuracy_over(entries.iter().filter(|e| e.k1 != Some(true)));
        assert_eq!(
            (correct, judged),
            (0, 2),
            "the K1-tagged entry must not count toward this view at all"
        );
    }

    #[test]
    fn k3_excluded_view_drops_only_k3_true_entries() {
        let mut tagged = entry("openssl", Some("incomplete"), None, None);
        tagged.k3 = Some(true);
        let entries = [
            tagged,
            entry("git", Some("correct"), None, None),
            entry("git-lfs", Some("incomplete"), None, None),
        ];
        let (correct, judged) = accuracy_over(entries.iter().filter(|e| e.k3 != Some(true)));
        assert_eq!(
            (correct, judged),
            (1, 2),
            "the K3-tagged entry must not count toward this view at all"
        );
    }

    #[test]
    fn cmd_report_runs_cleanly_over_a_mixed_k1_k2_k3_and_forced_sample() {
        // Smoke test: build a verdict file exercising every field this
        // task added (k1, k2, k3, include_reason) and confirm `cmd_report`
        // runs to completion without panicking on any of them.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(
            tmp.path(),
            42,
            &[
                ("clang", "ok"),
                ("busybox", "ok"),
                ("zoxide", "ok"),
                ("openssl", "suspicious"),
            ],
        );
        let mut f = load(&path).unwrap();
        f.entries[0].verdict = Some("wrong".to_string());
        f.entries[0].k1 = Some(true);
        f.entries[1].verdict = Some("wrong".to_string());
        f.entries[1].k2 = Some(true);
        f.entries[2].verdict = Some("correct".to_string());
        f.entries[2].include_reason = Some("unaudited promotion example".to_string());
        f.entries[3].verdict = Some("incomplete".to_string());
        f.entries[3].k3 = Some(true);
        save(&path, &f).unwrap();

        cmd_report(tmp.path(), 42).unwrap();
    }

    // -------------------------------------------------------------
    // `xtask audit spot-audit` (spec §13.1b's sixth rule) — real binaries,
    // real argv (AGENTS.md §3.1), same convention `queue.rs`'s own
    // `cmd_sample` tests use.
    // -------------------------------------------------------------

    #[test]
    fn spot_audit_draws_the_same_tools_for_the_same_event_and_seed() {
        let tmp = tempfile::tempdir().unwrap();
        let promoted = vec!["sh".to_string(), "cat".to_string(), "ls".to_string()];
        cmd_spot_audit(tmp.path(), 700, "demo-event", &promoted, 2, 99).unwrap();
        let first: Vec<String> = load(&verdict_path(tmp.path(), 700))
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.tool)
            .collect();

        // A second, independent verdict file drawn with the same event name
        // and draw seed must draw exactly the same tools — the whole point
        // of a reproducible draw (never hand-picked, never re-rolled).
        cmd_spot_audit(tmp.path(), 701, "demo-event", &promoted, 2, 99).unwrap();
        let second: Vec<String> = load(&verdict_path(tmp.path(), 701))
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.tool)
            .collect();

        assert_eq!(first.len(), 2);
        assert_eq!(first, second);
    }

    #[test]
    fn spot_audit_different_events_can_draw_different_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let promoted = vec!["sh".to_string(), "cat".to_string(), "ls".to_string()];
        cmd_spot_audit(tmp.path(), 710, "event-a", &promoted, 1, 5).unwrap();
        cmd_spot_audit(tmp.path(), 711, "event-b", &promoted, 1, 5).unwrap();
        let a = load(&verdict_path(tmp.path(), 710)).unwrap();
        let b = load(&verdict_path(tmp.path(), 711)).unwrap();
        // Same draw seed, different event names: `stratum_seed` mixes the
        // event name in, so the two draws are not forced to correlate.
        // This does not assert they *always* differ (a same-tool draw is
        // possible by chance with only 3 candidates) — it asserts the
        // mechanism actually consulted the event name, via the stratum
        // labels below, which is the property that matters.
        assert_eq!(
            effective_stratum(&a.entries[0]),
            "spot-audit:event-a".to_string()
        );
        assert_eq!(
            effective_stratum(&b.entries[0]),
            "spot-audit:event-b".to_string()
        );
    }

    #[test]
    fn spot_audit_takes_the_whole_promoted_set_when_smaller_than_the_sample_size() {
        let tmp = tempfile::tempdir().unwrap();
        // The exact edge case named in spec §13.1b: the bundled-short-flag
        // promotion had only 5 promoted tools, below the 5-10 target.
        // Modeled here with 2 real tools against a sample of 8.
        let promoted = vec!["sh".to_string(), "cat".to_string()];
        cmd_spot_audit(tmp.path(), 720, "small-family", &promoted, 8, 1).unwrap();
        let file = load(&verdict_path(tmp.path(), 720)).unwrap();
        assert_eq!(
            file.entries.len(),
            2,
            "every promoted tool must be audited when the promoted set is smaller than --sample \
             — never a padded count, never a silently smaller draw"
        );
        for entry in &file.entries {
            assert_eq!(
                entry.spot_audit_event.as_deref(),
                Some("small-family"),
                "every drawn tool must be tagged with the promotion event it spot-checks"
            );
            assert!(
                entry
                    .include_reason
                    .as_deref()
                    .unwrap()
                    .contains("smaller than the requested sample size"),
                "the shortfall must be recorded in the entry, not just printed and forgotten"
            );
        }
    }

    #[test]
    fn spot_audit_is_idempotent_and_does_not_duplicate_an_already_present_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let promoted = vec!["sh".to_string(), "cat".to_string()];
        cmd_spot_audit(tmp.path(), 730, "repeat-event", &promoted, 8, 3).unwrap();
        cmd_spot_audit(tmp.path(), 730, "repeat-event", &promoted, 8, 3).unwrap();
        let file = load(&verdict_path(tmp.path(), 730)).unwrap();
        assert_eq!(file.entries.len(), 2, "re-running must not duplicate tools");
    }

    /// The exact shape of the real bundled-short-flag backfill (spec
    /// §13.1b): a tool the spot-audit's random draw names is *already* in
    /// the manifest with a real prior verdict, recorded against a parse a
    /// grammar fix has since changed. `cmd_spot_audit` must tag it into the
    /// new stratum without duplicating it, without touching its verdict or
    /// note, and without silently dropping it from the draw either.
    #[test]
    fn spot_audit_tags_an_already_reviewed_entry_without_touching_its_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 740, &[("tmux", "ok"), ("cat", "ok")]);
        {
            let mut f = load(&path).unwrap();
            let tmux = f.entries.iter_mut().find(|e| e.tool == "tmux").unwrap();
            tmux.verdict = Some("wrong".to_string());
            tmux.note = "bundled-short-flag collapse, pre-fix".to_string();
            save(&path, &f).unwrap();
        }

        let promoted = vec!["tmux".to_string()];
        cmd_spot_audit(
            tmp.path(),
            740,
            "bundled-short-flag-942890d",
            &promoted,
            8,
            11,
        )
        .unwrap();

        let after = load(&path).unwrap();
        assert_eq!(
            after.entries.len(),
            2,
            "the existing entry must not be duplicated"
        );
        let tmux = after.entries.iter().find(|e| e.tool == "tmux").unwrap();
        assert_eq!(
            tmux.spot_audit_event.as_deref(),
            Some("bundled-short-flag-942890d"),
            "an already-present tool named in the draw must still be tagged into the stratum"
        );
        assert_eq!(
            tmux.verdict.as_deref(),
            Some("wrong"),
            "a pre-existing verdict must survive untouched — only `xtask audit amend` may \
             correct it, never a draw"
        );
        assert_eq!(tmux.note, "bundled-short-flag collapse, pre-fix");
        // The untouched second tool is unaffected.
        let cat = after.entries.iter().find(|e| e.tool == "cat").unwrap();
        assert!(cat.spot_audit_event.is_none());
    }

    #[test]
    fn spot_audit_refuses_an_empty_promoted_list() {
        let tmp = tempfile::tempdir().unwrap();
        let err = cmd_spot_audit(tmp.path(), 740, "empty-event", &[], 8, 1).unwrap_err();
        assert!(err.to_string().contains("named no tools"));
    }

    #[test]
    fn spot_audit_entries_are_reported_under_their_own_stratum_row_in_cmd_report() {
        let tmp = tempfile::tempdir().unwrap();
        let promoted = vec!["sh".to_string(), "cat".to_string()];
        cmd_spot_audit(tmp.path(), 750, "reported-event", &promoted, 8, 2).unwrap();
        {
            let mut f = load(&verdict_path(tmp.path(), 750)).unwrap();
            for e in &mut f.entries {
                e.verdict = Some("correct".to_string());
            }
            save(&verdict_path(tmp.path(), 750), &f).unwrap();
        }
        // Smoke test: must not panic, and every entry's effective stratum
        // must be the per-event row, distinct from ordinary parse-status
        // strata and from `forced-inclusion`.
        cmd_report(tmp.path(), 750).unwrap();
        let f = load(&verdict_path(tmp.path(), 750)).unwrap();
        for e in &f.entries {
            let stratum = effective_stratum(e);
            assert_eq!(stratum, "spot-audit:reported-event");
            assert_ne!(stratum, FORCED_INCLUSION_STRATUM);
        }
    }

    // -------------------------------------------------------------
    // Amendment: `cmd_amend` and aggregate computation reading it
    // -------------------------------------------------------------

    /// `cmd_amend` end to end: the original verdict/note on disk are
    /// untouched, the amendment is appended, and `accuracy_over` — the
    /// shared machinery every accuracy number in `cmd_report` goes through
    /// — counts the *amended* value, not the original. This is the
    /// concrete regression test for "aggregate computation uses the
    /// amended verdict, while the file still shows the original".
    #[test]
    fn cmd_amend_updates_aggregate_accuracy_while_preserving_the_original_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 900, &[("tmux", "ok"), ("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            f.entries[0].k1 = Some(true);
            f.entries[1].verdict = Some("correct".to_string());
            save(&path, &f).unwrap();
        }

        // Before amending: both entries count as correct.
        let before = load(&path).unwrap();
        assert_eq!(accuracy_over(before.entries.iter()), (2, 2));

        cmd_amend(
            tmp.path(),
            900,
            "tmux",
            "wrong",
            Some("bundled-short-flag collapse, same shape judged wrong elsewhere".to_string()),
            "reviewer inconsistency caught in reconciliation".to_string(),
        )
        .unwrap();

        let after = load(&path).unwrap();
        let tmux = after.entries.iter().find(|e| e.tool == "tmux").unwrap();
        // The file still shows the original verdict and (empty) note.
        assert_eq!(tmux.verdict.as_deref(), Some("correct"));
        assert_eq!(tmux.note, "");
        // ...plus a complete amendment record.
        assert_eq!(tmux.amendments.len(), 1);
        assert_eq!(tmux.amendments[0].previous_verdict, "correct");
        assert_eq!(tmux.amendments[0].new_verdict, "wrong");
        assert_eq!(
            tmux.amendments[0].reason,
            "reviewer inconsistency caught in reconciliation"
        );
        // Aggregate accuracy now reflects the amendment: one correct, one
        // wrong, out of two judged — not two correct.
        assert_eq!(accuracy_over(after.entries.iter()), (1, 2));
    }

    #[test]
    fn cmd_amend_rejects_an_unknown_tool() {
        let tmp = tempfile::tempdir().unwrap();
        write_sample_file(tmp.path(), 901, &[("sh", "ok")]);
        let err = cmd_amend(
            tmp.path(),
            901,
            "does-not-exist",
            "wrong",
            Some("note".to_string()),
            "reason".to_string(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }

    #[test]
    fn cmd_amend_rejects_a_blank_reason_and_leaves_the_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 902, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            save(&path, &f).unwrap();
        }
        let err = cmd_amend(
            tmp.path(),
            902,
            "sh",
            "wrong",
            Some("note".to_string()),
            "   ".to_string(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("reason"));
        let after = load(&path).unwrap();
        assert!(after.entries[0].amendments.is_empty());
        assert_eq!(after.entries[0].verdict.as_deref(), Some("correct"));
    }

    #[test]
    fn cmd_amend_rejects_a_wrong_verdict_missing_its_note() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 903, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            save(&path, &f).unwrap();
        }
        let err =
            cmd_amend(tmp.path(), 903, "sh", "wrong", None, "reason".to_string()).unwrap_err();
        assert!(err.to_string().contains("note"));
    }

    /// A manifest with no amended entries at all still reports cleanly —
    /// `wilson_caveat_lines`'s zero-amendment branch, exercised through the
    /// same `cmd_report` entry point real usage goes through.
    #[test]
    fn cmd_report_runs_cleanly_with_zero_amendments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 904, &[("sh", "ok")]);
        let mut f = load(&path).unwrap();
        f.entries[0].verdict = Some("correct".to_string());
        save(&path, &f).unwrap();
        cmd_report(tmp.path(), 904).unwrap();
    }

    /// `cmd_report` (and therefore its printed accuracy figures) run
    /// cleanly over a manifest containing an amendment, exercising
    /// `wilson_caveat_lines`'s non-zero branch end to end.
    #[test]
    fn cmd_report_runs_cleanly_with_an_amended_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 905, &[("tmux", "ok"), ("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            f.entries[1].verdict = Some("correct".to_string());
            save(&path, &f).unwrap();
        }
        cmd_amend(
            tmp.path(),
            905,
            "tmux",
            "wrong",
            Some("bundled-short-flag collapse".to_string()),
            "reviewer inconsistency caught in reconciliation".to_string(),
        )
        .unwrap();
        cmd_report(tmp.path(), 905).unwrap();
    }

    #[test]
    fn verdict_favorability_orders_correct_above_incomplete_above_wrong() {
        assert!(verdict_favorability("correct") > verdict_favorability("incomplete"));
        assert!(verdict_favorability("incomplete") > verdict_favorability("wrong"));
        assert_eq!(verdict_favorability("skip"), None);
    }
}
