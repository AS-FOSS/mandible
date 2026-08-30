//! `cargo run -p xtask -- audit`: a bounded, random, human-reviewed sample
//! of real tools, comparing raw captured `--help` text against the parsed
//! tree — the first instrument in this project that compares output to
//! *truth* rather than to itself.
//!
//! # Why this exists
//!
//! Every prior instrument measures agreement with the parser, not with
//! reality: the corpus asserts "the parser still does what it did", the
//! coverage sweep counts what the parser produced, snapshots bless whatever
//! came out, and [`crate::misattribution`] — the project's first genuine
//! correctness instrument — found ~4 broken tools in 2,266 (0.18%), which
//! cannot explain a maintainer-observed 25-33% error rate on hand
//! inspection. Two of the four tools ever actually read by a human (`git`,
//! `lsof`) had serious defects invisible to every automated gate. The real
//! accuracy is unknown; this module measures it, on a sample small enough
//! for a human to review by hand (~30s/tool, so 80 tools is an afternoon)
//! and large enough for the resulting rate to mean something (`n=80` gives
//! roughly ±8-10 points at 95% confidence — see [`wilson_interval`]).
//!
//! Crucially, the review effort is **capitalized**: every reviewed tool can
//! become a `corpus/` fixture (spec §13.2, `corpus/README.md`), so one pass
//! over a tool produces two things — a data point in the accuracy number,
//! and a permanent regression-net entry encoding *verified* truth rather
//! than a blessed guess. `corpus/lsof/4.95.0` is the cautionary tale this
//! guards against: committed green by `--bless` without this kind of read.
//!
//! # Shape
//!
//! - `xtask audit freeze` (`crate::queue::cmd_freeze`) sweeps `PATH` once,
//!   classifies every tool by parse status (`ok`/`low-confidence`/
//!   `verbatim`/`no-tier`, plus whatever other status
//!   [`crate::status::compute`] actually produces for the population, e.g.
//!   `suspicious` — never a fixed four-way bucket forced onto the real
//!   data), shuffle-stratifies the result with a recorded seed, and writes
//!   the ordered list plus the raw captured bytes behind it to
//!   `<dir>/queue.toml` / `<dir>/queue-captures/`. `xtask audit sample`
//!   (`crate::queue::cmd_sample`) then just advances a cursor through that
//!   frozen queue and persists the next slice to a resumable verdict file —
//!   see `crate::queue`'s own doc comment for the full design, why it
//!   replaced a live re-sweep on every draw, and the caveats freezing a
//!   population honestly carries.
//! - [`cmd_review`] is the interactive loop: raw text and parsed tree side
//!   by side, a one-word verdict, persisted after every tool so an
//!   interrupted session resumes rather than restarts.
//! - [`cmd_emit`]/[`cmd_ingest`] are the non-interactive twin of the same
//!   loop — this machine has no tty (AGENTS.md §3.2), so a review workflow
//!   that only works interactively is untestable here and unusable there.
//!   `emit` writes every pending pair to a file for offline reading;
//!   `ingest` reads a plain-text verdicts file back in.
//! - [`cmd_report`] renders per-stratum and overall accuracy with an
//!   explicit sample size and confidence interval — **never a bare
//!   percentage**, which is the specific thing that misled this project
//!   before (`%flags_text`/`%described`, spec §13.1b).
//! - [`cmd_fixtures`] turns a reviewed tool into a `corpus/`-shaped fixture:
//!   a `correct` verdict is a human assertion of correctness exactly like
//!   `--bless` (`corpus/README.md`'s own words) and gets a real
//!   `expected.snap`; `incomplete`/`wrong` get `[xfail]` with the
//!   reviewer's note as `reason`. See that function's doc comment for why
//!   it stages into a scratch directory by default rather than writing
//!   straight into the gated `corpus/` tree.
//!
//! # No cherry-picking, structurally
//!
//! There is no "skip this one" that silently reshapes the sample:
//! [`cmd_review`]'s only responses are `correct`/`incomplete`/`wrong`/
//! `skip`, and `skip` is *recorded*, not omitted — a skipped tool still
//! occupies its slot in the verdict file and is visible in
//! [`cmd_report`]'s output, just excluded from the accuracy ratio (there is
//! nothing to judge). The draw itself never consults the tool's own status
//! or name when deciding who gets sampled — see
//! `crate::queue::shuffle_stratify`, which only ever sees `(tool, stratum)`
//! pairs and a seeded shuffle.

use crate::existence::{self, FabricationKind};
use crate::misattribution::RecordingProbe;
use crate::status;
use mandible_core::audit::{
    extract_tag_override, family_meaning, load, parse_verdict_word, save, tag_display,
    verdict_path, AuditFile, AuditMeta, Entry,
};
use mandible_core::{CommandNode, Entity};
use mandible_extract::exec::ExecOutput;
use mandible_extract::{default_tiers_with_probe, ExtractionResult, Runner};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

/// The manifest schema itself — [`Entry`], [`AuditFile`], [`AuditMeta`], the
/// load/save functions, and the verdict-word/tag-override parsers — lives in
/// [`mandible_core::audit`], not here, so `mandible --review` can read and
/// write the exact same `audit/<seed>.toml` file this module produces
/// without a second, drifting copy of the format. See that module's own doc
/// comment for the full rationale. What stays here: drawing the sample and
/// computing the K1/K2/K3 pre-tag suggestions, both of which need this
/// crate's own detectors (`status`, `existence`, `misattribution`) and a
/// live extraction pass — `mandible --review` never recomputes either, it
/// only displays what's already in the file.
///
/// The synthetic stratum label [`cmd_report`] uses for every entry carrying
/// an [`Entry::include_reason`], so a force-included tool is tallied
/// separately from the ordinary stratified draw rather than blended into
/// its nominal [`Entry::stratum`] — see that field's doc comment.
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

// ---------------------------------------------------------------------
// K1/K2 pre-tagging (see [`Entry::k1`]/[`Entry::k2`]'s doc comments for the
// full rationale). Each `_signature` function derives a suggested tag from
// structural properties of the extracted tree/raw text alone — no tool name
// is ever consulted, so these stay within AGENTS.md §1's "no per-tool
// logic" invariant exactly like every other detector in this workspace.
// ---------------------------------------------------------------------

/// True for a flag matching the GCC-family single-dash-long-option defect
/// signature: the short-flag grammar took one character as `short` and
/// glued the rest of a multi-character single-dash spelling onto
/// `value_name` (`-fdump-scos` -> `short=Some('f')`, `long=None`,
/// `value_name=Some("dump-scos")`). See [`Entry::k1`].
fn is_k1_flag(flag: &Entity) -> bool {
    flag.short().is_some() && flag.long().is_none() && flag.value_name.is_some()
}

/// `(matching, total)` flag counts across `node` and every descendant, for
/// the K1 pre-tag's display line (e.g. "839/1454 flags match").
fn k1_signature_stats(node: &CommandNode) -> (usize, usize) {
    let mut matching = node.flags().filter(|f| is_k1_flag(f)).count();
    let mut total = node.flags().count();
    for child in &node.subcommands {
        let (m, t) = k1_signature_stats(child);
        matching += m;
        total += t;
    }
    (matching, total)
}

/// The K1 pre-tag suggestion: `Some(true)` when `root`'s tree contains at
/// least one [`is_k1_flag`] match anywhere, `None` when it contains none
/// (nothing to flag — never `Some(false)`, since there is no "confirmed not
/// K1" state worth asserting for a tool that never exhibited the shape in
/// the first place).
fn k1_signature(root: &CommandNode) -> Option<bool> {
    let (matching, _) = k1_signature_stats(root);
    if matching > 0 {
        Some(true)
    } else {
        None
    }
}

/// True when `name` occurs as *some* whitespace-delimited token anywhere in
/// `raw` — not restricted to a line's first token the way
/// `existence::line_start_words` is. Punctuation immediately touching the
/// token (as in a comma-separated list) is trimmed the same way the K2
/// false-positive class actually presents. Used only to *explain* an
/// existing existence-detector fabrication, never to suppress one directly
/// — see [`k2_signature_stats`].
fn token_occurs_anywhere(raw: &str, name: &str) -> bool {
    raw.split_whitespace().any(|tok| {
        tok.trim_matches(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')) == name
    })
}

/// `(attributable, total)` counts of `report`'s subcommand-kind
/// fabrications that are plausibly explained by the existence detector's
/// own multi-column/comma-separated tokenization gap (K2) rather than
/// genuine parser fabrication.
///
/// **That gap is closed** — `existence::list_row_words` reads a whole list
/// row now, so a real grid or comma-joined index produces no fabrication
/// for this to attribute, and in practice this returns `(0, 0)` for the
/// tools it was written for. It is kept, unchanged in behaviour, as the
/// regression signal: a fabrication that *is* attributable here again means
/// the list-row rule stopped recognising a layout it used to.
///
/// A fabrication is "attributable" when its
/// name occurs as *some* token anywhere in the raw text
/// ([`token_occurs_anywhere`]), just not at the line-start position the
/// detector itself requires. Flag-kind fabrications are out of scope here —
/// the tokenizer gap this class explains is specific to how
/// `line_start_words` reads *lines*, which only ever gates subcommand
/// names; a flag spelling's existence check ([`existence::spelling_occurs`])
/// already scans the whole raw text unconditionally and has no equivalent
/// gap to explain.
fn k2_signature_stats(report: &existence::ExistenceReport, raw: &str) -> (usize, usize) {
    let subcommand_names: Vec<&str> = report
        .fabrications
        .iter()
        .filter(|f| f.kind == FabricationKind::Subcommand)
        .map(|f| f.name.as_str())
        .collect();
    let total = subcommand_names.len();
    let attributable = subcommand_names
        .iter()
        .filter(|name| token_occurs_anywhere(raw, name))
        .count();
    (attributable, total)
}

/// The K2 pre-tag suggestion: `Some(true)` when *every* subcommand-kind
/// existence fabrication for this tool is attributable to the detector's
/// own tokenizer gap (near-certainly detector noise, not a parser defect),
/// `Some(false)` when at least one is not (worth a real look — could be a
/// genuine [M-10]-shaped fabrication), `None` when the tool has no
/// subcommand-kind fabrications to judge at all.
fn k2_signature(report: &existence::ExistenceReport, raw: &str) -> Option<bool> {
    let (attributable, total) = k2_signature_stats(report, raw);
    if total == 0 {
        None
    } else {
        Some(attributable == total)
    }
}

/// True for a node carrying nothing at all: no flags, no subcommands, and
/// no summary — the same `empty` predicate `status::structure_sanity`'s own
/// `count_suspicious` uses, reused here rather than redefined so the two
/// "is this node genuinely empty" checks in this codebase can never drift
/// apart.
fn is_bare_stub(node: &CommandNode) -> bool {
    node.flags().next().is_none() && node.subcommands.is_empty() && node.summary.is_none()
}

/// True for a bare stub ([`is_bare_stub`]) that is *also* not
/// [`CommandNode::heading_attested`] and not
/// [`CommandNode::invocation_attested`] — its name came from a native/cobra
/// artifact (e.g. a `__complete` candidate) rather than a recognized
/// `--help` heading or a headingless invocation table (spec §7 Tier B).
/// This is provable from the single extraction pass this pre-tag is
/// computed from: `help_text::raw_help` refuses to probe any node whose
/// `heading_attested` bit is false (`mandible-extract/src/
/// help_text/mod.rs`) — `invocation_attested` is deliberately never checked
/// by that gate either, by spec §6's own decision — so unlike an ordinary
/// un-recursed subcommand — merely not fetched *yet* — this one
/// structurally cannot ever be, live navigation included. `git-lfs`'s tree
/// is the motivating case: 36 nodes, 34 of them exactly this shape, which
/// is also why its `status::compute` label is `suspicious`.
///
/// A headingless-table node still counts here even though it *is*
/// existence-attested: it is genuinely never probed, so K3's "review this
/// gap" suggestion is still the honest signal for it. `invocation_attested`
/// only exempts a node from being counted as *fabricated* (see
/// [`crate::status::structure_sanity`], which is the detector that actually
/// gates `min_status`) — it does not make the node any less permanently
/// un-probed.
fn is_attestation_gated_stub(node: &CommandNode) -> bool {
    is_bare_stub(node) && !node.heading_attested
}

/// Count of [`is_attestation_gated_stub`] matches across `node` and every
/// descendant — called only on `root`'s subcommands, never on `root`
/// itself, matching `status::structure_sanity`'s own root-exclusion
/// (`root` is the literal executable name resolved from `PATH`, never
/// something a tier guessed at, so it needs no heading to attest to).
fn count_attestation_gated_stubs(node: &CommandNode) -> usize {
    let this = usize::from(is_attestation_gated_stub(node));
    this + node
        .subcommands
        .iter()
        .map(count_attestation_gated_stubs)
        .sum::<usize>()
}

/// Total flag count across `node` and every descendant.
fn total_flags(node: &CommandNode) -> usize {
    node.flags().count() + node.subcommands.iter().map(total_flags).sum::<usize>()
}

/// True when `root` has at least one subcommand yet the whole tree — root
/// included — carries zero flags anywhere. `openssl`'s shape: the top-level
/// `--help` is a bare command grid with no options section at all, so the
/// single root-only extraction pass this pre-tag is computed from
/// ([`classify_one`]/[`Runner::extract_full`], which never recurses) never
/// surfaces a single flag, and each of its 151 subcommands' own help
/// genuinely was never fetched. Most real tools' root `--help` documents at
/// least one flag (`-h`/`--version` if nothing else), which is what makes
/// "zero flags anywhere, but subcommands exist" a reasonable single-pass
/// proxy for this specific gap rather than every multi-level tool's
/// ordinary lazy-fill state (which would otherwise over-tag almost every
/// sampled tool, since `extract_full` never recurses into subcommands at
/// all).
fn has_unfetched_subcommand_help(root: &CommandNode) -> bool {
    !root.subcommands.is_empty() && total_flags(root) == 0
}

/// The K3 pre-tag suggestion (see [`mandible_core::audit::Entry::k3`]):
/// `Some(true)` when `root`'s single-pass snapshot shows either known
/// cause — an attestation-gated stub anywhere in the tree, or the
/// whole-tree-zero-flags shape — `None` otherwise. Same "no `Some(false)`"
/// convention as [`k1_signature`]: there is nothing to assert-not for a
/// tool that shows neither shape.
fn k3_signature(root: &CommandNode) -> Option<bool> {
    let gated_stubs: usize = root
        .subcommands
        .iter()
        .map(count_attestation_gated_stubs)
        .sum();
    if gated_stubs > 0 || has_unfetched_subcommand_help(root) {
        Some(true)
    } else {
        None
    }
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
/// stated reason is exactly the kind of unauditable claim spec.md Appendix A
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

/// `xtask audit spot-audit` (spec §13.1b's sixth rule): the spot-audit
/// stratum mechanism that section named as missing. Draws `--sample` tools
/// **at random** from `promoted` — the tool list one specific mass-`ok`
/// promotion event actually changed, never the whole fleet — classifies
/// each with one fresh extraction pass ([`classify_one`], same "no second
/// probe" property [`Classified`]'s doc comment already gives every other
/// entry point here), and merges them into `<dir>/<seed>.toml` as their own
/// `spot-audit:<event>` stratum ([`effective_stratum`]), reported by
/// [`cmd_report`] alongside the ordinary parse-status strata and
/// [`FORCED_INCLUSION_STRATUM`].
///
/// **The draw is reproducible, not hand-picked.** `draw_seed` mixed with
/// `event` via [`crate::rng::stratum_seed`] — the exact per-stratum seed
/// mix `crate::queue::shuffle_stratify` already uses for the frozen
/// queue — seeds a Fisher-Yates shuffle ([`crate::rng::seeded_shuffle`])
/// over `promoted`; the same event name and seed always draw the same
/// tools, and two different events never share a correlated draw pattern.
///
/// **A promoted set smaller than `sample` is handled explicitly, never
/// silently.** When `promoted.len() < sample`, every promoted tool is
/// drawn — not a padded count pretending the full sample size was met, and
/// not a silently smaller draw with nothing said about it — and the
/// printed summary states the shortfall in plain words. This is the exact
/// edge case the bundled-short-flag backfill (5 promoted tools, below the
/// 5–10 target because the family had only 5 audited members) hits.
///
/// **A tool already present in the verdict file is tagged, never
/// duplicated or silently skipped.** A promotion event's own promoted set
/// frequently overlaps the ordinary stratified draw that a prior audit
/// already sampled — the motivating case here (spec §13.1b's backfill) is
/// exactly this: all 5 bundled-short-flag-promoted tools were already
/// present in `audit/2.toml`, reviewed **against the pre-fix parse**, and
/// three were judged `wrong`/one `incomplete` *for that same
/// bundled-short-flag defect* — the tool a promotion event just fixed.
/// Silently skipping an already-present tool (this function's first
/// version did) would mean the spot-audit stratum never gains a row for
/// it at all, defeating the entire point. Re-classifying and overwriting
/// its verdict outright would silently destroy a real prior human
/// judgment. So instead: an already-present entry is re-tagged with
/// [`Entry::spot_audit_event`] (moving it into this event's reported row
/// without touching its stratum, verdict, note, or history), and its
/// existing verdict — now potentially stale against a changed parse — is
/// left exactly as recorded for a human to re-review and correct via
/// `xtask audit amend` (never for this function to overwrite; amending is
/// a deliberate, reasoned act, not a side effect of a draw). Only a tool
/// genuinely new to the file is classified fresh and added as a pending
/// entry. Re-running this command with the same inputs is safe either way:
/// an already-tagged entry is left alone on a second pass.
pub fn cmd_spot_audit(
    dir: &Path,
    seed: u64,
    event: &str,
    promoted: &[String],
    sample: usize,
    draw_seed: u64,
) -> anyhow::Result<()> {
    if promoted.is_empty() {
        anyhow::bail!("--promoted named no tools — nothing to spot-audit for event {event:?}");
    }

    let mut pool = promoted.to_vec();
    crate::rng::seeded_shuffle(&mut pool, crate::rng::stratum_seed(draw_seed, event));
    let take_n = sample.min(pool.len());
    let drawn: Vec<String> = pool.into_iter().take(take_n).collect();

    let path = verdict_path(dir, seed);
    let mut file = if path.is_file() {
        load(&path)?
    } else {
        AuditFile {
            meta: AuditMeta {
                seed,
                sample_size: 0,
            },
            entries: Vec::new(),
        }
    };

    let reason = if promoted.len() < sample {
        format!(
            "spot-audit of promotion event {event:?}: {} of {} promoted tool(s) drawn (seed \
             {draw_seed}) — every promoted tool was audited because the promoted set was \
             smaller than the requested sample size ({sample})",
            drawn.len(),
            promoted.len(),
        )
    } else {
        format!(
            "spot-audit of promotion event {event:?}: {} of {} promoted tool(s) drawn at random \
             (seed {draw_seed})",
            drawn.len(),
            promoted.len(),
        )
    };

    let existing_tools: HashSet<String> = file.entries.iter().map(|e| e.tool.clone()).collect();
    let mut added = 0usize;
    let mut tagged_existing = 0usize;
    for tool in &drawn {
        if existing_tools.contains(tool) {
            if let Some(existing) = file.entries.iter_mut().find(|e| &e.tool == tool) {
                if existing.spot_audit_event.is_none() {
                    existing.spot_audit_event = Some(event.to_string());
                    if existing.include_reason.is_none() {
                        existing.include_reason = Some(reason.clone());
                    }
                    tagged_existing += 1;
                }
            }
            continue;
        }
        let classified = classify_one(tool);
        let mut entry = entry_from_classified(tool.clone(), &classified, Some(reason.clone()));
        entry.spot_audit_event = Some(event.to_string());
        file.entries.push(entry);
        added += 1;
    }
    file.entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    save(&path, &file)?;

    println!(
        "spot-audit:{event}: drew {} of {} promoted tool(s)",
        drawn.len(),
        promoted.len(),
    );
    if promoted.len() < sample {
        println!(
            "note: the promoted set has only {} tool(s), fewer than the requested sample size \
             {sample} — every promoted tool was audited rather than silently sampling fewer or \
             padding the count to look like a full draw.",
            promoted.len(),
        );
    }
    println!(
        "{added} new pending entr{s} written, {tagged_existing} already-present entr{s2} tagged \
         into this stratum, at {} ({} tool(s) now in stratum spot-audit:{event})",
        path.display(),
        drawn.len(),
        s = if added == 1 { "y" } else { "ies" },
        s2 = if tagged_existing == 1 { "y" } else { "ies" },
    );
    if tagged_existing > 0 {
        println!(
            "note: {tagged_existing} of those were already in the file with a prior verdict — \
             that verdict is left exactly as recorded (it may now be stale against a changed \
             parse) for a human to re-review and correct via `xtask audit amend`, never \
             overwritten by this draw."
        );
    }
    Ok(())
}

/// Render `node` as the same YAML snapshot shown side by side with a tool's
/// raw `--help` text in [`cmd_review`]/[`cmd_emit`] — shared so the two
/// entry points can never render a tree differently.
pub(crate) fn render_snapshot(node: Option<&CommandNode>) -> String {
    match node {
        Some(node) => {
            let snapshot = mandible_core::to_snapshot(node);
            serde_yaml::to_string(&snapshot)
                .unwrap_or_else(|e| format!("(snapshot serialization failed: {e})\n"))
        }
        None => "(no root produced by any tier)\n".to_string(),
    }
}

/// `xtask audit review`: the interactive loop. Presents the raw `--help`
/// text and the parsed tree for every still-pending entry, one at a time,
/// reads a verdict line (`<word> [note...]`) from `input`, and persists the
/// file after **every** entry — an interrupted session (killed process,
/// closed terminal, EOF on `input`) leaves everything answered so far
/// recorded and everything else still pending, so a re-run resumes exactly
/// where it stopped rather than re-asking or restarting.
///
/// Deliberately line-buffered (`<word><Enter>`), not a raw single-keystroke
/// terminal mode: this environment has no tty (AGENTS.md §3.2 — `enable raw
/// mode` fails with "No such device or address" here), so a design that
/// depended on raw mode would be unwritten code from this box's point of
/// view. A short word plus Enter is close enough to "one keystroke" for the
/// ~30s/tool target, and — unlike a raw-mode reader — it works identically
/// whether `input` is a real terminal or (as every test here uses) a
/// `Cursor` over a fixed byte string, which is what makes this loop
/// testable at all without a pty.
pub fn cmd_review(
    dir: &Path,
    seed: u64,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let pending: Vec<usize> = file.pending().collect();
    if pending.is_empty() {
        writeln!(output, "nothing pending in {}", path.display())?;
        return Ok(());
    }
    writeln!(
        output,
        "{} pending of {} total. Verdict: c(orrect) / i(ncomplete) / w(rong) / s(kip), \
         optionally followed by a space and a note. Add `k1=true`/`k1=false`/`k2=true`/\
         `k2=false`/`k3=true`/`k3=false` anywhere in the note to override a pre-tag; omitting \
         it confirms the suggestion shown below. Blank line or end of input stops \
         (already-recorded verdicts are saved after every tool).",
        pending.len(),
        file.entries.len()
    )?;

    for idx in pending {
        let tool = file.entries[idx].tool.clone();
        let stratum = file.entries[idx].stratum.clone();
        let k1 = file.entries[idx].k1;
        let k2 = file.entries[idx].k2;
        let k3 = file.entries[idx].k3;
        let include_reason = file.entries[idx].include_reason.clone();
        let classified = classify_one(&tool);
        writeln!(output, "\n=== {tool}  (stratum: {stratum}) ===")?;
        if let Some(reason) = &include_reason {
            writeln!(output, "forced inclusion: {reason}")?;
        }
        writeln!(
            output,
            "{}",
            tag_display("K1 (single-dash-long defect)", k1, "k1")
        )?;
        writeln!(
            output,
            "{}",
            tag_display("K2 (existence-detector tokenizer gap)", k2, "k2")
        )?;
        writeln!(
            output,
            "{}",
            tag_display("K3 (subcommand help never fetched)", k3, "k3")
        )?;
        writeln!(output, "--- raw --help ---")?;
        writeln!(
            output,
            "{}",
            classified
                .raw_text
                .as_deref()
                .unwrap_or("(no output captured)")
        )?;
        writeln!(output, "--- parsed tree ---")?;
        writeln!(
            output,
            "{}",
            render_snapshot(classified.result.root.as_ref())
        )?;
        write!(output, "verdict> ")?;
        output.flush()?;

        let mut line = String::new();
        let bytes_read = input.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF: stop here, everything already answered is already saved.
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let word = parts.next().unwrap_or("");
        let mut note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)?;
        let k1_override = extract_tag_override(&mut note, "k1");
        let k2_override = extract_tag_override(&mut note, "k2");
        let k3_override = extract_tag_override(&mut note, "k3");

        file.entries[idx].verdict = Some(verdict.to_string());
        file.entries[idx].note = note;
        if let Some(v) = k1_override {
            file.entries[idx].k1 = Some(v);
        }
        if let Some(v) = k2_override {
            file.entries[idx].k2 = Some(v);
        }
        if let Some(v) = k3_override {
            file.entries[idx].k3 = Some(v);
        }
        save(&path, &file)?;
        writeln!(output, "recorded: {verdict}")?;
    }

    writeln!(
        output,
        "\n{} pending remain in {}.",
        file.pending().count(),
        path.display()
    )?;
    Ok(())
}

/// `xtask audit emit`: write every pending pair (raw text + parsed tree) to
/// its own file under `emit_dir`, for a reviewer without a live terminal —
/// or without this machine's tty at all — to read offline and judge on
/// their own schedule. The counterpart, [`cmd_ingest`], reads the resulting
/// verdicts back in.
pub fn cmd_emit(dir: &Path, seed: u64, emit_dir: &Path) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;
    std::fs::create_dir_all(emit_dir)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", emit_dir.display()))?;

    let pending: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.verdict.is_none())
        .collect();
    for entry in &pending {
        let classified = classify_one(&entry.tool);
        let mut buf = String::new();
        buf.push_str(&format!(
            "tool: {}\nstratum: {}\n",
            entry.tool, entry.stratum
        ));
        if let Some(reason) = &entry.include_reason {
            buf.push_str(&format!("forced inclusion: {reason}\n"));
        }
        buf.push_str(&format!(
            "{}\n{}\n{}\n\n",
            tag_display("K1 (single-dash-long defect)", entry.k1, "k1"),
            tag_display("K2 (existence-detector tokenizer gap)", entry.k2, "k2"),
            tag_display("K3 (subcommand help never fetched)", entry.k3, "k3"),
        ));
        buf.push_str("=== raw --help ===\n");
        buf.push_str(
            classified
                .raw_text
                .as_deref()
                .unwrap_or("(no output captured)"),
        );
        buf.push_str("\n\n=== parsed tree ===\n");
        buf.push_str(&render_snapshot(classified.result.root.as_ref()));
        let file_path = emit_dir.join(format!("{}.txt", sanitize_filename(&entry.tool)));
        std::fs::write(&file_path, buf)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", file_path.display()))?;
    }

    println!(
        "emitted {} pending pair(s) to {}",
        pending.len(),
        emit_dir.display()
    );
    println!(
        "review offline, then write a verdicts file (one line per tool: `<tool> <verdict> \
         [note...]`, optionally including `k1=true`/`k1=false`/`k2=true`/`k2=false`/\
         `k3=true`/`k3=false` anywhere in the note to override a pre-tag) and run: \
         cargo run -p xtask -- audit ingest --seed {seed} --verdicts <file>"
    );
    Ok(())
}

/// A tool name is never empty and, on every platform this project targets,
/// never contains `/` (§ `resolve_tool`'s own PATH-search doesn't accept
/// path separators in a bare tool name either), so this exists only to be
/// defensive about the one other filesystem-hostile case worth naming.
/// `pub(crate)` so `crate::queue`'s capture directory naming
/// (`queue-captures/<tool>/`) can use the same rule.
pub(crate) fn sanitize_filename(tool: &str) -> String {
    tool.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect()
}

/// `xtask audit ingest`: read a plain verdicts file (`# comments` and blank
/// lines ignored; otherwise `<tool> <verdict> [note...]` per line) and
/// apply it to `path`'s entries. An unknown tool name is reported, not
/// silently dropped. An entry that already carries a verdict is left alone
/// unless `overwrite` is set — so re-running `ingest` on a file that
/// includes already-applied lines is safe and idempotent, the same
/// resumability property `crate::queue::cmd_sample`/[`cmd_review`] give the
/// rest of this workflow.
pub fn cmd_ingest(
    dir: &Path,
    seed: u64,
    verdicts_path: &Path,
    overwrite: bool,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let raw = std::fs::read_to_string(verdicts_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", verdicts_path.display()))?;

    let mut applied = 0usize;
    let mut already = 0usize;
    let mut unknown: Vec<String> = Vec::new();

    for (lineno, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, char::is_whitespace);
        let tool = parts.next().unwrap_or("");
        let word = parts.next().unwrap_or("");
        let mut note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)
            .map_err(|e| anyhow::anyhow!("{}:{}: {e}", verdicts_path.display(), lineno + 1))?;
        let k1_override = extract_tag_override(&mut note, "k1");
        let k2_override = extract_tag_override(&mut note, "k2");
        let k3_override = extract_tag_override(&mut note, "k3");

        // The same obligation the TUI enforces, applied here so the two
        // entry paths cannot disagree about what a complete record is. The
        // override tokens are already stripped above, so a line whose only
        // content was `k1=false` correctly counts as noteless.
        if mandible_core::audit::verdict_requires_note(verdict) && note.trim().is_empty() {
            anyhow::bail!(
                "{}:{}: verdict {:?} for {:?} needs a note — for wrong/incomplete the note is \
                 the finding, and an entry naming a tool with nothing about what was wrong \
                 gives later triage nothing to act on",
                verdicts_path.display(),
                lineno + 1,
                verdict,
                tool,
            );
        }

        let Some(entry) = file.entries.iter_mut().find(|e| e.tool == tool) else {
            unknown.push(tool.to_string());
            continue;
        };
        if entry.verdict.is_some() && !overwrite {
            already += 1;
            continue;
        }
        entry.verdict = Some(verdict.to_string());
        entry.note = note;
        if let Some(v) = k1_override {
            entry.k1 = Some(v);
        }
        if let Some(v) = k2_override {
            entry.k2 = Some(v);
        }
        if let Some(v) = k3_override {
            entry.k3 = Some(v);
        }
        applied += 1;
    }

    save(&path, &file)?;
    println!(
        "applied {applied} verdict(s); {already} already recorded (use --overwrite to replace); \
         {} unknown tool name(s) not in the sample{}",
        unknown.len(),
        if unknown.is_empty() {
            String::new()
        } else {
            format!(": {}", unknown.join(", "))
        }
    );
    Ok(())
}

/// `xtask audit amend`: correct one already-recorded verdict without
/// destroying it — see `mandible_core::audit::amend`'s doc comment for the
/// full mechanism this wraps. This is the **only** entry point that touches
/// [`mandible_core::audit::Entry::amendments`]; there is deliberately no
/// TUI counterpart (unlike `review`, which has both a terminal loop here
/// and an in-app twin in `mandible --review`).
///
/// **Why a subcommand and not a TUI flow:** `mandible --review`'s own loop
/// (`mandible/src/app_runner.rs::run_review`) walks
/// [`mandible_core::audit::AuditFile::needing_attention`] — pending entries
/// and verdicts with a missing obligatory note — and stops there by
/// construction; it never revisits an entry that already carries a
/// complete `correct` verdict, which is exactly the shape an amendment
/// needs to reach (`tmux` was `correct`, with a note that satisfied every
/// obligation already in place). Adding "browse every already-judged entry
/// and pick one to correct" to that loop is a real, separate feature — a
/// new navigation mode orthogonal to the linear pending-entry walk the
/// rest of that module is built around — not a natural extension of it.
/// This command needs no tty (AGENTS.md §3.2, same reasoning `emit`/
/// `ingest` already documented for this module) and is fully covered by
/// `cargo nextest run`, whereas a TUI flow would join `run_review` on the
/// "not exercised by the automated test suite" list. Nothing here stops a
/// future in-app amend key from being added on top of the same
/// `mandible_core::audit::amend` function this calls — see that function's
/// own doc comment, which already treats `mandible --review` as sharing the
/// schema `xtask` owns — but it is not required to satisfy this task's "not
/// by hand-editing TOML" bar, since this command already clears it.
pub fn cmd_amend(
    dir: &Path,
    seed: u64,
    tool: &str,
    new_verdict_word: &str,
    new_note: Option<String>,
    reason: String,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let new_verdict = mandible_core::audit::parse_verdict_word(new_verdict_word)?;
    let entry = file
        .entries
        .iter_mut()
        .find(|e| e.tool == tool)
        .ok_or_else(|| anyhow::anyhow!("{tool:?} not found in {}", path.display()))?;
    let previous_effective = entry.effective_verdict().map(str::to_string);
    mandible_core::audit::amend(entry, new_verdict, new_note.unwrap_or_default(), reason)?;
    let amendment_count = entry.amendments.len();
    save(&path, &file)?;
    println!(
        "amended {tool}: {} -> {new_verdict} ({amendment_count} amendment(s) now recorded for \
         this entry)",
        previous_effective.as_deref().unwrap_or("(none)"),
    );
    Ok(())
}

/// Wilson score interval for a binomial proportion at (approximately) 95%
/// confidence (`z = 1.96`). Chosen over the naive
/// `p ± z*sqrt(p(1-p)/n)` normal approximation because that one produces
/// nonsensical bounds outside `[0, 1]` at exactly the small-`n`,
/// near-0-or-1 proportions a first audit run is likely to hit (e.g. `n=5`,
/// `k=5` "100% correct so far"), which is a bad first impression for the
/// one number this whole instrument exists to report honestly. Returns
/// `(lower, upper)` as fractions in `[0, 1]`; `(0.0, 1.0)` for `n == 0`,
/// since nothing has been judged and the honest statement is "no
/// information", not a point estimate.
fn wilson_interval(k: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let z = 1.96_f64;
    let n = n as f64;
    let p = k as f64 / n;
    let denom = 1.0 + z * z / n;
    let center = p + z * z / (2.0 * n);
    let adj = z * ((p * (1.0 - p) / n) + (z * z / (4.0 * n * n))).sqrt();
    (
        ((center - adj) / denom).max(0.0),
        ((center + adj) / denom).min(1.0),
    )
}

struct StratumTally {
    correct: usize,
    judged: usize,
    skipped: usize,
    pending: usize,
    /// Judged `wrong`/`incomplete` entries with [`Entry::is_display_only`]
    /// true — kept out of `judged` (and therefore out of `accuracy_over`'s
    /// denominator, spec §13.1c/task #28) but still tallied and printed,
    /// the same "recorded, not omitted" treatment `skipped` already gets.
    out_of_scope: usize,
}

/// The stratum label a report groups `entry` under.
///
/// Checked in priority order:
/// 1. [`Entry::spot_audit_event`], if present — `spot-audit:<event>`, one
///    row **per promotion event** (spec §13.1b's sixth rule). Checked
///    first because a spot-audit entry may also carry an
///    [`Entry::include_reason`] documenting the draw itself (which event,
///    how many of the promoted set existed, the seed) — that field is
///    provenance here, not the bucketing signal.
/// 2. [`FORCED_INCLUSION_STRATUM`], for any other force-included entry
///    (`include_reason.is_some()`), so it never silently blends into (and
///    skews) the random draw's own per-status numbers.
/// 3. The entry's own nominal [`Entry::stratum`] (its parse status at draw
///    time) otherwise.
fn effective_stratum(entry: &Entry) -> String {
    if let Some(event) = &entry.spot_audit_event {
        format!("spot-audit:{event}")
    } else if entry.include_reason.is_some() {
        FORCED_INCLUSION_STRATUM.to_string()
    } else {
        entry.stratum.clone()
    }
}

/// A plain `(correct, judged)` accuracy tally over whatever subset of
/// entries `keep` selects — the shared machinery behind every accuracy
/// number [`cmd_report`] prints, all-inclusive or K1/K2-filtered alike, so
/// every view is computed the same way.
///
/// Reads [`Entry::effective_verdict`], never the raw [`Entry::verdict`]
/// field directly — an amended entry's corrected verdict is what the
/// project actually believes about that tool, and every aggregate number
/// this instrument reports must reflect that correction, not the
/// superseded original sitting in the file for history's sake.
///
/// **Also skips every [`Entry::is_display_only`] entry, unconditionally,
/// regardless of which caller's filtered iterator it was handed.** The
/// maintainer's ruling (task #28) is that a display/rendering-only finding
/// "[is] not accuracy, ... probably [a] UI rendering issue. parsing was
/// fine" — it is a real, kept finding (still visible in
/// [`cmd_report`]'s stratum table and its own out-of-scope line, still a
/// `wrong`/`incomplete` verdict on disk, still a `[xfail]` fixture), just
/// never part of what this function is answering. Doing the skip in one
/// shared place, rather than in each of `cmd_report`'s five call sites,
/// is what makes "out" mean the same thing in the headline figure, every
/// K-view, and the per-stratum table all at once.
fn accuracy_over<'a>(entries: impl Iterator<Item = &'a Entry>) -> (usize, usize) {
    let mut correct = 0usize;
    let mut judged = 0usize;
    for entry in entries {
        if entry.is_display_only() {
            continue;
        }
        match entry.effective_verdict() {
            Some("correct") => {
                correct += 1;
                judged += 1;
            }
            Some("incomplete") | Some("wrong") => judged += 1,
            _ => {}
        }
    }
    (correct, judged)
}

/// Print one `label`, count, accuracy and 95% CI line, in the shared format
/// every accuracy line in this report uses — never a bare percentage.
fn print_accuracy_line(label: &str, correct: usize, judged: usize) {
    let (lo, hi) = wilson_interval(correct, judged);
    let acc = if judged == 0 {
        "  n/a".to_string()
    } else {
        format!("{:>4.1}%", correct as f64 / judged as f64 * 100.0)
    };
    println!(
        "{label:<24}  {correct:>5}/{judged:<6}  {acc}   [{:>5.1}%, {:>5.1}%]",
        lo * 100.0,
        hi * 100.0,
    );
}

/// How favorable a verdict word is to the parser, for [`print_wilson_caveat`]'s
/// amendment-direction tally: `correct` is the best outcome, `wrong` the
/// worst, `incomplete` between the two. `skip` has no comparable
/// favorability (there is nothing to judge), so it is deliberately absent —
/// an amendment into or out of `skip` is not counted as a directional move
/// either way.
fn verdict_favorability(verdict: &str) -> Option<i32> {
    match verdict {
        "correct" => Some(2),
        "incomplete" => Some(1),
        "wrong" => Some(0),
        _ => None,
    }
}

/// Print the standing caveat every accuracy figure this report produces
/// needs: a Wilson interval bounds *sampling* error — how much this
/// particular sample's accuracy could plausibly differ from a fresh draw of
/// the same size — and says nothing about *reviewer* error, since every
/// verdict in the file came from one person's read with no independent
/// cross-check. The one thing this module can say honestly about reviewer
/// error is derived from the amendment record itself, never asserted as a
/// standing fact: it tallies, from every [`Entry::amendments`] entry in
/// `file`, how many corrections moved a verdict toward a more favorable
/// outcome (`wrong`/`incomplete` -> `correct`, an original that was too
/// harsh) versus a less favorable one (`correct`/`incomplete` -> `wrong`,
/// an original that was too generous, via [`verdict_favorability`]) and
/// reports the actual balance rather than a hardcoded claim about which
/// direction reviewer error tends to run — that balance is exactly the
/// kind of thing that changes as more amendments are recorded, and a
/// caveat that stopped being true the day after it was written would be
/// worse than no caveat at all.
fn print_wilson_caveat(file: &AuditFile) {
    let mut amended_count = 0usize;
    let mut toward_more_favorable = 0usize;
    let mut toward_less_favorable = 0usize;
    for entry in &file.entries {
        if !entry.amendments.is_empty() {
            amended_count += 1;
        }
        for amendment in &entry.amendments {
            if let (Some(before), Some(after)) = (
                verdict_favorability(&amendment.previous_verdict),
                verdict_favorability(&amendment.new_verdict),
            ) {
                match after.cmp(&before) {
                    std::cmp::Ordering::Greater => toward_more_favorable += 1,
                    std::cmp::Ordering::Less => toward_less_favorable += 1,
                    std::cmp::Ordering::Equal => {}
                }
            }
        }
    }
    println!(
        "\nnote: the 95% CI above bounds sampling error only — how much this sample's accuracy \
         could plausibly vary on a fresh draw of the same size — never reviewer error. Read the \
         accuracy figure as \"accuracy of the parser as judged by this reviewer,\" not an \
         absolute truth."
    );
    if amended_count == 0 {
        println!(
            "note: no verdict in this file has been amended yet (`xtask audit amend`) — this \
             says nothing about whether the recorded verdicts are all correct, only that none \
             has been corrected so far."
        );
    } else {
        println!(
            "note: {amended_count} verdict(s) carry a recorded amendment; of the corrections \
             with a comparable direction, {toward_less_favorable} made the verdict less \
             favorable to the parser (an originally too-generous read) and \
             {toward_more_favorable} made it more favorable (an originally too-harsh read).{}",
            if toward_less_favorable > toward_more_favorable {
                " More corrections have gone the generous-to-harsh direction than the reverse \
                 so far, so this accuracy figure likely still reads a little high."
            } else if toward_more_favorable > toward_less_favorable {
                " More corrections have gone the harsh-to-generous direction than the reverse \
                 so far, so this accuracy figure likely still reads a little low."
            } else {
                " The corrections so far do not lean toward either direction."
            }
        );
    }
}

/// `xtask audit report`: per-stratum and overall accuracy, each stated as a
/// count and a confidence interval — never a bare percentage (spec's own
/// complaint about `%flags_text`/`%described`, spec §13.1b, is exactly what
/// this line format exists to avoid repeating). Also lists every tool
/// judged `wrong` or `incomplete`, since those are the next bugs to fix.
///
/// Also reports accuracy under four K1/K2 views — all-inclusive,
/// K1-excluded, K2-excluded, and both-excluded — rather than every
/// combination separately: [`Entry::k1`]/[`Entry::k2`]'s doc comments have
/// the full rationale for each known class, and reporting all four
/// pairwise combinations here would be unwieldy for the same "never a bare
/// percentage, but also never noise" discipline this module already
/// applies everywhere else.
/// The `skip` verdicts, named — the denominator's other half.
///
/// The stratum table prints a `skipped` *count* per stratum and nothing
/// else, and `accuracy_over` excludes those entries from every accuracy
/// figure in the report. A count alone makes the exclusion unauditable:
/// a reader can see that nine tools left the denominator but not which
/// ones, so "62.4% correct" cannot be checked against what was actually
/// judged. `skip` is recorded, not omitted (spec §16), and this is what
/// recording it looks like in the rendered report — every skipped tool by
/// name, with the reviewer's reason where one was given and an explicit
/// `(no reason recorded)` where none was, since `skip` is the one verdict
/// that does not require a note and inventing one here would be
/// fabricating the very justification a reader came to check.
///
/// Returns whole lines (the section header included) rather than printing,
/// so the content is testable without capturing stdout.
fn skipped_lines(file: &AuditFile) -> Vec<String> {
    let mut skipped: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.effective_verdict() == Some("skip"))
        .collect();
    if skipped.is_empty() {
        return Vec::new();
    }
    skipped.sort_by(|a, b| a.tool.cmp(&b.tool));
    let mut lines = vec![
        String::new(),
        format!(
            "tools skipped ({} — recorded, never omitted; excluded from every accuracy figure \
         above, so this is the list that makes that exclusion checkable):",
            skipped.len()
        ),
    ];
    for entry in skipped {
        let reason = if entry.effective_note().trim().is_empty() {
            "(no reason recorded)"
        } else {
            entry.effective_note()
        };
        lines.push(format!("  {:<24} {}", entry.tool, reason));
    }
    lines
}

pub fn cmd_report(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;

    let mut by_stratum: BTreeMap<String, StratumTally> = BTreeMap::new();
    for entry in &file.entries {
        let tally = by_stratum
            .entry(effective_stratum(entry))
            .or_insert(StratumTally {
                correct: 0,
                judged: 0,
                skipped: 0,
                pending: 0,
                out_of_scope: 0,
            });
        match entry.effective_verdict() {
            None => tally.pending += 1,
            Some("skip") => tally.skipped += 1,
            Some("correct") => {
                tally.correct += 1;
                tally.judged += 1;
            }
            // A display-only finding is judged (`wrong`/`incomplete`, never
            // `skip` — see `Entry::is_display_only`'s doc comment on why
            // `skip` is the wrong tool for this), so it must not fall into
            // the catch-all `judged` arm below: that is precisely the
            // count `accuracy_over` also excludes it from. Checked before
            // the catch-all, not after, so it can never double-count.
            Some(_) if entry.is_display_only() => tally.out_of_scope += 1,
            Some(_) => tally.judged += 1,
        }
    }

    println!(
        "audit seed={seed} sample_size={} ({} entries total)",
        file.meta.sample_size,
        file.entries.len()
    );
    println!();
    println!(
        "stratum             correct/judged   accuracy   95% CI            skipped   pending   \
         out-of-scope"
    );
    let mut overall_correct = 0usize;
    let mut overall_judged = 0usize;
    let mut overall_skipped = 0usize;
    let mut overall_pending = 0usize;
    let mut overall_out_of_scope = 0usize;
    for (stratum, t) in &by_stratum {
        let (lo, hi) = wilson_interval(t.correct, t.judged);
        let acc = if t.judged == 0 {
            "  n/a".to_string()
        } else {
            format!("{:>4.1}%", t.correct as f64 / t.judged as f64 * 100.0)
        };
        println!(
            "{stratum:<18}  {:>5}/{:<6}  {acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}   {:>12}",
            t.correct,
            t.judged,
            lo * 100.0,
            hi * 100.0,
            t.skipped,
            t.pending,
            t.out_of_scope,
        );
        overall_correct += t.correct;
        overall_judged += t.judged;
        overall_skipped += t.skipped;
        overall_pending += t.pending;
        overall_out_of_scope += t.out_of_scope;
    }
    let (lo, hi) = wilson_interval(overall_correct, overall_judged);
    let overall_acc = if overall_judged == 0 {
        "  n/a".to_string()
    } else {
        format!(
            "{:>4.1}%",
            overall_correct as f64 / overall_judged as f64 * 100.0
        )
    };
    println!(
        "{:<18}  {:>5}/{:<6}  {overall_acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}   {:>12}",
        "OVERALL",
        overall_correct,
        overall_judged,
        lo * 100.0,
        hi * 100.0,
        overall_skipped,
        overall_pending,
        overall_out_of_scope,
    );
    if overall_judged > 0 && overall_judged < 30 {
        println!(
            "\nnote: n={overall_judged} judged so far — the interval above is wide at this size; \
             keep reviewing for a number worth acting on (spec's own target is ~60-100)."
        );
    }
    if overall_out_of_scope > 0 {
        let mut names: Vec<&str> = file
            .entries
            .iter()
            .filter(|e| e.is_display_only())
            .map(|e| e.tool.as_str())
            .collect();
        names.sort_unstable();
        println!(
            "\nnote: {overall_out_of_scope} finding(s) are display-only and are excluded from \
             every accuracy figure above, not dropped — the maintainer's ruling (task #28) is \
             that a display/rendering defect is a real finding but not an accuracy one: {}. See \
             the 'display-only findings (kept, out of scope)' section below for each one's note \
             in full.",
            names.join(", "),
        );
    }
    print_wilson_caveat(&file);

    let k1_tagged = file.entries.iter().filter(|e| e.k1 == Some(true)).count();
    let k2_tagged = file.entries.iter().filter(|e| e.k2 == Some(true)).count();
    let k3_tagged = file.entries.iter().filter(|e| e.k3 == Some(true)).count();
    println!(
        "\nK1/K2/K3 sensitivity ({k1_tagged} entr{k1_s} tagged K1, {k2_tagged} entr{k2_s} \
         tagged K2, {k3_tagged} entr{k3_s} tagged K3 — see mandible_core::audit's \
         Entry::k1/k2/k3 doc comments and this module's *_signature functions):",
        k1_s = if k1_tagged == 1 { "y" } else { "ies" },
        k2_s = if k2_tagged == 1 { "y" } else { "ies" },
        k3_s = if k3_tagged == 1 { "y" } else { "ies" },
    );
    println!("view                      correct/judged   accuracy   95% CI");
    let (c, j) = accuracy_over(file.entries.iter());
    print_accuracy_line("all-inclusive", c, j);
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k1 != Some(true)));
    print_accuracy_line("K1-excluded", c, j);
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k2 != Some(true)));
    print_accuracy_line("K2-excluded", c, j);
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k3 != Some(true)));
    print_accuracy_line("K3-excluded", c, j);
    let (c, j) = accuracy_over(
        file.entries
            .iter()
            .filter(|e| e.k1 != Some(true) && e.k2 != Some(true) && e.k3 != Some(true)),
    );
    print_accuracy_line("K1+K2+K3-excluded", c, j);

    let mut flagged: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| matches!(e.effective_verdict(), Some("wrong") | Some("incomplete")))
        .collect();
    flagged.sort_by(|a, b| a.tool.cmp(&b.tool));
    if !flagged.is_empty() {
        println!("\ntools judged wrong or incomplete (the next bugs):");
        for entry in flagged {
            let amended_tag = if entry.amendments.is_empty() {
                ""
            } else {
                " [amended]"
            };
            // Stays in this list — it is still a `wrong`/`incomplete`
            // verdict on disk and a real finding — but tagged so a reader
            // scanning "the next bugs" does not mistake a rendering fix
            // for a parser fix. `accuracy_over` has already excluded it
            // from every count printed above; this tag is why the two
            // views (this list and the headline) don't silently disagree
            // about which tools are counted where.
            let scope_tag = if entry.is_display_only() {
                " [display-only, excluded from accuracy — see below]"
            } else {
                ""
            };
            println!(
                "  {:<24} {:<11} {}{amended_tag}{scope_tag}",
                entry.tool,
                entry.effective_verdict().unwrap_or(""),
                entry.effective_note(),
            );
        }
    }

    for line in skipped_lines(&file) {
        println!("{line}");
    }

    let mut out_of_scope: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.is_display_only())
        .collect();
    out_of_scope.sort_by(|a, b| a.tool.cmp(&b.tool));
    if !out_of_scope.is_empty() {
        println!(
            "\ndisplay-only findings (kept, out of scope — real UI bugs, excluded from accuracy \
             per the maintainer's task #28 ruling; family meaning: {}):",
            family_meaning("display-only").unwrap_or("?"),
        );
        for entry in out_of_scope {
            println!(
                "  {:<24} {:<11} {}",
                entry.tool,
                entry.effective_verdict().unwrap_or(""),
                entry.effective_note(),
            );
        }
    }
    Ok(())
}

/// `xtask audit fixtures`: turn every reviewed (non-`skip`) entry into a
/// `corpus/README.md`-shaped fixture directory under `corpus_dir` — capture
/// files, a pre-filled `meta.toml`, and (for `correct`) an `expected.snap`.
///
/// **Stages by default, does not write into the gated `corpus/` tree.**
/// `corpus_dir` defaults to `<dir>/<seed>/fixtures`, not `corpus/`. This is
/// not a convenience default, it is load-bearing: a `wrong`/`incomplete`
/// verdict becomes a `[xfail]` block, and `corpus/README.md`'s own
/// lifecycle rule — confirmed empirically against this exact runner while
/// building this command — is that `xtask corpus` treats an `[xfail]`
/// fixture with **no currently-failing `[contract]` field** as "the bug
/// appears fixed" and fails the run (`SnapshotCheck::Missing` is legal only
/// while `[xfail]`, so with no contract to fail either, every check
/// vacuously passes). What check *should* fail is exactly the kind of
/// tool-specific judgment (which flags are missing, what a description got
/// mixed up with) that only a human reviewer can supply — the same
/// judgment this whole audit exists to capture and that automating away
/// here would mean fabricating. So this command writes a real, honest
/// `[xfail]` with the reviewer's note as `reason`, plus the one contract
/// field it *can* derive without guessing (`expected_framework`, which is
/// simply what Tier A′ detected, not a claim about correctness) and a
/// prominent comment naming the gap. Staging keeps that gap from silently
/// breaking `cargo run -p xtask -- corpus` for anyone who runs this
/// command and then adds the tool count without reading the output —
/// promoting a staged fixture into `corpus/` is a small, deliberate act,
/// same spirit as `--bless` itself.
///
/// A `correct` verdict needs none of that: `corpus/README.md` says a
/// `correct` verdict *is* a human assertion of correctness, in those
/// words, so those fixtures get a real `expected.snap` and may ship green
/// immediately, wherever `corpus_dir` points.
pub fn cmd_fixtures(
    dir: &Path,
    seed: u64,
    corpus_dir: &Path,
    only: Option<Vec<String>>,
    force: bool,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;
    let mut written = 0usize;
    let mut skipped_no_capture = 0usize;
    let mut skipped_verdict = 0usize;
    let mut skipped_exists = 0usize;

    for entry in &file.entries {
        // Reads the effective (post-amendment) verdict and note: a fixture
        // generated from an entry that was amended after review must
        // reflect the corrected truth, not the superseded original — the
        // whole point of an amendment is that the project's belief about
        // the tool changed, and a fixture is exactly the kind of durable
        // artifact that would otherwise silently encode the old, wrong
        // belief forever.
        let Some(verdict) = entry.effective_verdict() else {
            continue;
        };
        let note = entry.effective_note();
        if verdict == "skip" {
            skipped_verdict += 1;
            continue;
        }
        if let Some(only) = &only {
            if !only.iter().any(|t| t == &entry.tool) {
                continue;
            }
        }

        let fixture_dir = corpus_dir
            .join(&entry.tool)
            .join(format!("audit-seed{seed}"));
        if fixture_dir.exists() && !force {
            println!(
                "{}: {} already exists — pass --force to overwrite (review the existing \
                 fixture for machine-specific content first, corpus/README.md step 3)",
                entry.tool,
                fixture_dir.display()
            );
            skipped_exists += 1;
            continue;
        }

        let classified = classify_one(&entry.tool);
        let Some((argv_tail, output)) = classified.raw_capture else {
            println!(
                "{}: no raw capture available, skipping fixture emission",
                entry.tool
            );
            skipped_no_capture += 1;
            continue;
        };

        std::fs::create_dir_all(&fixture_dir)
            .map_err(|e| anyhow::anyhow!("creating {}: {e}", fixture_dir.display()))?;
        std::fs::write(fixture_dir.join("help.txt"), &output.stdout)?;
        if !output.stderr.is_empty() {
            std::fs::write(fixture_dir.join("help.stderr.txt"), &output.stderr)?;
        }

        let mut argv = vec![entry.tool.clone()];
        argv.extend(argv_tail);
        let framework = classified
            .result
            .root
            .as_ref()
            .and_then(|r| r.detected_framework.clone())
            .unwrap_or_else(|| "generic".to_string());

        let mut meta = String::new();
        meta.push_str("# Generated by `xtask audit fixtures` (corpus/README.md's workflow) —\n");
        meta.push_str(&format!(
            "# reviewed under seed {seed}, verdict {verdict:?}. See that file's own\n"
        ));
        meta.push_str(
            "# review-any-fixture-for-machine-specific-content note before committing.\n\n",
        );
        // An agent generated this fixture, so `[bless] provenance` starts at
        // the conservative default. Only a human may change this value to
        // "human" or "agent-then-human" (corpus/README.md's `[bless]`
        // section, the mirror of the rule that an agent must never claim
        // `verdict_scope`) — never widen it mechanically here.
        meta.push_str("# An agent generated this fixture; only a human may change the value\n");
        meta.push_str("# below (corpus/README.md's `[bless]` section).\n");
        meta.push_str("[bless]\n");
        meta.push_str("provenance = \"agent\"\n\n");
        meta.push_str("[tool]\n");
        meta.push_str(&format!("name = {:?}\n", entry.tool));
        meta.push_str(&format!("version = \"audit-seed{seed}\"\n"));
        meta.push_str("captured_with = \"xtask audit\"\n\n");
        meta.push_str("[[capture]]\n");
        meta.push_str(&format!("argv = {argv:?}\n"));
        meta.push_str("stdout = \"help.txt\"\n");
        if !output.stderr.is_empty() {
            meta.push_str("stderr = \"help.stderr.txt\"\n");
        }
        if let Some(code) = output.exit_code {
            if code != 0 {
                meta.push_str(&format!("exit_code = {code}\n"));
            }
        }
        meta.push('\n');

        match verdict {
            "correct" => {
                meta.push_str("[contract]\n");
                meta.push_str(&format!("expected_framework = {framework:?}\n"));
                if let Some(root) = classified.result.root.as_ref() {
                    let status = status::compute(&classified.result);
                    meta.push_str(&format!("min_status = {:?}\n", status.label));
                    meta.push_str(&format!("min_subcommands = {}\n", root.subcommands.len()));
                    let flags = sample_flag_specs(root);
                    if !flags.is_empty() {
                        meta.push_str(&format!("must_contain_flags = {flags:?}\n"));
                    }
                    meta.push('\n');
                    let rendered = render_snapshot(Some(root));
                    std::fs::write(fixture_dir.join("expected.snap"), rendered)?;
                }
            }
            "incomplete" | "wrong" => {
                meta.push_str("[contract]\n");
                meta.push_str(&format!("expected_framework = {framework:?}\n"));
                meta.push_str(
                    "# TODO(human): add at least one field above (min_status/min_subcommands/\n\
                     # must_contain_flags/must_contain_flags_by_path) that captures the specific\n\
                     # defect the reviewer's note describes and currently FAILS against the raw\n\
                     # capture above — xtask can't derive this without guessing at what the tool\n\
                     # should have said, which is exactly the judgment this audit exists to add.\n\
                     # Until then `cargo run -p xtask -- corpus` reports this fixture as \"the bug\n\
                     # appears fixed\" (nothing here is currently falsifiable) if it's moved into\n\
                     # a gated corpus directory — see corpus/README.md's xfail lifecycle rules.\n\n",
                );
                meta.push_str("[xfail]\n");
                meta.push_str("broken = true\n");
                let reason = if note.is_empty() {
                    format!(
                        "reviewer marked this {verdict} under xtask audit (seed {seed}); \
                         no note was recorded"
                    )
                } else {
                    note.to_string()
                };
                meta.push_str(&format!("reason = {reason:?}\n"));
            }
            _ => {}
        }

        std::fs::write(fixture_dir.join("meta.toml"), meta)?;
        println!("wrote {} ({verdict})", fixture_dir.display());
        written += 1;
    }

    println!(
        "\n{written} fixture(s) written to {}; {skipped_verdict} skip-verdict, \
         {skipped_exists} already existed, {skipped_no_capture} had no capture",
        corpus_dir.display()
    );
    if corpus_dir != Path::new("corpus") {
        println!(
            "staged, not gated — review, add any needed [contract] fields to the \
             incomplete/wrong ones, then move what's ready into corpus/ and run \
             `cargo run -p xtask -- corpus`."
        );
    }
    Ok(())
}

/// A small, generically-derived `must_contain_flags` sample for a
/// `correct`-verdict fixture's `[contract]` — the root's first few
/// canonical spellings, long preferred over short (matching
/// `corpus/README.md`'s own git example). Capped rather than exhaustive:
/// the point is a coarse regression spot-check a reviewer can extend, not a
/// duplicate of `expected.snap`.
const SAMPLE_FLAG_CAP: usize = 5;

fn sample_flag_specs(root: &CommandNode) -> Vec<String> {
    root.flags()
        .filter_map(|f| {
            f.long()
                .map(|l| format!("--{l}"))
                .or_else(|| f.short().map(|s| format!("-{s}")))
        })
        .take(SAMPLE_FLAG_CAP)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::audit::AuditMeta;
    use mandible_core::{Provenance, Source};
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
    /// `print_wilson_caveat`'s zero-amendment branch, exercised through the
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
    /// `print_wilson_caveat`'s non-zero branch end to end.
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
