//! The corpus regression runner (spec §13.2, `corpus/README.md`): replays
//! every fixture under `corpus/<tool>/<version>/` through the real tiered
//! extraction pipeline with **zero subprocesses**, via the `Transcript`
//! replay seam (`mandible_extract::exec::Transcript`), and fails loudly
//! when a parse regresses.
//!
//! This module only ever *reads* `corpus/`; nothing here is reachable from
//! the `mandible` binary (`corpus/README.md`: "the `mandible` binary never
//! reads this directory"). It lives in `xtask` for exactly that reason.
//!
//! # What "replay every fixture" means
//!
//! Each fixture's `meta.toml` lists `[[capture]]` entries — real argv paired
//! with the captured bytes a tier would have gotten back for it. This
//! module turns that list into a [`Transcript`], builds a synthetic
//! [`ResolvedTool`] (a fixture is never a real path on this machine — see
//! [`resolved_tool`]), and drives [`mandible_extract::default_tiers_with_probe`]
//! through [`Runner`] exactly as the real `mandible` binary would, root
//! extraction plus a bounded recursive fill into every discovered
//! subcommand (spec §5.2's cascade, mirrored here without the background
//! pool since a replay has no I/O to overlap). A fixture that only
//! captured its root `--help` (both fixtures shipped with this runner) is
//! unaffected by the recursive step: every child `fill_node` call misses
//! the transcript, is recorded as a per-tier error exactly as spec §5.3
//! prescribes, and contributes nothing — so the tree comes out identical
//! to a root-only extraction. A future fixture that captures a
//! subcommand's own `--help` (or, for cobra, both its `""` and `"-"`
//! probes) is picked up automatically the moment its `[[capture]]` entry
//! exists, with no runner change required.
//!
//! # Checks, per fixture
//!
//! - **(a) Snapshot match** against `expected.snap` — a plain byte
//!   comparison against `mandible_core::to_snapshot` run through the same
//!   `serde_yaml` serializer `--bless` writes with, never an `insta` run
//!   (see [`render_snapshot`]'s doc comment on why).
//! - **(b) `[contract]`**: `expected_framework`, `min_status`,
//!   `min_subcommands`, `must_contain_flags` (see [`check_contract`]).
//! - **(c) Strict xfail**: a fixture marked `[xfail]` whose snapshot and
//!   contract *both* pass fails the run — the bug got fixed and the
//!   fixture must be promoted (`corpus/README.md`'s lifecycle rules).
//! - **(d) Parse-time ceiling**: [`MAX_FIXTURE_PARSE_TIME`], applied to
//!   every fixture regardless of xfail status — a slow parse is a bug
//!   (AGENTS.md's "never call an O(n)-or-worse function from inside a
//!   loop's own condition" entry, the 153-second incident) whether or not
//!   the fixture's *content* is expected to be broken.

use mandible_core::CommandNode;
use mandible_extract::exec::{ExecOutput, Transcript};
use mandible_extract::{default_tiers_with_probe, ResolvedTool, Runner};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Spec's mechanical net for the O(n²)-in-a-loop class of bug (AGENTS.md:
/// a genuinely degenerate input once took 153s instead of milliseconds).
/// Deliberately coarse — 100ms is nowhere near what a single-fixture
/// in-memory parse should ever cost, so this never fires on ordinary
/// millisecond-scale variance and exists only to catch the next
/// accidental quadratic loop before it ships.
const MAX_FIXTURE_PARSE_TIME: Duration = Duration::from_millis(100);

/// Bound on the recursive per-fixture tree fill (see this module's doc
/// comment), mirroring spec §5.2's whole-tree warming cap. No shipped
/// fixture comes close; this exists so a future pathological fixture
/// (or a bug that makes a tier report a node as its own child) can't spin
/// the runner forever.
const MAX_FIXTURE_NODES: usize = 4096;

/// One `[[capture]]` entry in `meta.toml` (`corpus/README.md`): the real
/// argv a contributor would type, and where its captured bytes live,
/// relative to the fixture's own directory.
#[derive(Debug, Clone, Deserialize)]
struct CaptureMeta {
    /// The full command line, argv[0] included, e.g. `["git", "--help"]`.
    /// [`Transcript`] keys on [`mandible_extract::exec::InertArgv::args`],
    /// which excludes argv[0] — see [`Fixture::build_transcript`] for
    /// where that gets stripped.
    argv: Vec<String>,
    /// Path (relative to the fixture directory) to the captured stdout.
    stdout: String,
    /// Path (relative to the fixture directory) to the captured stderr,
    /// omitted when the capture produced none.
    #[serde(default)]
    stderr: Option<String>,
    /// The captured exit code. Defaults to 0 (the overwhelmingly common
    /// case — most `--help` invocations exit cleanly), so a well-behaved
    /// capture doesn't need to spell it out.
    #[serde(default)]
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolMeta {
    name: String,
    #[allow(dead_code)] // descriptive metadata; not consulted by the runner
    version: String,
    #[allow(dead_code)]
    #[serde(default)]
    platform: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    captured_with: Option<String>,
}

/// `[contract]`: the normative half of a fixture (`corpus/README.md`) —
/// promises that may only weaken via an explicit, reviewable edit to this
/// file. Every field is optional; an absent field asserts nothing.
#[derive(Debug, Clone, Default, Deserialize)]
struct ContractMeta {
    #[serde(default)]
    expected_framework: Option<String>,
    #[serde(default)]
    min_status: Option<String>,
    #[serde(default)]
    min_subcommands: Option<usize>,
    #[serde(default)]
    must_contain_flags: Vec<String>,
}

/// `[xfail]`: present only while the fixture's bug is unfixed.
#[derive(Debug, Clone, Deserialize)]
struct XfailMeta {
    broken: bool,
    #[allow(dead_code)] // surfaced to a human reading meta.toml, not checked
    #[serde(default)]
    reason: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    issue: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Meta {
    tool: ToolMeta,
    #[serde(default, rename = "capture")]
    captures: Vec<CaptureMeta>,
    #[serde(default)]
    contract: ContractMeta,
    #[serde(default)]
    xfail: Option<XfailMeta>,
}

/// One discovered `corpus/<tool>/<version>/` fixture.
struct Fixture {
    /// `corpus/<tool>/<version>` — used for error messages and to resolve
    /// `expected.snap` and every capture's file path.
    dir: PathBuf,
    /// `<tool>/<version>`, for report labels.
    label: String,
    meta: Meta,
}

impl Fixture {
    fn expected_snap_path(&self) -> PathBuf {
        self.dir.join("expected.snap")
    }

    /// Build the [`Transcript`] this fixture's tiers will replay against,
    /// reading every `[[capture]]`'s stdout/stderr bytes from disk and
    /// stripping argv[0] before using the rest as the transcript key —
    /// [`Transcript`] keys on the real argument vector a tier sends
    /// ([`mandible_extract::exec::InertArgv::args`]), which never includes
    /// the tool name itself. `corpus/README.md` documents this same
    /// stripping rule for anyone reading `meta.toml` by hand.
    fn build_transcript(&self) -> anyhow::Result<Transcript> {
        let mut pairs = Vec::with_capacity(self.meta.captures.len());
        for capture in &self.meta.captures {
            if capture.argv.is_empty() {
                anyhow::bail!(
                    "{}: a [[capture]] entry's argv must include the tool name (argv[0])",
                    self.label
                );
            }
            let key = capture.argv[1..].to_vec();
            let stdout = read_capture_file(&self.dir, &capture.stdout)?;
            let stderr = match &capture.stderr {
                Some(name) => read_capture_file(&self.dir, name)?,
                None => Vec::new(),
            };
            pairs.push((
                key,
                ExecOutput {
                    stdout,
                    stderr,
                    exit_code: Some(capture.exit_code.unwrap_or(0)),
                    timed_out: false,
                },
            ));
        }
        Ok(Transcript::new(pairs))
    }

    /// A synthetic [`ResolvedTool`]: a fixture never corresponds to a real
    /// path on the machine running the corpus suite (that's the entire
    /// point of replaying from frozen bytes instead of `PATH`), so this
    /// fabricates one exactly the way `mandible-extract`'s own
    /// transcript-replay tests do (see
    /// `help_text::mod::extract_node_replays_from_a_transcript_keyed_on_the_real_argv`)
    /// — every detecting tier only ever checks `path.is_some()`, never
    /// that the path resolves to a real file.
    fn resolved_tool(&self) -> ResolvedTool {
        ResolvedTool {
            name: self.meta.tool.name.clone(),
            path: Some(PathBuf::from(format!(
                "/corpus-replay/{}",
                self.meta.tool.name
            ))),
            version: None,
        }
    }
}

fn read_capture_file(fixture_dir: &Path, relative: &str) -> anyhow::Result<Vec<u8>> {
    let path = fixture_dir.join(relative);
    std::fs::read(&path).map_err(|e| anyhow::anyhow!("reading capture {}: {e}", path.display()))
}

/// Discover every `corpus/<tool>/<version>/meta.toml` under `corpus_root`,
/// in a deterministic (sorted) order so a report is diffable run to run.
fn discover_fixtures(corpus_root: &Path) -> anyhow::Result<Vec<Fixture>> {
    let mut out = Vec::new();
    let mut tool_dirs: Vec<PathBuf> = std::fs::read_dir(corpus_root)
        .map_err(|e| anyhow::anyhow!("reading corpus root {}: {e}", corpus_root.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    tool_dirs.sort();

    for tool_dir in tool_dirs {
        let mut version_dirs: Vec<PathBuf> = std::fs::read_dir(&tool_dir)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", tool_dir.display()))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .map(|entry| entry.path())
            .collect();
        version_dirs.sort();

        for dir in version_dirs {
            let meta_path = dir.join("meta.toml");
            if !meta_path.is_file() {
                continue;
            }
            let raw = std::fs::read_to_string(&meta_path)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", meta_path.display()))?;
            let meta: Meta = toml::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("parsing {}: {e}", meta_path.display()))?;
            let label = format!(
                "{}/{}",
                tool_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            out.push(Fixture { dir, label, meta });
        }
    }
    Ok(out)
}

/// Extract a fixture's full tree: root extraction, then a bounded
/// recursive fill into every discovered subcommand (see this module's doc
/// comment). Returns `None` when no tier produced a root at all.
fn extract_tree(runner: &Runner, resolved: &ResolvedTool) -> Option<CommandNode> {
    let result = runner.extract_full_for(resolved);
    let root = result.root?;
    let mut budget = MAX_FIXTURE_NODES.saturating_sub(1);
    Some(warm(
        runner,
        resolved,
        root,
        std::slice::from_ref(&resolved.name),
        &mut budget,
    ))
}

fn warm(
    runner: &Runner,
    resolved: &ResolvedTool,
    mut node: CommandNode,
    path: &[String],
    budget: &mut usize,
) -> CommandNode {
    let children = std::mem::take(&mut node.subcommands);
    let mut filled = Vec::with_capacity(children.len());
    for child in children {
        if *budget == 0 {
            filled.push(child);
            continue;
        }
        *budget -= 1;
        let mut child_path = path.to_vec();
        child_path.push(child.name.clone());
        let fill = runner.fill_node(resolved, &child_path, child);
        filled.push(warm(runner, resolved, fill.node, &child_path, budget));
    }
    node.subcommands = filled;
    node
}

/// Render `node` in exactly the format `expected.snap` fixtures use:
/// `mandible_core::to_snapshot` through plain `serde_yaml::to_string`,
/// **not** `insta`'s snapshot macro. `insta` prepends a `source:`/
/// `expression:` header meant for its own review workflow, which a CLI
/// binary can't drive sanely (`insta`'s harness assumes a test binary and
/// per-test dynamic snapshot paths) — see the corpus work order's design
/// decision on why this crate does a plain file compare instead. Using
/// the same `to_snapshot` + `serde_yaml` pair the format is defined by
/// (`mandible_core::snapshot`'s doc comment) is what keeps this single-
/// sourced rather than a second, driftable serialization.
fn render_snapshot(node: &CommandNode) -> anyhow::Result<String> {
    let snapshot = mandible_core::to_snapshot(node);
    serde_yaml::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serializing snapshot: {e}"))
}

/// The result of comparing a fixture's freshly-rendered tree against its
/// `expected.snap`.
enum SnapshotCheck {
    /// No tier produced a root at all — there is nothing to compare.
    NoRoot,
    /// `expected.snap` doesn't exist. Legal only for an `[xfail]` fixture
    /// (`corpus/README.md` step 4: "a fixture marked broken has no
    /// expected tree yet").
    Missing,
    /// Byte-identical.
    Match,
    /// Differs, starting at the given 1-indexed line.
    Mismatch {
        line: usize,
        expected: String,
        actual: String,
    },
}

fn check_snapshot(fixture: &Fixture, root: Option<&CommandNode>) -> anyhow::Result<SnapshotCheck> {
    let Some(root) = root else {
        return Ok(SnapshotCheck::NoRoot);
    };
    let snap_path = fixture.expected_snap_path();
    if !snap_path.is_file() {
        return Ok(SnapshotCheck::Missing);
    }
    let expected = std::fs::read_to_string(&snap_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", snap_path.display()))?;
    let actual = render_snapshot(root)?;
    if expected == actual {
        return Ok(SnapshotCheck::Match);
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let first_diff = expected_lines
        .iter()
        .zip(actual_lines.iter())
        .position(|(e, a)| e != a)
        .unwrap_or_else(|| expected_lines.len().min(actual_lines.len()));
    Ok(SnapshotCheck::Mismatch {
        line: first_diff + 1,
        expected: expected_lines
            .get(first_diff)
            .unwrap_or(&"<end of file>")
            .to_string(),
        actual: actual_lines
            .get(first_diff)
            .unwrap_or(&"<end of file>")
            .to_string(),
    })
}

/// A single `[contract]` field that failed, human-readable and naming the
/// actual value alongside what was required — spec's own example of a
/// good failure message (`corpus/README.md`'s companion work order):
/// "git: min_subcommands 20, got 23 — OK; snapshot mismatch at
/// .positionals[1].name".
struct ContractFailure(String);

/// Check every field the `[contract]` sets against `root`, returning one
/// [`ContractFailure`] per violated field (empty = every check that was
/// actually specified passed). A field left unset in `meta.toml` asserts
/// nothing and is silently skipped.
fn check_contract(contract: &ContractMeta, root: Option<&CommandNode>) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    let Some(root) = root else {
        // No root at all trivially fails every contract field that was
        // actually specified — name them all rather than one opaque
        // "no root" line, so the report reads the same shape whether the
        // failure is "wrong tree" or "no tree".
        if contract.expected_framework.is_some() {
            failures.push(ContractFailure(
                "expected_framework: no root produced".into(),
            ));
        }
        if contract.min_status.is_some() {
            failures.push(ContractFailure("min_status: no root produced".into()));
        }
        if contract.min_subcommands.is_some() {
            failures.push(ContractFailure("min_subcommands: no root produced".into()));
        }
        if !contract.must_contain_flags.is_empty() {
            failures.push(ContractFailure(
                "must_contain_flags: no root produced".into(),
            ));
        }
        return failures;
    };

    if let Some(expected) = &contract.expected_framework {
        let actual = root
            .detected_framework
            .clone()
            .unwrap_or_else(|| "generic".to_string());
        if &actual != expected {
            failures.push(ContractFailure(format!(
                "expected_framework: expected {expected:?}, got {actual:?}"
            )));
        }
    }

    if let Some(min_status) = &contract.min_status {
        let result_stub = extraction_result_stub(root.clone());
        let status = crate::status::compute(&result_stub);
        if !crate::status::meets_min_status(status.label, min_status) {
            failures.push(ContractFailure(format!(
                "min_status: required at least {min_status:?}, got {:?}",
                status.label
            )));
        }
    }

    if let Some(min) = contract.min_subcommands {
        let got = root.subcommands.len();
        if got < min {
            failures.push(ContractFailure(format!(
                "min_subcommands: required at least {min}, got {got}"
            )));
        }
    }

    let missing_flags: Vec<&str> = contract
        .must_contain_flags
        .iter()
        .filter(|spec| !flag_present(root, spec))
        .map(|s| s.as_str())
        .collect();
    if !missing_flags.is_empty() {
        failures.push(ContractFailure(format!(
            "must_contain_flags: missing {}",
            missing_flags.join(", ")
        )));
    }

    failures
}

/// Wrap a root already produced by [`extract_tree`] back into an
/// [`mandible_extract::ExtractionResult`] shape so [`crate::status::compute`]
/// (which the coverage harness also drives, spec's "one status
/// definition" requirement) can be reused here without a second
/// implementation. `tier_statuses`/`tool`/`elapsed` are irrelevant to
/// `status::compute`, which only ever looks at `root`.
fn extraction_result_stub(root: CommandNode) -> mandible_extract::ExtractionResult {
    mandible_extract::ExtractionResult {
        tool: root.name.clone(),
        root: Some(root),
        tier_statuses: Vec::new(),
        elapsed: Duration::ZERO,
    }
}

/// Whether `root`'s own flags satisfy a `must_contain_flags` spec:
/// `--long-name` matches [`mandible_core::Flag::long`], `-x` matches
/// [`mandible_core::Flag::short`], anything else is matched against
/// `long` verbatim. Root-level only, not recursive — `must_contain_flags`
/// documents what a tool *publishes at its root* (spec's git example:
/// `--paginate`, `--git-dir`), not "somewhere in the whole tree".
fn flag_present(root: &CommandNode, spec: &str) -> bool {
    if let Some(long) = spec.strip_prefix("--") {
        root.flags.iter().any(|f| f.long.as_deref() == Some(long))
    } else if let Some(short) = spec.strip_prefix('-') {
        short
            .chars()
            .next()
            .is_some_and(|c| root.flags.iter().any(|f| f.short == Some(c)))
    } else {
        root.flags.iter().any(|f| f.long.as_deref() == Some(spec))
    }
}

/// One fixture's outcome, for the runner's summary counters.
enum Outcome {
    /// Not marked `[xfail]`, and every check passed.
    Green,
    /// Marked `[xfail]`, and at least one check failed as expected.
    XfailAsExpected,
    /// A real failure: either a non-`[xfail]` fixture with a failing
    /// check, a mis-set-up fixture (no snapshot and no `[xfail]`), or a
    /// strict-xfail violation (an `[xfail]` fixture where every check now
    /// passes).
    Failed(String),
}

/// Run the corpus suite. `bless` rewrites every fixture's `expected.snap`
/// to match its freshly-extracted tree instead of checking it — see this
/// module's doc comment on why blessing an `[xfail]` fixture is legal
/// (spec's documented promotion workflow blesses *before* removing
/// `[xfail]`, and the strict-xfail check on the next plain run is what
/// then reminds a contributor to remove it).
pub fn run(corpus_root: &Path, bless: bool) -> anyhow::Result<CorpusReport> {
    let fixtures = discover_fixtures(corpus_root)?;
    if fixtures.is_empty() {
        anyhow::bail!(
            "no fixtures found under {} (expected corpus/<tool>/<version>/meta.toml)",
            corpus_root.display()
        );
    }

    let mut lines = Vec::new();
    let mut outcomes = Vec::new();
    let mut timing_violations = Vec::new();

    for fixture in &fixtures {
        let transcript = fixture.build_transcript()?;
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let resolved = fixture.resolved_tool();

        let start = Instant::now();
        let root = extract_tree(&runner, &resolved);
        let elapsed = start.elapsed();

        if elapsed > MAX_FIXTURE_PARSE_TIME {
            timing_violations.push(format!(
                "{}: parsed in {:?}, exceeding the {:?} ceiling",
                fixture.label, elapsed, MAX_FIXTURE_PARSE_TIME
            ));
        }

        if bless {
            match &root {
                Some(root) => {
                    let rendered = render_snapshot(root)?;
                    std::fs::write(fixture.expected_snap_path(), &rendered).map_err(|e| {
                        anyhow::anyhow!("writing {}: {e}", fixture.expected_snap_path().display())
                    })?;
                    lines.push(format!("blessed {} ({:?})", fixture.label, elapsed));
                }
                None => {
                    lines.push(format!(
                        "{}: no root produced — nothing to bless",
                        fixture.label
                    ));
                }
            }
            continue;
        }

        let snapshot_check = check_snapshot(fixture, root.as_ref())?;
        let contract_failures = check_contract(&fixture.meta.contract, root.as_ref());
        let is_xfail = fixture.meta.xfail.as_ref().is_some_and(|x| x.broken);

        // A missing `expected.snap` is legal only for a fixture still
        // marked `[xfail]` (corpus/README.md step 4: "a fixture marked
        // broken has no expected tree yet"). For anything else it's a
        // real gap — a "green" fixture with no pinned tree asserts
        // nothing about structure at all, defeating the ratchet.
        let snapshot_ok = match &snapshot_check {
            SnapshotCheck::Match => true,
            SnapshotCheck::Missing => is_xfail,
            SnapshotCheck::NoRoot | SnapshotCheck::Mismatch { .. } => false,
        };
        let all_pass = snapshot_ok && contract_failures.is_empty();

        let mut detail = Vec::new();
        for failure in &contract_failures {
            detail.push(format!("contract: {}", failure.0));
        }
        match &snapshot_check {
            SnapshotCheck::Match => detail.push("snapshot: match".to_string()),
            SnapshotCheck::Missing if is_xfail => {
                detail.push("snapshot: none yet (legal while [xfail])".to_string())
            }
            SnapshotCheck::Missing => detail.push(
                "snapshot: missing expected.snap (required unless marked [xfail])".to_string(),
            ),
            SnapshotCheck::NoRoot => {
                detail.push("snapshot: no root produced by any tier".to_string())
            }
            SnapshotCheck::Mismatch {
                line,
                expected,
                actual,
            } => detail.push(format!(
                "snapshot mismatch at expected.snap:{line}: expected `{expected}` got `{actual}`"
            )),
        }

        let outcome = if is_xfail {
            if all_pass {
                // The promote message belongs in `detail` too, not just
                // the returned `Outcome::Failed` message — otherwise it
                // only ever reaches `CorpusReport::failures` and never
                // the per-fixture line this loop prints, which is
                // supposed to be the "name the tool and what broke" text
                // a human actually reads.
                detail.push(
                    "[xfail] but every check now passes — the bug appears fixed; promote it \
                     (remove [xfail], commit expected.snap if it isn't already)"
                        .to_string(),
                );
                Outcome::Failed(format!("{}: {}", fixture.label, detail.join("; ")))
            } else {
                Outcome::XfailAsExpected
            }
        } else if all_pass {
            Outcome::Green
        } else {
            Outcome::Failed(format!("{}: {}", fixture.label, detail.join("; ")))
        };

        let status_word = match &outcome {
            Outcome::Green => "ok",
            Outcome::XfailAsExpected => "xfail (as expected)",
            Outcome::Failed(_) => "FAIL",
        };
        lines.push(format!(
            "{:<24} {:<20} ({:?})  {}",
            fixture.label,
            status_word,
            elapsed,
            detail.join("; ")
        ));
        outcomes.push(outcome);
    }

    let failed_count = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Failed(_)))
        .count();
    let mut failures: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Failed(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    failures.extend(timing_violations.iter().cloned());

    let green = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Green))
        .count();
    let xfail = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::XfailAsExpected))
        .count();

    lines.push(String::new());
    if bless {
        lines.push(format!("blessed {} fixture(s)", fixtures.len()));
    } else {
        lines.push(format!(
            "{} fixture(s): {green} ok, {xfail} xfail (as expected), {failed_count} failed",
            fixtures.len(),
        ));
        for violation in &timing_violations {
            lines.push(format!("TIMING: {violation}"));
        }
    }

    Ok(CorpusReport {
        text: lines.join("\n"),
        failures,
        bless,
    })
}

/// The outcome of a full corpus run.
pub struct CorpusReport {
    /// Human-readable per-fixture results plus a summary line.
    pub text: String,
    /// Every reason the run should fail (contract/snapshot/strict-xfail
    /// violations, plus any parse-time-ceiling violations), empty when
    /// everything is clean. Always empty in `--bless` mode.
    pub failures: Vec<String>,
    bless: bool,
}

impl CorpusReport {
    /// True when this run should exit non-zero.
    pub fn failed(&self) -> bool {
        !self.bless && !self.failures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A minimal but complete two-fixture corpus in a temp dir: one clean
    /// green fixture and one deliberately-still-broken xfail fixture, both
    /// using the generic layout parser (a plain `Usage:`/`Options:`/
    /// `Commands:` shape any framework-less tool would produce) so the
    /// test needs no real tool's captured bytes.
    struct TestCorpus {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// `mytool --help`: one flag, one subcommand block under a recognized
    /// heading (spec §7 Tier B rule 1) so it parses as real structure, not
    /// verbatim.
    const MYTOOL_HELP: &str = "Usage: mytool [OPTIONS] <COMMAND>\n\nOptions:\n  -v, --verbose   be noisy\n\nCommands:\n  run    run the thing\n";

    fn green_fixture(root: &Path) {
        let dir = root.join("mytool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "mytool"
version = "1.0"

[[capture]]
argv = ["mytool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "ok"
min_subcommands = 1
must_contain_flags = ["--verbose"]
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
    }

    fn broken_xfail_fixture(root: &Path) {
        let dir = root.join("brokentool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "brokentool"
version = "1.0"

[[capture]]
argv = ["brokentool", "--help"]
stdout = "help.txt"

[contract]
min_subcommands = 999

[xfail]
broken = true
reason = "deliberately impossible contract, for the runner's own tests"
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
    }

    fn setup() -> TestCorpus {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        TestCorpus { _dir: dir, root }
    }

    #[test]
    fn green_fixture_with_bless_then_check_round_trips_clean() {
        let corpus = setup();
        green_fixture(&corpus.root);

        let blessed = run(&corpus.root, true).expect("bless run succeeds");
        assert!(!blessed.failed());
        assert!(corpus.root.join("mytool/1.0/expected.snap").is_file());

        let checked = run(&corpus.root, false).expect("check run succeeds");
        assert!(
            !checked.failed(),
            "freshly-blessed fixture must check clean: {}",
            checked.text
        );
    }

    #[test]
    fn xfail_fixture_that_still_fails_does_not_fail_the_run() {
        let corpus = setup();
        broken_xfail_fixture(&corpus.root);
        let report = run(&corpus.root, false).expect("check run succeeds");
        assert!(
            !report.failed(),
            "an xfail fixture that still fails its contract must not fail the run: {}",
            report.text
        );
    }

    /// Strict xfail, the direction that matters: an `[xfail]` fixture
    /// whose contract and snapshot both now pass must fail the run.
    #[test]
    fn xfail_fixture_that_now_passes_fails_the_run() {
        let corpus = setup();
        let dir = corpus.root.join("fixedtool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "fixedtool"
version = "1.0"

[[capture]]
argv = ["fixedtool", "--help"]
stdout = "help.txt"

[contract]
min_subcommands = 1

[xfail]
broken = true
reason = "was broken; this test fixture is deliberately no longer broken"
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        // Bless first (spec's documented promotion order: accept the
        // snapshot, *then* the strict-xfail check on the next plain run
        // is what tells the contributor to remove [xfail]).
        run(&corpus.root, true).expect("bless run succeeds");

        let report = run(&corpus.root, false).expect("check run succeeds");
        assert!(
            report.failed(),
            "an xfail fixture whose checks now all pass must fail the run"
        );
        assert!(report
            .failures
            .iter()
            .any(|f| f.contains("fixedtool") && f.contains("promote")));
    }

    #[test]
    fn missing_expected_snap_without_xfail_is_a_run_failure() {
        let corpus = setup();
        let dir = corpus.root.join("misconfigured/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "misconfigured"
version = "1.0"

[[capture]]
argv = ["misconfigured", "--help"]
stdout = "help.txt"
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        let report = run(&corpus.root, false).expect("check run succeeds");
        assert!(
            report.failed(),
            "a non-[xfail] fixture with no expected.snap asserts nothing about structure at \
             all, which defeats the ratchet — it must fail the run rather than pass silently: {}",
            report.text
        );
        assert!(report
            .failures
            .iter()
            .any(|f| f.contains("misconfigured") && f.contains("expected.snap")));
    }

    /// The transcript key strips argv[0] — a capture with the tool name
    /// left in place in `argv` but a tier that (correctly) never sends
    /// argv[0] to `Transcript::run` must still hit.
    #[test]
    fn transcript_key_strips_argv_zero() {
        let corpus = setup();
        green_fixture(&corpus.root);
        let fixtures = discover_fixtures(&corpus.root).unwrap();
        let fixture = fixtures.iter().find(|f| f.label == "mytool/1.0").unwrap();
        let transcript = fixture.build_transcript().unwrap();
        let keys: Vec<Vec<String>> = transcript.argvs().cloned().collect();
        assert_eq!(keys, vec![vec!["--help".to_string()]]);
    }

    /// Recursive fill: a root capture whose subcommand block names `sub`,
    /// plus a second `[[capture]]` for `mytool sub --help` carrying its
    /// own flag, must appear in the final tree with that flag present —
    /// proving `extract_tree`'s recursive `fill_node` walk actually
    /// reaches captured subcommands, not just the root.
    #[test]
    fn recursive_fill_picks_up_a_captured_subcommand() {
        let corpus = setup();
        let dir = corpus.root.join("deeptool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "deeptool"
version = "1.0"

[[capture]]
argv = ["deeptool", "--help"]
stdout = "help.txt"

[[capture]]
argv = ["deeptool", "sub", "--help"]
stdout = "help-sub.txt"
"#,
        );
        write(
            &dir.join("help.txt"),
            "Usage: deeptool <COMMAND>\n\nCommands:\n  sub    does a thing\n",
        );
        write(
            &dir.join("help-sub.txt"),
            "Usage: deeptool sub [OPTIONS]\n\nOptions:\n  --deep   a subcommand-only flag\n",
        );

        let fixtures = discover_fixtures(&corpus.root).unwrap();
        let fixture = fixtures.iter().find(|f| f.label == "deeptool/1.0").unwrap();
        let transcript = fixture.build_transcript().unwrap();
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let root = extract_tree(&runner, &fixture.resolved_tool())
            .expect("deeptool's root parses to real structure");

        assert_eq!(root.subcommands.len(), 1, "{root:?}");
        let sub = &root.subcommands[0];
        assert_eq!(sub.name, "sub");
        assert!(
            sub.flags.iter().any(|f| f.long.as_deref() == Some("deep")),
            "the recursive fill must have picked up sub's own captured --help: {sub:?}"
        );
    }

    #[test]
    fn discover_fixtures_finds_both_seed_fixtures_in_the_real_corpus() {
        // The real corpus/, not a temp dir — proves the two shipped
        // fixtures are actually discoverable by name, not just present as
        // files.
        let real_corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
        let fixtures = discover_fixtures(&real_corpus).expect("real corpus/ parses");
        let labels: Vec<&str> = fixtures.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.starts_with("tar/")), "{labels:?}");
        assert!(labels.iter().any(|l| l.starts_with("git/")), "{labels:?}");
    }
}
