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
//!   `min_subcommands`, `must_contain_flags`, `must_contain_flags_by_path`
//!   (see [`check_contract`]).
//! - **(c) Strict xfail**: a fixture marked `[xfail]` whose snapshot and
//!   contract *both* pass fails the run — the bug got fixed and the
//!   fixture must be promoted (`corpus/README.md`'s lifecycle rules).
//! - **(d) Parse-time ceiling**: [`MAX_FIXTURE_PARSE_TIME`], applied to
//!   every fixture regardless of xfail status — a slow parse is a bug
//!   (AGENTS.md's "never call an O(n)-or-worse function from inside a
//!   loop's own condition" entry, the 153-second incident) whether or not
//!   the fixture's *content* is expected to be broken. **This one warns
//!   rather than failing the run**; it is the only wall-clock-dependent
//!   check here, and [`MAX_FIXTURE_PARSE_TIME`] explains why that
//!   distinction is load-bearing rather than a softening.
//!
//! Checks (a) through (c) block. Check (d) warns. That split is the whole
//! reason this gate can be strict at all: everything that blocks is
//! deterministic, so a red is always a real parser change and never the
//! shared runner having a bad afternoon.

use crate::coverage::ScoreFormat;
use mandible_core::{CommandNode, Flag, Provenance, Source, Text};
use mandible_extract::exec::{ExecOutput, Transcript};
use mandible_extract::{default_tiers_with_probe, ResolvedTool, Runner};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Spec's mechanical net for the O(n²)-in-a-loop class of bug (AGENTS.md:
/// a genuinely degenerate input once took 153s instead of milliseconds).
/// Deliberately coarse — 100ms is nowhere near what a single-fixture
/// in-memory parse should ever cost, so this never fires on ordinary
/// millisecond-scale variance and exists only to catch the next
/// accidental quadratic loop before it ships.
///
/// **This check warns; it does not fail the run.** It is the one
/// nondeterministic ingredient in an otherwise fully deterministic runner —
/// every other check reads frozen bytes and spawns nothing, so a red is
/// always a real parser change, which is what earns this gate the right to
/// block where the live PATH sweep does not. Wall-clock on shared CI
/// hardware is not that. Measured on this project's own runners:
/// `waagent2.0` took 41.9s in one sweep and 21.4s in the next **with
/// identical code**, roughly 40x contention — so the ~100x headroom this
/// ceiling has over a real ~1ms fixture parse is a thinner margin than it
/// looks.
///
/// The asymmetry decides it. A missed quadratic is caught by the next run,
/// by the sweep's own `ms` column, or by a warning nobody could miss (the
/// motivating incident was 153,000ms against a 100ms line — it does not
/// arrive as a borderline reading). One false red on timing, by contrast,
/// teaches people that a red corpus run can be ignored, and that habit
/// costs the correctness checks their authority. Blocking on flaky
/// wall-clock would reproduce, inside the one gate immune to it, the exact
/// failure `path-sweep.yml` is deliberately non-gating to avoid.
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
    /// Same spot-check as `must_contain_flags`, but for a subcommand's own
    /// flags rather than the root's — keyed by the subcommand's path,
    /// space-separated and *not* including the tool's own name (`"restore"`,
    /// `"remote add"`). General on purpose: the key is a path any tool's
    /// tree can have, never a tool name, so this generalizes the same way
    /// `must_contain_flags` does rather than adding a git-specific check.
    /// Exists because `must_contain_flags` alone can only ever assert what
    /// a tool publishes at its *root* (`flag_present`'s doc comment) — most
    /// of what a fix like [M-16]'s `-h` fallback or a flag-spec grammar fix
    /// changes shows up on a subcommand instead.
    #[serde(default)]
    must_contain_flags_by_path: std::collections::BTreeMap<String, Vec<String>>,
    /// Which dimensions of this fixture's tree a human actually verified
    /// before blessing it — the machine-readable replacement for the
    /// "SCOPE OF REVIEW" prose comment 36 `audit-seed2` fixtures each
    /// carried a copy of (`git show c9bfe76`). `check_contract` never
    /// reads this: it is not itself a check, it is a record of which
    /// checks a human *stood in for* by eye when they blessed the
    /// snapshot — `expected.snap` freezes every field regardless
    /// (`corpus/README.md`: "a full expected.snap freezes everything"),
    /// so without this, a passing fixture cannot be told apart from one
    /// whose flags were reviewed but whose descriptions never were. It
    /// is carried into every report (`show_fixture`, the text and
    /// markdown corpus reports) so that question is answered by reading
    /// data, not by grepping `meta.toml` prose.
    ///
    /// **Absent means "no scope claimed", not "full scope".** A blessed
    /// snapshot always freezes every field whether or not a human looked
    /// at it, so treating silence as "everything verified" would let the
    /// exact overclaim this field exists to prevent survive by omission
    /// — the lsof cautionary tale (`corpus/README.md`) was precisely a
    /// blessed tree nobody had actually read. An empty scope makes the
    /// weakest possible claim (none at all), which is the only safe
    /// default when the entire point is to never overclaim. See
    /// `verdict_scope_defaults_to_empty_when_absent`.
    #[serde(default)]
    verdict_scope: Vec<VerdictScope>,
}

/// One dimension of a fixture's tree that a `verdict_scope` entry can
/// claim was actually reviewed by a human before the fixture was
/// blessed. `Flags` and `Subcommands` are the two the seed-2 audit
/// workflow actually checks (`corpus/README.md`, `git show c9bfe76`);
/// `Descriptions` and `Usage` exist so a fixture whose bless *did*
/// include a prose read — `corpus/README.md`'s own full bless workflow
/// asks a reviewer to check "every flag's description against the line
/// it came from" — has somewhere to say so, rather than the schema
/// hard-coding today's one audit's scope as the only kind of review that
/// can ever be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VerdictScope {
    Flags,
    Subcommands,
    Descriptions,
    Usage,
}

impl VerdictScope {
    fn as_str(self) -> &'static str {
        match self {
            VerdictScope::Flags => "flags",
            VerdictScope::Subcommands => "subcommands",
            VerdictScope::Descriptions => "descriptions",
            VerdictScope::Usage => "usage",
        }
    }
}

/// Render a `verdict_scope` list for a report: the comma-joined
/// dimension names, or `"unscoped"` when the list is empty. Centralized
/// so every surface (`show_fixture`, the text report's per-fixture
/// detail, the markdown table's `scope` column) renders an absent scope
/// the same visible, unmissable way — never blank space that reads the
/// same as "not shown at all", which would quietly defeat the reason
/// this field exists.
fn verdict_scope_label(scope: &[VerdictScope]) -> String {
    if scope.is_empty() {
        "unscoped".to_string()
    } else {
        scope
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
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

/// Contract-weakening detection (this task's process fix #1): "Three
/// separate subagent attempts have weakened a fixture contract to force a
/// pass" — lowering `min_subcommands`, shrinking `must_contain_flags`, or
/// marking a previously-enforced fixture `[xfail]` all make a real failure
/// disappear without fixing anything, and `corpus/README.md` already
/// permits this ("Weakening a contract... only via an explicit edit to
/// `meta.toml`, justified in the PR description") — it must stay legal,
/// it must stop being *quiet*.
///
/// **This module has no git access, on purpose**, and cannot gain any: the
/// workspace-wide `tests/no_process_outside_exec.rs` invariant forbids
/// `std::process`/`Command::new` in `xtask/src` exactly as it does in every
/// other crate but `mandible-extract/src/exec/`, so this runner could never
/// legitimately shell out to `git diff` to see what the *previous* commit's
/// `meta.toml` said. Faking that — inferring "committed" from whatever
/// happens to be on disk right now — would be worse than not checking at
/// all, since a fixture is always compared against itself. The honest
/// alternative implemented here: this function takes a **second, plain
/// corpus directory** (`baseline_root`) and diffs `[contract]` fields
/// between it and the current one — no git anywhere in this process. `git`
/// access belongs to whatever *invokes* this binary, which can
/// legitimately populate that directory however it likes (a CI step
/// running `git archive <base-ref> corpus | tar -x`, e.g.) and pass it via
/// `--baseline-dir`. Nothing here assumes that wiring exists; with no
/// `--baseline-dir`, this function is never called and nothing changes.
///
/// Returns one `"CONTRACT WEAKENED: <fixture> <field>"` line per weakened
/// field, most-recognizable field first within a fixture, fixtures in
/// their usual sorted order. Reported, not gated — see `run`'s doc comment
/// on why this must not fail a build the moment it's introduced: a
/// contract may legitimately weaken (the corpus's own documented lifecycle
/// rule), and this exists to make sure a reviewer sees it, not to
/// second-guess a deliberate, justified edit.
fn contract_weakened_lines(current: &[Fixture], baseline: &[Fixture]) -> Vec<String> {
    let mut lines = Vec::new();
    for base in baseline {
        let Some(now) = current.iter().find(|f| f.label == base.label) else {
            lines.push(format!(
                "CONTRACT WEAKENED: {} fixture-removed (present in baseline, missing now)",
                base.label
            ));
            continue;
        };
        let (b, n) = (&base.meta.contract, &now.meta.contract);

        // A framework assertion that's simply gone is a removed check —
        // never flagged for merely *changing* to a different framework
        // name, since that has no natural "weaker/stronger" ordering and a
        // real detection improvement legitimately changes it.
        if b.expected_framework.is_some() && n.expected_framework.is_none() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} expected_framework (assertion removed)",
                base.label
            ));
        }

        if let Some(base_status) = &b.min_status {
            let base_rank = crate::status::status_rank(base_status);
            let now_rank = n.min_status.as_deref().and_then(crate::status::status_rank);
            if now_rank < base_rank {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} min_status ({:?} -> {:?})",
                    base.label,
                    base_status,
                    n.min_status.as_deref().unwrap_or("(removed)"),
                ));
            }
        }

        if let Some(base_min) = b.min_subcommands {
            let now_min = n.min_subcommands.unwrap_or(0);
            if now_min < base_min {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} min_subcommands ({base_min} -> {now_min})",
                    base.label
                ));
            }
        }

        let missing_flags: Vec<&str> = b
            .must_contain_flags
            .iter()
            .filter(|spec| !n.must_contain_flags.iter().any(|s| s == *spec))
            .map(String::as_str)
            .collect();
        if !missing_flags.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_contain_flags (dropped: {})",
                base.label,
                missing_flags.join(", ")
            ));
        }

        for (path, base_specs) in &b.must_contain_flags_by_path {
            let now_specs = n.must_contain_flags_by_path.get(path);
            let missing: Vec<&str> = base_specs
                .iter()
                .filter(|spec| !now_specs.is_some_and(|specs| specs.iter().any(|s| s == *spec)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} must_contain_flags_by_path[{path:?}] (dropped: {})",
                    base.label,
                    missing.join(", ")
                ));
            }
        }

        let base_xfail = base.meta.xfail.as_ref().is_some_and(|x| x.broken);
        let now_xfail = now.meta.xfail.as_ref().is_some_and(|x| x.broken);
        if !base_xfail && now_xfail {
            lines.push(format!(
                "CONTRACT WEAKENED: {} xfail (newly marked broken — contract failures no longer fail the run)",
                base.label
            ));
        }
    }
    lines
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
        if !contract.must_contain_flags_by_path.is_empty() {
            failures.push(ContractFailure(
                "must_contain_flags_by_path: no root produced".into(),
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

    for (path, specs) in &contract.must_contain_flags_by_path {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_contain_flags_by_path: no node at path {path:?}"
            )));
            continue;
        };
        let missing: Vec<&str> = specs
            .iter()
            .filter(|spec| !flag_present(node, spec))
            .map(|s| s.as_str())
            .collect();
        if !missing.is_empty() {
            failures.push(ContractFailure(format!(
                "must_contain_flags_by_path[{path:?}]: missing {}",
                missing.join(", ")
            )));
        }
    }

    failures
}

/// Resolve a space-separated subcommand path (`"restore"`, `"remote add"`)
/// against `root`'s own `subcommands`, one path segment per level — the
/// same walk [`mandible_core::noderef::resolve`] does for the TUI's own
/// addressing, reimplemented narrowly here rather than pulled in because
/// this only ever needs a name match, never alias resolution.
fn find_node_by_path<'a>(root: &'a CommandNode, path: &str) -> Option<&'a CommandNode> {
    let mut node = root;
    for segment in path.split_whitespace() {
        node = node.subcommands.iter().find(|c| c.name == segment)?;
    }
    Some(node)
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

/// Whether `node`'s own flags satisfy a `must_contain_flags`/
/// `must_contain_flags_by_path` spec: `--long-name` matches
/// [`mandible_core::Flag::long`], `-x` matches [`mandible_core::Flag::short`],
/// anything else is matched against `long` verbatim. Only ever checks the
/// one node it's given, never recursing into its subcommands itself —
/// `must_contain_flags` calls this with the fixture's root (what a tool
/// *publishes at its root*, spec's git example: `--paginate`, `--git-dir`);
/// `must_contain_flags_by_path` calls it with whatever node
/// [`find_node_by_path`] resolved.
fn flag_present(node: &CommandNode, spec: &str) -> bool {
    if let Some(long) = spec.strip_prefix("--") {
        node.flags.iter().any(|f| f.long.as_deref() == Some(long))
    } else if let Some(short) = spec.strip_prefix('-') {
        short
            .chars()
            .next()
            .is_some_and(|c| node.flags.iter().any(|f| f.short == Some(c)))
    } else {
        node.flags.iter().any(|f| f.long.as_deref() == Some(spec))
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
///
/// `format` selects how a *checking* run (`bless: false`) renders its
/// report: [`ScoreFormat::Text`] is the plain per-fixture lines this
/// module has always produced (unchanged); [`ScoreFormat::Markdown`]
/// additionally builds a semantic before/after transition table (see
/// [`render_markdown_report`]) for `$GITHUB_STEP_SUMMARY`. `format` is
/// ignored while blessing — bless has no "review artifact" to format,
/// only a rewrite-and-report-what-was-written action.
pub fn run(corpus_root: &Path, bless: bool, format: ScoreFormat) -> anyhow::Result<CorpusReport> {
    run_with_baseline(corpus_root, bless, format, None)
}

/// [`run`], additionally comparing every fixture's `[contract]` against
/// `baseline_root` (a second, plain corpus directory — never git) and
/// printing a prominent `CONTRACT WEAKENED: ...` line for every field that
/// got weaker. See [`contract_weakened_lines`] for the full rule and why
/// this takes a directory instead of a git ref. `None` skips the check
/// entirely (unchanged behavior from before this existed) — this is what
/// [`run`] passes, so every existing caller is unaffected.
pub fn run_with_baseline(
    corpus_root: &Path,
    bless: bool,
    format: ScoreFormat,
    baseline_root: Option<&Path>,
) -> anyhow::Result<CorpusReport> {
    run_with_ceiling(
        corpus_root,
        bless,
        format,
        MAX_FIXTURE_PARSE_TIME,
        baseline_root,
    )
}

/// [`run`], with the parse-time ceiling injected rather than taken from
/// [`MAX_FIXTURE_PARSE_TIME`].
///
/// Exists so the warn-don't-block property is testable: a real fixture
/// parses in well under a millisecond, so the only way to exercise a
/// ceiling violation is to lower the ceiling. Without this seam the
/// property could only be checked by editing the constant by hand — which
/// is how it was verified originally, and exactly the kind of unguarded
/// behaviour a later refactor silently reverses.
fn run_with_ceiling(
    corpus_root: &Path,
    bless: bool,
    format: ScoreFormat,
    max_parse_time: Duration,
    baseline_root: Option<&Path>,
) -> anyhow::Result<CorpusReport> {
    let fixtures = discover_fixtures(corpus_root)?;
    if fixtures.is_empty() {
        anyhow::bail!(
            "no fixtures found under {} (expected corpus/<tool>/<version>/meta.toml)",
            corpus_root.display()
        );
    }
    // Contract-weakening lines (see `contract_weakened_lines`'s doc
    // comment) — computed once, up front, from plain `meta.toml` reads
    // only (no extraction, no git), so a failure here (a malformed
    // baseline directory) surfaces before any of the real, expensive work
    // below starts.
    let weakened: Vec<String> = match baseline_root {
        Some(dir) => {
            let baseline_fixtures = discover_fixtures(dir)?;
            contract_weakened_lines(&fixtures, &baseline_fixtures)
        }
        None => Vec::new(),
    };

    let mut lines = Vec::new();
    let mut outcomes = Vec::new();
    let mut timing_violations = Vec::new();
    // Only populated for a checking run (`bless: false`) — see
    // `render_markdown_report`. Cheap to build unconditionally alongside
    // `lines`: every fixture here is a few KiB, never worth gating on.
    let mut fixture_rows: Vec<FixtureRow> = Vec::new();

    for fixture in &fixtures {
        let transcript = fixture.build_transcript()?;
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let resolved = fixture.resolved_tool();

        let start = Instant::now();
        let root = extract_tree(&runner, &resolved);
        let elapsed = start.elapsed();

        if elapsed > max_parse_time {
            timing_violations.push(format!(
                "{}: parsed in {:?}, exceeding the {:?} ceiling",
                fixture.label, elapsed, max_parse_time
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

        // Only when a scope is actually recorded — an unscoped fixture
        // (the overwhelming majority, today) stays silent here exactly
        // as it always has; `show_fixture` is where "unscoped" is spelled
        // out explicitly for a fixture being inspected one at a time.
        // Purely informational: never affects `all_pass`, since a scope
        // claim is a record of what a human checked, not itself a check.
        if !fixture.meta.contract.verdict_scope.is_empty() {
            detail.push(format!(
                "verdict_scope: {}",
                verdict_scope_label(&fixture.meta.contract.verdict_scope)
            ));
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

        fixture_rows.push(FixtureRow {
            label: fixture.label.clone(),
            status_word,
            detail: detail.clone(),
            current: summarize(&fixture.meta.tool.name, root.as_ref()),
            previous: previous_summary(fixture),
            verdict_scope: fixture.meta.contract.verdict_scope.clone(),
        });

        outcomes.push(outcome);
    }

    let failed_count = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Failed(_)))
        .count();
    let failures: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Failed(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    // Deliberately NOT extended with `timing_violations` — the parse-time
    // ceiling warns, it does not fail the run. See
    // [`MAX_FIXTURE_PARSE_TIME`] for why.

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
            // "warning", not a bare label: this line does not fail the
            // run, and a reader who cannot tell that from the output will
            // either chase a non-failure or learn to skim past real ones.
            lines.push(format!("warning: slow parse (does not fail): {violation}"));
        }
    }

    // Markdown only applies to a checking run — see `run`'s doc comment.
    // It fully replaces `lines.join(...)` rather than appending to it: the
    // markdown report is a self-contained artifact (table plus `<details>`
    // for anything capped), not the plain-text report with formatting
    // bolted on.
    let text = if !bless && matches!(format, ScoreFormat::Markdown) {
        render_markdown_report(
            &fixture_rows,
            fixtures.len(),
            green,
            xfail,
            failed_count,
            &weakened,
        )
    } else {
        // Contract-weakening lines go first, ahead of every per-fixture
        // line and the summary — "prominent... so a reviewer skimming a
        // green run cannot miss it" means the very top of the output, not
        // one more line in a footer nobody reads once green.
        let mut prefixed = weakened;
        prefixed.extend(lines);
        prefixed.join("\n")
    };

    Ok(CorpusReport {
        text,
        failures,
        bless,
    })
}

/// One side of a fixture's semantic comparison (spec companion work
/// order's requirement: a report must say *what changed*, never present a
/// text diff) — either the tree `expected.snap` currently pins, or the
/// tree this run just extracted. Built once via [`summarize`] so both
/// sides go through identical logic; no separate "previous" rules to drift
/// from "current" ones.
struct TreeSummary {
    /// [`crate::status::compute`]'s label — the *same* function
    /// `check_contract`'s `min_status` check uses, so a status shown here
    /// can never disagree with what actually gated the run (status.rs's
    /// own doc comment: "two independent definitions of 'status' will
    /// drift, and the drift will be discovered at the worst possible
    /// time").
    status: &'static str,
    nodes: usize,
    flags: usize,
    /// Every subcommand's dotted path (`"bisect start"`, not just
    /// `"start"`), so two same-named subcommands under different parents
    /// are never conflated.
    subcommands: BTreeSet<String>,
    /// Every flag's canonical spelling (`--long` if present, else `-x`).
    flag_names: BTreeSet<String>,
}

/// Build a [`TreeSummary`] for `root` (`None` when no tier produced
/// anything, e.g. a "current" side with no root, or an unresolvable
/// `expected.snap`). `tool` only feeds the status stub's `tool` field,
/// which `crate::status::compute` never actually reads.
fn summarize(tool: &str, root: Option<&CommandNode>) -> TreeSummary {
    let stub = mandible_extract::ExtractionResult {
        tool: tool.to_string(),
        root: root.cloned(),
        tier_statuses: Vec::new(),
        elapsed: Duration::ZERO,
    };
    let status = crate::status::compute(&stub);
    let mut subcommands = BTreeSet::new();
    let mut flag_names = BTreeSet::new();
    if let Some(r) = root {
        collect_subcommand_paths(r, "", &mut subcommands);
        collect_flag_names(r, &mut flag_names);
    }
    TreeSummary {
        status: status.label,
        nodes: root.map(count_nodes).unwrap_or(0),
        flags: root.map(count_flags).unwrap_or(0),
        subcommands,
        flag_names,
    }
}

fn count_nodes(node: &CommandNode) -> usize {
    1 + node.subcommands.iter().map(count_nodes).sum::<usize>()
}

fn count_flags(node: &CommandNode) -> usize {
    node.flags.len() + node.subcommands.iter().map(count_flags).sum::<usize>()
}

fn collect_flag_names(node: &CommandNode, out: &mut BTreeSet<String>) {
    for f in &node.flags {
        if let Some(long) = &f.long {
            out.insert(format!("--{long}"));
        } else if let Some(short) = f.short {
            out.insert(format!("-{short}"));
        }
    }
    for child in &node.subcommands {
        collect_flag_names(child, out);
    }
}

fn collect_subcommand_paths(node: &CommandNode, prefix: &str, out: &mut BTreeSet<String>) {
    for child in &node.subcommands {
        let path = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix} {}", child.name)
        };
        out.insert(path.clone());
        collect_subcommand_paths(child, &path, out);
    }
}

/// Minimal, tolerant mirror of `expected.snap`'s compact YAML shape
/// (`mandible_core::snapshot::NodeSnapshot`), carrying only what
/// [`TreeSummary`] needs. `NodeSnapshot` itself derives only `Serialize`
/// (its own doc comment: the compact form is a one-way review artifact,
/// not a round-trip format — every empty `Vec`/`None` field is omitted
/// entirely), so a direct `Deserialize` of the full IR would fail on
/// "missing field" for nearly every real fixture. `#[serde(default)]` on
/// every field here is what makes an omitted key deserialize back to the
/// same "empty" value its serializer chose not to write, instead of an
/// error.
#[derive(Debug, Clone, Default, Deserialize)]
struct SnapFlag {
    #[serde(default)]
    short: Option<char>,
    #[serde(default)]
    long: Option<String>,
    /// Needed for nothing this module names directly, but
    /// [`crate::status::compute`]'s `pct_flags_with_text` (and therefore the
    /// `low-confidence` vs `ok` status boundary) reads it — omitting it
    /// would make every reconstructed "previous" tree look 0% described
    /// regardless of the real fixture, which would desync `TreeSummary`'s
    /// two sides even when `expected.snap` and the fresh extraction are
    /// byte-identical.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SnapNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    flags: Vec<SnapFlag>,
    #[serde(default)]
    subcommands: Vec<SnapNode>,
    #[serde(default)]
    unparsed: Vec<String>,
    #[serde(default)]
    heading_attested: bool,
}

/// Rebuild a real (if synthetic-provenance) [`CommandNode`] from a
/// [`SnapNode`], so [`summarize`] can run the *exact same* status/count/
/// name-collection logic over `expected.snap`'s tree that it runs over a
/// freshly extracted one — one code path for both sides of the
/// comparison, never a second one hand-written against the compact YAML
/// shape. Every field `SnapNode` doesn't carry (provenance detail,
/// descriptions, positionals, ...) is irrelevant to what [`summarize`]
/// reads and is left at `CommandNode::new`'s defaults.
fn snap_to_command_node(n: &SnapNode) -> CommandNode {
    let mut node = CommandNode::new(n.name.clone(), Provenance::single(Source::HelpText));
    node.summary = n.summary.as_deref().map(Text::sanitize);
    node.heading_attested = n.heading_attested;
    node.unparsed = n.unparsed.iter().map(|s| Text::sanitize(s)).collect();
    node.flags = n
        .flags
        .iter()
        .map(|f| {
            let mut flag = Flag::long("", Provenance::single(Source::HelpText));
            flag.long = f.long.clone();
            flag.short = f.short;
            flag.description = f.description.as_deref().map(Text::sanitize);
            flag
        })
        .collect();
    node.subcommands = n.subcommands.iter().map(snap_to_command_node).collect();
    node
}

/// Read and convert a fixture's `expected.snap` into a [`TreeSummary`],
/// `None` when it doesn't exist yet (legal for an unfixed `[xfail]`
/// fixture, `corpus/README.md` step 4) or fails to parse (a corrupt or
/// hand-edited file — degrade to "no baseline" rather than panicking on
/// tool input, per AGENTS.md's `unwrap()` rule).
fn previous_summary(fixture: &Fixture) -> Option<TreeSummary> {
    let snap_path = fixture.expected_snap_path();
    if !snap_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&snap_path).ok()?;
    let snap: SnapNode = serde_yaml::from_str(&raw).ok()?;
    let converted = snap_to_command_node(&snap);
    Some(summarize(&fixture.meta.tool.name, Some(&converted)))
}

/// One fixture's row in the markdown transition report: the same
/// pass/fail classification and detail lines the text report already
/// computed, plus the two [`TreeSummary`] sides [`render_markdown_report`]
/// diffs.
struct FixtureRow {
    label: String,
    status_word: &'static str,
    /// `contract: ...` / `snapshot: ...` lines, reused to decide which
    /// remedy to name (§ this module's doc comment on `render_markdown_report`).
    detail: Vec<String>,
    current: TreeSummary,
    previous: Option<TreeSummary>,
    /// What a human actually verified before this fixture was blessed
    /// (`ContractMeta::verdict_scope`'s doc comment) — surfaced as the
    /// table's own `scope` column so a reviewer sees, without opening
    /// `meta.toml`, that a green row's descriptions may still be
    /// unreviewed prose.
    verdict_scope: Vec<VerdictScope>,
}

/// Cap on how many names a single markdown table cell shows inline before
/// folding the rest into "+N more" — keeps a large-fixture regression
/// scannable. Not load-bearing (informational only): the complete list is
/// never dropped, only moved into a `<details>` block below the table (see
/// [`capped_names`]), matching `path-sweep.yml`'s own summary step
/// (`<details><summary>Per-tool tables</summary>`) for exactly the same
/// reason — GitHub's step-summary UI renders `<details>` natively, so a
/// reviewer who wants the full list expands it without ever leaving the
/// page, and one who doesn't isn't shown a 1,000-line dump by default.
const MARKDOWN_NAME_CAP: usize = 8;

/// Render `names` as a capped, comma-joined inline string, plus (only when
/// truncated) the complete list for a `<details>` block.
fn capped_names(names: &BTreeSet<String>) -> (String, Option<String>) {
    if names.is_empty() {
        return (String::new(), None);
    }
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    if all.len() <= MARKDOWN_NAME_CAP {
        return (all.join(", "), None);
    }
    let shown = all[..MARKDOWN_NAME_CAP].join(", ");
    let capped = format!("{shown}, +{} more", all.len() - MARKDOWN_NAME_CAP);
    (capped, Some(all.join(", ")))
}

/// Escape the one GFM table-breaking character. Same reasoning and same
/// narrow scope as `coverage::md_escape` — tool/flag/subcommand names are
/// the only free-form content a table cell here ever holds.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Render the corpus run as a GitHub-flavored markdown **transition
/// report** for `$GITHUB_STEP_SUMMARY` — never a text diff of
/// `expected.snap`.
///
/// `corpus/README.md` calls a snapshot mismatch the review step itself:
/// `expected.snap` is descriptive, "subject to snapshot review", and a red
/// run that forces a human to look *is* that review. But a 1,000+ line
/// YAML diff is unreviewable — a reviewer facing one either reads none of
/// it, or worse, learns to approve it unread, which quietly deletes the
/// ratchet this whole harness exists to build (the PATH sweep's flapping
/// history, AGENTS.md, is what an ignored gate costs once trust in it is
/// gone). So instead of lines, this reports **what changed semantically**:
/// status, node/flag counts, and which *named* subcommands/flags appeared
/// or disappeared — the level of detail a reviewer can act on without
/// opening the YAML at all.
///
/// Passing fixtures are included too, compactly (one row, "no change") —
/// a report that only shows failures gives no sense of what the ratchet
/// is actually protecting.
///
/// The remedy named for a failing row depends on *which* check failed, and
/// the two are never blurred: a snapshot mismatch names `--bless` (the
/// descriptive half may be accepted after review); a `[contract]`
/// violation names a deliberate `meta.toml` edit (the normative half may
/// only weaken by an explicit, reviewable change — never by blessing).
fn render_markdown_report(
    rows: &[FixtureRow],
    total: usize,
    green: usize,
    xfail: usize,
    failed: usize,
    weakened: &[String],
) -> String {
    let mut out = String::new();
    // Contract weakening, ahead of even the report's own heading — this is
    // the one signal `corpus/README.md`'s own lifecycle rules say is
    // *legal* but must never be *quiet* (three separate subagent attempts
    // have weakened a fixture's `[contract]` to force a pass). A `>
    // [!WARNING]` GFM alert renders with its own colored icon on GitHub,
    // which a plain bullet or bold line does not — the point is that this
    // cannot be skimmed past the way the rest of a green run can.
    if !weakened.is_empty() {
        out.push_str("> [!WARNING]\n");
        for line in weakened {
            out.push_str(&format!("> {}\n", md_escape(line)));
        }
        out.push('\n');
    }
    out.push_str("## Corpus regression report\n\n");
    out.push_str(
        "Every fixture below is replayed through the real tiered extraction pipeline from \
         frozen bytes — zero subprocesses, nothing environment-dependent, no live tool version \
         to drift. A red row is always a real parser change, never runner flakiness \
         (`corpus/README.md`); that is exactly why this check is a hard gate rather than a \
         reported-only sweep like the PATH sweep.\n\n",
    );
    out.push_str(&format!(
        "**{total} fixture(s):** {green} ok, {xfail} xfail (as expected), {failed} failed.\n\n",
    ));
    out.push_str("| fixture | outcome | status | nodes | flags | scope | change |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    let mut details_sections: Vec<String> = Vec::new();

    for row in rows {
        let (status_cell, nodes_cell, flags_cell, mut change_parts) = match &row.previous {
            None => (
                format!("{} (no baseline)", row.current.status),
                row.current.nodes.to_string(),
                row.current.flags.to_string(),
                vec!["no `expected.snap` yet".to_string()],
            ),
            Some(prev) => {
                let status_cell = if prev.status == row.current.status {
                    prev.status.to_string()
                } else {
                    format!("{} → {}", prev.status, row.current.status)
                };
                let nodes_cell = if prev.nodes == row.current.nodes {
                    prev.nodes.to_string()
                } else {
                    format!("{}→{}", prev.nodes, row.current.nodes)
                };
                let flags_cell = if prev.flags == row.current.flags {
                    prev.flags.to_string()
                } else {
                    format!("{}→{}", prev.flags, row.current.flags)
                };

                let mut parts = Vec::new();
                let removed_subs: BTreeSet<String> = prev
                    .subcommands
                    .difference(&row.current.subcommands)
                    .cloned()
                    .collect();
                let added_subs: BTreeSet<String> = row
                    .current
                    .subcommands
                    .difference(&prev.subcommands)
                    .cloned()
                    .collect();
                let removed_flags: BTreeSet<String> = prev
                    .flag_names
                    .difference(&row.current.flag_names)
                    .cloned()
                    .collect();
                let added_flags: BTreeSet<String> = row
                    .current
                    .flag_names
                    .difference(&prev.flag_names)
                    .cloned()
                    .collect();

                for (kind, names) in [
                    ("removed subcommands", &removed_subs),
                    ("added subcommands", &added_subs),
                    ("removed flags", &removed_flags),
                    ("added flags", &added_flags),
                ] {
                    if names.is_empty() {
                        continue;
                    }
                    let count = names.len();
                    let (capped, full) = capped_names(names);
                    parts.push(format!("{kind} ({count}): {capped}"));
                    if let Some(full) = full {
                        details_sections.push(format!(
                            "<details><summary>{} — {count} {kind}</summary>\n\n{full}\n\n</details>",
                            row.label,
                        ));
                    }
                }
                if parts.is_empty() {
                    // A failing fixture whose tracked dimensions all match
                    // still has a real snapshot mismatch somewhere the
                    // summary cannot see: confidence, provenance, usage
                    // lines, a reworded description. Printing "no change"
                    // beside FAIL reads as a bug in the runner rather than
                    // a regression in the parser, and sends the reviewer
                    // hunting in the wrong place. Measured: scaling
                    // `compute_confidence` by 0.8 produced exactly that
                    // row in a real CI run.
                    parts.push(
                        if row.status_word == "FAIL" {
                            "differs outside status/nodes/flags — check confidence, \
                             usage or descriptions in the snapshot diff"
                        } else {
                            "no change"
                        }
                        .to_string(),
                    );
                }
                (status_cell, nodes_cell, flags_cell, parts)
            }
        };

        if row.status_word == "FAIL" {
            let has_snapshot_mismatch = row
                .detail
                .iter()
                .any(|d| d.starts_with("snapshot mismatch") || d.contains("missing expected.snap"));
            let has_contract_failure = row.detail.iter().any(|d| d.starts_with("contract:"));
            if has_snapshot_mismatch {
                change_parts.push(
                    "**fix:** `cargo run -p xtask -- corpus --bless`, then review the \
                     `expected.snap` diff"
                        .to_string(),
                );
            }
            if has_contract_failure {
                change_parts.push(
                    "**fix:** `[contract]` is normative — edit `meta.toml` deliberately \
                     (never `--bless`) and justify the change in the PR"
                        .to_string(),
                );
            }
        }

        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            md_escape(&row.label),
            row.status_word,
            md_escape(&status_cell),
            nodes_cell,
            flags_cell,
            md_escape(&verdict_scope_label(&row.verdict_scope)),
            md_escape(&change_parts.join("; ")),
        ));
    }

    if !details_sections.is_empty() {
        out.push('\n');
        for section in &details_sections {
            out.push_str(section);
            out.push('\n');
        }
    }

    out
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

/// Print one fixture's captured help text beside the tree the parser makes
/// of it, then return.
///
/// A fixture is otherwise only inspectable by opening a `meta.toml`, an
/// `expected.snap` and one or more capture files separately and holding all
/// three in your head, which makes "what does this fixture actually claim"
/// a question answered by trusting a summary rather than by looking. This
/// renders the same side-by-side comparison `xtask audit emit` produces for
/// a live tool, sourced from the frozen capture instead, so an `[xfail]`
/// fixture's asserted defect can be seen directly in the parse.
///
/// Deliberately read-only and separate from the checking run: it neither
/// blesses nor fails, so it can be used freely while investigating.
pub fn show_fixture(corpus_root: &Path, pattern: &str) -> anyhow::Result<()> {
    let fixtures = discover_fixtures(corpus_root)?;
    let matches: Vec<&Fixture> = fixtures
        .iter()
        .filter(|f| f.label.contains(pattern))
        .collect();

    let fixture = match matches.as_slice() {
        [] => anyhow::bail!(
            "no fixture matching {pattern:?} under {}. Available: {}",
            corpus_root.display(),
            fixtures
                .iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        [one] => *one,
        many => anyhow::bail!(
            "{pattern:?} matches {} fixtures: {}. Narrow it.",
            many.len(),
            many.iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    println!("fixture: {}", fixture.label);
    println!("path:    {}", fixture.dir.display());
    if let Some(xfail) = &fixture.meta.xfail {
        println!(
            "status:  [xfail] {}",
            xfail.reason.as_deref().unwrap_or("(no reason recorded)")
        );
    } else {
        println!("status:  expected to pass");
    }
    if fixture.meta.contract.verdict_scope.is_empty() {
        println!(
            "scope:   unscoped — no dimension of this tree is asserted human-verified \
             (a passing snapshot check still freezes every field, descriptions included)"
        );
    } else {
        println!(
            "scope:   {} — only these dimensions were human-verified before this fixture was \
             blessed; the rest of the tree is frozen but unreviewed",
            verdict_scope_label(&fixture.meta.contract.verdict_scope)
        );
    }
    println!();

    for capture in &fixture.meta.captures {
        let argv = capture.argv.join(" ");
        let files: [(&str, Option<&str>); 2] = [
            ("stdout", Some(capture.stdout.as_str())),
            ("stderr", capture.stderr.as_deref()),
        ];
        for (label, file) in files {
            let Some(name) = file else { continue };
            let bytes = std::fs::read(fixture.dir.join(name))?;
            if bytes.is_empty() {
                continue;
            }
            println!("=== captured: {argv}  ({label}) ===");
            println!("{}", String::from_utf8_lossy(&bytes));
        }
    }

    let transcript = fixture.build_transcript()?;
    let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
    let resolved = fixture.resolved_tool();
    println!("=== parsed tree ===");
    match extract_tree(&runner, &resolved) {
        Some(node) => println!("{}", render_snapshot(&node)?),
        None => println!("(no tier produced a root node)"),
    }
    Ok(())
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

    /// A fixture with every `[contract]` field set to something a later
    /// edit can weaken — the baseline side of every
    /// `contract_weakened_lines` test below. Explicit parameters for the
    /// two fields tests actually vary, rather than raw-string TOML
    /// injection: a naive `format!` splice let an override land inside
    /// `[contract.must_contain_flags_by_path]` instead of replacing the
    /// top-level key it was meant to override (TOML disallows a real
    /// duplicate key in one table, which is exactly the bug this signature
    /// change closes off structurally).
    fn full_contract_fixture(root: &Path, min_subcommands: usize, must_contain_flags: &[&str]) {
        let dir = root.join("fulltool/1.0");
        let flags_toml = must_contain_flags
            .iter()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        write(
            &dir.join("meta.toml"),
            &format!(
                r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "ok"
min_subcommands = {min_subcommands}
must_contain_flags = [{flags_toml}]

[contract.must_contain_flags_by_path]
run = ["--source", "--staged"]
"#
            ),
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
    }

    #[test]
    fn green_fixture_with_bless_then_check_round_trips_clean() {
        let corpus = setup();
        green_fixture(&corpus.root);

        let blessed = run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");
        assert!(!blessed.failed());
        assert!(corpus.root.join("mytool/1.0/expected.snap").is_file());

        let checked = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
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
        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
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
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
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
        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
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

    /// `must_contain_flags_by_path`: a spot-check against a *subcommand's*
    /// own flags, not the root's — the generalization `must_contain_flags`
    /// alone can't express (`flag_present`'s doc comment: root-only).
    /// Reuses `deeptool`'s two-capture shape (root plus one subcommand
    /// `--help`) so the recursive fill actually populates `sub`'s flags
    /// before the contract check runs.
    #[test]
    fn must_contain_flags_by_path_checks_a_subcommands_own_flags() {
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

[contract.must_contain_flags_by_path]
sub = ["--deep"]
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

        let report = run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");
        assert!(!report.failed(), "{}", report.text);

        let checked = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(!checked.failed(), "{}", checked.text);
    }

    /// The failing direction: a flag the subcommand doesn't actually have
    /// must be named in the failure, and a path that doesn't resolve at
    /// all must say so rather than panicking or silently passing.
    #[test]
    fn must_contain_flags_by_path_names_a_missing_flag_and_an_unknown_path() {
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

[contract.must_contain_flags_by_path]
sub = ["--deep", "--nonexistent"]
"missing-node" = ["--anything"]
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

        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(report.failed());
        assert!(
            report.text.contains("--nonexistent"),
            "the missing flag must be named: {}",
            report.text
        );
        assert!(
            report.text.contains("missing-node"),
            "the unresolvable path must be named: {}",
            report.text
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

    /// A freshly-blessed, byte-identical fixture must report "no change"
    /// in markdown, on **both** sides of the comparison agreeing on
    /// status — the regression test for the bug this module's first draft
    /// actually shipped: `SnapFlag` without a `description` field made
    /// every reconstructed "previous" tree look 0% described regardless of
    /// the real fixture, desyncing `status` (`low-confidence` vs `ok`)
    /// even when nothing had changed.
    #[test]
    fn markdown_report_on_a_matching_fixture_says_no_change() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run(&corpus.root, false, ScoreFormat::Markdown).expect("check run succeeds");
        assert!(!report.failed(), "{}", report.text);
        assert!(
            report.text.contains("no change"),
            "an unchanged fixture must be reported as such, not silently omitted: {}",
            report.text
        );
        // Never a raw YAML diff: the compact snapshot's own field names
        // (`heading_attested`, `provenance`) must not leak into the
        // markdown report as if it were the file's literal text.
        assert!(!report.text.contains("heading_attested"));
        assert!(!report.text.contains("provenance"));
    }

    /// The parse-time ceiling **warns; it never fails the run.**
    ///
    /// This is a deliberate, argued property (see
    /// [`MAX_FIXTURE_PARSE_TIME`]), not an oversight, so it needs a test
    /// that fails if someone "fixes" it back. It was previously verified
    /// only by editing the constant by hand, which guards nothing —
    /// confirmed genuine by reverting the property and watching this fail.
    ///
    /// A zero ceiling makes every fixture violate it, which is the only way
    /// to exercise this at all: a real fixture parses in well under a
    /// millisecond.
    #[test]
    fn an_exceeded_parse_time_ceiling_warns_but_does_not_fail() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run_with_ceiling(&corpus.root, false, ScoreFormat::Text, Duration::ZERO, None)
            .expect("check run succeeds");

        assert!(
            !report.failed(),
            "a slow parse must not fail the run — only correctness checks block: {}",
            report.text
        );
        assert!(
            report.text.contains("warning: slow parse"),
            "...but it must still be reported, and legibly as a warning: {}",
            report.text
        );
    }

    /// A mismatch the summary's own dimensions cannot see must not be
    /// reported as "no change".
    ///
    /// Found in a real CI run, not by a unit test: scaling
    /// `compute_confidence` by 0.8 left status, node count, flag count and
    /// every flag name identical, so the row rendered as
    /// `tar/1.35 | FAIL | ... | no change`. "FAIL, no change" reads as a
    /// bug in the runner rather than a regression in the parser, and points
    /// the reviewer at the wrong thing. The row must instead say the
    /// difference lies outside the tracked dimensions.
    #[test]
    fn markdown_report_never_says_no_change_on_a_failing_fixture() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        // Perturb only `confidence` in the blessed snapshot — nothing the
        // semantic summary tracks. Equivalent to a parser change that moves
        // confidence and nothing else.
        let snap_path = corpus.root.join("mytool/1.0/expected.snap");
        let snap = std::fs::read_to_string(&snap_path).expect("blessed snapshot exists");
        let perturbed = snap.replace("confidence:", "confidence: 0.01 #");
        assert_ne!(snap, perturbed, "the fixture must carry a confidence field");
        std::fs::write(&snap_path, perturbed).expect("rewrite snapshot");

        let report = run(&corpus.root, false, ScoreFormat::Markdown).expect("check run succeeds");
        assert!(
            report.failed(),
            "a perturbed snapshot must fail: {}",
            report.text
        );
        assert!(
            !report.text.contains("no change"),
            "a failing fixture must never be described as unchanged: {}",
            report.text
        );
        assert!(
            report.text.contains("differs outside"),
            "the row must say where to look instead: {}",
            report.text
        );
    }

    /// The markdown report's whole reason to exist: when a snapshot
    /// mismatches, it must name *which* flag disappeared — not report a
    /// line count, and never the raw YAML — and it must point at
    /// `--bless` as the remedy, not at editing `meta.toml` (that remedy is
    /// reserved for a `[contract]` violation, a different failure this
    /// fixture doesn't trigger).
    #[test]
    fn markdown_report_on_a_snapshot_mismatch_names_the_lost_flag() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        // Simulate a regression: the fixture's captured `--help` gains a
        // second flag that the blessed `expected.snap` doesn't know about
        // yet (as if a grammar change started recognizing it, or — the
        // regression direction that matters — stopped recognizing an
        // existing one). Either way this must show up as a *named* flag,
        // not a line-number diff.
        write(
            &corpus.root.join("mytool/1.0/help.txt"),
            "Usage: mytool [OPTIONS] <COMMAND>\n\nOptions:\n  -v, --verbose   be noisy\n  -q, --quiet   be silent\n\nCommands:\n  run    run the thing\n",
        );

        let report = run(&corpus.root, false, ScoreFormat::Markdown).expect("check run succeeds");
        assert!(report.failed());
        assert!(
            report.text.contains("--quiet"),
            "the specific flag that changed must be named: {}",
            report.text
        );
        assert!(
            report.text.contains("--bless"),
            "a snapshot mismatch must name --bless as the remedy: {}",
            report.text
        );
        assert!(
            !report.text.contains("edit `meta.toml`"),
            "a snapshot mismatch is not a [contract] violation and must not point at meta.toml: {}",
            report.text
        );
    }

    // --- contract-weakening detection (process fix #1) ---
    //
    // Every test below builds two *plain directories* (never git) and
    // calls `contract_weakened_lines` or `run_with_baseline` directly —
    // exactly the interface a CI step would drive after populating
    // `baseline_root` with `git archive <base-ref> corpus | tar -x`. None
    // of this module ever shells out to git; see `contract_weakened_lines`'s
    // own doc comment for why that's a hard boundary, not an oversight.

    #[test]
    fn identical_contracts_produce_no_weakening_lines() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        full_contract_fixture(&current.root, 1, &["--verbose", "--quiet"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        assert!(contract_weakened_lines(&cur_fixtures, &base_fixtures).is_empty());
    }

    /// A **tightened** contract (more flags required, not fewer) must
    /// never be flagged — only a field getting *weaker* is the signal.
    #[test]
    fn a_tightened_contract_is_not_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        full_contract_fixture(&current.root, 1, &["--verbose", "--quiet", "--extra"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        assert!(contract_weakened_lines(&cur_fixtures, &base_fixtures).is_empty());
    }

    /// A fixture whose `[contract]` sets only `min_subcommands`, matched
    /// against what `MYTOOL_HELP` actually contains (one subcommand,
    /// `run`) — used by the two end-to-end tests below, which (unlike
    /// [`full_contract_fixture`]'s pure `contract_weakened_lines` tests)
    /// really do run extraction: a contract demanding flags the capture
    /// doesn't have would fail its *own* check for unrelated reasons and
    /// make "weakening is reported, never gated" untestable in isolation.
    fn minimal_min_subcommands_fixture(root: &Path, min_subcommands: usize) {
        write(
            &root.join("fulltool/1.0/meta.toml"),
            &format!(
                r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
min_subcommands = {min_subcommands}
"#
            ),
        );
        write(&root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
    }

    #[test]
    fn lowered_min_subcommands_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 20, &["--verbose", "--quiet"]);
        full_contract_fixture(&current.root, 1, &["--verbose", "--quiet"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(
            lines.iter().any(|l| l.contains("min_subcommands")),
            "{lines:?}"
        );
    }

    #[test]
    fn lowered_min_status_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        // Overwrite min_status from "ok" down to "verbatim" — a real
        // per-tool downgrade, so write the whole contract explicitly
        // rather than relying on TOML's last-key-wins (fragile and easy
        // to break by reordering the base template).
        write(
            &current.root.join("fulltool/1.0/meta.toml"),
            r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "verbatim"
min_subcommands = 1
must_contain_flags = ["--verbose", "--quiet"]

[contract.must_contain_flags_by_path]
run = ["--source", "--staged"]
"#,
        );
        write(&current.root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(lines.iter().any(|l| l.contains("min_status")), "{lines:?}");
    }

    #[test]
    fn a_dropped_must_contain_flag_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        write(
            &current.root.join("fulltool/1.0/meta.toml"),
            r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "ok"
min_subcommands = 1
must_contain_flags = ["--verbose"]

[contract.must_contain_flags_by_path]
run = ["--source", "--staged"]
"#,
        );
        write(&current.root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("must_contain_flags") && l.contains("--quiet")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_dropped_must_contain_flags_by_path_entry_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        write(
            &current.root.join("fulltool/1.0/meta.toml"),
            r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "ok"
min_subcommands = 1
must_contain_flags = ["--verbose", "--quiet"]

[contract.must_contain_flags_by_path]
run = ["--source"]
"#,
        );
        write(&current.root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("must_contain_flags_by_path") && l.contains("--staged")),
            "{lines:?}"
        );
    }

    /// A fixture newly marked `[xfail]` — the exact move "weaken a
    /// contract to force a pass" describes when the underlying promise
    /// didn't change, only whether failing it still fails the run.
    #[test]
    fn newly_marked_xfail_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        write(
            &current.root.join("fulltool/1.0/meta.toml"),
            r#"
[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
expected_framework = "generic"
min_status = "ok"
min_subcommands = 1
must_contain_flags = ["--verbose", "--quiet"]

[contract.must_contain_flags_by_path]
run = ["--source", "--staged"]

[xfail]
broken = true
reason = "newly broken"
"#,
        );
        write(&current.root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(lines.iter().any(|l| l.contains("xfail")), "{lines:?}");
    }

    /// A fixture present in the baseline but deleted entirely in the
    /// current tree — `corpus/README.md`'s "never deleted because it
    /// became inconvenient" rule, made loud instead of trusted.
    #[test]
    fn a_removed_fixture_is_flagged() {
        let baseline = setup();
        let current = setup();
        full_contract_fixture(&baseline.root, 1, &["--verbose", "--quiet"]);
        green_fixture(&current.root); // some *other* fixture, "fulltool" absent
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("fulltool/1.0") && l.contains("fixture-removed")),
            "{lines:?}"
        );
    }

    /// End-to-end through [`run_with_baseline`]: the weakening line reaches
    /// the actual text report, appears before every other line ("prominent
    /// ... a reviewer skimming a green run cannot miss it"), and — the
    /// point of "reported, not gated" — does not make an otherwise-clean
    /// run fail.
    #[test]
    fn run_with_baseline_surfaces_weakening_prominently_and_does_not_gate_on_it() {
        let baseline = setup();
        let current = setup();
        minimal_min_subcommands_fixture(&baseline.root, 20);
        minimal_min_subcommands_fixture(&current.root, 1);
        // Bless the current side so the snapshot itself is clean — the
        // only thing wrong here is the weakened contract.
        run(&current.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run_with_baseline(
            &current.root,
            false,
            ScoreFormat::Text,
            Some(&baseline.root),
        )
        .expect("check run succeeds");
        assert!(
            !report.failed(),
            "contract weakening must be reported, never gated: {}",
            report.text
        );
        assert!(report.text.contains("CONTRACT WEAKENED"));
        assert!(report.text.contains("min_subcommands"));
        let weakened_line_pos = report.text.find("CONTRACT WEAKENED").unwrap();
        let fixture_line_pos = report.text.find("fulltool/1.0").unwrap();
        // The weakening line must be the *first* mention of the fixture,
        // i.e. it precedes the fixture's own per-fixture result line.
        assert!(weakened_line_pos <= fixture_line_pos);
    }

    /// Same end-to-end check for the markdown format: the warning must
    /// render as a GFM alert block, ahead of the report's own heading.
    #[test]
    fn run_with_baseline_markdown_renders_a_warning_alert_first() {
        let baseline = setup();
        let current = setup();
        minimal_min_subcommands_fixture(&baseline.root, 20);
        minimal_min_subcommands_fixture(&current.root, 1);
        run(&current.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run_with_baseline(
            &current.root,
            false,
            ScoreFormat::Markdown,
            Some(&baseline.root),
        )
        .expect("check run succeeds");
        assert!(report.text.starts_with("> [!WARNING]"));
        assert!(report.text.contains("CONTRACT WEAKENED"));
        let warning_pos = report.text.find("[!WARNING]").unwrap();
        let heading_pos = report.text.find("## Corpus regression report").unwrap();
        assert!(warning_pos < heading_pos);
    }

    /// With no baseline given (the default — nothing wired to call this
    /// yet), behavior must be byte-identical to before this feature
    /// existed: no weakening lines, nothing gated differently.
    #[test]
    fn no_baseline_means_no_weakening_check_at_all() {
        let current = setup();
        green_fixture(&current.root);
        run(&current.root, true, ScoreFormat::Text).expect("bless run succeeds");
        let report = run(&current.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(!report.text.contains("CONTRACT WEAKENED"));
    }

    // --- verdict_scope (machine-readable "SCOPE OF REVIEW") ---

    /// The argued default: a fixture with no `verdict_scope` key at all
    /// must parse to an empty list, never to every dimension. Silence
    /// must mean "no claim made", not "everything reviewed" — `bless`
    /// freezes descriptions whether or not a human read them, so the only
    /// safe reading of an absent field is that nothing is being claimed
    /// about it.
    #[test]
    fn verdict_scope_defaults_to_empty_when_absent() {
        let corpus = setup();
        green_fixture(&corpus.root); // no [contract] verdict_scope key
        let fixtures = discover_fixtures(&corpus.root).unwrap();
        let fixture = fixtures.iter().find(|f| f.label == "mytool/1.0").unwrap();
        assert!(
            fixture.meta.contract.verdict_scope.is_empty(),
            "an absent verdict_scope must default to empty (no scope claimed), never to \
             every dimension"
        );
        assert_eq!(
            verdict_scope_label(&fixture.meta.contract.verdict_scope),
            "unscoped"
        );
    }

    /// The documented value set parses into the matching enum variants,
    /// in the order written.
    #[test]
    fn verdict_scope_parses_the_documented_values() {
        let corpus = setup();
        let dir = corpus.root.join("scopedtool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "scopedtool"
version = "1.0"

[[capture]]
argv = ["scopedtool", "--help"]
stdout = "help.txt"

[contract]
verdict_scope = ["flags", "subcommands", "descriptions", "usage"]
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        let fixtures = discover_fixtures(&corpus.root).unwrap();
        let fixture = fixtures
            .iter()
            .find(|f| f.label == "scopedtool/1.0")
            .unwrap();
        assert_eq!(
            fixture.meta.contract.verdict_scope,
            vec![
                VerdictScope::Flags,
                VerdictScope::Subcommands,
                VerdictScope::Descriptions,
                VerdictScope::Usage,
            ]
        );
        assert_eq!(
            verdict_scope_label(&fixture.meta.contract.verdict_scope),
            "flags, subcommands, descriptions, usage"
        );
    }

    /// An unrecognized scope word must fail to parse, loudly, naming the
    /// offending fixture — never be silently dropped or treated as an
    /// unknown-but-tolerated variant, since a typo here would otherwise
    /// quietly under-claim (or, worse, a future rename of a value could
    /// silently keep matching the old string forever with a permissive
    /// deserializer).
    #[test]
    fn verdict_scope_rejects_an_unknown_value() {
        let corpus = setup();
        let dir = corpus.root.join("badscope/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "badscope"
version = "1.0"

[[capture]]
argv = ["badscope", "--help"]
stdout = "help.txt"

[contract]
verdict_scope = ["flags", "vibes"]
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        let result = discover_fixtures(&corpus.root);
        let err = match result {
            Ok(_) => panic!("an unrecognized verdict_scope value must fail to parse"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("badscope"), "{err}");
    }

    /// Once a fixture is blessed and passes, a set `verdict_scope` shows
    /// up in the plain-text per-fixture report line — the runner surfaces
    /// what a green result actually means, not just that it's green.
    #[test]
    fn verdict_scope_appears_in_the_text_report_when_set() {
        let corpus = setup();
        let dir = corpus.root.join("scopedtool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "scopedtool"
version = "1.0"

[[capture]]
argv = ["scopedtool", "--help"]
stdout = "help.txt"

[contract]
verdict_scope = ["flags", "subcommands"]
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(!report.failed(), "{}", report.text);
        assert!(
            report.text.contains("verdict_scope: flags, subcommands"),
            "{}",
            report.text
        );
    }

    /// An unscoped fixture must not gain a `verdict_scope:` line — the
    /// text report's shape for every fixture shipped before this field
    /// existed must stay byte-for-byte unchanged.
    #[test]
    fn unscoped_fixture_has_no_verdict_scope_line_in_the_text_report() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(!report.failed(), "{}", report.text);
        assert!(!report.text.contains("verdict_scope"), "{}", report.text);
    }

    /// The markdown table always carries a `scope` column, including
    /// `"unscoped"` for a fixture with no recorded scope — a reviewer
    /// scanning the transition report should never have to open
    /// `meta.toml` to learn that a passing row's descriptions were never
    /// looked at.
    #[test]
    fn markdown_report_includes_the_scope_column() {
        let corpus = setup();
        green_fixture(&corpus.root);
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");

        let report = run(&corpus.root, false, ScoreFormat::Markdown).expect("check run succeeds");
        assert!(!report.failed(), "{}", report.text);
        assert!(report.text.contains("| scope |"), "{}", report.text);
        assert!(report.text.contains("unscoped"), "{}", report.text);
    }
}
