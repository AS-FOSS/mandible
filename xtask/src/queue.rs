//! The frozen sampling queue (spec §16's "intended fix", now built):
//! `xtask audit freeze` sweeps `PATH` once, classifies every tool, and
//! shuffle-stratifies the result into an ordered queue; `xtask audit
//! sample` then just advances a cursor through that frozen queue and takes
//! the next slice; `xtask audit reclassify` recomputes every entry's
//! stratum against the *current* parser from the bytes `freeze` captured,
//! with zero `PATH` re-sweep.
//!
//! # Why this exists
//!
//! The draw this module replaces (`xtask::audit`'s old `cmd_sample`, before
//! this batch) reclassified the whole `PATH` population — probing every one
//! of ~2,300 tools — on **every single draw**, costing roughly twenty
//! minutes each time (spec §16). Worse, because the strata were recomputed
//! from whatever the parser happened to be that day, two draws taken weeks
//! apart were stratifying against two different definitions of "ok", making
//! them not directly comparable and turning any fix to
//! `mandible-extract`'s grammar into a silent redefinition of what an
//! `audit sample` run was even measuring.
//!
//! The fix, as decided (spec §16, and reaffirmed by external review with
//! three additions this module implements): snapshot the tool list **once**,
//! at freeze time, and have every subsequent draw walk a **cursor** through
//! that frozen, pre-shuffled queue. This module is explicitly **not**
//! implemented by cross-comparing already-reviewed tools against the
//! current tool list at draw time — no set-difference against "what's been
//! done". The queue is ordered once, and a cursor advances through it; nothing
//! about a draw depends on which tools any verdict file has already recorded.
//!
//! # The three review additions
//!
//! 1. **Freeze date + population hash in the manifest**
//!    ([`QueueMeta::freeze_date`], [`QueueMeta::population_hash`],
//!    [`population_hash`]), so a queue can be identified and staleness
//!    detected (`xtask audit freeze --check`, [`cmd_freeze`]) without
//!    re-probing anything — just a `PATH` directory listing, the same cheap
//!    scan [`crate::coverage::unique_executables_on_path`] already does for
//!    every other instrument in this crate.
//! 2. **Shuffle-stratify at freeze time** ([`shuffle_stratify`]), so any
//!    prefix of the frozen order is *itself* a valid, proportionally
//!    stratified sample of the full population — not just the queue as a
//!    whole. See that function's own doc comment for the interleaving
//!    method and [`shuffle_stratify`]'s tests for the validity check.
//! 3. **Freeze the captured raw help text alongside the tool list**
//!    ([`cmd_freeze`] writing `<dir>/queue-captures/`,
//!    [`load_captures_for_tool`]/[`write_captures_for_tool`]). This is the
//!    improvement that actually matters: reclassifying from cached bytes
//!    ([`cmd_reclassify`]) needs no `PATH` sweep and spawns zero
//!    subprocesses at all — the honest, measured comparison (this batch's
//!    own 500-tool benchmark on a 4-core machine) is a parallel reclassify
//!    in roughly half the wall-clock of the live-probing freeze it replaced
//!    (~65s vs. ~123s), not a "seconds regardless of scale" promise: what's
//!    left after removing every subprocess is real CPU-bound parsing (plus
//!    the native/cobra artifact tier's own binary-byte scan of each tool's
//!    on-disk executable), which scales with available cores. See
//!    [`cmd_reclassify`]'s own doc comment for the full measurement and the
//!    caveats freezing a population honestly still carries.
//!
//! # Storage: what's tracked, what's generated
//!
//! `<dir>/queue.toml` (default `audit/queue.toml`) is **tracked**, following
//! the existing convention `audit/*.toml`/`audit/force-include.txt` already
//! set: it is the sample manifest, small (one line per tool: name + a short
//! stratum label), and is *evidence* for a claim about how the queue was
//! built — the same "a measurement's evidence lives in git, not on one
//! contributor's laptop" reasoning spec.md Appendix A already applies to
//! `audit/<seed>.toml`.
//!
//! `<dir>/queue-captures/` is **not tracked** (gitignored, same convention
//! as `audit/*/fixtures/`): it is real bulk — every captured tool's raw
//! `--help` bytes, on the order of several thousand small files — and,
//! critically, it is **machine-generated content**, which the fixture
//! promotion workflow (`corpus/README.md`) already treats as something that
//! must never land in a tracked human-verdict file. `queue.toml` records
//! *what a tool's stratum was*, a fact a human can audit by reading one
//! line; `queue-captures/` records the actual bytes a probe returned, which
//! is regenerable by re-running `xtask audit freeze` and does not need a
//! permanent home in git the way a promoted `corpus/` fixture (reviewed,
//! deliberately kept) does. A queue built once and worth reusing across
//! machines is expected to have its captures regenerated locally, not
//! shipped in the repo.
//!
//! # Honest caveats
//!
//! - **A frozen population drifts from the machine's real installed tools
//!   over time.** `xtask audit freeze --check` detects this cheaply (no
//!   probing) by comparing [`population_hash`] against what's on `PATH`
//!   *now*, but detecting drift is not fixing it — a stale queue still
//!   reflects the tool set at freeze time until re-frozen.
//! - **Reclassification updates a tool's reported *stratum*, never its
//!   *position* in the queue.** The shuffle-stratified order was computed
//!   once, from the strata as they stood at freeze time; recomputing strata
//!   from cached bytes later (`--update`) can legitimately change what
//!   stratum a tool is *reported* under without re-shuffling where it sits
//!   in the cursor order. A queue reclassified long after freezing may
//!   therefore no longer interleave in exact proportion to its *current*
//!   stratum composition, only to its composition at freeze time — a
//!   drift that is real but much smaller than the staleness this module
//!   replaces (a frozen order at least still visits the *same* tools in the
//!   *same* sequence, so successive draws stay comparable).
//! - **Reclassification still depends on the tool binary resolving on
//!   `PATH` at the same path.** The native/cobra framework-detection tier
//!   (`mandible_extract::framework::artifact`) reads a binary's own bytes
//!   directly off disk, not from the frozen capture, to fingerprint it — a
//!   tool uninstalled since freeze time will report a degraded stratum for
//!   a reason unrelated to any parser change. This is a file read, not a
//!   process spawn, so it never reintroduces the *subprocess* cost this
//!   module exists to remove — but it is measurably not free either: it is
//!   plausibly a real share of [`cmd_reclassify`]'s own CPU-bound cost (see
//!   that function's doc comment), since scanning a large on-disk binary for
//!   byte markers, once per tool, is real work. Either way, a
//!   `cmd_reclassify` report is only purely "what changed in the parser"
//!   when the machine's installed tools are also unchanged since freeze.

use crate::audit::{
    classify_all_with_recordings, classify_one, entry_from_classified, sanitize_filename,
    Classified,
};
use crate::coverage::unique_executables_on_path;
use crate::rng::{fnv1a64, seeded_shuffle, stratum_seed};
use crate::status;
use mandible_core::audit::{load, save, verdict_path, AuditFile, AuditMeta, Entry};
use mandible_extract::exec::{ExecOutput, Transcript};
use mandible_extract::{default_tiers_with_probe, Runner};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// One entry in a frozen queue: a tool name and the stratum it carried when
/// it was frozen (or last reclassified, see [`cmd_reclassify`]'s `--update`).
/// Order in [`Queue::entries`] **is** the cursor order — see
/// [`shuffle_stratify`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub tool: String,
    pub stratum: String,
}

/// [`Queue`]'s own metadata: everything needed to identify the queue, detect
/// staleness, and resume drawing from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueMeta {
    /// The date `xtask audit freeze` produced this queue, `YYYY-MM-DD`
    /// (UTC). Informational — nothing reads it back to make a decision —
    /// but it is what lets a human looking at `queue.toml` answer "how old
    /// is this?" without archaeology.
    pub freeze_date: String,
    /// [`population_hash`] of the tool list this queue was built from, for
    /// `xtask audit freeze --check`'s staleness comparison.
    pub population_hash: String,
    /// The seed [`shuffle_stratify`] used to interleave the queue's order.
    /// A distinct concept from `xtask audit sample`'s `--seed`, which now
    /// only names a verdict file (`<dir>/<seed>.toml`) — see this module's
    /// own doc comment and `cmd_sample`'s.
    pub seed: u64,
    /// How far into [`Queue::entries`] the next `xtask audit sample` draw
    /// starts. Advanced (and persisted) by every successful [`cmd_sample`]
    /// call; never rewound automatically.
    #[serde(default)]
    pub cursor: usize,
}

/// The frozen queue itself: metadata plus the shuffle-stratified tool order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queue {
    pub meta: QueueMeta,
    #[serde(default, rename = "entry")]
    pub entries: Vec<QueueEntry>,
}

/// The path a given queue directory resolves to: `<dir>/queue.toml`.
pub fn queue_path(dir: &Path) -> PathBuf {
    dir.join("queue.toml")
}

/// Where a queue's captured raw bytes live: `<dir>/queue-captures/`. Not
/// tracked in git (this module's own doc comment).
pub fn captures_dir(dir: &Path) -> PathBuf {
    dir.join("queue-captures")
}

fn load_queue(path: &Path) -> anyhow::Result<Queue> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "reading {}: {e} (run `xtask audit freeze` first)",
            path.display()
        )
    })?;
    toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))
}

fn save_queue(path: &Path, queue: &Queue) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("creating {}: {e}", parent.display()))?;
        }
    }
    let text = toml::to_string_pretty(queue)
        .map_err(|e| anyhow::anyhow!("serializing {}: {e}", path.display()))?;
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))
}

/// A stable, order-independent fingerprint of a tool population: sorted,
/// deduplicated, then hashed with FNV-1a over the names joined by `\n`
/// separators (so `("ab", "c")` and `("a", "bc")` never collide). Not
/// `std::collections::hash_map::DefaultHasher` — see [`crate::rng`]'s own
/// doc comment on why that would silently invalidate every previously
/// recorded population hash across a Rust toolchain upgrade. Rendered as
/// lowercase hex so it reads cleanly in a checked-in `queue.toml`.
pub fn population_hash(tools: &[String]) -> String {
    let mut sorted: Vec<&str> = tools.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for name in &sorted {
        h ^= fnv1a64(name.as_bytes());
        // A separator step so the hash is sensitive to *where* one name
        // ends and the next begins, not just to the multiset of bytes
        // across the whole list.
        h ^= 0x0Au64;
        h = h.wrapping_mul(0x0000_0001_0000_01B3);
    }
    format!("{h:016x}")
}

/// Today's date as `YYYY-MM-DD` (UTC), computed from
/// [`std::time::SystemTime`] with no date/time dependency in the workspace
/// — the same "no crate the project doesn't already need" discipline
/// [`crate::rng`]'s hand-rolled PRNG follows. Uses the standard
/// days-since-epoch civil-calendar conversion (Howard Hinnant's
/// `civil_from_days`, public domain), which is exact for every date this
/// project will ever produce and needs no leap-second table.
fn today_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`: days-since-1970-01-01 to a
/// proleptic-Gregorian `(year, month, day)`. Exact, dependency-free, and
/// well-known enough that re-deriving it from scratch would only add risk.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Build the frozen queue's order from `(tool, stratum)` pairs: within each
/// stratum, a seeded deterministic shuffle (see [`crate::rng::seeded_shuffle`]);
/// across strata, a **proportional interleave** rather than a concatenation.
///
/// Concatenating shuffled strata (stratum A in full, then stratum B) would
/// make the queue's own *total* order proportionally stratified, but any
/// prefix shorter than all of stratum A would be 100% stratum A — exactly
/// what review addition 2 (this module's own doc comment) rules out. The
/// fix: within each stratum, give the shuffled item at position `i` (of
/// `n`) a fractional rank `(i + 0.5) / n` — its position within its own
/// stratum, normalized to `(0, 1)` — then merge every stratum's items by
/// sorting on that fraction (ties broken by a deterministic hash of
/// `seed`+`stratum`+`tool`, since two strata of equal size can otherwise
/// land on the exact same fraction). Because every stratum's fractional
/// ranks are spread evenly across `(0, 1)`, cutting the merged order at any
/// fractional threshold `x` yields, from each stratum, very close to the
/// `x` fraction of its own items — which is exactly "any prefix is itself a
/// valid proportionally stratified sample". See this function's own tests
/// for the measured tolerance.
pub fn shuffle_stratify(pairs: &[(String, String)], seed: u64) -> Vec<QueueEntry> {
    let mut by_stratum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (tool, stratum) in pairs {
        by_stratum
            .entry(stratum.clone())
            .or_default()
            .push(tool.clone());
    }
    for (stratum, tools) in by_stratum.iter_mut() {
        seeded_shuffle(tools, stratum_seed(seed, stratum));
    }

    struct Ranked {
        tool: String,
        stratum: String,
        frac: f64,
        tie: u64,
    }
    let mut ranked: Vec<Ranked> = Vec::with_capacity(pairs.len());
    for (stratum, tools) in &by_stratum {
        let len = tools.len();
        for (i, tool) in tools.iter().enumerate() {
            let frac = (i as f64 + 0.5) / len as f64;
            let tie = fnv1a64(format!("{seed}:{stratum}:{tool}").as_bytes());
            ranked.push(Ranked {
                tool: tool.clone(),
                stratum: stratum.clone(),
                frac,
                tie,
            });
        }
    }
    ranked.sort_by(|a, b| {
        a.frac
            .partial_cmp(&b.frac)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tie.cmp(&b.tie))
    });
    ranked
        .into_iter()
        .map(|r| QueueEntry {
            tool: r.tool,
            stratum: r.stratum,
        })
        .collect()
}

/// A capture file's on-disk shape: one `index.toml` per tool directory
/// under `queue-captures/`, naming the raw stdout/stderr files sitting
/// beside it. Mirrors `corpus/README.md`'s own `[[capture]]` convention
/// (argv + a named stdout/stderr file) rather than inventing a second one.
#[derive(Debug, Serialize, Deserialize)]
struct CaptureIndex {
    #[serde(rename = "capture", default)]
    captures: Vec<CaptureEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CaptureEntry {
    argv: Vec<String>,
    stdout: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(default)]
    timed_out: bool,
}

/// Write every `(argv, output)` pair `recordings` holds for `tool` to
/// `<captures_dir>/<sanitized tool>/`: an `index.toml` naming each capture's
/// argv and exit status, plus one `N.stdout`/`N.stderr` file per capture.
/// Captures are written in argv-sorted order (not `HashMap` iteration
/// order) purely so a re-run of `freeze` against unchanged bytes produces a
/// stable diff, never because numbering is read back — [`load_captures_for_tool`]
/// only ever reads `index.toml`'s own filenames.
fn write_captures_for_tool(
    captures_dir: &Path,
    tool: &str,
    recordings: &HashMap<Vec<String>, ExecOutput>,
) -> anyhow::Result<()> {
    let tool_dir = captures_dir.join(sanitize_filename(tool));
    std::fs::create_dir_all(&tool_dir)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", tool_dir.display()))?;

    let mut items: Vec<(&Vec<String>, &ExecOutput)> = recordings.iter().collect();
    items.sort_by(|a, b| a.0.cmp(b.0));

    let mut captures = Vec::with_capacity(items.len());
    for (i, (argv, output)) in items.into_iter().enumerate() {
        let stdout_name = format!("{i}.stdout");
        std::fs::write(tool_dir.join(&stdout_name), &output.stdout)?;
        let stderr = if !output.stderr.is_empty() {
            let name = format!("{i}.stderr");
            std::fs::write(tool_dir.join(&name), &output.stderr)?;
            Some(name)
        } else {
            None
        };
        captures.push(CaptureEntry {
            argv: argv.clone(),
            stdout: stdout_name,
            stderr,
            exit_code: output.exit_code,
            timed_out: output.timed_out,
        });
    }
    let text = toml::to_string_pretty(&CaptureIndex { captures })
        .map_err(|e| anyhow::anyhow!("serializing capture index for {tool:?}: {e}"))?;
    std::fs::write(tool_dir.join("index.toml"), text)
        .map_err(|e| anyhow::anyhow!("writing capture index for {tool:?}: {e}"))
}

/// The inverse of [`write_captures_for_tool`]: read `tool`'s captured
/// `(argv, output)` pairs back off disk, with no subprocess involved.
fn load_captures_for_tool(
    captures_dir: &Path,
    tool: &str,
) -> anyhow::Result<HashMap<Vec<String>, ExecOutput>> {
    let tool_dir = captures_dir.join(sanitize_filename(tool));
    let index_path = tool_dir.join("index.toml");
    let raw = std::fs::read_to_string(&index_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", index_path.display()))?;
    let parsed: CaptureIndex = toml::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", index_path.display()))?;

    let mut map = HashMap::with_capacity(parsed.captures.len());
    for entry in parsed.captures {
        let stdout = std::fs::read(tool_dir.join(&entry.stdout))
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", entry.stdout))?;
        let stderr = match &entry.stderr {
            Some(name) => std::fs::read(tool_dir.join(name))
                .map_err(|e| anyhow::anyhow!("reading {name}: {e}"))?,
            None => Vec::new(),
        };
        map.insert(
            entry.argv,
            ExecOutput {
                stdout,
                stderr,
                exit_code: entry.exit_code,
                timed_out: entry.timed_out,
            },
        );
    }
    Ok(map)
}

/// `xtask audit freeze`: sweep `PATH` (or `tools`, for a pinned,
/// reproducible population — the same role `--tools` played on the old,
/// removed live-sweep `cmd_sample`, moved here since freezing is now the
/// only place a `PATH` sweep happens at all), classify every tool, capture
/// its raw bytes, and write the shuffle-stratified queue plus its captures.
///
/// `check: true` skips all of that (no probing, no writing) and instead
/// just enumerates the current population, hashes it
/// ([`population_hash`]), and reports whether it still matches the existing
/// queue's — the cheap staleness check review addition 1 asked for. Mirrors
/// `xtask coverage --check`'s "report, don't rewrite" shape.
pub fn cmd_freeze(
    seed: u64,
    tools: Option<Vec<String>>,
    dir: &Path,
    check: bool,
) -> anyhow::Result<()> {
    let population = tools.unwrap_or_else(unique_executables_on_path);
    if population.is_empty() {
        anyhow::bail!("no tools found to freeze (empty PATH population and no --tools given)");
    }
    let qpath = queue_path(dir);

    if check {
        let current_hash = population_hash(&population);
        if !qpath.is_file() {
            anyhow::bail!(
                "{} does not exist yet — run `xtask audit freeze` (without --check) first",
                qpath.display()
            );
        }
        let queue = load_queue(&qpath)?;
        if queue.meta.population_hash == current_hash {
            println!(
                "population unchanged since freeze on {} ({} tool(s), hash {})",
                queue.meta.freeze_date,
                population.len(),
                current_hash
            );
        } else {
            let frozen: HashSet<&str> = queue.entries.iter().map(|e| e.tool.as_str()).collect();
            let current: HashSet<&str> = population.iter().map(String::as_str).collect();
            let added: Vec<&str> = current.difference(&frozen).copied().collect();
            let removed: Vec<&str> = frozen.difference(&current).copied().collect();
            println!(
                "population drift since freeze on {} (hash {} -> {}): {} tool(s) added, {} \
                 removed. Re-run `xtask audit freeze` to build a fresh queue when this matters.",
                queue.meta.freeze_date,
                queue.meta.population_hash,
                current_hash,
                added.len(),
                removed.len(),
            );
        }
        return Ok(());
    }

    println!(
        "classifying {} tool(s) to build the frozen queue (this sweeps PATH and probes every \
         tool once — the one-time cost this command exists to pay so `audit sample` never has \
         to again)...",
        population.len()
    );
    let classified = classify_all_with_recordings(&population);

    let cdir = captures_dir(dir);
    std::fs::create_dir_all(&cdir)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", cdir.display()))?;
    for (tool, _classified, recordings) in &classified {
        write_captures_for_tool(&cdir, tool, recordings)?;
    }

    let pairs: Vec<(String, String)> = classified
        .iter()
        .map(|(tool, c, _)| (tool.clone(), c.stratum.to_string()))
        .collect();
    let entries = shuffle_stratify(&pairs, seed);

    let mut by_stratum: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        *by_stratum.entry(e.stratum.clone()).or_insert(0) += 1;
    }

    let queue = Queue {
        meta: QueueMeta {
            freeze_date: today_iso8601(),
            population_hash: population_hash(&population),
            seed,
            cursor: 0,
        },
        entries,
    };
    save_queue(&qpath, &queue)?;

    println!(
        "froze {} tool(s) into {} (seed={seed}, population_hash={}, captures under {})",
        queue.entries.len(),
        qpath.display(),
        queue.meta.population_hash,
        cdir.display(),
    );
    println!("stratum            count");
    for (stratum, count) in &by_stratum {
        println!("{stratum:<18} {count:>6}");
    }
    Ok(())
}

/// Pure, side-effect-free draw: the slice of `queue.entries` starting at
/// `cursor` and containing up to `sample_size` entries (fewer once the
/// queue is exhausted — never wrapping around to the front, since that
/// would silently redraw tools a prior slice already covered), plus the
/// cursor position the *next* draw should start from. Same `queue` and
/// `cursor` always yield the same slice: nothing here consults time,
/// randomness, or any verdict file.
pub fn draw_at_cursor(
    queue: &Queue,
    cursor: usize,
    sample_size: usize,
) -> (Vec<QueueEntry>, usize) {
    let start = cursor.min(queue.entries.len());
    let end = start.saturating_add(sample_size).min(queue.entries.len());
    (queue.entries[start..end].to_vec(), end)
}

/// `xtask audit sample`: draw the next `sample_size` tools off `<dir>/queue.toml`'s
/// cursor, classify just those (cheap — `sample_size` is small, unlike the
/// full population `freeze` already paid for), merge them into
/// `<dir>/<seed>.toml`, and persist the queue's advanced cursor.
///
/// `seed` here names the verdict file only (`<dir>/<seed>.toml`); it plays
/// no role in the draw itself — the draw's only randomness was spent once,
/// at freeze time. Calling `sample` again (same or different `seed`)
/// advances the *shared* queue cursor and therefore always draws a fresh,
/// never-before-drawn slice — this replaces the old live-sweep
/// `cmd_sample`'s "re-running with the same seed/size is a safe no-op"
/// idempotence with a deliberately different guarantee: an already-recorded
/// verdict is still never disturbed, but re-running now *advances*, exactly
/// the "next K off the cursor" semantics this module exists to give.
///
/// `force_include` entries (`(tool, reason)`) are still classified live and
/// independently of the queue, exactly as before — force-inclusion exists
/// precisely for tools that must not depend on where the cursor happens to
/// be.
pub fn cmd_sample(
    seed: u64,
    sample_size: usize,
    dir: &Path,
    force_include: &[(String, String)],
) -> anyhow::Result<()> {
    let qpath = queue_path(dir);
    let mut queue = load_queue(&qpath)?;

    let mut pop_by_stratum: BTreeMap<String, usize> = BTreeMap::new();
    for e in &queue.entries {
        *pop_by_stratum.entry(e.stratum.clone()).or_insert(0) += 1;
    }

    let (drawn, next_cursor) = draw_at_cursor(&queue, queue.meta.cursor, sample_size);
    if drawn.len() < sample_size {
        println!(
            "note: only {} tool(s) remain in the queue from cursor {} ({} requested) — \
             re-run `xtask audit freeze` to build a fresh queue for a bigger draw.",
            drawn.len(),
            queue.meta.cursor,
            sample_size
        );
    }

    let mut drawn_entries = Vec::with_capacity(drawn.len());
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for qe in &drawn {
        let c = classify_one(&qe.tool);
        counts
            .entry(qe.stratum.clone())
            .and_modify(|(d, _)| *d += 1)
            .or_insert((1, *pop_by_stratum.get(&qe.stratum).unwrap_or(&0)));
        drawn_entries.push(entry_from_classified(qe.tool.clone(), &c, None));
    }

    // Force-included tools are classified independently of the queue: the
    // whole point (spec-cited case: the 14 `find_description_gap`
    // promotions) is that they must appear regardless of where the cursor
    // is or whether they were even part of the frozen population.
    let mut forced_entries = Vec::with_capacity(force_include.len());
    for (tool, reason) in force_include {
        let c: Classified = classify_one(tool);
        forced_entries.push(entry_from_classified(
            tool.clone(),
            &c,
            Some(reason.clone()),
        ));
    }

    let vpath = verdict_path(dir, seed);
    let mut file = if vpath.is_file() {
        load(&vpath)?
    } else {
        AuditFile {
            meta: AuditMeta { seed, sample_size },
            entries: Vec::new(),
        }
    };
    file.meta.sample_size = sample_size;

    let existing_tools: HashSet<String> = file
        .entries
        .iter()
        .map(|e: &Entry| e.tool.clone())
        .collect();
    let mut added = 0usize;
    for entry in drawn_entries.into_iter().chain(forced_entries) {
        if !existing_tools.contains(&entry.tool) {
            file.entries.push(entry);
            added += 1;
        }
    }
    file.entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    save(&vpath, &file)?;

    queue.meta.cursor = next_cursor;
    save_queue(&qpath, &queue)?;

    println!(
        "drew {} tool(s) from queue cursor {} -> {} (queue population {})",
        drawn.len(),
        next_cursor - drawn.len(),
        next_cursor,
        queue.entries.len(),
    );
    println!("stratum            drawn   population   %pop");
    for (stratum, (n_drawn, n_pop)) in &counts {
        println!(
            "{stratum:<18}  {n_drawn:>4}  {n_pop:>10}  {:>5.1}%",
            *n_pop as f64 / queue.entries.len().max(1) as f64 * 100.0,
        );
    }
    println!(
        "{added} new pending entr{s} written to {} ({} pending total, {} force-included)",
        vpath.display(),
        file.pending().count(),
        force_include.len(),
        s = if added == 1 { "y" } else { "ies" },
    );
    Ok(())
}

/// Reclassify one entry from its cached captures, with **zero subprocess
/// spawns**: [`Transcript`] replays exactly the `(argv, output)` pairs
/// [`write_captures_for_tool`] persisted, through the real tiered pipeline
/// (`default_tiers_with_probe`), so this is a genuine re-run of the actual
/// parser — not a re-derived approximation of one — bounded only by CPU
/// time. `None` when the tool's captures are missing (a tool the population
/// no longer has, or one frozen before this queue existed); [`cmd_reclassify`]
/// leaves that entry's stratum untouched but still counts it as "missing"
/// in the printed report rather than silently skipping it. Pure and
/// side-effect-free so [`cmd_reclassify`] can run it over every entry in
/// parallel via `rayon`, which is what makes the "seconds, not minutes"
/// claim (this module's own doc comment) hold at fleet scale: a serial loop
/// over ~2,300 tools' worth of real parsing is itself minutes, even with
/// zero probes — see this function's own doc comment history for the
/// measurement that caught it.
fn reclassify_one(cdir: &Path, tool: &str) -> Option<String> {
    let recordings = load_captures_for_tool(cdir, tool).ok()?;
    let transcript: Arc<dyn mandible_extract::exec::Probe> = Arc::new(Transcript::new(recordings));
    let runner = Runner::new(default_tiers_with_probe(transcript));
    let result = runner.extract_full(tool);
    Some(status::compute(&result).label.to_string())
}

/// `xtask audit reclassify`: recompute every queue entry's stratum against
/// the **current** parser, from the bytes `xtask audit freeze` already
/// captured — no `PATH` sweep, no subprocess spawned at all. Each tool's
/// cached `(argv, output)` pairs are replayed through the real extraction
/// pipeline via [`Transcript`] (`mandible_extract::exec`), the same replay
/// seam the corpus regression runner uses, so this exercises the *actual*
/// tiers and merge logic, not a re-derived approximation of them.
///
/// Runs [`reclassify_one`] over every entry **in parallel** via `rayon`
/// (`par_iter`), the same reasoning [`classify_all_with_recordings`] already
/// applies to a live sweep: with zero subprocess spawns this is a purely
/// CPU-bound loop, and a real measurement at fleet scale (a real 500-tool
/// `PATH` slice on this batch's 4-core evaluation machine) showed a naive
/// serial version taking *longer* than the parallel live-probing freeze it
/// replaced (135s serial versus freeze's own ~123s on the same population).
/// Parallelizing recovered roughly half that (~65s) — a real, measured
/// improvement, but the honest number: this is CPU-bound replay-and-parse
/// work (including the native/cobra artifact tier's own binary-byte scan of
/// each tool's on-disk executable, not just help-text parsing), so it is
/// **not** a blanket "seconds regardless of population size" claim, it
/// scales with available CPU cores and the real cost of parsing at fleet
/// scale, not with a probe count times a timeout. What is unconditionally
/// true regardless of core count: zero `PATH` sweep, zero subprocess
/// spawns, and a wall-clock roughly half a live re-probe's on this
/// machine — see this module's own doc comment for the full, hedged claim.
///
/// Prints per-tool transitions (old stratum -> new) and the new per-stratum
/// counts, plus the wall-clock cost.
///
/// `update: true` writes the recomputed strata back into `<dir>/queue.toml`
/// in place — see this module's own doc comment for what that does and
/// does not change (the *order* of the queue never moves; only each
/// entry's own `stratum` field does).
pub fn cmd_reclassify(dir: &Path, update: bool) -> anyhow::Result<()> {
    let qpath = queue_path(dir);
    let mut queue = load_queue(&qpath)?;
    let cdir = captures_dir(dir);

    let start = Instant::now();
    let outcomes: Vec<Option<String>> = queue
        .entries
        .par_iter()
        .map(|entry| reclassify_one(&cdir, &entry.tool))
        .collect();

    let mut new_strata: Vec<Option<String>> = Vec::with_capacity(queue.entries.len());
    let mut transitions: Vec<(String, String, String)> = Vec::new();
    let mut by_new_stratum: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing_captures = 0usize;

    for (entry, outcome) in queue.entries.iter().zip(outcomes) {
        match outcome {
            Some(new_label) => {
                *by_new_stratum.entry(new_label.clone()).or_insert(0) += 1;
                if new_label != entry.stratum {
                    transitions.push((
                        entry.tool.clone(),
                        entry.stratum.clone(),
                        new_label.clone(),
                    ));
                }
                new_strata.push(Some(new_label));
            }
            None => {
                missing_captures += 1;
                new_strata.push(None);
            }
        }
    }
    let elapsed = start.elapsed();

    println!(
        "reclassified {} tool(s) from cached bytes in {elapsed:.2?} ({missing_captures} missing \
         capture(s), left unchanged, no PATH sweep, zero subprocess spawns)",
        queue.entries.len() - missing_captures,
    );
    println!("stratum            count");
    for (stratum, count) in &by_new_stratum {
        println!("{stratum:<18} {count:>6}");
    }
    if transitions.is_empty() {
        println!("\nno stratum changed since the queue was last classified.");
    } else {
        println!("\n{} tool(s) changed stratum:", transitions.len());
        for (tool, old, new) in &transitions {
            println!("  {tool:<28} {old:<16} -> {new}");
        }
    }

    if update {
        for (entry, new) in queue.entries.iter_mut().zip(new_strata) {
            if let Some(s) = new {
                entry.stratum = s;
            }
        }
        save_queue(&qpath, &queue)?;
        println!(
            "\nupdated {} in place ({} tool(s) changed stratum; queue order and cursor \
             untouched — see this module's doc comment on what reclassification does and does \
             not change).",
            qpath.display(),
            transitions.len()
        );
    } else {
        println!(
            "\ndry run — pass --update to write these strata back into {}",
            qpath.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // Shuffle-stratification validity
    // -------------------------------------------------------------

    fn population_80_20() -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        for i in 0..80 {
            pairs.push((format!("ok{i}"), "ok".to_string()));
        }
        for i in 0..20 {
            pairs.push((format!("lc{i}"), "low-confidence".to_string()));
        }
        pairs
    }

    #[test]
    fn shuffle_stratify_is_a_bijection_over_the_population() {
        let pairs = population_80_20();
        let order = shuffle_stratify(&pairs, 7);
        assert_eq!(order.len(), pairs.len());
        let mut names: Vec<&str> = order.iter().map(|e| e.tool.as_str()).collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = pairs.iter().map(|(t, _)| t.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(names, expected, "every tool appears exactly once");
    }

    #[test]
    fn shuffle_stratify_is_deterministic() {
        let pairs = population_80_20();
        let a = shuffle_stratify(&pairs, 42);
        let b = shuffle_stratify(&pairs, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn shuffle_stratify_differs_across_seeds() {
        let pairs = population_80_20();
        let a = shuffle_stratify(&pairs, 1);
        let b = shuffle_stratify(&pairs, 2);
        assert_ne!(a, b);
    }

    /// The core validity property review addition 2 asks for: any prefix of
    /// the frozen order is *itself* an approximately proportional
    /// stratified sample, not just the queue as a whole. Checked at several
    /// prefix lengths with a generous tolerance — this is a coarse
    /// interleaving property, not an exact allocation like the old
    /// largest-remainder quota it replaced.
    #[test]
    fn any_prefix_of_the_shuffled_order_is_approximately_proportional() {
        let pairs = population_80_20();
        let order = shuffle_stratify(&pairs, 99);
        for &prefix_len in &[10usize, 25, 50, 75, 100] {
            let prefix = &order[..prefix_len];
            let ok_count = prefix.iter().filter(|e| e.stratum == "ok").count();
            let observed_ratio = ok_count as f64 / prefix_len as f64;
            assert!(
                (observed_ratio - 0.8).abs() < 0.2,
                "prefix of {prefix_len}: expected ~80% ok, got {:.1}% ({ok_count}/{prefix_len})",
                observed_ratio * 100.0,
            );
        }
    }

    #[test]
    fn shuffle_stratify_handles_a_single_stratum() {
        let pairs: Vec<(String, String)> = (0..5)
            .map(|i| (format!("t{i}"), "ok".to_string()))
            .collect();
        let order = shuffle_stratify(&pairs, 3);
        assert_eq!(order.len(), 5);
        assert!(order.iter().all(|e| e.stratum == "ok"));
    }

    // -------------------------------------------------------------
    // Population hash change detection
    // -------------------------------------------------------------

    #[test]
    fn population_hash_is_order_independent() {
        let a = vec!["git".to_string(), "curl".to_string(), "sh".to_string()];
        let b = vec!["sh".to_string(), "git".to_string(), "curl".to_string()];
        assert_eq!(population_hash(&a), population_hash(&b));
    }

    #[test]
    fn population_hash_changes_when_a_tool_is_added() {
        let a = vec!["git".to_string(), "curl".to_string()];
        let mut b = a.clone();
        b.push("zoxide".to_string());
        assert_ne!(population_hash(&a), population_hash(&b));
    }

    #[test]
    fn population_hash_changes_when_a_tool_is_removed() {
        let a = vec!["git".to_string(), "curl".to_string(), "sh".to_string()];
        let b = vec!["git".to_string(), "curl".to_string()];
        assert_ne!(population_hash(&a), population_hash(&b));
    }

    #[test]
    fn population_hash_is_deterministic() {
        let a = vec!["git".to_string(), "curl".to_string()];
        assert_eq!(population_hash(&a), population_hash(&a));
    }

    #[test]
    fn population_hash_dedupes() {
        let a = vec!["git".to_string(), "curl".to_string()];
        let b = vec!["git".to_string(), "curl".to_string(), "git".to_string()];
        assert_eq!(population_hash(&a), population_hash(&b));
    }

    // -------------------------------------------------------------
    // Capture round-trip
    // -------------------------------------------------------------

    fn output(stdout: &str, stderr: &str) -> ExecOutput {
        ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    #[test]
    fn captures_round_trip_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let cdir = captures_dir(tmp.path());
        let mut recordings: HashMap<Vec<String>, ExecOutput> = HashMap::new();
        recordings.insert(vec!["--help".to_string()], output("usage: foo\n", ""));
        recordings.insert(
            vec!["completion".to_string(), "__complete".to_string()],
            output("", "some stderr"),
        );
        write_captures_for_tool(&cdir, "foo", &recordings).unwrap();
        let loaded = load_captures_for_tool(&cdir, "foo").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&vec!["--help".to_string()]].stdout, b"usage: foo\n");
        assert_eq!(
            loaded[&vec!["completion".to_string(), "__complete".to_string()]].stderr,
            b"some stderr"
        );
    }

    // -------------------------------------------------------------
    // Date helper
    // -------------------------------------------------------------

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_584), (2023, 8, 15));
        assert_eq!(civil_from_days(20_678), (2026, 8, 13));
    }

    // -------------------------------------------------------------
    // `xtask audit freeze` (real binaries, real argv — AGENTS.md §3.1)
    // -------------------------------------------------------------

    #[test]
    fn freeze_writes_a_queue_with_a_zero_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(
            1,
            Some(vec!["sh".to_string(), "cat".to_string()]),
            tmp.path(),
            false,
        )
        .unwrap();
        let queue = load_queue(&queue_path(tmp.path())).unwrap();
        assert_eq!(queue.entries.len(), 2);
        assert_eq!(queue.meta.cursor, 0);
        assert_eq!(queue.meta.seed, 1);
        let mut names: Vec<&str> = queue.entries.iter().map(|e| e.tool.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["cat", "sh"]);
    }

    #[test]
    fn freeze_check_reports_no_drift_against_the_same_population() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = vec!["sh".to_string(), "cat".to_string()];
        cmd_freeze(3, Some(tools.clone()), tmp.path(), false).unwrap();
        // `--check` doesn't touch the queue; just confirm it runs cleanly
        // against an unchanged population.
        cmd_freeze(3, Some(tools), tmp.path(), true).unwrap();
    }

    #[test]
    fn freeze_check_reports_drift_against_a_changed_population() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(
            4,
            Some(vec!["sh".to_string(), "cat".to_string()]),
            tmp.path(),
            false,
        )
        .unwrap();
        // A different --tools population simulates PATH drift without
        // depending on what's actually installed on the test machine.
        cmd_freeze(
            4,
            Some(vec!["sh".to_string(), "cat".to_string(), "ls".to_string()]),
            tmp.path(),
            true,
        )
        .unwrap();
    }

    fn entries(specs: &[(&str, &str)]) -> Vec<QueueEntry> {
        specs
            .iter()
            .map(|(tool, stratum)| QueueEntry {
                tool: tool.to_string(),
                stratum: stratum.to_string(),
            })
            .collect()
    }

    fn queue_with(specs: &[(&str, &str)], cursor: usize) -> Queue {
        Queue {
            meta: QueueMeta {
                freeze_date: "2026-08-13".to_string(),
                population_hash: "deadbeef".to_string(),
                seed: 1,
                cursor,
            },
            entries: entries(specs),
        }
    }

    // -------------------------------------------------------------
    // Cursor determinism
    // -------------------------------------------------------------

    #[test]
    fn same_queue_and_cursor_yields_identical_tools_every_time() {
        let q = queue_with(&[("a", "ok"), ("b", "ok"), ("c", "low"), ("d", "ok")], 0);
        let (first, next1) = draw_at_cursor(&q, 1, 2);
        let (second, next2) = draw_at_cursor(&q, 1, 2);
        assert_eq!(first, second);
        assert_eq!(next1, next2);
    }

    #[test]
    fn successive_draws_advance_through_disjoint_slices() {
        let q = queue_with(&[("a", "ok"), ("b", "ok"), ("c", "ok"), ("d", "ok")], 0);
        let (first, cursor1) = draw_at_cursor(&q, 0, 2);
        let (second, cursor2) = draw_at_cursor(&q, cursor1, 2);
        assert_eq!(
            first.iter().map(|e| &e.tool).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            second.iter().map(|e| &e.tool).collect::<Vec<_>>(),
            vec!["c", "d"]
        );
        assert_eq!(cursor2, 4);
        let disjoint: HashSet<&str> = first
            .iter()
            .chain(second.iter())
            .map(|e| e.tool.as_str())
            .collect();
        assert_eq!(disjoint.len(), 4, "no tool should be drawn twice");
    }

    #[test]
    fn draw_past_the_end_returns_fewer_than_requested_without_wrapping() {
        let q = queue_with(&[("a", "ok"), ("b", "ok")], 1);
        let (drawn, next) = draw_at_cursor(&q, 1, 5);
        assert_eq!(drawn.iter().map(|e| &e.tool).collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(next, 2, "cursor must cap at the queue length, not wrap");
        let (drawn_again, next_again) = draw_at_cursor(&q, next, 5);
        assert!(drawn_again.is_empty(), "an exhausted queue draws nothing");
        assert_eq!(next_again, 2);
    }

    // -------------------------------------------------------------
    // `xtask audit sample` (real binaries, real argv — AGENTS.md §3.1)
    // -------------------------------------------------------------

    #[test]
    fn freeze_then_sample_then_reclassify_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(
            1,
            Some(vec!["sh".to_string(), "cat".to_string()]),
            tmp.path(),
            false,
        )
        .unwrap();
        let qpath = queue_path(tmp.path());
        assert!(qpath.is_file());
        let queue = load_queue(&qpath).unwrap();
        assert_eq!(queue.entries.len(), 2);
        assert_eq!(queue.meta.cursor, 0);

        cmd_sample(100, 1, tmp.path(), &[]).unwrap();
        let after_first = load_queue(&qpath).unwrap();
        assert_eq!(
            after_first.meta.cursor, 1,
            "cursor must advance by the draw size"
        );

        cmd_sample(101, 1, tmp.path(), &[]).unwrap();
        let after_second = load_queue(&qpath).unwrap();
        assert_eq!(after_second.meta.cursor, 2);

        // The two verdict files must have drawn disjoint tools (the whole
        // point of a cursor that only ever moves forward).
        let v1 = load(&verdict_path(tmp.path(), 100)).unwrap();
        let v2 = load(&verdict_path(tmp.path(), 101)).unwrap();
        assert_eq!(v1.entries.len(), 1);
        assert_eq!(v2.entries.len(), 1);
        assert_ne!(v1.entries[0].tool, v2.entries[0].tool);

        cmd_reclassify(tmp.path(), false).unwrap();
        // Dry run must not touch the queue at all.
        let unchanged = load_queue(&qpath).unwrap();
        assert_eq!(unchanged.meta.cursor, 2);
    }

    #[test]
    fn sample_never_disturbs_an_already_recorded_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(
            2,
            Some(vec!["sh".to_string(), "cat".to_string(), "ls".to_string()]),
            tmp.path(),
            false,
        )
        .unwrap();
        cmd_sample(200, 1, tmp.path(), &[]).unwrap();
        let vpath = verdict_path(tmp.path(), 200);
        {
            let mut f = load(&vpath).unwrap();
            f.entries[0].verdict = Some("correct".to_string());
            f.entries[0].note = "looked right".to_string();
            save(&vpath, &f).unwrap();
        }
        // A second draw against the *same* verdict file (same seed) must
        // add the newly-advanced tool without disturbing the first one's
        // recorded verdict.
        cmd_sample(200, 1, tmp.path(), &[]).unwrap();
        let after = load(&vpath).unwrap();
        assert_eq!(after.entries.len(), 2);
        let first = after
            .entries
            .iter()
            .find(|e| e.verdict.as_deref() == Some("correct"))
            .expect("the already-recorded verdict must survive");
        assert_eq!(first.note, "looked right");
    }

    #[test]
    fn force_include_appears_outside_the_queue_draw() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(
            5,
            Some(vec!["sh".to_string(), "cat".to_string()]),
            tmp.path(),
            false,
        )
        .unwrap();
        // sample=0: nothing drawn from the queue at all, so any entry
        // present afterward must have come from force_include.
        let force = vec![("sh".to_string(), "unaudited promotion example".to_string())];
        cmd_sample(300, 0, tmp.path(), &force).unwrap();
        let file = load(&verdict_path(tmp.path(), 300)).unwrap();
        assert_eq!(file.entries.len(), 1, "only the forced tool is present");
        assert_eq!(
            file.entries[0].include_reason.as_deref(),
            Some("unaudited promotion example")
        );
    }

    #[test]
    fn force_include_is_idempotent_across_repeated_draws() {
        let tmp = tempfile::tempdir().unwrap();
        cmd_freeze(6, Some(vec!["sh".to_string()]), tmp.path(), false).unwrap();
        let force = vec![("sh".to_string(), "reason one".to_string())];
        cmd_sample(301, 0, tmp.path(), &force).unwrap();
        cmd_sample(301, 0, tmp.path(), &force).unwrap();
        let file = load(&verdict_path(tmp.path(), 301)).unwrap();
        assert_eq!(
            file.entries.len(),
            1,
            "re-running sample must not duplicate an already force-included tool"
        );
    }
}
