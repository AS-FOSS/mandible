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
//!    improvement that actually matters: the twenty minutes was the cost of
//!    *probing*, not of classifying, so reclassifying from cached bytes
//!    ([`cmd_reclassify`]) is nearly free and needs no `PATH` sweep at all —
//!    see that function's own doc comment for the measured cost and the
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
//!   a reason unrelated to any parser change. This is a lightweight file
//!   read, not a process spawn, so it does not reintroduce the cost this
//!   module exists to remove, but it does mean `cmd_reclassify`'s report is
//!   not purely "what changed in the parser" unless the machine's installed
//!   tools are also unchanged since freeze.

use crate::audit::sanitize_filename;
use crate::rng::{fnv1a64, seeded_shuffle, stratum_seed};
use mandible_extract::exec::ExecOutput;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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
}
