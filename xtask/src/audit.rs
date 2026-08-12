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
//! - [`cmd_sample`] draws a **deterministic, stratified** sample (by parse
//!   status — `ok`/`low-confidence`/`verbatim`/`no-tier`, plus whatever
//!   other status [`crate::status::compute`] actually produces for the
//!   population, e.g. `suspicious` — never a fixed four-way bucket forced
//!   onto the real data) and persists it to a resumable verdict file.
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
//! or name when deciding who gets sampled — see [`sample_stratified`],
//! which only ever sees `(tool, stratum)` pairs and a seeded shuffle.

use crate::coverage::unique_executables_on_path;
use crate::existence::{self, FabricationKind};
use crate::misattribution::RecordingProbe;
use crate::status;
use mandible_core::audit::{
    extract_tag_override, load, parse_verdict_word, save, tag_display, verdict_path, AuditFile,
    AuditMeta, Entry,
};
use mandible_core::{CommandNode, Flag};
use mandible_extract::exec::ExecOutput;
use mandible_extract::{default_tiers_with_probe, ExtractionResult, Runner};
use rayon::prelude::*;
use std::collections::BTreeMap;
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

// ---------------------------------------------------------------------
// A minimal, dependency-free deterministic PRNG.
//
// The workspace carries no `rand` dependency, and this task doesn't need
// cryptographic quality — only that the same seed always produces the same
// draw and different seeds produce (with overwhelming probability)
// different draws. SplitMix64 is the standard, well-analyzed choice for
// exactly that: one multiply-xor-shift step per call, no external state
// beyond a single u64.
// ---------------------------------------------------------------------

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `0..n`. Not perfectly unbiased (the classic
    /// modulo-reduction skew), which is irrelevant here: `n` is a tool
    /// count in the thousands at most, `u64::MAX / n` is astronomically
    /// larger, and the property this whole module needs is
    /// reproducibility, not cryptographic uniformity.
    fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }
}

/// Deterministic Fisher-Yates shuffle, seeded — the only source of
/// randomness [`sample_stratified`] uses. Same `seed` and `items`, in the
/// same starting order, always produces the same permutation.
fn seeded_shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64::new(seed);
    for i in (1..items.len()).rev() {
        let j = rng.below(i + 1);
        items.swap(i, j);
    }
}

/// Derive a per-stratum seed from the run's `--seed` and the stratum's own
/// name, via a small FNV-1a mix. Without this, shuffling every stratum with
/// the *same* raw seed would make the strata's internal orders correlated
/// (the same relative shuffle pattern applied to each), which is a subtler
/// but real form of non-independence in the draw.
fn stratum_seed(seed: u64, stratum: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in stratum.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0001_0000_01B3);
    }
    h
}

/// One tool's classification: its drawn/measured stratum, the extracted
/// tree, and (when available) the raw captured text and the exact capture
/// needed to write a corpus fixture — all obtained from **one** extraction
/// pass, via [`RecordingProbe`], never a second probe of the tool (same "no
/// new probes" property [`crate::misattribution`] documents).
struct Classified {
    stratum: &'static str,
    result: ExtractionResult,
    raw_text: Option<String>,
    raw_capture: Option<(Vec<String>, ExecOutput)>,
}

fn classify_one(tool: &str) -> Classified {
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

/// Classify every tool in `tools` in parallel (each is an independent
/// subprocess round-trip, same reasoning as `coverage::run_over`'s own
/// `par_iter`).
fn classify_all(tools: &[String]) -> Vec<(String, Classified)> {
    tools
        .par_iter()
        .map(|t| (t.clone(), classify_one(t)))
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
fn is_k1_flag(flag: &Flag) -> bool {
    flag.short.is_some() && flag.long.is_none() && flag.value_name.is_some()
}

/// `(matching, total)` flag counts across `node` and every descendant, for
/// the K1 pre-tag's display line (e.g. "839/1454 flags match").
fn k1_signature_stats(node: &CommandNode) -> (usize, usize) {
    let mut matching = node.flags.iter().filter(|f| is_k1_flag(f)).count();
    let mut total = node.flags.len();
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
/// own known multi-column/comma-separated tokenization gap (K2, see
/// `xtask/src/existence.rs`'s `line_start_words` doc comment) rather than
/// genuine parser fabrication: a fabrication is "attributable" when its
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
    node.flags.is_empty() && node.subcommands.is_empty() && node.summary.is_none()
}

/// True for a bare stub ([`is_bare_stub`]) that is *also* not
/// [`CommandNode::heading_attested`] — its name came from a native/cobra
/// artifact (e.g. a `__complete` candidate) rather than a recognized
/// `--help` heading. This is provable from the single extraction pass this
/// pre-tag is computed from: `help_text::raw_help` refuses to probe any
/// node whose `heading_attested` bit is false (`mandible-extract/src/
/// help_text/mod.rs`), so unlike an ordinary un-recursed subcommand — merely
/// not fetched *yet* — this one structurally cannot ever be, live
/// navigation included. `git-lfs`'s tree is the motivating case: 36 nodes,
/// 34 of them exactly this shape, which is also why its
/// `status::compute` label is `suspicious`.
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
    node.flags.len() + node.subcommands.iter().map(total_flags).sum::<usize>()
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
/// [`sample_stratified`] (the ordinary draw) and [`cmd_sample`]'s
/// force-include path, so the two can never compute a K1/K2/K3 suggestion
/// differently.
fn entry_from_classified(
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
        amendments: Vec::new(),
    }
}

/// A drawn sample's per-stratum accounting, for [`cmd_sample`]'s printed
/// proof that the draw is proportionally stratified: `(drawn, population)`.
type StratumCounts = BTreeMap<String, (usize, usize)>;

/// Draw a **proportionally stratified** sample of size `sample_size` from
/// `classified`: each stratum's share of the sample matches its share of
/// the population (largest-remainder rounding to land on the requested
/// total exactly), and within a stratum the specific tools are chosen by a
/// seeded, deterministic shuffle (see [`seeded_shuffle`]).
///
/// Proportional, not equal-quota per stratum: the audit's whole purpose is
/// to find out whether `ok` means anything, which requires the sample to
/// reflect how the real population actually splits across statuses, not a
/// fixed quota that would either starve a tiny stratum or force-inflate it
/// relative to its real share.
fn sample_stratified(
    classified: &[(String, Classified)],
    sample_size: usize,
    seed: u64,
) -> (Vec<Entry>, StratumCounts) {
    let total = classified.len();
    let by_tool: std::collections::HashMap<&str, &Classified> =
        classified.iter().map(|(t, c)| (t.as_str(), c)).collect();
    let mut by_stratum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (tool, c) in classified {
        by_stratum
            .entry(c.stratum.to_string())
            .or_default()
            .push(tool.clone());
    }

    // Largest-remainder allocation: base quota is the floor of the exact
    // proportional share, then the leftover slots (sample_size minus the
    // sum of floors) go to the strata with the largest fractional
    // remainder, ties broken by stratum name for determinism.
    let mut quotas: BTreeMap<String, usize> = BTreeMap::new();
    let mut remainders: Vec<(String, f64)> = Vec::new();
    let mut allocated = 0usize;
    for (stratum, tools) in &by_stratum {
        let exact = if total == 0 {
            0.0
        } else {
            sample_size as f64 * tools.len() as f64 / total as f64
        };
        let base = (exact.floor() as usize).min(tools.len());
        quotas.insert(stratum.clone(), base);
        allocated += base;
        remainders.push((stratum.clone(), exact - base as f64));
    }
    remainders.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut leftover = sample_size
        .saturating_sub(allocated)
        .min(total.saturating_sub(allocated));
    for (stratum, _) in &remainders {
        if leftover == 0 {
            break;
        }
        let cap = by_stratum[stratum].len();
        let q = quotas.get_mut(stratum).expect("stratum present");
        if *q < cap {
            *q += 1;
            leftover -= 1;
        }
    }

    let mut entries = Vec::new();
    let mut counts: StratumCounts = BTreeMap::new();
    for (stratum, mut tools) in by_stratum {
        let population = tools.len();
        seeded_shuffle(&mut tools, stratum_seed(seed, &stratum));
        let quota = quotas.get(&stratum).copied().unwrap_or(0).min(tools.len());
        counts.insert(stratum.clone(), (quota, population));
        for tool in tools.into_iter().take(quota) {
            let c = by_tool
                .get(tool.as_str())
                .expect("every drawn tool came from `classified`");
            entries.push(entry_from_classified(tool, c, None));
        }
    }
    entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    (entries, counts)
}

/// Read a force-include file: `<tool> <reason...>` per line (`#` comments
/// and blank lines ignored — the same convention [`cmd_ingest`]'s verdicts
/// file uses), for [`cmd_sample`]'s `force_include` parameter. A reason is
/// required, not optional: an unconditional inclusion with no stated reason
/// is exactly the kind of unauditable claim spec.md Appendix A exists to
/// rule out (see `Entry::include_reason`'s doc comment).
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

/// `xtask audit sample`: (re)compute the deterministic, stratified draw and
/// merge it into `path`, never disturbing an entry already present (so a
/// resumed or repeated `sample` invocation is a no-op on top of prior
/// progress — see this module's doc comment). `force_include` entries
/// (`(tool, reason)`, see [`load_force_include`]) are merged in
/// *unconditionally*, in addition to the stratified draw and independent of
/// `sample_size` — the draw's quota accounting in `counts` never counts
/// them, so a force-included tool never displaces a randomly-drawn one.
pub fn cmd_sample(
    seed: u64,
    sample_size: usize,
    tools: Option<Vec<String>>,
    dir: &Path,
    force_include: &[(String, String)],
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let population = tools.unwrap_or_else(unique_executables_on_path);
    if population.is_empty() {
        anyhow::bail!("no tools found to sample from (empty PATH population and no --tools given)");
    }
    println!(
        "classifying {} tool(s) to stratify by parse status...",
        population.len()
    );
    let classified = classify_all(&population);
    let (drawn, counts) = sample_stratified(&classified, sample_size, seed);

    // Force-included tools are classified independently of `population`:
    // the whole point (spec-cited case: the 14 `find_description_gap`
    // promotions) is that they must appear in the sample regardless of
    // whether a `--tools` shortcut or a stale `PATH` happens to include
    // them (see this task's own doc comment: "`--tools` shortcuts will not
    // find them").
    let mut forced_entries = Vec::new();
    for (tool, reason) in force_include {
        let already_classified = classified.iter().find(|(t, _)| t == tool);
        let c;
        let classified_ref = match already_classified {
            Some((_, existing)) => existing,
            None => {
                c = classify_one(tool);
                &c
            }
        };
        forced_entries.push(entry_from_classified(
            tool.clone(),
            classified_ref,
            Some(reason.clone()),
        ));
    }

    let mut file = if path.is_file() {
        let existing = load(&path)?;
        if existing.meta.seed != seed || existing.meta.sample_size != sample_size {
            anyhow::bail!(
                "{} already exists with seed={} sample_size={} (asked for seed={seed} \
                 sample_size={sample_size}) — use a different --dir/--seed, or delete it \
                 if this is a deliberate re-draw",
                path.display(),
                existing.meta.seed,
                existing.meta.sample_size,
            );
        }
        existing
    } else {
        AuditFile {
            meta: AuditMeta { seed, sample_size },
            entries: Vec::new(),
        }
    };

    let existing_tools: std::collections::HashSet<String> =
        file.entries.iter().map(|e| e.tool.clone()).collect();
    let mut added = 0usize;
    for entry in drawn.into_iter().chain(forced_entries) {
        if !existing_tools.contains(&entry.tool) {
            file.entries.push(entry);
            added += 1;
        }
    }
    file.entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    save(&path, &file)?;

    println!(
        "seed={seed} sample_size={sample_size} population={}",
        population.len()
    );
    println!("stratum            drawn   population   %pop   %sample");
    for (stratum, (n_drawn, n_pop)) in &counts {
        println!(
            "{stratum:<18}  {n_drawn:>4}  {n_pop:>10}  {:>5.1}%  {:>6.1}%",
            *n_pop as f64 / population.len() as f64 * 100.0,
            if sample_size == 0 {
                0.0
            } else {
                *n_drawn as f64 / sample_size as f64 * 100.0
            },
        );
    }
    println!(
        "{added} new pending entr{s} written to {} ({} pending total, {} force-included)",
        path.display(),
        file.pending().count(),
        force_include.len(),
        s = if added == 1 { "y" } else { "ies" },
    );
    Ok(())
}

fn render_snapshot(node: Option<&CommandNode>) -> String {
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
/// resumability property [`cmd_sample`]/[`cmd_review`] give the rest of
/// this workflow.
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
}

/// The stratum label a report groups `entry` under: [`FORCED_INCLUSION_STRATUM`]
/// for a force-included entry regardless of its nominal [`Entry::stratum`],
/// so it never silently blends into (and skews) the random draw's own
/// per-status numbers — see [`Entry::include_reason`]'s doc comment.
fn effective_stratum(entry: &Entry) -> &str {
    if entry.include_reason.is_some() {
        FORCED_INCLUSION_STRATUM
    } else {
        entry.stratum.as_str()
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
fn accuracy_over<'a>(entries: impl Iterator<Item = &'a Entry>) -> (usize, usize) {
    let mut correct = 0usize;
    let mut judged = 0usize;
    for entry in entries {
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
pub fn cmd_report(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;

    let mut by_stratum: BTreeMap<String, StratumTally> = BTreeMap::new();
    for entry in &file.entries {
        let tally = by_stratum
            .entry(effective_stratum(entry).to_string())
            .or_insert(StratumTally {
                correct: 0,
                judged: 0,
                skipped: 0,
                pending: 0,
            });
        match entry.effective_verdict() {
            None => tally.pending += 1,
            Some("skip") => tally.skipped += 1,
            Some("correct") => {
                tally.correct += 1;
                tally.judged += 1;
            }
            Some(_) => tally.judged += 1,
        }
    }

    println!(
        "audit seed={seed} sample_size={} ({} entries total)",
        file.meta.sample_size,
        file.entries.len()
    );
    println!();
    println!("stratum             correct/judged   accuracy   95% CI            skipped   pending");
    let mut overall_correct = 0usize;
    let mut overall_judged = 0usize;
    let mut overall_skipped = 0usize;
    let mut overall_pending = 0usize;
    for (stratum, t) in &by_stratum {
        let (lo, hi) = wilson_interval(t.correct, t.judged);
        let acc = if t.judged == 0 {
            "  n/a".to_string()
        } else {
            format!("{:>4.1}%", t.correct as f64 / t.judged as f64 * 100.0)
        };
        println!(
            "{stratum:<18}  {:>5}/{:<6}  {acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}",
            t.correct,
            t.judged,
            lo * 100.0,
            hi * 100.0,
            t.skipped,
            t.pending,
        );
        overall_correct += t.correct;
        overall_judged += t.judged;
        overall_skipped += t.skipped;
        overall_pending += t.pending;
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
        "{:<18}  {:>5}/{:<6}  {overall_acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}",
        "OVERALL",
        overall_correct,
        overall_judged,
        lo * 100.0,
        hi * 100.0,
        overall_skipped,
        overall_pending,
    );
    if overall_judged > 0 && overall_judged < 30 {
        println!(
            "\nnote: n={overall_judged} judged so far — the interval above is wide at this size; \
             keep reviewing for a number worth acting on (spec's own target is ~60-100)."
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
            println!(
                "  {:<24} {:<11} {}{amended_tag}",
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
    root.flags
        .iter()
        .filter_map(|f| {
            f.long
                .as_deref()
                .map(|l| format!("--{l}"))
                .or_else(|| f.short.map(|s| format!("-{s}")))
        })
        .take(SAMPLE_FLAG_CAP)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};
    use std::io::Cursor;
    use std::path::PathBuf;

    fn synthetic_classified(specs: &[(&str, &str)]) -> Vec<(String, Classified)> {
        // Builds fake (tool, stratum) pairs without touching a real
        // extraction pipeline, so `sample_stratified`'s allocation math can
        // be tested in isolation from anything that spawns a process.
        specs
            .iter()
            .map(|(tool, stratum)| {
                let stratum_static: &'static str = match *stratum {
                    "ok" => "ok",
                    "low-confidence" => "low-confidence",
                    "verbatim" => "verbatim",
                    "no-tier" => "no-tier",
                    "suspicious" => "suspicious",
                    other => panic!("unexpected test stratum {other}"),
                };
                (
                    tool.to_string(),
                    Classified {
                        stratum: stratum_static,
                        result: ExtractionResult {
                            tool: tool.to_string(),
                            root: None,
                            tier_statuses: Vec::new(),
                            elapsed: std::time::Duration::ZERO,
                        },
                        raw_text: None,
                        raw_capture: None,
                    },
                )
            })
            .collect()
    }

    fn population_80_20() -> Vec<(String, Classified)> {
        // 80 "ok", 20 "low-confidence" — an easy-to-check 4:1 split.
        let mut specs: Vec<(String, &str)> = Vec::new();
        for i in 0..80 {
            specs.push((format!("ok{i}"), "ok"));
        }
        for i in 0..20 {
            specs.push((format!("lc{i}"), "low-confidence"));
        }
        let borrowed: Vec<(&str, &str)> = specs.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        synthetic_classified(&borrowed)
    }

    #[test]
    fn same_seed_draws_the_same_sample_twice() {
        let population = population_80_20();
        let (a, _) = sample_stratified(&population, 10, 42);
        let (b, _) = sample_stratified(&population, 10, 42);
        let names_a: Vec<&str> = a.iter().map(|e| e.tool.as_str()).collect();
        let names_b: Vec<&str> = b.iter().map(|e| e.tool.as_str()).collect();
        assert_eq!(names_a, names_b, "identical seed must draw identical tools");
    }

    #[test]
    fn different_seed_draws_a_different_sample() {
        let population = population_80_20();
        let (a, _) = sample_stratified(&population, 10, 1);
        let (b, _) = sample_stratified(&population, 10, 2);
        let names_a: std::collections::BTreeSet<&str> = a.iter().map(|e| e.tool.as_str()).collect();
        let names_b: std::collections::BTreeSet<&str> = b.iter().map(|e| e.tool.as_str()).collect();
        assert_ne!(
            names_a, names_b,
            "different seeds should (overwhelmingly) draw different sets"
        );
    }

    #[test]
    fn sample_is_proportionally_stratified() {
        let population = population_80_20();
        // 100 population, 4:1 split; a sample of 20 should draw ~16 ok / ~4
        // low-confidence (exact, since 20 * 0.8 = 16 and 20 * 0.2 = 4 land
        // on whole numbers with no rounding ambiguity).
        let (entries, counts) = sample_stratified(&population, 20, 7);
        assert_eq!(entries.len(), 20);
        let (ok_drawn, ok_pop) = counts["ok"];
        let (lc_drawn, lc_pop) = counts["low-confidence"];
        assert_eq!(ok_pop, 80);
        assert_eq!(lc_pop, 20);
        assert_eq!(
            ok_drawn, 16,
            "80% of the population should be ~80% of the sample"
        );
        assert_eq!(
            lc_drawn, 4,
            "20% of the population should be ~20% of the sample"
        );
    }

    #[test]
    fn sample_never_exceeds_a_strata_population() {
        // A stratum with only 2 tools can never contribute more than 2,
        // even if proportional rounding would otherwise ask for more.
        let population =
            synthetic_classified(&[("a", "ok"), ("b", "ok"), ("c", "no-tier"), ("d", "no-tier")]);
        let (entries, counts) = sample_stratified(&population, 4, 99);
        assert_eq!(
            entries.len(),
            4,
            "cannot draw more than the total population"
        );
        for (_, (drawn, pop)) in counts {
            assert!(drawn <= pop);
        }
    }

    #[test]
    fn sample_total_never_exceeds_requested_size_or_population() {
        let population = population_80_20();
        let (entries, _) = sample_stratified(&population, 1000, 5);
        assert_eq!(
            entries.len(),
            100,
            "requesting more than the population caps at the population"
        );
    }

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
    fn sample_merge_is_idempotent_and_never_touches_recorded_verdicts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_sample_file(tmp.path(), 55, &[("sh", "ok")]);
        {
            let mut f = load(&path).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            f.entries[0].note = "already reviewed".to_string();
            save(&path, &f).unwrap();
        }
        // Re-running sample with the same population/seed/size must not
        // disturb the already-recorded verdict.
        cmd_sample(55, 1, Some(vec!["sh".to_string()]), tmp.path(), &[]).unwrap();
        let after = load(&path).unwrap();
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].verdict.as_deref(), Some("correct"));
        assert_eq!(after.entries[0].note, "already reviewed");
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

    fn k1_flag() -> Flag {
        let mut f = Flag::long("", Provenance::single(Source::HelpText));
        f.short = Some('f');
        f.long = None;
        f.value_name = Some("dump-scos".to_string());
        f
    }

    fn ordinary_flag(short: char, long: &str) -> Flag {
        let mut f = Flag::long(long, Provenance::single(Source::HelpText));
        f.short = Some(short);
        f
    }

    #[test]
    fn k1_signature_flags_the_gcc_single_dash_long_shape() {
        let mut root = CommandNode::new("clang", Provenance::single(Source::HelpText));
        root.flags.push(k1_flag());
        root.flags.push(ordinary_flag('v', "verbose"));
        assert_eq!(k1_signature(&root), Some(true));
    }

    #[test]
    fn k1_signature_is_none_when_no_flag_matches() {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.flags.push(ordinary_flag('v', "verbose"));
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
        child.flags.push(k1_flag());
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
        root.flags.push(k1_flag());
        root.flags.push(ordinary_flag('v', "verbose"));
        let mut child = CommandNode::new("sub", Provenance::single(Source::HelpText));
        child.flags.push(k1_flag());
        root.subcommands.push(child);
        assert_eq!(k1_signature_stats(&root), (2, 3));
    }

    // -------------------------------------------------------------
    // K2 pre-tag
    // -------------------------------------------------------------

    #[test]
    fn k2_signature_is_true_when_every_fabrication_is_a_multi_column_token() {
        // Real busybox/openssl shape: several names on one line, only the
        // first is a "line start word", but every name is a whitespace
        // token somewhere on that line.
        let raw = "asn1parse         ca                ciphers           cmp\n";
        let mut root = CommandNode::new("openssl", Provenance::single(Source::HelpText));
        for name in ["asn1parse", "ca", "ciphers", "cmp"] {
            root.subcommands
                .push(CommandNode::new(name, Provenance::single(Source::HelpText)));
        }
        let report = existence::detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            3,
            "only the first column is a line-start word; the other three are flagged"
        );
        assert_eq!(k2_signature(&report, raw), Some(true));
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
        root.flags
            .push(Flag::long("version", Provenance::single(Source::HelpText)));
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
        root.flags
            .push(Flag::long("version", Provenance::single(Source::HelpText)));
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

    #[test]
    fn cmd_sample_force_includes_tools_outside_the_stratified_draw() {
        let tmp = tempfile::tempdir().unwrap();
        // sample_size=0: nothing from the stratified draw at all, so any
        // entry present afterward must have come from force_include.
        let force = vec![("sh".to_string(), "unaudited promotion example".to_string())];
        cmd_sample(
            100,
            0,
            Some(vec!["sh".to_string(), "cat".to_string()]),
            tmp.path(),
            &force,
        )
        .unwrap();
        let file = load(&verdict_path(tmp.path(), 100)).unwrap();
        assert_eq!(file.entries.len(), 1, "only the forced tool is present");
        assert_eq!(
            file.entries[0].include_reason.as_deref(),
            Some("unaudited promotion example")
        );
    }

    #[test]
    fn cmd_sample_force_include_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let force = vec![("sh".to_string(), "reason one".to_string())];
        cmd_sample(101, 0, Some(vec!["sh".to_string()]), tmp.path(), &force).unwrap();
        cmd_sample(101, 0, Some(vec!["sh".to_string()]), tmp.path(), &force).unwrap();
        let file = load(&verdict_path(tmp.path(), 101)).unwrap();
        assert_eq!(
            file.entries.len(),
            1,
            "re-running sample must not duplicate an already force-included tool"
        );
    }

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
            amendments: Vec::new(),
        };
        assert_eq!(effective_stratum(&e), "ok");
        e.include_reason = Some("unaudited promotion".to_string());
        assert_eq!(effective_stratum(&e), FORCED_INCLUSION_STRATUM);
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
            amendments: Vec::new(),
        }
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
