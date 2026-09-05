//! The corpus regression runner (spec §13.2, `corpus/README.md`): replays
//! every fixture under `corpus/<tool>/<version>/` through the real tiered
//! extraction pipeline with zero subprocesses, via the `Transcript` replay
//! seam (`mandible_extract::exec::Transcript`), and fails loudly when a
//! parse regresses.
//!
//! This module only reads `corpus/`; nothing here is reachable from the
//! `mandible` binary (`corpus/README.md`).
//!
//! Each fixture's `meta.toml` lists `[[capture]]` entries (argv paired with
//! captured bytes). This module builds a [`Transcript`] and a synthetic
//! [`ResolvedTool`] ([`resolved_tool`]) and drives
//! [`mandible_extract::default_tiers_with_probe`] through [`Runner`] as
//! the real binary would: root extraction plus a bounded recursive fill
//! (spec §5.2's cascade).
//!
//! # Checks, per fixture
//!
//! - (a) Snapshot match against `expected.snap` — plain byte comparison,
//!   never an `insta` run ([`render_snapshot`]).
//! - (b) `[contract]`: `expected_framework`, `min_status`,
//!   `min_subcommands`, `must_contain_flags`, `must_contain_flags_by_path`,
//!   `must_contain_positionals`, `must_contain_modifiers`,
//!   `must_not_contain_flags`, `must_keep_separate`, `must_attach_choices`,
//!   `must_describe` ([`check_contract`]).
//! - (c) Strict xfail: an `[xfail]` fixture whose snapshot and contract
//!   both pass fails the run — the bug is fixed, promote it
//!   (`corpus/README.md`'s lifecycle rules).
//! - (d) Parse-time ceiling ([`MAX_FIXTURE_PARSE_TIME`]), regardless of
//!   xfail status. Warns rather than failing the run.
//!
//! (a)-(c) block; (d) warns — the only nondeterministic check here, so a
//! red on (a)-(c) is always a real parser change.

use crate::coverage::ScoreFormat;
use mandible_core::{CommandNode, Dashes, Entity, EntityKind, Provenance, Source, Spelling, Text};
use mandible_extract::exec::{ExecOutput, Transcript};
use mandible_extract::{default_tiers_with_probe, ResolvedTool, Runner};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod contract;
mod markdown;
mod report;
mod runner;
mod summary;

pub(crate) use contract::*;
pub(crate) use markdown::*;
pub(crate) use report::*;
pub(crate) use runner::*;
pub(crate) use summary::*;

/// Mechanical net for the O(n²)-in-a-loop class of bug (AGENTS.md: a
/// degenerate input once took 153s instead of milliseconds). Deliberately
/// coarse — 100ms is nowhere near a single-fixture in-memory parse's real
/// cost.
///
/// Warns, does not fail the run: the one nondeterministic check here (CI
/// wall-clock contention measured up to ~40x on this project's runners),
/// so blocking on it would let a flaky timing red teach people to ignore
/// the corpus gate.
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
pub(crate) struct ContractMeta {
    #[serde(default)]
    expected_framework: Option<String>,
    #[serde(default)]
    min_status: Option<String>,
    #[serde(default)]
    min_subcommands: Option<usize>,
    #[serde(default)]
    must_contain_flags: Vec<String>,
    /// Same spot-check as `must_contain_flags`, for a subcommand's own
    /// flags — keyed by the subcommand's path, space-separated, no tool
    /// name (`"restore"`, `"remote add"`). `must_contain_flags` alone can
    /// only assert what a tool publishes at its root.
    #[serde(default)]
    must_contain_flags_by_path: std::collections::BTreeMap<String, Vec<String>>,
    /// Root positional operands the tree must carry, by name. Matched on
    /// `Entity::primary_name` exactly, root only — same scope as
    /// `must_contain_flags`.
    ///
    /// A trailing `...` on an entry (`"file..."`) additionally requires
    /// the operand to be repeatable, written the way a tool writes it.
    /// `[file...]` with the dots glued loses the marker today while
    /// `[file ...]` keeps it, and no other field can state the
    /// difference.
    #[serde(default)]
    must_contain_positionals: Vec<String>,
    /// Modifier letters the tree must carry (spec §4.5, §7 Tier B
    /// "Modifier tables"). Written as the bare letter (`"a"`, `"D"`),
    /// matched on `Entity::primary_name`, root only. Case is significant:
    /// `ar` documents `[D]`/`[u]` as distinct from `[d]`/`[U]`.
    #[serde(default)]
    must_contain_modifiers: Vec<String>,
    /// Environment variable names the tree must carry (spec §4.5, §7 Tier B
    /// "Environment sections"). Written as the variable's own spelling
    /// (`"NODE_DEBUG"`), matched on `Entity::primary_name`, root only. Case
    /// is significant (shell semantics).
    #[serde(default)]
    must_contain_env_vars: Vec<String>,
    /// Root flag spellings the tree must **not** carry — the only negative
    /// claim in a `[contract]` ("the parser invented this"), added because
    /// `corpus/mariadb-check/2.7.4`'s defaults-table header ruler was read
    /// as a 31-dash flag with no falsifiable way to say it shouldn't be
    /// (spec §14).
    ///
    /// Matched by [`flag_present`], negated: `--foo` asserts no root flag
    /// has long name `foo`; `-x` asserts none has short name `x`. Claims
    /// nothing about the raw text (the mariadb ruler still occurs there —
    /// the existence oracle is correctly silent on this defect), nothing
    /// about the flag's other spelling, and root only (no by-path
    /// analogue). A tree with no root satisfies this vacuously and is not
    /// reported, unlike every positive field above.
    #[serde(default)]
    must_not_contain_flags: Vec<String>,
    /// Spelling groups that must resolve to *distinct* root-flag entities
    /// — the other shape a negative claim can take, guarding against the
    /// alias-run fold merging unrelated flags onto one multi-spelling
    /// entity (a reverted defect: an earlier fold merged rows on
    /// description equality and fused unrelated flags, e.g. `-w` and
    /// `-X`). Each inner list names spellings that must not all resolve
    /// to the same entity; matched the way `must_contain_flags` matches,
    /// root only. A tree with no root satisfies this vacuously, the same
    /// reasoning as `must_not_contain_flags`.
    #[serde(default)]
    must_keep_separate: Vec<Vec<String>>,
    /// A root flag's choice values it must carry, keyed by the flag's own
    /// spelling (matched the way `must_contain_flags` matches). A positive
    /// claim: the flag must exist and its `Entity::choices` must include
    /// every named value, so a choices block that attached to the wrong
    /// flag is checkable instead of only visible in a snapshot diff.
    #[serde(default)]
    must_attach_choices: std::collections::BTreeMap<String, Vec<String>>,
    /// A root flag's rendered description must contain this text, keyed
    /// by the flag's own spelling (matched the way `must_contain_flags`
    /// matches). Substring match after collapsing runs of whitespace on
    /// both sides to a single space (descriptions wrap); case-sensitive.
    /// Issue #102 item 5: makes description recovery checkable instead of
    /// guarded only by the snapshot.
    #[serde(default)]
    must_describe: std::collections::BTreeMap<String, String>,
    /// A root flag's value placeholder must contain this text, keyed by
    /// the flag's own spelling (matched the way `must_contain_flags`
    /// matches). Substring match after collapsing whitespace, the same
    /// rule `must_describe` uses. Satisfied when ANY entity carrying that
    /// spelling matches, since a tool can document one spelling on two
    /// rows (`vim`'s `-r` and `-r (with file name)`).
    ///
    /// The gap it closes: a value spec that lost a token
    /// (`-V[N][fname]` read as `-V` plus `N`) leaves every other field
    /// intact, so nothing but the snapshot could see it.
    #[serde(default)]
    must_value_name: std::collections::BTreeMap<String, String>,
    /// A root positional's rendered description must contain this text,
    /// keyed by the positional's own name (matched on
    /// `Entity::primary_name`, root only, same scope as
    /// `must_contain_positionals`). The positional analogue of
    /// `must_describe`: `check_must_describe` walks only `root.flags()`,
    /// so no field could assert `invoke-rc.d`'s `action` carries its own
    /// documented description until this one existed. Substring match
    /// after collapsing whitespace, the same rule `must_describe` uses.
    #[serde(default)]
    must_describe_positional: std::collections::BTreeMap<String, String>,
    /// A root flag's rendered description must NOT contain this text,
    /// keyed by the flag's own spelling (matched the way `must_describe`
    /// matches) — the mirror of `must_not_contain_flags` one level down.
    /// Closes the gap `corpus/nfsslower-bpfcc/0.29.1` found: an unheaded
    /// example block folds onto a flag's real, correct description, and
    /// `must_describe`'s substring check still passes because the real
    /// text is still present, just followed by everything the fold
    /// added. A tree with no root, or the flag itself absent, satisfies
    /// this vacuously, the same reasoning `must_not_contain_flags` uses.
    #[serde(default)]
    must_not_describe: std::collections::BTreeMap<String, String>,
    /// Which dimensions of this fixture's tree a human actually verified
    /// before blessing it — machine-readable replacement for the
    /// "SCOPE OF REVIEW" prose comment (`git show c9bfe76`). Not itself a
    /// check: `expected.snap` freezes every field regardless, so this is
    /// what tells a fully-reviewed fixture apart from a partially-reviewed
    /// one. Carried into every report.
    ///
    /// Absent means "no scope claimed", not "full scope" — the lsof
    /// cautionary tale (`corpus/README.md`) was a blessed tree nobody had
    /// read. See `verdict_scope_defaults_to_empty_when_absent`.
    #[serde(default)]
    verdict_scope: Vec<VerdictScope>,
}

/// One dimension of a fixture's tree that a `verdict_scope` entry can
/// claim was reviewed by a human before blessing. `Flags`/`Subcommands`
/// are what the seed-2 audit workflow checks; `Descriptions`/`Usage`
/// cover a bless that included a full prose read (`corpus/README.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictScope {
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

/// `[bless]`: who blessed this fixture's `expected.snap` — the complement
/// to `verdict_scope` (which records what a human reviewed). Required,
/// unlike every other `[contract]`/`[xfail]` field: a fixture without it
/// fails to load (`discover_fixtures`'s explicit guard), so the
/// conservative "agent" default must always be an explicit assertion.
#[derive(Debug, Clone, Deserialize)]
struct BlessMeta {
    provenance: BlessProvenance,
}

/// Who blessed a fixture's current `expected.snap`, no human review
/// implied by default.
///
/// - `Human` — a human blessed (or re-blessed) the current snapshot.
/// - `AgentThenHuman` — an agent blessed the tree; a human reviewed it
///   later and recorded a `verdict_scope` without re-blessing.
/// - `Agent` — an agent blessed it and no human has reviewed the tree
///   (the conservative default).
///
/// An agent may only ever write `Agent` here; only a human may record
/// `Human` or `AgentThenHuman`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BlessProvenance {
    Human,
    AgentThenHuman,
    Agent,
}

impl BlessProvenance {
    fn as_str(self) -> &'static str {
        match self {
            BlessProvenance::Human => "human",
            BlessProvenance::AgentThenHuman => "agent-then-human",
            BlessProvenance::Agent => "agent",
        }
    }
}

/// Render a `[bless] provenance` value for a report — centralized so every
/// surface (`show_fixture`, the text report's per-fixture detail, the
/// markdown table's `provenance` column) renders it the same way, mirroring
/// [`verdict_scope_label`]. Unlike `verdict_scope_label`, there is no
/// "absent" case to spell out: `provenance` is required on every fixture,
/// so this always has a value to print.
fn provenance_label(provenance: BlessProvenance) -> &'static str {
    provenance.as_str()
}

/// Count `human` / `agent-then-human` / `agent` occurrences among a set of
/// provenance values — used to split the `ok` count in both the text and
/// markdown summaries, so "N ok" can never be misread as "N human-verified"
/// (`corpus/README.md`'s `[bless]` section).
fn provenance_counts(values: impl Iterator<Item = BlessProvenance>) -> (usize, usize, usize) {
    let mut human = 0;
    let mut agent_then_human = 0;
    let mut agent = 0;
    for v in values {
        match v {
            BlessProvenance::Human => human += 1,
            BlessProvenance::AgentThenHuman => agent_then_human += 1,
            BlessProvenance::Agent => agent += 1,
        }
    }
    (human, agent_then_human, agent)
}

/// Render the `(human, agent_then_human, agent)` split as the parenthetical
/// clause that follows an `ok` count, e.g. `"(12 human, 3 agent-then-human,
/// 45 agent)"`. Shared by the text and markdown summaries so the wording
/// never drifts between the two.
fn provenance_split_label(counts: (usize, usize, usize)) -> String {
    let (human, agent_then_human, agent) = counts;
    format!("{human} human, {agent_then_human} agent-then-human, {agent} agent")
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
    bless: BlessMeta,
}

/// One discovered `corpus/<tool>/<version>/` fixture.
pub(crate) struct Fixture {
    /// `corpus/<tool>/<version>` — used for error messages and to resolve
    /// `expected.snap` and every capture's file path.
    dir: PathBuf,
    /// `<tool>/<version>`, for report labels.
    label: String,
    meta: Meta,
}

impl Fixture {
    /// `<tool>/<version>`, for report labels.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    /// The tool this fixture captures, as `meta.toml` names it.
    pub(crate) fn tool_name(&self) -> &str {
        &self.meta.tool.name
    }

    /// The **root help document** this fixture froze: the captured output
    /// of whichever `[[capture]]` is the plain root `--help`/`-h` probe,
    /// decoded lossily, preferring stdout and falling back to stderr for
    /// the tools that print help there and exit nonzero (`openssl`, `ip` —
    /// spec Appendix A). `None` when the fixture has no such capture, or
    /// when it captured nothing at all.
    ///
    /// Exists for [`crate::residue`], which needs the same bytes a tier
    /// parsed rather than a re-probe. Chosen by argv *shape* — never by
    /// capture order, which a multi-probe fixture (a cobra transcript
    /// carries several) does not guarantee.
    pub(crate) fn root_help_text(&self) -> anyhow::Result<Option<String>> {
        let pick = self
            .meta
            .captures
            .iter()
            .find(|c| c.argv.get(1..) == Some(&["--help".to_string()]))
            .or_else(|| {
                self.meta
                    .captures
                    .iter()
                    .find(|c| c.argv.get(1..) == Some(&["-h".to_string()]))
            });
        let Some(capture) = pick else {
            return Ok(None);
        };
        let stdout = read_capture_file(&self.dir, &capture.stdout)?;
        if !stdout.is_empty() {
            return Ok(Some(String::from_utf8_lossy(&stdout).into_owned()));
        }
        match &capture.stderr {
            Some(name) => {
                let stderr = read_capture_file(&self.dir, name)?;
                if stderr.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(String::from_utf8_lossy(&stderr).into_owned()))
                }
            }
            None => Ok(None),
        }
    }

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
    pub(crate) fn build_transcript(&self) -> anyhow::Result<Transcript> {
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
    pub(crate) fn resolved_tool(&self) -> ResolvedTool {
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
pub(crate) fn discover_fixtures(corpus_root: &Path) -> anyhow::Result<Vec<Fixture>> {
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
            // Friendly, fixture-naming guard ahead of the generic serde
            // error `Meta`'s required `bless: BlessMeta` field would
            // otherwise produce (a bare "missing field `bless`" with no
            // pointer to what that means or where to read about it).
            let value: toml::Value = toml::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("parsing {}: {e}", meta_path.display()))?;
            if value
                .get("bless")
                .and_then(|b| b.get("provenance"))
                .is_none()
            {
                anyhow::bail!(
                    "{}: missing [bless] provenance — every fixture must record who blessed \
                     its expected tree (`human`/`agent-then-human`/`agent`); see \
                     corpus/README.md",
                    meta_path.display()
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};
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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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

    /// `must_contain_positionals` in both directions. A contract field
    /// that cannot be seen to fail asserts nothing, and this one exists
    /// precisely because two fixtures spent a release naming a dropped
    /// operand in a comment while testing something else.
    #[test]
    fn must_contain_positionals_names_the_operands_that_are_missing() {
        let contract = ContractMeta {
            must_contain_positionals: vec!["pid".into(), "interval".into()],
            ..ContractMeta::default()
        };
        let mut root = CommandNode::new("uobjnew", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_positionals: missing pid, interval"]
        );

        let mut pid = Entity::positional("pid", Provenance::single(Source::HelpText));
        pid.required = true;
        root.entities.push(pid);
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_positionals: missing interval"]
        );

        root.entities.push(Entity::positional(
            "interval",
            Provenance::single(Source::HelpText),
        ));
        assert!(check_contract(&contract, Some(&root)).is_empty());
        // No root at all is a failure of the same field, never a silent pass.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_positionals: no root produced"]
        );
    }

    /// `must_contain_modifiers` in both directions, the same way
    /// `must_contain_positionals` is exercised above and for the same
    /// reason: a contract field that cannot be seen to fail asserts
    /// nothing.
    ///
    /// Case is asserted explicitly. `ar` and `llvm-ar` both document `[u]`
    /// and `[U]` as different modifiers, so a matcher that folded case
    /// would satisfy half of a real fixture's list twice over while the
    /// other half went missing — and it would do it silently, since every
    /// letter it was asked for would appear to be present.
    #[test]
    fn must_contain_modifiers_names_the_letters_that_are_missing() {
        let contract = ContractMeta {
            must_contain_modifiers: vec!["a".into(), "U".into()],
            ..ContractMeta::default()
        };
        let mut root = CommandNode::new("ar", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_modifiers: missing a, U"]
        );

        root.entities
            .push(Entity::modifier('a', Provenance::single(Source::HelpText)));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_modifiers: missing U"]
        );

        // The lowercase twin does not satisfy the uppercase claim.
        root.entities
            .push(Entity::modifier('u', Provenance::single(Source::HelpText)));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_modifiers: missing U"]
        );

        root.entities
            .push(Entity::modifier('U', Provenance::single(Source::HelpText)));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // A flag spelled with the same letter is a different item and
        // never satisfies a modifier claim.
        let flag_only = ContractMeta {
            must_contain_modifiers: vec!["v".into()],
            ..ContractMeta::default()
        };
        let mut flagged = CommandNode::new("ar", Provenance::single(Source::HelpText));
        flagged.entities.push(Entity::flag_short(
            'v',
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&flag_only, Some(&flagged))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_modifiers: missing v"]
        );

        // No root at all is a failure of the same field, never a silent pass.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_modifiers: no root produced"]
        );
    }

    /// `must_contain_env_vars` in both directions, the same way
    /// `must_contain_modifiers` is exercised above and for the same reason.
    ///
    /// Case is asserted explicitly: a variable name's case is meaningful to
    /// the shell (`NODE_DEBUG` and `node_debug` are different variables),
    /// so a matcher that folded case could satisfy a claim with the wrong
    /// variable and never say so.
    #[test]
    fn must_contain_env_vars_names_the_variables_that_are_missing() {
        let contract = ContractMeta {
            must_contain_env_vars: vec!["NODE_DEBUG".into(), "NO_COLOR".into()],
            ..ContractMeta::default()
        };
        let mut root = CommandNode::new("node", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_env_vars: missing NODE_DEBUG, NO_COLOR"]
        );

        root.entities.push(Entity::env_var_item(
            "NODE_DEBUG",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_env_vars: missing NO_COLOR"]
        );

        // The lowercase twin does not satisfy the uppercase claim.
        root.entities.push(Entity::env_var_item(
            "no_color",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_env_vars: missing NO_COLOR"]
        );

        root.entities.push(Entity::env_var_item(
            "NO_COLOR",
            Provenance::single(Source::HelpText),
        ));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // A flag spelled with the same word is a different item and never
        // satisfies an env-var claim.
        let flag_only = ContractMeta {
            must_contain_env_vars: vec!["DEBUG".into()],
            ..ContractMeta::default()
        };
        let mut flagged = CommandNode::new("node", Provenance::single(Source::HelpText));
        flagged.entities.push(Entity::flag_long(
            "DEBUG",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&flag_only, Some(&flagged))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_env_vars: missing DEBUG"]
        );

        // No root at all is a failure of the same field, never a silent pass.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_env_vars: no root produced"]
        );
    }

    /// `must_not_contain_flags` in both directions, plus the two things it
    /// deliberately does not claim. The motivating instance is a phantom
    /// long name (`corpus/mariadb-check/2.7.4`'s header ruler), so a
    /// dash-only spelling is the one exercised here rather than a tidy
    /// synthetic name.
    #[test]
    fn must_not_contain_flags_names_the_flags_the_parser_invented() {
        // Written as a contributor would type it: the full 33-dash ruler,
        // whose long name after `--` is 31 dashes.
        let ruler = "---------------------------------";
        let contract = ContractMeta {
            must_not_contain_flags: vec![ruler.into(), "--bogus".into()],
            ..ContractMeta::default()
        };
        let mut root = CommandNode::new("mariadb-check", Provenance::single(Source::HelpText));

        // A tree that invents neither is clean.
        root.entities.push(Entity::flag_long(
            "check",
            Provenance::single(Source::HelpText),
        ));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // The phantom appears: reported, and named by the spelling the
        // fixture author wrote, not by the stripped long name.
        root.entities.push(Entity::flag_long(
            "-------------------------------",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_not_contain_flags: present ---------------------------------"]
        );

        // Both present, both named, in the fixture's own order.
        root.entities.push(Entity::flag_long(
            "bogus",
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_not_contain_flags: present ---------------------------------, --bogus"]
        );

        // What it does not claim (1): nothing about the other spelling. A
        // `--bogus` entry says nothing about a *short* `-b`.
        let short_only = ContractMeta {
            must_not_contain_flags: vec!["--b".into()],
            ..ContractMeta::default()
        };
        let mut shorty = CommandNode::new("t", Provenance::single(Source::HelpText));
        let short_flag = Entity::flag_short('b', Provenance::single(Source::HelpText));
        shorty.entities.push(short_flag);
        assert!(check_contract(&short_only, Some(&shorty)).is_empty());

        // What it does not claim (2): nothing below the root. A subcommand
        // carrying the forbidden spelling is out of scope.
        let mut with_child = CommandNode::new("t", Provenance::single(Source::HelpText));
        let mut child = CommandNode::new("sub", Provenance::single(Source::HelpText));
        child.entities.push(Entity::flag_long(
            "bogus",
            Provenance::single(Source::HelpText),
        ));
        with_child.subcommands.push(child);
        let bogus_only = ContractMeta {
            must_not_contain_flags: vec!["--bogus".into()],
            ..ContractMeta::default()
        };
        assert!(check_contract(&bogus_only, Some(&with_child)).is_empty());

        // No root at all satisfies a negative claim vacuously — the one
        // place this field is *not* symmetric with the positive ones, and
        // reporting it would be a violation of a promise that holds.
        assert!(check_contract(&contract, None).is_empty());
    }

    /// End-to-end: a real `[contract]` with `must_not_contain_flags` set,
    /// read from `meta.toml` rather than constructed in Rust, so the serde
    /// field name is exercised too. `MYTOOL_HELP` has `--verbose` and no
    /// `--invented`, so the first spelling fails the run and the second
    /// does not.
    #[test]
    fn must_not_contain_flags_is_read_from_meta_toml_and_fails_the_run() {
        for (forbidden, should_fail) in [("--verbose", true), ("--invented", false)] {
            let corpus = setup();
            let dir = corpus.root.join("negtool/1.0");
            write(
                &dir.join("meta.toml"),
                &format!(
                    r#"
[bless]
provenance = "agent"

[tool]
name = "negtool"
version = "1.0"

[[capture]]
argv = ["negtool", "--help"]
stdout = "help.txt"

[contract]
must_not_contain_flags = ["{forbidden}"]
"#
                ),
            );
            write(&dir.join("help.txt"), MYTOOL_HELP);
            run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");
            let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
            assert_eq!(
                report.failed(),
                should_fail,
                "must_not_contain_flags = [{forbidden:?}]: {}",
                report.text
            );
            if should_fail {
                assert!(
                    report
                        .text
                        .contains("must_not_contain_flags: present --verbose"),
                    "{}",
                    report.text
                );
            }
        }
    }

    /// `must_keep_separate` in both directions: two spellings that resolve
    /// to two different entities pass; two spellings folded onto one
    /// entity fail, naming the group and the spellings that collapsed.
    /// Built by hand rather than through a real fold, since this is the
    /// exact defect an earlier alias-run fold caused (`-w`/`-X` fused by a
    /// description-equality merge) — the check must catch it regardless of
    /// how the fold happens.
    #[test]
    fn must_keep_separate_names_the_group_and_spellings_that_collapsed() {
        let contract = ContractMeta {
            must_keep_separate: vec![
                vec!["-w".into(), "-X".into()],
                vec!["-C".into(), "-CC".into()],
            ],
            ..ContractMeta::default()
        };

        // Two entities, one spelling each: nothing collapsed.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(Entity::flag_short(
            'w',
            Provenance::single(Source::HelpText),
        ));
        root.entities.push(Entity::flag_short(
            'X',
            Provenance::single(Source::HelpText),
        ));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // `-w` and `-X` folded onto one multi-spelling entity: reported,
        // naming the group and the two spellings that share it. `-C`/`-CC`
        // never even resolve here, so that group is silent.
        let mut fused = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut fused_entity = Entity::flag_short('w', Provenance::single(Source::HelpText));
        fused_entity.spellings.push(Spelling::short('X'));
        fused.entities.push(fused_entity);
        assert_eq!(
            check_contract(&contract, Some(&fused))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_keep_separate: [\"-w\", \"-X\"] collapsed onto one entity: -w, -X"]
        );

        // A spelling absent from the tree entirely never collapses with
        // anything — the group is silent unless two of its spellings both
        // resolved to the same entity.
        let solo_only = ContractMeta {
            must_keep_separate: vec![vec!["-w".into(), "-Z".into()]],
            ..ContractMeta::default()
        };
        let mut solo = CommandNode::new("tool", Provenance::single(Source::HelpText));
        solo.entities.push(Entity::flag_short(
            'w',
            Provenance::single(Source::HelpText),
        ));
        assert!(check_contract(&solo_only, Some(&solo)).is_empty());

        // No root at all is a negative claim, satisfied vacuously — same
        // reasoning as `must_not_contain_flags`.
        assert!(check_contract(&contract, None).is_empty());
    }

    /// `must_attach_choices` in both directions: the flag missing, the
    /// flag present but missing a choice value, and the flag carrying
    /// every named value.
    #[test]
    fn must_attach_choices_names_the_flag_or_the_missing_values() {
        let mut choices = std::collections::BTreeMap::new();
        choices.insert(
            "--warnings".to_string(),
            vec!["gnu".to_string(), "obsolete".to_string()],
        );
        let contract = ContractMeta {
            must_attach_choices: choices,
            ..ContractMeta::default()
        };

        // Flag absent entirely.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_attach_choices[\"--warnings\"]: flag not present"]
        );

        // Flag present, but its choices don't carry either named value.
        let mut warnings = Entity::flag_long("warnings", Provenance::single(Source::HelpText));
        warnings.choices.push(mandible_core::Choice::bare("gnu"));
        root.entities.push(warnings);
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_attach_choices[\"--warnings\"]: missing obsolete"]
        );

        // Both values attached: clean.
        root.entities
            .iter_mut()
            .find(|e| e.spellings.iter().any(|s| s.name == "warnings"))
            .unwrap()
            .choices
            .push(mandible_core::Choice::bare("obsolete"));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // No root at all fails this positive claim exactly as
        // `must_contain_flags` fails.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_attach_choices: no root produced"]
        );
    }

    /// `must_describe` in both directions, plus the whitespace-collapsing
    /// rule (a wrapped description) and the truncated mismatch message.
    #[test]
    fn must_describe_names_the_flag_or_shows_expected_vs_actual() {
        let mut describe = std::collections::BTreeMap::new();
        describe.insert(
            "--target".to_string(),
            "the triple to build for".to_string(),
        );
        let contract = ContractMeta {
            must_describe: describe,
            ..ContractMeta::default()
        };

        // Flag absent entirely.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_describe[\"--target\"]: flag not present"]
        );

        // Flag present, description doesn't contain the expected text.
        let mut target = Entity::flag_long("target", Provenance::single(Source::HelpText));
        target.description = Some(mandible_core::Text::sanitize("build for this host only"));
        root.entities.push(target);
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec![
                "must_describe[\"--target\"]: expected description to contain \"the triple to build for\", got \"build for this host only\""
            ]
        );

        // A wrapped description collapses to the same text a fixture
        // author would have typed on one line.
        root.entities
            .iter_mut()
            .find(|e| e.spellings.iter().any(|s| s.name == "target"))
            .unwrap()
            .description = Some(mandible_core::Text::sanitize(
            "set   the triple\nto build for   and stop",
        ));
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // No root at all fails this positive claim exactly as
        // `must_contain_flags` fails.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_describe: no root produced"]
        );
    }

    /// `must_value_name` in both directions, including the case that
    /// distinguishes it from `must_describe`: one spelling heading two
    /// rows, where only the second carries the value the contract names.
    #[test]
    fn must_value_name_scans_every_entity_carrying_the_spelling() {
        let mut values = std::collections::BTreeMap::new();
        values.insert("-r".to_string(), "file name".to_string());
        let contract = ContractMeta {
            must_value_name: values,
            ..ContractMeta::default()
        };

        // Flag absent entirely.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_value_name[\"-r\"]: flag not present"]
        );

        // One row, no value at all.
        root.entities.push(Entity::flag_short(
            'r',
            Provenance::single(Source::HelpText),
        ));
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec![
                "must_value_name[\"-r\"]: expected a value name containing \"file name\", got [\"\"]"
            ]
        );

        // A second row for the same spelling, carrying a truncated value:
        // both are reported, and neither satisfies the claim.
        let mut second = Entity::flag_short('r', Provenance::single(Source::HelpText));
        second.value_name = Some("(with".to_string());
        root.entities.push(second);
        assert_eq!(
            check_contract(&contract, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec![
                "must_value_name[\"-r\"]: expected a value name containing \"file name\", got [\"\", \"(with\"]"
            ]
        );

        // The whole value on the second row satisfies it, even though the
        // first row still has none. `must_describe` would stop at the
        // first row and report a failure here.
        root.entities.last_mut().unwrap().value_name = Some("(with file name)".to_string());
        assert!(check_contract(&contract, Some(&root)).is_empty());

        // No root at all fails this positive claim.
        assert_eq!(
            check_contract(&contract, None)
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_value_name: no root produced"]
        );
    }

    /// A `must_contain_positionals` entry ending in `...` asserts the
    /// operand is repeatable as well as present. Without the suffix the
    /// same entry says nothing about repetition.
    #[test]
    fn must_contain_positionals_suffix_asserts_repetition() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(Entity::positional(
            "file",
            Provenance::single(Source::HelpText),
        ));

        let plain = ContractMeta {
            must_contain_positionals: vec!["file".to_string()],
            ..ContractMeta::default()
        };
        assert!(check_contract(&plain, Some(&root)).is_empty());

        let repeated = ContractMeta {
            must_contain_positionals: vec!["file...".to_string()],
            ..ContractMeta::default()
        };
        assert_eq!(
            check_contract(&repeated, Some(&root))
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            vec!["must_contain_positionals: missing file..."]
        );

        root.entities.last_mut().unwrap().repeatable = true;
        assert!(check_contract(&repeated, Some(&root)).is_empty());
        assert!(check_contract(&plain, Some(&root)).is_empty());
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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
            sub.flags().any(|f| f.long() == Some("deep")),
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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
        // Never a raw YAML diff: the compact snapshot's own field name
        // `heading_attested`, and `ProvenanceSnapshot`'s own `sources` key
        // (`mandible_core::snapshot`), must not leak into the markdown
        // report as if it were the file's literal text. (The bare word
        // "provenance" legitimately appears now — the `[bless] provenance`
        // column this report adds — so it is no longer the right marker for
        // a YAML leak; `sources:` is, since nothing else in this report
        // produces that key.)
        assert!(!report.text.contains("heading_attested"));
        assert!(!report.text.contains("sources:"));
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

    /// A fixture whose `[contract]` pins a modifier list, for the
    /// weakening test below. Separate from [`full_contract_fixture`]
    /// rather than a fourth parameter on it: every existing caller of that
    /// helper would have to grow an argument it does not care about.
    fn modifier_contract_fixture(root: &Path, must_contain_modifiers: &[&str]) {
        let list = must_contain_modifiers
            .iter()
            .map(|m| format!("{m:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        write(
            &root.join("fulltool/1.0/meta.toml"),
            &format!(
                r#"
[bless]
provenance = "agent"

[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
must_contain_modifiers = [{list}]
"#
            ),
        );
        write(&root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
    }

    /// Dropping a letter from `must_contain_modifiers` is a weakening and
    /// must be reported by name, exactly as dropping a flag or an operand
    /// is. Without this arm the new field would be the one contract field
    /// a `--baseline-dir` run could not see shrink.
    #[test]
    fn dropping_a_required_modifier_is_reported_as_weakening() {
        let baseline = setup();
        let current = setup();
        modifier_contract_fixture(&baseline.root, &["a", "U", "v"]);
        modifier_contract_fixture(&current.root, &["a", "v"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("must_contain_modifiers (dropped: U)"),
            "{lines:?}"
        );

        // Adding one is a tightening, never flagged.
        let tightened = setup();
        modifier_contract_fixture(&tightened.root, &["a", "U", "v", "D"]);
        let tightened_fixtures = discover_fixtures(&tightened.root).unwrap();
        assert!(contract_weakened_lines(&tightened_fixtures, &base_fixtures).is_empty());
    }

    fn env_var_contract_fixture(root: &Path, must_contain_env_vars: &[&str]) {
        let list = must_contain_env_vars
            .iter()
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        write(
            &root.join("fulltool/1.0/meta.toml"),
            &format!(
                r#"
[bless]
provenance = "agent"

[tool]
name = "fulltool"
version = "1.0"

[[capture]]
argv = ["fulltool", "--help"]
stdout = "help.txt"

[contract]
must_contain_env_vars = [{list}]
"#
            ),
        );
        write(&root.join("fulltool/1.0/help.txt"), MYTOOL_HELP);
    }

    /// Dropping a variable from `must_contain_env_vars` is a weakening and
    /// must be reported by name, the same way dropping a modifier letter is
    /// — without this arm the new field would be the one contract field a
    /// `--baseline-dir` run could not see shrink.
    #[test]
    fn dropping_a_required_env_var_is_reported_as_weakening() {
        let baseline = setup();
        let current = setup();
        env_var_contract_fixture(&baseline.root, &["NODE_DEBUG", "NO_COLOR", "FORCE_COLOR"]);
        env_var_contract_fixture(&current.root, &["NODE_DEBUG", "FORCE_COLOR"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("must_contain_env_vars (dropped: NO_COLOR)"),
            "{lines:?}"
        );

        // Adding one is a tightening, never flagged.
        let tightened = setup();
        env_var_contract_fixture(
            &tightened.root,
            &["NODE_DEBUG", "NO_COLOR", "FORCE_COLOR", "NODE_OPTIONS"],
        );
        let tightened_fixtures = discover_fixtures(&tightened.root).unwrap();
        assert!(contract_weakened_lines(&tightened_fixtures, &base_fixtures).is_empty());
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
[bless]
provenance = "agent"

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

    /// A `[contract]` carrying only `must_not_contain_flags`, written
    /// straight into `meta.toml`. `entries` is spliced as a TOML array so
    /// the baseline and current sides differ in exactly that one field.
    fn negative_contract_fixture(root: &Path, entries: &[&str]) {
        let list = entries
            .iter()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        write(
            &root.join("negtool/1.0/meta.toml"),
            &format!(
                r#"
[bless]
provenance = "agent"

[tool]
name = "negtool"
version = "1.0"

[[capture]]
argv = ["negtool", "--help"]
stdout = "help.txt"

[contract]
must_not_contain_flags = [{list}]
"#
            ),
        );
        write(&root.join("negtool/1.0/help.txt"), MYTOOL_HELP);
    }

    /// A negative claim weakens by *losing* an entry, exactly as a positive
    /// one does. Without this the field could be deleted from a fixture in
    /// a PR and nothing would say so — which is the whole failure mode
    /// `contract_weakened_lines` exists to prevent, and a contract field
    /// that cannot weaken-detect is one that can be quietly deleted.
    #[test]
    fn a_dropped_must_not_contain_flag_is_flagged() {
        let baseline = setup();
        let current = setup();
        let ruler = "---------------------------------";
        negative_contract_fixture(&baseline.root, &[ruler, "--invented"]);
        negative_contract_fixture(&current.root, &["--invented"]);
        let base_fixtures = discover_fixtures(&baseline.root).unwrap();
        let cur_fixtures = discover_fixtures(&current.root).unwrap();
        let lines = contract_weakened_lines(&cur_fixtures, &base_fixtures);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("must_not_contain_flags") && l.contains(ruler)),
            "{lines:?}"
        );

        // Dropping the field entirely is the same weakening, not a
        // special case that slips through.
        let emptied = setup();
        negative_contract_fixture(&emptied.root, &[]);
        let emptied_fixtures = discover_fixtures(&emptied.root).unwrap();
        let lines = contract_weakened_lines(&emptied_fixtures, &base_fixtures);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("must_not_contain_flags") && l.contains("--invented")),
            "{lines:?}"
        );

        // Adding an entry tightens, and is never flagged.
        let tightened = setup();
        negative_contract_fixture(&tightened.root, &[ruler, "--invented", "--also"]);
        let tightened_fixtures = discover_fixtures(&tightened.root).unwrap();
        assert!(
            contract_weakened_lines(&tightened_fixtures, &base_fixtures).is_empty(),
            "a tightened negative contract must never be flagged"
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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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
[bless]
provenance = "agent"

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

    // --- [bless] provenance (human-vs-agent, the complement to verdict_scope) ---

    /// A fixture whose `meta.toml` has no `[bless]` table at all must fail
    /// to load, loudly, naming the offending file and pointing at
    /// `corpus/README.md` — never silently default to any provenance,
    /// since a silent default here is exactly the overclaim-by-omission
    /// this field exists to prevent (mirroring `verdict_scope`'s own
    /// "absent means no claim" rule, but for the bless act itself, where
    /// the only safe absent-default would be "agent" and the schema
    /// requires it be written down instead of assumed).
    #[test]
    fn missing_bless_table_fails_to_load_naming_the_fixture() {
        let corpus = setup();
        let dir = corpus.root.join("noblesstool/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[tool]
name = "noblesstool"
version = "1.0"

[[capture]]
argv = ["noblesstool", "--help"]
stdout = "help.txt"
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        let result = discover_fixtures(&corpus.root);
        let err = match result {
            Ok(_) => panic!("a fixture with no [bless] provenance must fail to load"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("noblesstool"), "{msg}");
        assert!(msg.contains("meta.toml"), "{msg}");
        assert!(msg.contains("[bless] provenance"), "{msg}");
        assert!(msg.contains("corpus/README.md"), "{msg}");
    }

    /// Same failure when `[bless]` is present but empty (its required
    /// `provenance` key missing) — the friendly guard checks the key, not
    /// just the table.
    #[test]
    fn bless_table_without_provenance_key_fails_to_load() {
        let corpus = setup();
        let dir = corpus.root.join("emptybless/1.0");
        write(
            &dir.join("meta.toml"),
            r#"
[bless]

[tool]
name = "emptybless"
version = "1.0"

[[capture]]
argv = ["emptybless", "--help"]
stdout = "help.txt"
"#,
        );
        write(&dir.join("help.txt"), MYTOOL_HELP);
        let result = discover_fixtures(&corpus.root);
        let err = match result {
            Ok(_) => panic!("a [bless] table with no provenance key must fail to load"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("emptybless"), "{err}");
    }

    /// The summary line splits the `ok` count by provenance, so "N ok" can
    /// never be misread as "N human-verified" (`corpus/README.md`'s
    /// `[bless]` section). One fixture of each provenance value, all
    /// green, must produce all three counts in the one summary line.
    #[test]
    fn summary_line_splits_ok_count_by_provenance() {
        let corpus = setup();
        for (name, provenance) in [
            ("humantool", "human"),
            ("mixedtool", "agent-then-human"),
            ("agenttool", "agent"),
        ] {
            let dir = corpus.root.join(format!("{name}/1.0"));
            write(
                &dir.join("meta.toml"),
                &format!(
                    r#"
[bless]
provenance = "{provenance}"

[tool]
name = "{name}"
version = "1.0"

[[capture]]
argv = ["{name}", "--help"]
stdout = "help.txt"
"#
                ),
            );
            write(&dir.join("help.txt"), MYTOOL_HELP);
        }
        run(&corpus.root, true, ScoreFormat::Text).expect("bless run succeeds");
        let report = run(&corpus.root, false, ScoreFormat::Text).expect("check run succeeds");
        assert!(!report.failed(), "{}", report.text);
        assert!(
            report
                .text
                .contains("3 ok (1 human, 1 agent-then-human, 1 agent)"),
            "{}",
            report.text
        );
    }
}
