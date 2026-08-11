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
use crate::misattribution::RecordingProbe;
use crate::status;
use mandible_core::CommandNode;
use mandible_extract::exec::ExecOutput;
use mandible_extract::{default_tiers_with_probe, ExtractionResult, Runner};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One entry in a verdict file: a sampled tool, its drawn stratum, and — once
/// reviewed — a verdict plus an optional note. `verdict: None` is the
/// "pending" state; every command that touches the file treats absence of a
/// verdict as "not yet reviewed", never as an implicit skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// The tool name as found on `PATH` (or supplied via `--tools`).
    pub tool: String,
    /// The [`status::compute`] label this tool had when it was drawn —
    /// recorded at draw time, not recomputed later, so a tool whose parse
    /// changes between `sample` and `review` (a grammar fix landing
    /// mid-session) still reports against the stratum it was actually
    /// drawn from.
    pub stratum: String,
    /// `"correct"` / `"incomplete"` / `"wrong"` / `"skip"`, or absent while
    /// pending. Stored as a plain string (not an enum) so a hand-edited
    /// verdict file with an unrecognized word fails loudly at the point of
    /// use (`parse_verdict_word`) rather than silently at deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// The reviewer's free-text note. Becomes an `[xfail]` `reason` for a
    /// `wrong`/`incomplete` fixture (see [`cmd_fixtures`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// The persisted state of one audit run: everything needed to resume, and
/// nothing that would make two runs of `sample` with the same `--seed`
/// disagree with each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFile {
    meta: AuditMeta,
    #[serde(default, rename = "entry")]
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditMeta {
    seed: u64,
    sample_size: usize,
}

impl AuditFile {
    fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.verdict.is_none())
            .map(|(i, _)| i)
    }
}

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
            entries.push(Entry {
                tool,
                stratum: stratum.clone(),
                verdict: None,
                note: String::new(),
            });
        }
    }
    entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    (entries, counts)
}

fn verdict_path(dir: &Path, seed: u64) -> PathBuf {
    dir.join(format!("{seed}.toml"))
}

fn load(path: &Path) -> anyhow::Result<AuditFile> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "reading {}: {e} (run `xtask audit sample` first)",
            path.display()
        )
    })?;
    toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))
}

fn save(path: &Path, file: &AuditFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("creating {}: {e}", parent.display()))?;
        }
    }
    let text = toml::to_string_pretty(file)
        .map_err(|e| anyhow::anyhow!("serializing {}: {e}", path.display()))?;
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))
}

/// `xtask audit sample`: (re)compute the deterministic, stratified draw and
/// merge it into `path`, never disturbing an entry already present (so a
/// resumed or repeated `sample` invocation is a no-op on top of prior
/// progress — see this module's doc comment).
pub fn cmd_sample(
    seed: u64,
    sample_size: usize,
    tools: Option<Vec<String>>,
    dir: &Path,
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
    for entry in drawn {
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
        "{added} new pending entr{s} written to {} ({} pending total)",
        path.display(),
        file.pending().count(),
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

/// Parse a verdict word (`c`/`correct`, `i`/`incomplete`, `w`/`wrong`,
/// `s`/`skip`) to its canonical spelling. Shared by [`cmd_review`] (typed
/// live) and [`cmd_ingest`] (read from a verdicts file), so the two entry
/// points can never disagree about what counts as a valid verdict.
fn parse_verdict_word(word: &str) -> anyhow::Result<&'static str> {
    match word {
        "c" | "correct" => Ok("correct"),
        "i" | "incomplete" => Ok("incomplete"),
        "w" | "wrong" => Ok("wrong"),
        "s" | "skip" => Ok("skip"),
        other => anyhow::bail!(
            "unrecognized verdict {other:?} — expected one of: c/correct, i/incomplete, w/wrong, s/skip"
        ),
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
         optionally followed by a space and a note. Blank line or end of input stops \
         (already-recorded verdicts are saved after every tool).",
        pending.len(),
        file.entries.len()
    )?;

    for idx in pending {
        let tool = file.entries[idx].tool.clone();
        let stratum = file.entries[idx].stratum.clone();
        let classified = classify_one(&tool);
        writeln!(output, "\n=== {tool}  (stratum: {stratum}) ===")?;
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
        let note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)?;

        file.entries[idx].verdict = Some(verdict.to_string());
        file.entries[idx].note = note;
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
            "tool: {}\nstratum: {}\n\n",
            entry.tool, entry.stratum
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
        "review offline, then write a verdicts file (one line per tool: `<tool> <verdict> [note...]`) \
         and run: cargo run -p xtask -- audit ingest --seed {seed} --verdicts <file>"
    );
    Ok(())
}

/// A tool name is never empty and, on every platform this project targets,
/// never contains `/` (§ `resolve_tool`'s own PATH-search doesn't accept
/// path separators in a bare tool name either), so this exists only to be
/// defensive about the one other filesystem-hostile case worth naming.
fn sanitize_filename(tool: &str) -> String {
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
        let note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)
            .map_err(|e| anyhow::anyhow!("{}:{}: {e}", verdicts_path.display(), lineno + 1))?;

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

/// `xtask audit report`: per-stratum and overall accuracy, each stated as a
/// count and a confidence interval — never a bare percentage (spec's own
/// complaint about `%flags_text`/`%described`, spec §13.1b, is exactly what
/// this line format exists to avoid repeating). Also lists every tool
/// judged `wrong` or `incomplete`, since those are the next bugs to fix.
pub fn cmd_report(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;

    let mut by_stratum: BTreeMap<String, StratumTally> = BTreeMap::new();
    for entry in &file.entries {
        let tally = by_stratum
            .entry(entry.stratum.clone())
            .or_insert(StratumTally {
                correct: 0,
                judged: 0,
                skipped: 0,
                pending: 0,
            });
        match entry.verdict.as_deref() {
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

    let mut flagged: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| matches!(e.verdict.as_deref(), Some("wrong") | Some("incomplete")))
        .collect();
    flagged.sort_by(|a, b| a.tool.cmp(&b.tool));
    if !flagged.is_empty() {
        println!("\ntools judged wrong or incomplete (the next bugs):");
        for entry in flagged {
            println!(
                "  {:<24} {:<11} {}",
                entry.tool,
                entry.verdict.as_deref().unwrap_or(""),
                entry.note,
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
        let Some(verdict) = entry.verdict.as_deref() else {
            continue;
        };
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
                let reason = if entry.note.is_empty() {
                    format!(
                        "reviewer marked this {verdict} under xtask audit (seed {seed}); \
                         no note was recorded"
                    )
                } else {
                    entry.note.clone()
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
    use std::io::Cursor;

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
        cmd_sample(55, 1, Some(vec!["sh".to_string()]), tmp.path()).unwrap();
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
}
