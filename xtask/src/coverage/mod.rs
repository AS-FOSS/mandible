//! The extraction coverage harness (spec §13.1): runs the full tiered
//! pipeline against every executable on `PATH` and emits a scoreboard, so a
//! parser change is checked fleet-wide rather than against one tool.
//!
//! Columns include a `framework` field (spec §7 Tier A′), a `verbatim`
//! status (spec §7 Tier B step 3), and a `--format markdown` mode the
//! framework-support CI workflow (spec §13.1a) consumes.

use crate::alternation;
use crate::bundling;
use crate::existence;
use crate::misattribution::{self, RecordingProbe};
use crate::repeated_char;
use crate::single_dash_long;
use mandible_extract::{default_tiers_with_probe, resolve_tool, ExtractionResult, Runner};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Fixed display width for the `tool` column in [`ScoreFormat::Text`]
/// output. Truncated (never just left unbounded) because real tool names
/// on a real `PATH` blow past any reasonable assumption —
/// `aarch64-linux-gnu-cpp-13` (24 chars), `UnicodeNameMappingGenerator-18`
/// (31 chars) — and an untruncated long name shoves every column after it
/// out of alignment for that one row, which is exactly the bug this
/// constant (and [`truncate_col`]) exists to fix.
pub(crate) const TOOL_COL_WIDTH: usize = 24;
/// Fixed display width for the `tier(s)` column, same reasoning.
pub(crate) const TIER_COL_WIDTH: usize = 18;
/// Fixed display width for the new `framework` column, same reasoning.
pub(crate) const FRAMEWORK_COL_WIDTH: usize = 26;
/// Fixed display width for the right-aligned `nodes` column.
pub(crate) const NODES_COL_WIDTH: usize = 7;
/// Fixed display width for the right-aligned `flags` column.
pub(crate) const FLAGS_COL_WIDTH: usize = 8;
/// Fixed display width for the right-aligned `%flags_text` column.
pub(crate) const PCT_COL_WIDTH: usize = 13;
/// Fixed display width for the right-aligned `ms` column.
pub(crate) const MS_COL_WIDTH: usize = 7;
/// Fixed display width for the right-aligned `suspect` column.
pub(crate) const SUSPECT_COL_WIDTH: usize = 8;
/// Fixed display width for the right-aligned `man` column.
pub(crate) const MAN_COL_WIDTH: usize = 6;
/// Fixed display width for the right-aligned `misattr` column.
///
/// All eight widths above are `pub(crate)`, not local to [`render_text`]:
/// [`crate::transition`] parses a rendered `ScoreFormat::Text` scoreboard
/// back into rows by slicing at these exact offsets. A single source of
/// truth avoids the two-copy drift risk `status.rs`'s doc comment warns
/// about.
pub(crate) const MISATTR_COL_WIDTH: usize = 9;
/// Fixed display width for the right-aligned `exist` column
/// ([`crate::existence`]'s fabrication count).
pub(crate) const EXISTENCE_COL_WIDTH: usize = 6;
/// Fixed display width for the right-aligned `bundle` column
/// ([`crate::bundling`]'s collapse count).
pub(crate) const BUNDLE_COL_WIDTH: usize = 7;

/// One tool's row in the scoreboard.
struct Row {
    tool: String,
    tiers: String,
    /// The detected framework (spec §7 Tier A′) plus how it was detected,
    /// e.g. `"clap (v3/v4) (artifact)"`, or `"—"` when unidentified. See
    /// [`framework_label`].
    framework: String,
    nodes: usize,
    /// Raw flag count, including usage-synopsis-only flags that can never
    /// carry a description ([M-15]). Kept separate from [`Self::describable`]
    /// per spec §13's metric design rules.
    flags: usize,
    /// Flags whose source could, in principle, carry a description — the
    /// denominator [`Self::pct_flags_with_text`] is computed over. See
    /// [`mandible_extract::ExtractionResult::describable_flag_count`].
    describable: usize,
    /// `None` when there are no describable flags to compute a percentage
    /// over.
    ///
    /// **Presence, not correctness** — never checks whether the attached
    /// text is the *right* text (`corpus/lsof/4.95.0`, `[xfail]`; see
    /// [`crate::misattribution`] for the accuracy instrument). Every
    /// scoreboard also carries `accuracy: unmeasured` —
    /// [`accuracy_unmeasured_line`].
    pct_flags_with_text: Option<f64>,
    ms: u128,
    /// Structure-sanity count (spec §13.1): descendant nodes failing
    /// [`mandible_core::is_command_name_shaped`], or with no flags,
    /// children, or summary. Non-zero forces `status` to `"suspicious"`
    /// regardless of `%described`, since fabricated structure *inflates*
    /// that number ([M-10]).
    suspicious_nodes: usize,
    /// True when the root node degraded to spec §7 Tier B step 3's
    /// verbatim rendering (`CommandNode::unparsed` non-empty) rather than
    /// producing any structure at all.
    verbatim: bool,
    /// True when the root `--help` probe's captured output was detected as
    /// a rendered man page (spec [M-16]) rather than ordinary help text —
    /// see [`root_is_man_shaped`]. A measurement column only: this is the
    /// exposure enumeration for a pending, not-yet-implemented safety
    /// decision (falling back to `-h` when this fires), so it is reported
    /// but never gated (spec [M-16], [`compute_aggregate`]'s doc comment
    /// on why `verbatim_count` gets the same treatment).
    man_shaped: bool,
    /// [`crate::misattribution`]'s own measurement: count of this tool's
    /// flag descriptions that contain a flag-shaped token attested at a
    /// column-aligned definition position elsewhere in the tool's raw
    /// captured `--help` text — `lsof`'s bug, generalized. **Not gated**
    /// (see that module's doc comment): a brand-new detector with a
    /// measured, nonzero false-positive rate must not fail a build the
    /// first time it runs.
    misattribution_suspect_count: usize,
    /// [`misattribution::MisattributionReport::column_aligned`]'s own
    /// report: whether this tool's raw text had at least one column offset
    /// that met the column-alignment recurrence bar at all — reported
    /// separately from `misattribution_suspect_count` because a tool can
    /// have a real multi-column table (`column_aligned: true`) whose
    /// descriptions just never happen to mention a neighbouring column, and
    /// that's a materially different "nothing to find here" than a tool
    /// whose text never had a second column in the first place.
    misattribution_column_aligned: bool,
    /// A few of this row's own suspects, pre-formatted for the sweep's
    /// `# misattribution-suspects (sample)` section — capped per row
    /// ([`MISATTRIBUTION_SAMPLES_PER_ROW`]) so one pathological tool with
    /// hundreds of suspect flags can't crowd out every other tool's sample
    /// from a fleet-wide report.
    misattribution_samples: Vec<String>,
    /// [`crate::existence`]'s own measurement: count of this tool's help-
    /// text-sourced subcommand names and flag spellings that do not occur
    /// literally in the tool's own raw captured `--help` text — [M-10]'s
    /// shape, generalized, and this task's own instrument. **Not gated**,
    /// same reasoning as `misattribution_suspect_count`: a brand-new
    /// detector with no fleet-wide baseline must not fail a build the
    /// first time it runs (spec §13.1b).
    existence_fabrication_count: usize,
    /// A few of this row's own fabrications, pre-formatted, mirroring
    /// [`Self::misattribution_samples`] — capped per row
    /// ([`EXISTENCE_SAMPLES_PER_ROW`]).
    existence_samples: Vec<String>,
    /// [`crate::bundling`]'s own measurement: count of this tool's synopsis
    /// flag clusters (`[-2CDlNuVv]`) read as one value-taking flag instead
    /// of the several boolean flags they name. **Not gated**, same
    /// reasoning as the two counts above: a brand-new detector with no
    /// fleet-wide baseline must not fail a build the first time it runs
    /// (spec §13.1b).
    bundle_collapse_count: usize,
    /// How many real flags this row's collapses destroyed — every cluster
    /// member after the first. Carried separately from
    /// `bundle_collapse_count` because the two answer different questions
    /// and differ by more than an order of magnitude on a single tool:
    /// `tcpdump` is *one* collapse and *25* destroyed flags, so a count of
    /// collapses alone says nothing about how much recall the defect costs.
    bundle_destroyed_flags: usize,
    /// A few of this row's own collapses, pre-formatted, mirroring
    /// [`Self::existence_samples`] — capped per row
    /// ([`BUNDLE_SAMPLES_PER_ROW`]).
    bundle_samples: Vec<String>,
    /// [`crate::alternation`]'s own measurement: flag spellings this tool
    /// writes inside a delimited alternation group (`{-i|--input}`,
    /// `[[-c|-C] cmd]`) that reach no flag in its tree, plus any that reach
    /// one still carrying the group's punctuation as a value.
    alternation_defect_count: usize,
    /// A few of this row's own, pre-formatted, mirroring
    /// [`Self::bundle_samples`] — capped per row
    /// ([`ALTERNATION_SAMPLES_PER_ROW`]).
    alternation_samples: Vec<String>,
    /// How many `commands:` tables this row's help text offers whose every
    /// name is missing from the tree (`crate::commandtable`). Shape A of
    /// the four-grammar `unparsed-subcommand` split; the other three
    /// shapes are deliberately not counted here — see that module.
    command_table_count: usize,
    /// [`crate::single_dash_long`]'s own measurement: count of this tool's
    /// option-table rows naming a single-dash long option (`-help`) that
    /// split into a one-character short flag plus a required value. The
    /// second of the three families sharing the `short && !long &&
    /// value_name` fingerprint. **Not gated until the family is repaired**,
    /// same reasoning as every count above: a brand-new detector with no
    /// fleet-wide baseline must not fail a build the first time it runs
    /// (spec §13.1b).
    single_dash_split_count: usize,
    /// A few of this row's own splits, pre-formatted, capped per row
    /// ([`SPLIT_SAMPLES_PER_ROW`]).
    single_dash_samples: Vec<String>,
    /// [`crate::repeated_char`]'s own measurement: count of this tool's
    /// repeated-character flags (`-vv`) read as the bare short flag carrying
    /// its own letter as a required value. The third family. Same gating
    /// note as above.
    repeated_char_misread_count: usize,
    /// A few of this row's own misreads, pre-formatted, capped per row
    /// ([`SPLIT_SAMPLES_PER_ROW`]).
    repeated_char_samples: Vec<String>,
    status: &'static str,
    /// This tool's field-level fingerprint (WS2 part 2,
    /// [`crate::transition`]'s per-tool diff): enough for `sweep-diff` to
    /// tell a per-flag description/choices/value_name *change* apart from a
    /// count that merely stayed the same. See [`build_fingerprint`]'s doc
    /// comment for why the scoreboard's existing columns (flag counts,
    /// `%flags_text`) cannot see this — that gap is exactly what let PR #14
    /// delete `pngfix`'s and `pod2man`'s descriptions and fabricate a
    /// choices list while `sweep-diff` reported the run unchanged.
    fingerprint: ToolFingerprint,
}

/// One entity's field-level fingerprint (flag, positional, modifier, or
/// env-var item — see [`ToolFingerprint::flags`]): whether it has a
/// description, a hash of the description text, a hash of its choices
/// list, and `value_name` verbatim. Named `FlagFingerprint` from when
/// flags were the only kind fingerprinted; every field applies unchanged
/// to every `EntityKind`.
///
/// Hashes, not full text, for description/choices — keeps the checked-in
/// scoreboard small; it only needs to know "did this change," and the
/// scoreboard files being diffed are on disk for a human to read if so.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlagFingerprint {
    has_description: bool,
    description_hash: Option<u64>,
    choices_hash: Option<u64>,
    value_name: Option<String>,
}

/// One tool's full field-level fingerprint: every entity — flag,
/// positional, modifier, env-var item — keyed by a stable per-node
/// identity ([`entity_identity`]; never `Entity::spelling`, which folds
/// the value placeholder in), plus the full set of subcommand paths.
///
/// Field still named `flags` from when flags were the only kind
/// fingerprinted; now holds every kind.
///
/// A size comparison against the scoreboard's `flags` column must filter
/// ids to `EntityKind::Flag` first, or it silently reports zero
/// duplicate-carrying tools instead of erroring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ToolFingerprint {
    flags: BTreeMap<String, FlagFingerprint>,
    subcommands: std::collections::BTreeSet<String>,
}

/// An entity's identity for fingerprinting: every documented spelling's
/// dash-prefixed name, excluding `value_name`/`choices`/description (the
/// fields this fingerprint exists to detect changes in — folding them into
/// the key would turn a `value_name` edit into a remove-then-add).
/// Prefixed with the owning node's dotted path and the entity's
/// `EntityKind` (via `{:?}`) so same-spelled entities on different
/// subcommands or of different kinds never collide.
///
/// Generic over `EntityKind` by construction (derived `{:?}`), not a match
/// arm — AGENTS.md §1: no per-kind branching to grow.
fn entity_identity(path: &str, entity: &mandible_core::Entity) -> String {
    let spelling = entity
        .spellings
        .iter()
        .map(|s| {
            let dash = match s.dashes {
                mandible_core::Dashes::None => "",
                mandible_core::Dashes::Single => "-",
                mandible_core::Dashes::Double => "--",
            };
            format!("{dash}{}", s.name)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{path}::{:?}::{spelling}", entity.kind)
}

/// FNV-1a over raw bytes — deterministic across processes and Rust std
/// versions (unlike `DefaultHasher`), needed because hashes from separate
/// `xtask` invocations are compared across a sweep.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Walk `root`'s tree and build its field-level [`ToolFingerprint`] — the
/// data [`Row::fingerprint`] carries and [`fingerprint_lines`] serializes
/// into the scoreboard's `#fp` footer, which [`crate::transition`] reads
/// back to diff at field granularity, since a count column alone cannot
/// distinguish "description text changed" from "it didn't."
///
/// Walks `node.entities` (every `EntityKind`), not `node.flags()` alone,
/// so a fingerprint isn't blind to env-var/modifier/positional changes.
/// See [`entity_identity`] for the no-per-kind-branching discipline.
fn build_fingerprint(root: Option<&mandible_core::CommandNode>) -> ToolFingerprint {
    let mut fp = ToolFingerprint::default();
    let Some(root) = root else {
        return fp;
    };
    fn walk(node: &mandible_core::CommandNode, path: &str, fp: &mut ToolFingerprint) {
        for entity in &node.entities {
            let id = entity_identity(path, entity);
            let description_hash = entity
                .description
                .as_ref()
                .map(|t| fnv1a(t.as_str().as_bytes()));
            let choices_hash = if entity.choices.is_empty() {
                None
            } else {
                // Name and per-choice description (ffmpeg's AVOption
                // constants) both feed the hash, so a description-only
                // edit still moves the fingerprint.
                let joined = entity
                    .choices
                    .iter()
                    .map(|c| match &c.description {
                        Some(d) => format!("{}\u{1e}{}", c.name, d.as_str()),
                        None => c.name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                Some(fnv1a(joined.as_bytes()))
            };
            fp.flags.insert(
                id,
                FlagFingerprint {
                    has_description: entity.description.is_some(),
                    description_hash,
                    choices_hash,
                    value_name: entity.value_name.clone(),
                },
            );
        }
        for sub in &node.subcommands {
            let sub_path = if path == "(root)" {
                sub.name.clone()
            } else {
                format!("{path}.{}", sub.name)
            };
            fp.subcommands.insert(sub_path.clone());
            walk(sub, &sub_path, fp);
        }
    }
    walk(root, "(root)", &mut fp);
    fp
}

/// Backslash-escape every character the `#fp` wire format uses as
/// structure, so the escaped output contains no raw separator character —
/// the read side (`crate::transition`'s `fp_unescape`) keeps its plain
/// `split`/`splitn` calls and only needs an unescape pass per field.
///
/// Escapes every separator, not just [`FP_FIELD_SEP`]: `value_name` is
/// free-form text lifted verbatim from a tool's own help output and can
/// contain any of them, e.g. `awk`'s `-L` flag value_name
/// `"fatal|invalid|no-ext"` collides with [`FP_FLAG_SEP`].
///
/// Escapes, per character: `\` -> `\\`, tab -> `\t`, newline -> `\n`,
/// [`FP_FLAG_SEP`] (`|`) -> `\p`, [`FP_SUBCOMMAND_SEP`] (`,`) -> `\c`,
/// [`FP_ID_SEP`] (`=`) -> `\e`, [`FP_ENTRY_SEP`] (`:`) -> `\s`.
///
/// Fixture: `corpus/awk/*/help.txt`, `corpus/gawk/*/help.txt`.
fn fp_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            FP_FIELD_SEP => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            FP_FLAG_SEP => out.push_str("\\p"),
            FP_SUBCOMMAND_SEP => out.push_str("\\c"),
            FP_ID_SEP => out.push_str("\\e"),
            FP_ENTRY_SEP => out.push_str("\\s"),
            _ => out.push(c),
        }
    }
    out
}

/// Top-level field separator inside one `#fp2` line (`#fp2 <tool>\t<subs>\t<entities>`)
/// — duplicated into `crate::transition` as its own `FP_FIELD_SEP` for the
/// same reason [`EXTRACT_TIMEOUT_MS`] is duplicated rather than imported: a
/// single well-known, stable character, re-measured in the same commit as
/// the other side if it ever changes. Escaped out of every emitted piece by
/// [`fp_escape`], same as the other three separators below.
const FP_FIELD_SEP: char = '\t';

/// Separator between entity entries inside one `#fp2` line's entity-entry
/// list (`<entity1>|<entity2>|...`) — mirrored in `crate::transition`.
const FP_FLAG_SEP: char = '|';

/// Separator between subcommand paths inside one `#fp2` line's subcommand
/// list (`<sub1>,<sub2>,...`) — mirrored in `crate::transition`.
const FP_SUBCOMMAND_SEP: char = ',';

/// Separator between one entity entry's id and its fields (`<id>=<fields>`) —
/// mirrored in `crate::transition`.
const FP_ID_SEP: char = '=';

/// Separator between one entity entry's fields
/// (`<has_desc>:<desc_hash>:<choices_hash>:<value_name>`) — mirrored in
/// `crate::transition`.
const FP_ENTRY_SEP: char = ':';

/// The `#fp2` line prefix ([`fingerprint_lines`]'s wire format, version 2).
/// A different literal from the pre-generalization `"#fp "` prefix
/// (`crate::transition::FP_LINE_PREFIX_V1`), not a bumped suffix: a v1
/// reader's `strip_prefix("#fp ")` doesn't match `"#fp2 ..."`, so it falls
/// back to the existing "predates the footer" path rather than misreading
/// v2 entity ids. See `crate::transition::FingerprintFormat` for why a
/// v1/v2 pair is refused outright rather than diffed.
const FP_LINE_PREFIX_V2: &str = "#fp2 ";

/// Render every row's [`ToolFingerprint`] as `#fp2` footer lines, one per
/// tool, in `rows`' existing sorted order ([`run_over`]).
///
/// One line per row unconditionally, even for empty `flags`/`subcommands`:
/// [`crate::transition`] tells "predates the footer" from "measured clean"
/// by whether a line exists, so skipping empty rows would hide a total
/// wipeout (entities on one side, none on the other) as "unmeasured"
/// instead of "every entity removed."
///
/// Line shape: `#fp2 <tool>\t<sub1>,<sub2>,...\t<entity1>|<entity2>|...`,
/// each entity `<id>=<has_desc:0/1>:<desc_hash-or-->:<choices_hash-or-->:<value_name-or-->`
/// (hex hashes, `id` carries its `EntityKind` tag). Every field individually
/// run through [`fp_escape`] first. Format version 2 — see
/// [`FP_LINE_PREFIX_V2`].
fn fingerprint_lines(rows: &[Row]) -> String {
    let mut out = String::new();
    for row in rows {
        let subs = row
            .fingerprint
            .subcommands
            .iter()
            .map(|s| fp_escape(s))
            .collect::<Vec<_>>()
            .join(",");
        let flags = row
            .fingerprint
            .flags
            .iter()
            .map(|(id, f)| {
                format!(
                    "{}={}:{}:{}:{}",
                    fp_escape(id),
                    if f.has_description { 1 } else { 0 },
                    f.description_hash
                        .map(|h| format!("{h:x}"))
                        .unwrap_or_else(|| "-".to_string()),
                    f.choices_hash
                        .map(|h| format!("{h:x}"))
                        .unwrap_or_else(|| "-".to_string()),
                    f.value_name
                        .as_deref()
                        .map(fp_escape)
                        .unwrap_or_else(|| "-".to_string()),
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        out.push_str(&format!(
            "{FP_LINE_PREFIX_V2}{}{FP_FIELD_SEP}{subs}{FP_FIELD_SEP}{flags}\n",
            fp_escape(&row.tool),
        ));
    }
    out
}

/// Cap on how many of one tool's own suspect descriptions feed the
/// fleet-wide sample section — see [`Row::misattribution_samples`].
const MISATTRIBUTION_SAMPLES_PER_ROW: usize = 3;

/// Cap on the total number of sample lines the fleet-wide
/// `# misattribution-suspects (sample)` section prints, mirroring
/// [`WORST_PARSED_LIMIT`]'s reasoning: a work-queue/audit aid needs to stay
/// scannable, not exhaustive — a human judging the false-positive rate
/// needs "enough to see the shape," not every hit on a full sweep.
const MISATTRIBUTION_SAMPLE_LIMIT: usize = 20;

/// Truncate a suspect's description to a length that keeps one sample line
/// readable — the full text is still in the tree the sweep already wrote,
/// this is a display concern only.
const MISATTRIBUTION_DESC_DISPLAY_LEN: usize = 70;

/// Cap on how many of one tool's own [`crate::existence`] fabrications feed
/// the fleet-wide sample section — mirrors
/// [`MISATTRIBUTION_SAMPLES_PER_ROW`]'s reasoning exactly.
const EXISTENCE_SAMPLES_PER_ROW: usize = 3;

/// Cap on the total number of sample lines the fleet-wide
/// `# existence-fabrications (sample)` section prints — mirrors
/// [`MISATTRIBUTION_SAMPLE_LIMIT`]'s reasoning exactly.
const EXISTENCE_SAMPLE_LIMIT: usize = 20;

/// Cap on how many of one tool's own [`crate::bundling`] collapses feed the
/// fleet-wide sample section — mirrors [`EXISTENCE_SAMPLES_PER_ROW`].
const BUNDLE_SAMPLES_PER_ROW: usize = 3;

/// Cap on the total number of sample lines the fleet-wide
/// `# bundled-short-flag collapses (sample)` section prints — mirrors
/// [`EXISTENCE_SAMPLE_LIMIT`].
const BUNDLE_SAMPLE_LIMIT: usize = 20;

/// Cap on how many of one tool's own [`crate::alternation`] findings feed
/// the fleet-wide sample section — mirrors [`BUNDLE_SAMPLES_PER_ROW`].
const ALTERNATION_SAMPLES_PER_ROW: usize = 3;

/// Cap on the total number of sample lines the fleet-wide
/// `# brace-alternation-flag defects (sample)` section prints — mirrors
/// [`BUNDLE_SAMPLE_LIMIT`].
const ALTERNATION_SAMPLE_LIMIT: usize = 20;
/// Cap on how many of one tool's own [`crate::single_dash_long`] splits or
/// [`crate::repeated_char`] misreads feed their fleet-wide sample sections —
/// mirrors [`BUNDLE_SAMPLES_PER_ROW`]. Shared by both families; read from
/// the same capture pass.
const SPLIT_SAMPLES_PER_ROW: usize = 3;

/// Cap on the total number of sample lines each of the two
/// fingerprint-sibling sections prints — mirrors [`BUNDLE_SAMPLE_LIMIT`].
const SPLIT_SAMPLE_LIMIT: usize = 20;

/// Aggregate stats. `pct_flags_with_text`, `no_tier_count`, and
/// `suspicious_count` are the regression gate (spec §13.1: "may not
/// worsen"); `verbatim_count`, `framework_detected_count`, and
/// `framework_counts` are reported for visibility but deliberately **not**
/// part of that gate — see [`compute_aggregate`]'s doc comment on why.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    /// Total flags with a description, across every tool, divided by total
    /// flags across every tool (not an average of per-tool percentages,
    /// so a handful of huge catalogs don't get diluted by many small
    /// no-flag tools).
    pub pct_flags_with_text: f64,
    /// Tools for which no tier produced a root node at all.
    pub no_tier_count: usize,
    /// Tools with at least one structurally-suspicious node (spec §13.1):
    /// a name failing [`mandible_core::is_command_name_shaped`], or a node
    /// with no flags, children, or summary. Gated like `no_tier_count` —
    /// [M-10] shipped `ok` at 100% described because `%described` can't
    /// see fabricated structure; this column can.
    pub suspicious_count: usize,
    /// Tools whose root degraded to verbatim (spec §7 Tier B step 3).
    /// **Not gated** — see [`compute_aggregate`].
    pub verbatim_count: usize,
    /// Tools at status `incomplete` (spec §6 rule 2b): a truncation
    /// confession was detected but not followed. **Not gated** — no
    /// baseline exists yet for this measurement.
    pub incomplete_count: usize,
    /// Tools whose root `--help` output was detected as a rendered man
    /// page ([M-16]). A subset of `verbatim_count`. **Not gated** — no
    /// baseline exists yet for this measurement.
    pub man_shaped_count: usize,
    /// Tools at status `ok` with zero flags at all ([M-15]). A synopsis
    /// flag is excluded from `pct_flags_with_text`'s denominator entirely
    /// (see [`mandible_extract::ExtractionResult::describable_flag_count`]),
    /// so this count and `pct_flags_with_text` move independently. **Not
    /// gated** — no baseline exists yet for this measurement.
    pub zero_flag_ok_count: usize,
    /// Tools for which Tier A′ identified a framework at all (spec §7
    /// Tier A′), regardless of method.
    pub framework_detected_count: usize,
    /// Per-framework tool counts (the framework's `Framework::name()`,
    /// without the detection-method suffix `[`framework_label`] adds to
    /// the per-row column), sorted by name for a stable, diffable
    /// scoreboard file.
    pub framework_counts: BTreeMap<String, usize>,
    /// Total tools scanned.
    pub total: usize,
    /// Raw numerator behind `pct_flags_with_text`, carried in the footer so a
    /// scoreboard produced in *shards* can be merged exactly. Recomputing
    /// the aggregate from the per-row `%flags_text` column cannot be exact
    /// — that column is rounded to whole percent — and a gated regression
    /// baseline must not be approximate. A full-PATH sweep is long enough
    /// to be worth running in shards, and CI's PATH sweep will want the
    /// same.
    pub described_flags: f64,
    /// **The** denominator behind `pct_flags_with_text` (spec §13's metric
    /// design rules) — the sum, across every tool, of flags whose source
    /// could have supplied a description. Excludes usage-synopsis-only
    /// flags; see
    /// [`mandible_extract::ExtractionResult::describable_flag_count`].
    pub describable_flags: f64,
    /// Raw flag total across every tool, including usage-synopsis-only
    /// ones — **not** `pct_flags_with_text`'s denominator (that's
    /// [`Self::describable_flags`]). Kept as its own number precisely so a
    /// fix that recovers real, honestly-undescribable flags is visible as
    /// recall gained rather than silently absent from every footer field,
    /// per spec §13's "keep the raw flag count visible" rule.
    pub total_flags: usize,
    /// Tools with at least one [`crate::misattribution`] suspect — the
    /// answer to "is `lsof` isolated, or is misattribution widespread?"
    /// **Not gated**: a brand-new detector with a measured, nonzero false-
    /// positive rate (see that module's doc comment) must not fail a build
    /// the first time it runs. Reported every run, compared against the
    /// previous one for visibility only (`xtask/src/main.rs`).
    pub misattribution_suspect_tools: usize,
    /// Tools whose raw captured text had at least one column-aligned
    /// secondary definition position at all — see
    /// [`Row::misattribution_column_aligned`]. Always `>=
    /// misattribution_suspect_tools`, and reported alongside it so a reader
    /// can see how often the strengthening signal fires versus how often it
    /// actually turns up a suspect. **Not gated**, same reasoning as
    /// `misattribution_suspect_tools`.
    pub misattribution_column_aligned_tools: usize,
    /// Tools with at least one [`crate::existence`] fabrication — a help-
    /// text-sourced subcommand name or flag spelling that does not occur
    /// literally in that tool's own raw captured text. This is the *other*
    /// half of what spec.md's WS4 originally called one "anti-fabrication
    /// oracle" — [`Self::misattribution_suspect_tools`]'s twin, with a
    /// different victim: [M-10]'s invented `tar`/`dd`/`less`/`apt-get`
    /// nodes, not `lsof`'s column-bled descriptions. **Not gated**, same
    /// reasoning as `misattribution_suspect_tools`: a brand-new detector
    /// with no fleet-wide baseline must not fail a build the first time it
    /// runs (spec §13.1b).
    pub existence_fabrication_tools: usize,
    /// Tools with at least one [`crate::bundling`] collapse — a synopsis
    /// cluster of bundled single-character switches (`[-2CDlNuVv]`) parsed
    /// as one flag carrying the rest as a required value. The third oracle,
    /// and the one the other two are structurally blind to: a collapsed
    /// `-2` *is* attested by [`Self::existence_fabrication_tools`]'s check
    /// (it occurs, literally, in the raw text) and carries no description
    /// for [`Self::misattribution_suspect_tools`]'s to misjudge, while the
    /// parse is badly wrong. **Not gated**, same reasoning as both:
    /// a brand-new detector with no fleet-wide baseline must not fail a
    /// build the first time it runs (spec §13.1b).
    pub bundle_collapse_tools: usize,
    /// Real flags destroyed by those collapses, fleet-wide — every cluster
    /// member after the first. This is the recall number;
    /// `bundle_collapse_tools` is only the blast radius.
    pub bundle_destroyed_flags: usize,
    /// Tools with at least one [`crate::alternation`] finding — a flag
    /// spelling written inside a delimited alternation group that reaches no
    /// flag in the tree, or one that reaches a flag still carrying the
    /// group's own punctuation as its value. The fourth oracle, and the one
    /// the three before it are blind to for three different reasons:
    /// `eqn`'s `--version` occurs literally in its raw text (so
    /// [`Self::existence_fabrication_tools`]'s check attests it), it carries
    /// no description for [`Self::misattribution_suspect_tools`]'s to
    /// misjudge, and its members are separated by `|` rather than glued, so
    /// the cluster grammar behind [`Self::bundle_collapse_tools`] neither
    /// helps nor hinders it.
    pub alternation_defect_tools: usize,
    /// Flag spellings those tools lost or mangled, fleet-wide. The recall
    /// number; `alternation_defect_tools` is only the blast radius.
    pub alternation_defect_flags: usize,
    /// Tools with at least one wholly-unparsed `commands:` table, fleet-
    /// wide. **Ratcheted at zero** (`detector::ratchet_at_zero`) rather
    /// than merely reported: the shape is fixed, and the gate is paired
    /// with the detector's own self-checks so a zero cannot be earned by
    /// deleting the rule.
    pub command_table_tools: usize,
    /// Tools with at least one [`crate::single_dash_long`] split — an
    /// option-table row naming a single-dash long option (`-help`) read as a
    /// one-character short flag plus a required value. The second of the
    /// three families sharing `bundle_collapse_tools`'s structural
    /// fingerprint, and blind to the same two oracles for the same reason:
    /// `-h` occurs in `qemu`'s raw text and carries a description, so
    /// nothing before this counted it. **Ratcheted at zero**
    /// (`detector::ratchet_at_zero`) since
    /// `help_text::sections::repair_single_dash_long_options` landed, on the
    /// same paired terms as `command_table_tools`: the count and the
    /// detector's own self-checks together, so a zero cannot be earned by
    /// deleting the rule.
    pub single_dash_split_tools: usize,
    /// Real flags lost to those splits, fleet-wide — one per split (the long
    /// spelling itself). Carried beside the tool count for the same reason
    /// `bundle_destroyed_flags` is, even though the ratio is milder here:
    /// the tool count is the blast radius, this is the recall cost.
    pub single_dash_split_flags: usize,
    /// Tools with at least one [`crate::repeated_char`] misread — `-vv` read
    /// as `-v` carrying its own letter as a value. The third family.
    pub repeated_char_tools: usize,
    /// Real flags lost to those misreads, fleet-wide — one per misread.
    pub repeated_char_flags: usize,
}

/// Output format for the rendered scoreboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ScoreFormat {
    /// Fixed-width plain text (the format checked into
    /// `coverage-scoreboard.txt`).
    Text,
    /// GitHub-flavored markdown, for `$GITHUB_STEP_SUMMARY` (spec
    /// §13.1a, batch 6 part 6).
    Markdown,
}

/// Keep every `total`-th tool starting at `index` — a stride, not a
/// contiguous block.
///
/// Contiguous slicing balances badly because expensive tools cluster
/// alphabetically: a machine with 23 `qemu-*-static` binaries (4 MB each,
/// and the artifact scanner reads deep into every one) puts them all in a
/// single chunk, which then takes longer than every other chunk combined.
/// A stride interleaves them, so each shard gets a comparable share of the
/// expensive ones and the slowest shard sets a much lower ceiling.
fn select_shard(tools: Vec<String>, index: usize, total: usize) -> Vec<String> {
    tools
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % total == index)
        .map(|(_, t)| t)
        .collect()
}

/// Enumerate unique executable names on `PATH`, run the full extraction
/// pipeline against each (in parallel — this is dozens to low thousands of
/// subprocess spawns and would otherwise take a very long time
/// sequentially), and return the scoreboard rows plus aggregate stats, in
/// tool-name order.
pub fn run(
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
) -> (String, Aggregate) {
    run_over(unique_executables_on_path(), shard, progress, format)
}

/// Same as [`run`], but over a caller-supplied tool list instead of
/// scanning `PATH`. Used by `--tools` to pin a fixed, reproducible set —
/// necessary for CI (spec §13.1's regression gate needs a tool inventory
/// that doesn't vary with the runner image) — and by tests.
pub fn run_over(
    mut tools: Vec<String>,
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
) -> (String, Aggregate) {
    tools.sort();
    tools.dedup();
    if let Some((index, total)) = shard {
        tools = select_shard(tools, index, total);
    }
    let mut rows: Vec<Row> = tools
        .par_iter()
        .map(|tool| {
            // Logged on both sides, flushed immediately, because the
            // *unmatched* line is the diagnosis. Several tools are in
            // flight at once, so "the last tool logged" is only ever a
            // shortlist — but a tool that started and never finished is
            // the one that took the process down. Start-only logging
            // narrowed three killed CI shards to two suspects each and
            // could not pick between them.
            if progress {
                use std::io::Write;
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err, "probe-start: {tool}");
                let _ = err.flush();
            }
            let row = score_one(tool);
            if progress {
                use std::io::Write;
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err, "probe-done:  {tool}");
                let _ = err.flush();
            }
            row
        })
        .collect();
    rows.sort_by(|a, b| a.tool.cmp(&b.tool));

    let aggregate = compute_aggregate(&rows);
    let table = match format {
        ScoreFormat::Text => render_text(&rows, &aggregate),
        ScoreFormat::Markdown => render_markdown(&rows, &aggregate),
    };
    (table, aggregate)
}

fn score_one(tool: &str) -> Row {
    let start = Instant::now();
    // A fresh [`RecordingProbe`] per tool (never shared across the sweep):
    // it's a transparent passthrough to [`mandible_extract::exec::LiveProbe`]
    // that also remembers the bytes each call returned, so
    // [`crate::misattribution`] can read the tool's own raw `--help` text
    // after extraction without a second probe — see that module's doc
    // comment. This spawns nothing a plain `default_tiers()` runner
    // wouldn't already have spawned.
    let probe = Arc::new(RecordingProbe::new());
    let runner = Runner::new(default_tiers_with_probe(probe.clone()));
    let result = runner.extract_full(tool);
    let ms = start.elapsed().as_millis();

    let tiers: Vec<&str> = result
        .tier_statuses
        .iter()
        .filter(|s| s.detected && s.error.is_none())
        .map(|s| short_tier_name(s.tier))
        .collect();
    let tiers_label = if tiers.is_empty() {
        "—".to_string()
    } else {
        tiers.join("+")
    };

    let framework = framework_label(tool, &result);
    let nodes = result.node_count();
    let flags = result.flag_count();
    let describable = result.describable_flag_count();

    // Status derivation (structure-sanity count, verbatim flag,
    // %described, and the final label) is computed once in `status.rs`
    // and shared verbatim with the corpus runner — see that module's doc
    // comment for why an independent second definition here would be a
    // drift risk, not a convenience.
    let status = crate::status::compute(&result);
    let man_shaped = root_is_man_shaped(&result);

    // Zero additional probes: `probe.root_help_text()` reads bytes the
    // `runner.extract_full` call above already fetched (see the doc
    // comment on `probe` above and `crate::misattribution`'s own doc
    // comment). Both the raw text and a root node are required — a tool
    // with no root, or whose only text came back empty, has nothing to
    // check.
    let (misattribution_suspect_count, misattribution_column_aligned, misattribution_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = misattribution::detect(&raw, root);
                let samples = report
                    .suspects
                    .iter()
                    .take(MISATTRIBUTION_SAMPLES_PER_ROW)
                    .map(format_misattribution_sample)
                    .collect();
                (report.suspect_count(), report.column_aligned, samples)
            }
            _ => (0, false, Vec::new()),
        };

    // Same captured text, zero additional probes — [`crate::existence`]'s
    // own doc comment on why re-reading `probe.root_help_text()` a second
    // time here (rather than sharing one `raw` binding with the
    // misattribution block above) costs nothing: both are cheap `Option<String>`
    // clones of bytes already in memory, not a second fetch.
    let (existence_fabrication_count, existence_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = existence::detect(&raw, root);
                let samples = report
                    .fabrications
                    .iter()
                    .take(EXISTENCE_SAMPLES_PER_ROW)
                    .map(format_existence_sample)
                    .collect();
                (report.fabrication_count(), samples)
            }
            _ => (0, Vec::new()),
        };

    // Third read of the same already-fetched capture, still zero probes —
    // see [`crate::bundling`]'s doc comment on why this detector needs no
    // new argv at all: the collapse is visible in the text the sweep
    // already has.
    let (bundle_collapse_count, bundle_destroyed_flags, bundle_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = bundling::detect(&raw, root);
                let samples = report
                    .collapses
                    .iter()
                    .take(BUNDLE_SAMPLES_PER_ROW)
                    .map(format_bundle_sample)
                    .collect();
                (
                    report.collapse_count(),
                    report.destroyed_flag_count(),
                    samples,
                )
            }
            _ => (0, 0, Vec::new()),
        };

    // Fourth read of the same already-fetched capture, still zero probes.
    let (alternation_defect_count, alternation_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = alternation::detect(&raw, root);
                let samples = report
                    .findings
                    .iter()
                    .take(ALTERNATION_SAMPLES_PER_ROW)
                    .map(format_alternation_sample)
                    .collect();
                (report.finding_count(), samples)
            }
            _ => (0, Vec::new()),
        };
    // Fourth read of the same already-fetched capture, still zero probes
    // — `crate::commandtable`'s shape is visible in the text the sweep
    // already has, exactly like the three detectors above.
    let command_table_count = match (probe.root_help_text(), result.root.as_ref()) {
        (Some(raw), Some(root)) if !raw.trim().is_empty() => {
            crate::commandtable::detect(&raw, root).missing.len()
        }
        _ => 0,
    };
    // The two remaining families of the three that share the `short &&
    // !long && value_name` fingerprint, read off the same capture on the
    // same pass and costing the same zero additional subprocess spawns.
    let (single_dash_split_count, single_dash_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = single_dash_long::detect(&raw, root);
                let samples = report
                    .splits
                    .iter()
                    .take(SPLIT_SAMPLES_PER_ROW)
                    .map(format_single_dash_sample)
                    .collect();
                (report.split_count(), samples)
            }
            _ => (0, Vec::new()),
        };
    let (repeated_char_misread_count, repeated_char_samples) =
        match (probe.root_help_text(), result.root.as_ref()) {
            (Some(raw), Some(root)) if !raw.trim().is_empty() => {
                let report = repeated_char::detect(&raw, root);
                let samples = report
                    .misreads
                    .iter()
                    .take(SPLIT_SAMPLES_PER_ROW)
                    .map(format_repeated_char_sample)
                    .collect();
                (report.misread_count(), samples)
            }
            _ => (0, Vec::new()),
        };

    Row {
        tool: tool.to_string(),
        tiers: tiers_label,
        framework,
        nodes,
        flags,
        describable,
        pct_flags_with_text: status.pct_flags_with_text,
        ms,
        suspicious_nodes: status.suspicious_nodes,
        verbatim: status.verbatim,
        man_shaped,
        misattribution_suspect_count,
        misattribution_column_aligned,
        misattribution_samples,
        existence_fabrication_count,
        existence_samples,
        bundle_collapse_count,
        bundle_destroyed_flags,
        bundle_samples,
        alternation_defect_count,
        alternation_samples,
        command_table_count,
        single_dash_split_count,
        single_dash_samples,
        repeated_char_misread_count,
        repeated_char_samples,
        status: status.label,
        fingerprint: build_fingerprint(result.root.as_ref()),
    }
}

/// One [`crate::alternation`] finding, rendered as a single audit-section
/// line: the group as the tool wrote it, and what went wrong with it.
fn format_alternation_sample(finding: &alternation::Finding) -> String {
    format!("{}: {} — {}", finding.path, finding.group, finding.detail)
}

/// One suspect, rendered as a single audit-section line: the tool/path, the
/// flag, its (length-capped) description, and which tokens triggered it.
fn format_misattribution_sample(suspect: &misattribution::Suspect) -> String {
    let mut desc = suspect.description.clone();
    if desc.chars().count() > MISATTRIBUTION_DESC_DISPLAY_LEN {
        desc = truncate_col(&desc, MISATTRIBUTION_DESC_DISPLAY_LEN);
    }
    format!(
        "{}: {} {:?} contains {}",
        suspect.path,
        suspect.flag,
        desc,
        suspect.offending_tokens.join(", "),
    )
}

/// One fabrication, rendered as a single audit-section line: which node
/// path carries it, whether it's a subcommand, a flag or an operand, and the specific
/// offending spelling — mirrors [`format_misattribution_sample`]'s shape.
fn format_existence_sample(fabrication: &existence::Fabrication) -> String {
    let kind = match fabrication.kind {
        existence::FabricationKind::Subcommand => "subcommand",
        existence::FabricationKind::Flag => "flag",
        existence::FabricationKind::Positional => "positional",
    };
    format!(
        "{}: invented {kind} {:?} not found in raw text",
        fabrication.path, fabrication.name,
    )
}

/// One collapse, rendered as a single audit-section line: which node path
/// carries it, the raw cluster, and how many real flags it destroyed —
/// mirrors [`format_existence_sample`]'s shape.
fn format_bundle_sample(collapse: &bundling::Collapse) -> String {
    format!(
        "{}: {:?} read as {} with a required value — {} real flag(s) destroyed",
        collapse.path, collapse.cluster, collapse.spelling, collapse.destroyed,
    )
}

/// One split, rendered as a single audit-section line — mirrors
/// [`format_bundle_sample`]'s shape.
fn format_single_dash_sample(split: &single_dash_long::Split) -> String {
    format!(
        "{}: {:?} split into {} plus a required value",
        split.path, split.token, split.spelling,
    )
}

/// One repeated-character misread, rendered as a single audit-section line —
/// mirrors [`format_single_dash_sample`]'s shape.
fn format_repeated_char_sample(misread: &repeated_char::Misread) -> String {
    format!(
        "{}: {:?} read as {} carrying its own letter as a value",
        misread.path, misread.token, misread.spelling,
    )
}

/// True when the root's captured `--help` output was detected as a
/// rendered man page (spec [M-16]).
///
/// Sends no new probe: reads `CommandNode::unparsed`, which `help_text::
/// build_node` sets to the raw captured lines whenever nothing parsed as
/// structure (spec §7 Tier B step 3), and re-runs
/// [`mandible_extract::help_text::is_man_page_banner`] over the first line
/// to tell a man-page banner apart from ordinary unparseable output.
fn root_is_man_shaped(result: &ExtractionResult) -> bool {
    let Some(root) = result.root.as_ref() else {
        return false;
    };
    let Some(first_line) = root.unparsed.iter().find(|t| !t.as_str().trim().is_empty()) else {
        return false;
    };
    mandible_extract::help_text::is_man_page_banner(first_line.as_str())
}

/// Compact `"<framework name> (<method>)"` label for the scoreboard's
/// `framework` column, or `"—"` when Tier A′ didn't identify one (spec §7
/// Tier A′ step 3). Framework name comes from `CommandNode::detected_framework`
/// (set only by Tier B, per-field authority resolution never lets `None`
/// displace `Some`). Method is re-derived via `framework::identify_from_artifact`
/// (memoized per binary path, spawns no process), since `Source`/`Provenance`
/// stay framework-agnostic (spec §4.2).
fn framework_label(tool: &str, result: &ExtractionResult) -> String {
    let Some(name) = result
        .root
        .as_ref()
        .and_then(|r| r.detected_framework.clone())
    else {
        return "—".to_string();
    };
    let resolved = resolve_tool(tool);
    let method = if mandible_extract::framework::identify_from_artifact(&resolved)
        .is_some_and(|f| f.name() == name)
    {
        "artifact"
    } else {
        "help-text"
    };
    format!("{name} ({method})")
}

/// Shorten a tier's internal name (e.g. `"known_specs::carapace"`) to the
/// spec's scoreboard vocabulary (`"carapace"`, `"help"`).
fn short_tier_name(name: &str) -> &str {
    match name {
        "known_specs::carapace" => "carapace",
        "help_text" => "help",
        other => other,
    }
}

/// Compute aggregate stats over `rows`.
///
/// `verbatim_count` is reported but not part of the regression gate (spec
/// §13.1's `--check`): a growing count can be a correct degrade-rather-
/// than-fabricate move (spec §7 Tier B step 3), not a regression.
/// `framework_detected_count`/`framework_counts` are unlisted for the same
/// reason — identifying more frameworks is progress, not a regression.
fn compute_aggregate(rows: &[Row]) -> Aggregate {
    let total_flags: usize = rows.iter().map(|r| r.flags).sum();
    let describable_flags: f64 = rows.iter().map(|r| r.describable as f64).sum();
    // Weighted by each row's *describable* count, not its raw flag count
    // (spec §13's metric design rules) — a row's `pct_flags_with_text` is
    // already described/describable, so multiplying it back by
    // `r.flags` here would silently reintroduce [M-15]'s defect by
    // crediting synopsis-only flags into a denominator they were just
    // excluded from.
    let described_flags: f64 = rows
        .iter()
        .map(|r| {
            r.pct_flags_with_text
                .map(|p| p / 100.0 * r.describable as f64)
                .unwrap_or(0.0)
        })
        .sum();
    let pct_flags_with_text = if describable_flags == 0.0 {
        0.0
    } else {
        described_flags / describable_flags * 100.0
    };
    let no_tier_count = rows.iter().filter(|r| r.status == "no-tier").count();
    let suspicious_count = rows.iter().filter(|r| r.status == "suspicious").count();
    let verbatim_count = rows.iter().filter(|r| r.verbatim).count();
    let incomplete_count = rows.iter().filter(|r| r.status == "incomplete").count();
    let man_shaped_count = rows.iter().filter(|r| r.man_shaped).count();
    let zero_flag_ok_count = rows
        .iter()
        .filter(|r| r.status == "ok" && r.flags == 0)
        .count();
    let misattribution_suspect_tools = rows
        .iter()
        .filter(|r| r.misattribution_suspect_count > 0)
        .count();
    let misattribution_column_aligned_tools = rows
        .iter()
        .filter(|r| r.misattribution_column_aligned)
        .count();
    let existence_fabrication_tools = rows
        .iter()
        .filter(|r| r.existence_fabrication_count > 0)
        .count();
    let bundle_collapse_tools = rows.iter().filter(|r| r.bundle_collapse_count > 0).count();
    let bundle_destroyed_flags: usize = rows.iter().map(|r| r.bundle_destroyed_flags).sum();
    let alternation_defect_tools = rows
        .iter()
        .filter(|r| r.alternation_defect_count > 0)
        .count();
    let alternation_defect_flags: usize = rows.iter().map(|r| r.alternation_defect_count).sum();
    let command_table_tools = rows.iter().filter(|r| r.command_table_count > 0).count();
    let single_dash_split_tools = rows
        .iter()
        .filter(|r| r.single_dash_split_count > 0)
        .count();
    let single_dash_split_flags: usize = rows.iter().map(|r| r.single_dash_split_count).sum();
    let repeated_char_tools = rows
        .iter()
        .filter(|r| r.repeated_char_misread_count > 0)
        .count();
    let repeated_char_flags: usize = rows.iter().map(|r| r.repeated_char_misread_count).sum();

    let mut framework_counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        if let Some(name) = framework_name_only(&row.framework) {
            *framework_counts.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    let framework_detected_count: usize = framework_counts.values().sum();

    Aggregate {
        pct_flags_with_text,
        no_tier_count,
        suspicious_count,
        verbatim_count,
        incomplete_count,
        man_shaped_count,
        zero_flag_ok_count,
        framework_detected_count,
        framework_counts,
        total: rows.len(),
        described_flags,
        describable_flags,
        total_flags,
        misattribution_suspect_tools,
        misattribution_column_aligned_tools,
        existence_fabrication_tools,
        bundle_collapse_tools,
        bundle_destroyed_flags,
        alternation_defect_tools,
        alternation_defect_flags,
        command_table_tools,
        single_dash_split_tools,
        single_dash_split_flags,
        repeated_char_tools,
        repeated_char_flags,
    }
}

/// Strip a row's `"<name> (<method>)"` framework label back down to just
/// the name, for aggregation; `None` for the unidentified sentinel `"—"`.
fn framework_name_only(label: &str) -> Option<&str> {
    if label == "—" {
        return None;
    }
    label.rsplit_once(" (").map(|(name, _)| name)
}

/// Truncate `s` to at most `width` characters, replacing the tail with a
/// single `…` marker when it doesn't fit. Character count, not
/// `unicode-width` — unlike `mandible-tui`'s rendering (which the
/// project's own invariants require display-width-safe truncation for,
/// since it draws into fixed terminal cells the user is actually looking
/// at), this is a plain-text developer report over tool names that are
/// overwhelmingly ASCII, so the extra dependency isn't justified here.
fn truncate_col(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let keep = width.saturating_sub(1);
    let mut truncated: String = chars[..keep].iter().collect();
    truncated.push('…');
    truncated
}

fn render_text(rows: &[Row], aggregate: &Aggregate) -> String {
    // Note the fixed `{:<N}` widths on the *truncated* string, not the
    // raw one: `{:<N}` only ever pads to a minimum width, it never
    // truncates — feeding it an untruncated long tool/tier/framework name
    // is exactly what let one long name shove every later column out of
    // alignment for that row.
    let mut out = String::new();
    out.push_str(&format!(
        "{:<tw$} {:<iw$} {:<fw$} {:>nw$}{:>flw$}{:>pw$}{:>msw$}{:>sw$}{:>manw$}{:>miw$}{:>ew$}{:>bw$}  {}\n",
        "tool",
        "tier(s)",
        "framework",
        "nodes",
        "flags",
        // Renamed from "%described" (spec §13.1/§13.1b): this column has
        // only ever measured whether a flag has text attached, never
        // whether the text is right — see `Row::pct_flags_with_text`'s doc
        // comment and the `accuracy: unmeasured` line below.
        "%flags_text",
        "ms",
        "suspect",
        "man",
        "misattr",
        "exist",
        "bundle",
        "status",
        tw = TOOL_COL_WIDTH,
        iw = TIER_COL_WIDTH,
        fw = FRAMEWORK_COL_WIDTH,
        nw = NODES_COL_WIDTH,
        flw = FLAGS_COL_WIDTH,
        pw = PCT_COL_WIDTH,
        msw = MS_COL_WIDTH,
        sw = SUSPECT_COL_WIDTH,
        manw = MAN_COL_WIDTH,
        miw = MISATTR_COL_WIDTH,
        ew = EXISTENCE_COL_WIDTH,
        bw = BUNDLE_COL_WIDTH,
    ));
    for row in rows {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{:<tw$} {:<iw$} {:<fw$} {:>nw$}{:>flw$}{:>pw$}{:>msw$}{:>sw$}{:>manw$}{:>miw$}{:>ew$}{:>bw$}  {}\n",
            truncate_col(&row.tool, TOOL_COL_WIDTH),
            truncate_col(&row.tiers, TIER_COL_WIDTH),
            truncate_col(&row.framework, FRAMEWORK_COL_WIDTH),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            if row.man_shaped { "yes" } else { "-" },
            row.misattribution_suspect_count,
            row.existence_fabrication_count,
            row.bundle_collapse_count,
            row.status,
            tw = TOOL_COL_WIDTH,
            iw = TIER_COL_WIDTH,
            fw = FRAMEWORK_COL_WIDTH,
            nw = NODES_COL_WIDTH,
            flw = FLAGS_COL_WIDTH,
            pw = PCT_COL_WIDTH,
            msw = MS_COL_WIDTH,
            sw = SUSPECT_COL_WIDTH,
            manw = MAN_COL_WIDTH,
            miw = MISATTR_COL_WIDTH,
            ew = EXISTENCE_COL_WIDTH,
            bw = BUNDLE_COL_WIDTH,
        ));
    }
    out.push_str(&aggregate_footer_line(aggregate));
    out.push('\n');
    out.push_str(&accuracy_unmeasured_line());
    out.push_str(&framework_summary_lines(aggregate));
    out.push_str(&worst_parsed_lines_text(&worst_parsed(rows)));
    out.push_str(&misattribution_sample_lines_text(rows));
    out.push_str(&existence_sample_lines_text(rows));
    out.push_str(&bundle_sample_lines_text(rows));
    out.push_str(&alternation_sample_lines_text(rows));
    out.push_str(&single_dash_sample_lines_text(rows));
    out.push_str(&repeated_char_sample_lines_text(rows));
    out.push_str(&fingerprint_lines(rows));
    out
}

/// The literal line every scoreboard carries until an instrument actually
/// measures whether a flag's attached text is correct, not just present
/// (spec §13.1's rename note). `pct_flags_with_text` answers presence
/// only — `corpus/lsof/4.95.0` (`[xfail]`) scored 79% while a quarter of
/// its flags were wrong. [`crate::misattribution`] is a first step but not
/// a general accuracy oracle.
fn accuracy_unmeasured_line() -> String {
    "# accuracy: unmeasured\n".to_string()
}

/// Cap on the worst-parsed audit section. Not load-bearing (this is a
/// work-queue aid, not a gated metric); 25 keeps the footer scannable
/// rather than dumping every imperfect tool on a full-`PATH` sweep.
const WORST_PARSED_LIMIT: usize = 25;

/// How many of a tool's *describable* flags the grammar failed to find a
/// description for. The ranking key below.
///
/// Measured against `row.describable`, not `row.flags` (spec §13's metric
/// design rules): a usage-synopsis-only flag was never a candidate for a
/// description in the first place, so counting it as "missing" here would
/// reopen exactly the trap this whole redefinition closes — a tool that
/// gains recall in flags nothing could ever describe would rank as having
/// gotten *worse*.
fn undescribed_flags(row: &Row) -> usize {
    match row.pct_flags_with_text {
        Some(pct) => {
            let described = (row.describable as f64) * (pct / 100.0);
            row.describable.saturating_sub(described.round() as usize)
        }
        // No describable flags at all, so nothing was missed.
        None => 0,
    }
}

/// The tools this harness parsed worst, ranked by how many flag
/// descriptions went missing, capped to [`WORST_PARSED_LIMIT`].
///
/// Ranked by undescribed-flag count, not percentage alone: a 150-flag
/// tool at 60% has more missing documentation than a 3-flag tool at 0%.
/// Not ranked by detection status — measurement showed unidentified and
/// identified tools score about the same (~90-92% described), so
/// detection isn't what separates a good result from a bad one. Ties
/// broken by tool name.
fn worst_parsed(rows: &[Row]) -> Vec<&Row> {
    let mut worst: Vec<&Row> = rows.iter().filter(|r| undescribed_flags(r) > 0).collect();
    worst.sort_by(|a, b| {
        undescribed_flags(b)
            .cmp(&undescribed_flags(a))
            .then_with(|| a.tool.cmp(&b.tool))
    });
    worst.truncate(WORST_PARSED_LIMIT);
    worst
}

/// Plain-text rendering of [`worst_parsed`]'s result, as
/// `#`-prefixed lines matching this module's other informational footer
/// sections (`framework_summary_lines`) — reported for visibility, not
/// re-parsed by `--check`, so the exact format isn't load-bearing.
fn worst_parsed_lines_text(worst: &[&Row]) -> String {
    if worst.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("# worst-parsed (most missing flag descriptions — the real work queue):\n");
    for (rank, row) in worst.iter().enumerate() {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "#   {:>2}. {:<30} {:>5} of {:>5} flags undescribed ({:>4}) {}\n",
            rank + 1,
            row.tool,
            undescribed_flags(row),
            row.flags,
            pct,
            row.framework,
        ));
    }
    out
}

/// Markdown rendering of [`worst_parsed`]'s result, for
/// [`render_markdown`].
fn worst_parsed_section_markdown(worst: &[&Row]) -> String {
    if worst.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Worst-parsed tools** (most missing flag descriptions, which is where grammar work pays off):\n\n| tool | undescribed | flags | %flags_text | framework |\n|---|---|---|---|---|\n",
    );
    for row in worst {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            undescribed_flags(row),
            row.flags,
            pct,
            md_escape(&row.framework),
        ));
    }
    out
}

/// Plain-text rendering of every row's [`Row::misattribution_samples`],
/// flattened and capped at [`MISATTRIBUTION_SAMPLE_LIMIT`] — a human-
/// readable sample of what [`crate::misattribution`] flagged, for judging
/// the false-positive rate (spec's own instruction: "report the rate, show
/// a sample of what it flags"). Mirrors [`worst_parsed_lines_text`]'s
/// "nothing to report → no section" convention.
fn misattribution_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.misattribution_samples.iter())
        .take(MISATTRIBUTION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# misattribution-suspects (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::existence_samples`], flattened
/// and capped at [`EXISTENCE_SAMPLE_LIMIT`] — mirrors
/// [`misattribution_sample_lines_text`]'s shape and "nothing to report → no
/// section" convention exactly.
fn existence_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.existence_samples.iter())
        .take(EXISTENCE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# existence-fabrications (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::bundle_samples`], flattened
/// and capped at [`BUNDLE_SAMPLE_LIMIT`] — mirrors
/// [`existence_sample_lines_text`]'s shape and "nothing to report → no
/// section" convention exactly.
fn bundle_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.bundle_samples.iter())
        .take(BUNDLE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# bundled-short-flag collapses (sample — not gated; judge the false-positive rate yourself):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::alternation_samples`],
/// flattened and capped at [`ALTERNATION_SAMPLE_LIMIT`] — mirrors
/// [`bundle_sample_lines_text`] exactly, including the "nothing to report →
/// no section" convention.
fn alternation_sample_lines_text(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.alternation_samples.iter())
        .take(ALTERNATION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# brace-alternation-flag defects (sample — reported, NOT gated; the residual is a subcommand-scoped flag this family cannot place, see xtask/src/main.rs):\n",
    );
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Plain-text rendering of every row's [`Row::single_dash_samples`] — the
/// second of the three families that share the `short && !long &&
/// value_name` fingerprint, printed as its own section for the same reason
/// [`bundle_sample_lines_text`] prints one: a count with no sample beside it
/// cannot be judged for false positives by anyone but its author. Mirrors
/// that function's "nothing to report -> no section" convention exactly.
fn single_dash_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.single_dash_samples.iter()),
        "# single-dash-long splits (sample — judge the false-positive rate yourself):\n",
    )
}

/// Twin of [`single_dash_sample_lines_text`] for [`crate::repeated_char`].
fn repeated_char_sample_lines_text(rows: &[Row]) -> String {
    sample_lines_text(
        rows.iter().flat_map(|r| r.repeated_char_samples.iter()),
        "# repeated-char-flag misreads (sample — judge the false-positive rate yourself):\n",
    )
}

/// The shared body of the two functions above: up to [`SPLIT_SAMPLE_LIMIT`]
/// samples under `heading`, or nothing at all when there are none. Factored
/// out rather than copied a fourth time — the three older sections predate
/// it and are left alone, but a fifth hand-written copy of the same ten
/// lines is exactly the drift risk `status.rs`'s own doc comment names.
fn sample_lines_text<'a>(samples: impl Iterator<Item = &'a String>, heading: &str) -> String {
    let samples: Vec<&String> = samples.take(SPLIT_SAMPLE_LIMIT).collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(heading);
    for (rank, sample) in samples.iter().enumerate() {
        out.push_str(&format!("#   {:>2}. {sample}\n", rank + 1));
    }
    out
}

/// Markdown rendering of [`alternation_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn alternation_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.alternation_samples.iter())
        .take(ALTERNATION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Brace-alternation-flag defects** (sample, reported and NOT gated — see `xtask/src/main.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown twin of [`single_dash_sample_lines_text`], for
/// [`render_markdown`].
fn single_dash_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.single_dash_samples.iter()),
        "\n**Single-dash-long splits** (sample — see \
         `xtask/src/single_dash_long.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`repeated_char_sample_lines_text`].
fn repeated_char_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.repeated_char_samples.iter()),
        "\n**Repeated-char-flag misreads** (sample — see \
         `xtask/src/repeated_char.rs`):\n\n| sample |\n|---|\n",
    )
}

/// The shared body of the two markdown sections above.
fn sample_section_markdown<'a>(samples: impl Iterator<Item = &'a String>, heading: &str) -> String {
    let samples: Vec<&String> = samples.take(SPLIT_SAMPLE_LIMIT).collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(heading);
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`bundle_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn bundle_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.bundle_samples.iter())
        .take(BUNDLE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Bundled-short-flag collapses** (sample, not gated — see `xtask/src/bundling.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`existence_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn existence_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.existence_samples.iter())
        .take(EXISTENCE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Existence fabrications** (sample, not gated — see `xtask/src/existence.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`misattribution_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn misattribution_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.misattribution_samples.iter())
        .take(MISATTRIBUTION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Misattribution suspects** (sample, not gated — see `xtask/src/misattribution.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// GitHub-flavored markdown table plus the same aggregate footer,
/// rendered as prose — spec §13.1a's framework-support workflow (batch 6
/// part 6) writes this straight to `$GITHUB_STEP_SUMMARY`, which GitHub
/// renders as markdown in the run's summary UI.
fn render_markdown(rows: &[Row], aggregate: &Aggregate) -> String {
    let mut out = String::new();
    out.push_str(
        "| tool | tier(s) | framework | nodes | flags | %flags_text | ms | suspect | man | misattr | exist | bundle | status |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for row in rows {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            md_escape(&row.tiers),
            md_escape(&row.framework),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            if row.man_shaped { "yes" } else { "-" },
            row.misattribution_suspect_count,
            row.existence_fabrication_count,
            row.bundle_collapse_count,
            row.status,
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "**Aggregate:** {:.2}% of flags carry text across {} tools, {} no-tier, {} suspicious, {} verbatim, {} man-shaped.\n\n",
        aggregate.pct_flags_with_text,
        aggregate.total,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
        aggregate.man_shaped_count,
    ));
    // See `accuracy_unmeasured_line`'s doc comment (the text format's
    // twin): `%flags_text` is presence, never correctness, and nothing yet
    // measures the latter fleet-wide.
    out.push_str("**Accuracy:** unmeasured.\n\n");
    out.push_str(&format!(
        "**Misattribution suspects:** {} tool(s) with at least one flag description containing \
         a flag-shaped token attested at a column-aligned position elsewhere in that tool's own \
         raw help text — not gated, see `xtask/src/misattribution.rs`.\n\n",
        aggregate.misattribution_suspect_tools,
    ));
    out.push_str(&format!(
        "**Existence fabrications:** {} tool(s) with at least one help-text-sourced subcommand \
         name or flag spelling not found literally in that tool's own raw help text — [M-10]'s \
         shape, generalized; not gated, see `xtask/src/existence.rs`.\n\n",
        aggregate.existence_fabrication_tools,
    ));
    out.push_str(&format!(
        "**Bundled-short-flag collapses:** {} tool(s) whose synopsis flag cluster was read as one \
         value-taking flag, destroying {} real flag(s) fleet-wide — not gated, see \
         `xtask/src/bundling.rs`.\n\n",
        aggregate.bundle_collapse_tools, aggregate.bundle_destroyed_flags,
    ));
    out.push_str(&format!(
        "**Brace-alternation-flag defects:** {} tool(s) whose delimited flag alternation \
         (`{{-i|--input}}`, `[[-c|-C] cmd]`) lost or mangled {} flag spelling(s) fleet-wide — \
         reported and NOT gated, see `xtask/src/main.rs` for the named residual.\n\n",
        aggregate.alternation_defect_tools, aggregate.alternation_defect_flags,
    ));
    out.push_str(&format!(
        "**Single-dash-long splits:** {} tool(s) whose option table names a single-dash long \
         option that was read as a one-character short flag plus a required value, losing {} \
         real flag(s) fleet-wide — see `xtask/src/single_dash_long.rs`.\n\n",
        aggregate.single_dash_split_tools, aggregate.single_dash_split_flags,
    ));
    out.push_str(&format!(
        "**Repeated-char-flag misreads:** {} tool(s) whose `-vv`-shaped flag was read as `-v` \
         carrying its own letter as a value, losing {} real flag(s) fleet-wide — see \
         `xtask/src/repeated_char.rs`.\n\n",
        aggregate.repeated_char_tools, aggregate.repeated_char_flags,
    ));
    out.push_str(&format!(
        "**Framework detection:** {}/{} tools ({:.1}%).\n",
        aggregate.framework_detected_count,
        aggregate.total,
        detection_rate_pct(aggregate),
    ));
    if !aggregate.framework_counts.is_empty() {
        out.push_str("\n**Per-framework counts:**\n\n");
        for (name, count) in &aggregate.framework_counts {
            out.push_str(&format!("- {}: {count}\n", md_escape(name)));
        }
    }
    out.push_str(&worst_parsed_section_markdown(&worst_parsed(rows)));
    out.push_str(&misattribution_sample_section_markdown(rows));
    out.push_str(&existence_sample_section_markdown(rows));
    out.push_str(&bundle_sample_section_markdown(rows));
    out.push_str(&alternation_sample_section_markdown(rows));
    out.push_str(&single_dash_sample_section_markdown(rows));
    out.push_str(&repeated_char_sample_section_markdown(rows));
    // The same machine-readable footer the text format carries, wrapped in
    // an HTML comment so it stays invisible when rendered but parseable by
    // whatever recombines shards. Without it a sharded markdown run could
    // only be merged by re-deriving totals from the rounded per-row
    // %flags_text column, which is exactly the approximation
    // `described_flags`/`total_flags` exist to avoid.
    out.push_str("\n<!-- ");
    out.push_str(aggregate_footer_line(aggregate).trim_end());
    out.push_str(" -->\n");
    out
}

/// Escape the one character (`|`) that would otherwise break a GFM table
/// cell. Tool names and framework labels are the only free-form content
/// here; a `|` in either is exotic but not impossible on a real `PATH`.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

fn detection_rate_pct(aggregate: &Aggregate) -> f64 {
    if aggregate.total == 0 {
        0.0
    } else {
        aggregate.framework_detected_count as f64 / aggregate.total as f64 * 100.0
    }
}

/// The single `# aggregate: ...` line every format carries — this is the
/// only line `parse_aggregate_footer` needs to understand, so it's kept
/// identical (modulo the new `verbatim_count` field) across text and
/// markdown output on purpose, even though markdown output isn't itself
/// meant to be re-parsed by `--check` (that always reads the plain-text
/// `coverage-scoreboard.txt`).
fn aggregate_footer_line(aggregate: &Aggregate) -> String {
    format!(
        "# aggregate: pct_flags_with_text={:.2} no_tier_count={} suspicious_count={} verbatim_count={} incomplete_count={} man_shaped_count={} zero_flag_ok_count={} misattribution_suspect_tools={} misattribution_column_aligned_tools={} existence_fabrication_tools={} bundle_collapse_tools={} bundle_destroyed_flags={} alternation_defect_tools={} alternation_defect_flags={} command_table_tools={} single_dash_split_tools={} single_dash_split_flags={} repeated_char_tools={} repeated_char_flags={} total={} described_flags={:.4} describable_flags={:.4} total_flags={}\n",
        aggregate.pct_flags_with_text,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
        aggregate.incomplete_count,
        aggregate.man_shaped_count,
        aggregate.zero_flag_ok_count,
        aggregate.misattribution_suspect_tools,
        aggregate.misattribution_column_aligned_tools,
        aggregate.existence_fabrication_tools,
        aggregate.bundle_collapse_tools,
        aggregate.bundle_destroyed_flags,
        aggregate.alternation_defect_tools,
        aggregate.alternation_defect_flags,
        aggregate.command_table_tools,
        aggregate.single_dash_split_tools,
        aggregate.single_dash_split_flags,
        aggregate.repeated_char_tools,
        aggregate.repeated_char_flags,
        aggregate.total,
        aggregate.described_flags,
        aggregate.describable_flags,
        aggregate.total_flags,
    )
}

/// Human-readable (not re-parsed) framework-detection summary: total
/// detection rate plus per-framework counts, sorted by name for a stable
/// diff.
fn framework_summary_lines(aggregate: &Aggregate) -> String {
    let mut out = format!(
        "# framework-detection: {}/{} tools ({:.1}%)\n",
        aggregate.framework_detected_count,
        aggregate.total,
        detection_rate_pct(aggregate),
    );
    if !aggregate.framework_counts.is_empty() {
        let counts: Vec<String> = aggregate
            .framework_counts
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        out.push_str(&format!("# framework-counts: {}\n", counts.join(", ")));
    }
    out
}

/// Parse the `# aggregate: ...` footer line this module writes, so
/// `--check` can compare against a prior run without re-parsing the whole
/// table. Only reads the single-line `key=value` aggregate footer —
/// `framework-detection`/`framework-counts` are informational only (see
/// [`framework_summary_lines`]) and never gated, so they don't need to
/// round-trip through this parser.
pub fn parse_aggregate_footer(scoreboard: &str) -> Option<Aggregate> {
    let line = scoreboard.lines().find(|l| l.starts_with("# aggregate:"))?;
    let mut pct_flags_with_text = None;
    let mut no_tier_count = None;
    // Older scoreboards (pre structure-sanity / pre-framework / pre-man-
    // shaped / pre-zero-flag columns) are missing `suspicious_count`/
    // `verbatim_count`/`man_shaped_count`/`zero_flag_ok_count` entirely;
    // default all four to 0 rather than failing to parse, so `--check`
    // against a not-yet-regenerated baseline still works for the fields
    // that did exist.
    let mut suspicious_count = 0usize;
    let mut verbatim_count = 0usize;
    // Brand-new field (spec §6 rule 2b, this batch): a scoreboard from
    // before the `incomplete` status existed has no such key at all, so
    // `--check` against one must still work.
    let mut incomplete_count = 0usize;
    let mut man_shaped_count = 0usize;
    let mut zero_flag_ok_count = 0usize;
    let mut described_flags = 0.0f64;
    // A scoreboard from before spec §13's metric redefinition has no
    // `describable_flags` field at all — its `pct_flags_with_text` was computed
    // over raw `total_flags` instead. Defaulting to 0.0 here (same pattern
    // as every other new-field default above) only affects reconstructing
    // an *exact* numerator/denominator for shard merging; `--check`
    // compares `pct_flags_with_text` values directly and never recomputes them
    // from this pair, so an old baseline still round-trips.
    let mut describable_flags = 0.0f64;
    let mut total_flags = 0usize;
    let mut total = None;
    // Brand-new field (spec §13.1's rename note, this task): a scoreboard
    // from before the misattribution detector existed has no such key at
    // all, so `--check` against one must still work.
    let mut misattribution_suspect_tools = 0usize;
    let mut misattribution_column_aligned_tools = 0usize;
    // Same reasoning, same pattern, brand new field (this task): a
    // scoreboard from before the existence detector existed has no such
    // key at all, so `--check` against one must still work.
    let mut existence_fabrication_tools = 0usize;
    // Same reasoning, same pattern, brand new field (this task): a
    // scoreboard from before the bundled-short-flag detector existed has no
    // such key at all, so `--check` against one must still work.
    let mut bundle_collapse_tools = 0usize;
    let mut bundle_destroyed_flags = 0usize;
    // Same reasoning again, brand new field (this task): a scoreboard
    // written before the brace-alternation detector existed carries no such
    // key, so `--check` against one must still work.
    let mut alternation_defect_tools = 0usize;
    let mut alternation_defect_flags = 0usize;
    let mut command_table_tools = 0usize;
    let mut single_dash_split_tools = 0usize;
    let mut single_dash_split_flags = 0usize;
    let mut repeated_char_tools = 0usize;
    let mut repeated_char_flags = 0usize;
    for field in line.trim_start_matches("# aggregate:").split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "pct_flags_with_text" => pct_flags_with_text = value.parse::<f64>().ok(),
            // Backward compatibility with every scoreboard written before
            // this rename (spec §13.1/§13.1b, Appendix B): the field is the
            // same ratio under its old, accuracy-implying name —
            // `pct_described`. Never written by this module anymore (see
            // `aggregate_footer_line`), only read.
            "pct_described" => pct_flags_with_text = value.parse::<f64>().ok(),
            "no_tier_count" => no_tier_count = value.parse::<usize>().ok(),
            "suspicious_count" => suspicious_count = value.parse::<usize>().ok()?,
            "verbatim_count" => verbatim_count = value.parse::<usize>().ok()?,
            "incomplete_count" => incomplete_count = value.parse::<usize>().ok()?,
            "man_shaped_count" => man_shaped_count = value.parse::<usize>().ok()?,
            "zero_flag_ok_count" => zero_flag_ok_count = value.parse::<usize>().ok()?,
            "misattribution_suspect_tools" => {
                misattribution_suspect_tools = value.parse::<usize>().ok()?
            }
            "misattribution_column_aligned_tools" => {
                misattribution_column_aligned_tools = value.parse::<usize>().ok()?
            }
            "existence_fabrication_tools" => {
                existence_fabrication_tools = value.parse::<usize>().ok()?
            }
            "bundle_collapse_tools" => bundle_collapse_tools = value.parse::<usize>().ok()?,
            "bundle_destroyed_flags" => bundle_destroyed_flags = value.parse::<usize>().ok()?,
            "alternation_defect_tools" => alternation_defect_tools = value.parse::<usize>().ok()?,
            "alternation_defect_flags" => alternation_defect_flags = value.parse::<usize>().ok()?,
            // Absent from a scoreboard written before this key existed,
            // which parses as 0 — the same value a healthy fleet produces,
            // so an older baseline stays comparable instead of failing.
            "command_table_tools" => command_table_tools = value.parse::<usize>().ok()?,
            "single_dash_split_tools" => single_dash_split_tools = value.parse::<usize>().ok()?,
            "single_dash_split_flags" => single_dash_split_flags = value.parse::<usize>().ok()?,
            "repeated_char_tools" => repeated_char_tools = value.parse::<usize>().ok()?,
            "repeated_char_flags" => repeated_char_flags = value.parse::<usize>().ok()?,
            "described_flags" => described_flags = value.parse::<f64>().ok()?,
            "describable_flags" => describable_flags = value.parse::<f64>().ok()?,
            "total_flags" => total_flags = value.parse::<usize>().ok()?,
            "total" => total = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(Aggregate {
        pct_flags_with_text: pct_flags_with_text?,
        no_tier_count: no_tier_count?,
        suspicious_count,
        verbatim_count,
        incomplete_count,
        man_shaped_count,
        zero_flag_ok_count,
        framework_detected_count: 0,
        framework_counts: BTreeMap::new(),
        total: total?,
        described_flags,
        describable_flags,
        total_flags,
        misattribution_suspect_tools,
        misattribution_column_aligned_tools,
        existence_fabrication_tools,
        bundle_collapse_tools,
        bundle_destroyed_flags,
        alternation_defect_tools,
        alternation_defect_flags,
        command_table_tools,
        single_dash_split_tools,
        single_dash_split_flags,
        repeated_char_tools,
        repeated_char_flags,
    })
}

/// Every uniquely-named executable file found in a `PATH` directory,
/// deduplicated by basename (the first directory to have a given name
/// wins, matching normal `PATH` resolution order) and sorted.
///
/// `pub(crate)`, not `pub`: `crate::audit`'s `sample` subcommand needs the
/// same population this module's own `run` scans by default (spec's audit
/// brief: "a deterministic draw from the tools on `PATH`"), and re-walking
/// `PATH` a second, independent way would risk the two enumerations quietly
/// disagreeing about what "every tool" means.
pub(crate) fn unique_executables_on_path() -> Vec<String> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_executable_file(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            seen.entry(name.to_string()).or_insert(path);
        }
    }
    seen.into_keys().collect()
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_its_own_footer_format() {
        let table = "tool  tier(s)\nfoo   carapace\n\n# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=2 verbatim_count=1 man_shaped_count=1 zero_flag_ok_count=4 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_flags_with_text, 42.5);
        assert_eq!(agg.no_tier_count, 3);
        assert_eq!(agg.suspicious_count, 2);
        assert_eq!(agg.verbatim_count, 1);
        assert_eq!(agg.incomplete_count, 0);
        assert_eq!(agg.man_shaped_count, 1);
        assert_eq!(agg.zero_flag_ok_count, 4);
        assert_eq!(agg.total, 10);
    }

    /// A scoreboard written before the structure-sanity column existed has
    /// no `suspicious_count` field at all — `--check` against it must
    /// still work (defaulting to 0) rather than treating the whole footer
    /// as unparseable.
    #[test]
    fn footer_without_suspicious_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.suspicious_count, 0);
    }

    /// Same for `verbatim_count`, added in batch 6 part 5: a scoreboard
    /// from before this batch has no such field.
    #[test]
    fn footer_without_verbatim_count_defaults_to_zero() {
        let table =
            "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.verbatim_count, 0);
    }

    /// Same for `incomplete_count` (spec §6 rule 2b, this batch): a
    /// scoreboard from before the `incomplete` status existed has no such
    /// field.
    #[test]
    fn footer_without_incomplete_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.incomplete_count, 0);
    }

    /// Round-trips through a freshly-written footer, unlike the
    /// backward-compatibility tests above which parse a hand-written one.
    #[test]
    fn incomplete_count_round_trips_through_a_freshly_written_footer() {
        let rows = vec![
            row("curl", 12, Some(100.0), "incomplete"),
            row("git", 34, Some(100.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.incomplete_count, 1);
        let line = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&line).unwrap();
        assert_eq!(parsed.incomplete_count, 1);
    }

    /// Same for `man_shaped_count`, added by this batch ([M-16]'s
    /// exposure enumeration): a scoreboard from before it exists has no
    /// such field, and `--check` against it must still work.
    #[test]
    fn footer_without_man_shaped_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.man_shaped_count, 0);
    }

    /// Same for `zero_flag_ok_count` ([M-15]): a scoreboard from before
    /// this metric existed has no such field, and `--check` against it
    /// must still work.
    #[test]
    fn footer_without_zero_flag_ok_count_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 man_shaped_count=1 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.zero_flag_ok_count, 0);
    }

    /// Same for `describable_flags` (spec §13's metric redefinition): a
    /// scoreboard from before it exists has no such field, and `--check`
    /// against it must still work — `--check` compares `pct_flags_with_text`
    /// values directly and never reconstructs them from this pair, so a
    /// pre-redefinition baseline still round-trips (see
    /// `parse_aggregate_footer`'s doc comment on this field).
    #[test]
    fn footer_without_describable_flags_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 suspicious_count=1 verbatim_count=1 man_shaped_count=1 zero_flag_ok_count=1 total=10 described_flags=4.2000 total_flags=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.describable_flags, 0.0);
    }

    /// A freshly-written footer round-trips `describable_flags` exactly —
    /// this is the field a sharded `--check` run needs to merge partial
    /// scoreboards without re-deriving `pct_flags_with_text` from the rounded
    /// per-row percentage column.
    #[test]
    fn footer_round_trips_describable_flags() {
        let rows = vec![row("git", 34, Some(100.0), "ok")];
        let mut only_row = rows;
        only_row[0].describable = 16;
        let agg = compute_aggregate(&only_row);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.describable_flags, 16.0);
    }

    /// spec §13.1/§13.1b's rename (this task): a scoreboard written under
    /// the old, accuracy-implying `pct_described` key must still parse —
    /// `--check` against a not-yet-regenerated baseline must not suddenly
    /// start failing to parse the footer at all just because the field
    /// changed names. See `aggregate_footer_line`: nothing written by this
    /// module ever emits the old key again, this is read-only compatibility.
    #[test]
    fn footer_reads_the_legacy_pct_described_key_name() {
        let table = "# aggregate: pct_described=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.pct_flags_with_text, 42.5);
    }

    /// Same pattern as every other new-column default: a scoreboard from
    /// before the misattribution detector existed has no such field.
    #[test]
    fn footer_without_misattribution_suspect_tools_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.misattribution_suspect_tools, 0);
    }

    #[test]
    fn footer_round_trips_misattribution_suspect_tools() {
        let mut suspect_row = row("lsof", 42, Some(79.0), "ok");
        suspect_row.misattribution_suspect_count = 1;
        let rows = vec![row("git", 34, Some(100.0), "ok"), suspect_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.misattribution_suspect_tools, 1);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.misattribution_suspect_tools, 1);
    }

    /// Same pattern as every other new-column default: a scoreboard from
    /// before the existence detector existed has no such field.
    #[test]
    fn footer_without_existence_fabrication_tools_defaults_to_zero() {
        let table = "# aggregate: pct_flags_with_text=42.50 no_tier_count=3 total=10\n";
        let agg = parse_aggregate_footer(table).unwrap();
        assert_eq!(agg.existence_fabrication_tools, 0);
    }

    #[test]
    fn footer_round_trips_existence_fabrication_tools() {
        let mut fabricated_row = row("tar", 42, Some(79.0), "ok");
        fabricated_row.existence_fabrication_count = 1;
        let rows = vec![row("git", 34, Some(100.0), "ok"), fabricated_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.existence_fabrication_tools, 1);
        let footer = aggregate_footer_line(&agg);
        let parsed = parse_aggregate_footer(&footer).unwrap();
        assert_eq!(parsed.existence_fabrication_tools, 1);
    }

    #[test]
    fn missing_footer_returns_none() {
        assert!(parse_aggregate_footer("no footer here\n").is_none());
    }

    #[test]
    fn short_tier_name_maps_known_names() {
        assert_eq!(short_tier_name("known_specs::carapace"), "carapace");
        assert_eq!(short_tier_name("help_text"), "help");
        assert_eq!(short_tier_name("something_else"), "something_else");
    }

    fn extraction_result_with_unparsed(tool: &str, first_line: &str) -> ExtractionResult {
        use mandible_core::{CommandNode, Provenance, Source, Text};
        let mut root = CommandNode::new(tool, Provenance::with_confidence(Source::HelpText, 0.0));
        root.unparsed = vec![Text::sanitize(first_line)];
        ExtractionResult {
            tool: tool.to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        }
    }

    /// True positive: a captured `unparsed` first line carrying the man
    /// banner shape is detected, via the exact same rule
    /// `sections::is_man_page_banner` exposes — this is `root_is_man_shaped`
    /// reading data the pipeline already captured, not a second probe.
    #[test]
    fn root_is_man_shaped_true_positive_on_a_captured_man_banner() {
        let result = extraction_result_with_unparsed(
            "git-bisect",
            "GIT-BISECT(1)     Git Manual     GIT-BISECT(1)",
        );
        assert!(root_is_man_shaped(&result));
    }

    /// A root that degraded to verbatim for an *ordinary* reason (the
    /// grammar just found nothing usable, no man banner involved) must not
    /// be counted as man-shaped — the two "gave up" reasons are distinct,
    /// which is the entire reason this function re-checks the captured
    /// text instead of trusting `verbatim` alone.
    #[test]
    fn root_is_man_shaped_false_when_verbatim_for_a_non_man_reason() {
        let result = extraction_result_with_unparsed(
            "mystery",
            "This tool prints only a friendly banner and nothing else.",
        );
        assert!(!root_is_man_shaped(&result));
    }

    /// git's own *root* must never register as man-shaped ([M-16]'s
    /// central subtlety: only its subcommands render man pages). A root
    /// that parsed real structure never carries `unparsed` at all, so
    /// there is no captured text to test the banner against.
    #[test]
    fn root_is_man_shaped_false_when_root_parsed_structurally() {
        use mandible_core::{CommandNode, Provenance, Source};
        let root = CommandNode::new("git", Provenance::with_confidence(Source::HelpText, 0.8));
        let result = ExtractionResult {
            tool: "git".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(!root_is_man_shaped(&result));
    }

    #[test]
    fn root_is_man_shaped_false_when_no_tier_produced_a_root() {
        let result = ExtractionResult {
            tool: "nothing".to_string(),
            root: None,
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(!root_is_man_shaped(&result));
    }

    /// `describable` defaults to `flags` — most tests here aren't about the
    /// synopsis-exclusion split itself, so every flag is describable unless
    /// a test overrides `.describable` afterwards (same pattern as
    /// `.verbatim`/`.man_shaped` below).
    fn row(
        tool: &str,
        flags: usize,
        pct_flags_with_text: Option<f64>,
        status: &'static str,
    ) -> Row {
        Row {
            tool: tool.to_string(),
            tiers: "help".to_string(),
            framework: "—".to_string(),
            command_table_count: 0,
            nodes: 1,
            flags,
            describable: flags,
            pct_flags_with_text,
            ms: 1,
            suspicious_nodes: 0,
            verbatim: false,
            man_shaped: false,
            misattribution_suspect_count: 0,
            misattribution_column_aligned: false,
            misattribution_samples: Vec::new(),
            existence_fabrication_count: 0,
            existence_samples: Vec::new(),
            bundle_collapse_count: 0,
            bundle_destroyed_flags: 0,
            bundle_samples: Vec::new(),
            alternation_defect_count: 0,
            alternation_samples: Vec::new(),
            single_dash_split_count: 0,
            single_dash_samples: Vec::new(),
            repeated_char_misread_count: 0,
            repeated_char_samples: Vec::new(),
            status,
            fingerprint: ToolFingerprint::default(),
        }
    }

    #[test]
    fn aggregate_weights_by_flag_count_not_per_tool_average() {
        let rows = vec![
            row("big", 100, Some(100.0), "ok"),
            row("small", 1, Some(0.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        // 100 described out of 101 total, not (100% + 0%)/2 = 50%.
        assert!((agg.pct_flags_with_text - (100.0 / 101.0 * 100.0)).abs() < 0.01);
    }

    /// spec §13's metric redefinition, at aggregate granularity: a tool
    /// whose flags are mostly undescribable-by-construction (synopsis-only)
    /// must not drag the fleet-wide ratio down for that reason — the
    /// aggregate is weighted by each row's *describable* count, not its
    /// raw flag count. Models the git shape directly: 34 raw flags, only
    /// 16 describable, all 16 described (spec's git fixture, post-fix).
    #[test]
    fn aggregate_weights_by_describable_count_not_raw_flag_count() {
        let mut git_like = row("git", 34, Some(100.0), "ok");
        git_like.describable = 16;
        let rows = vec![git_like];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.pct_flags_with_text, 100.0);
        assert_eq!(agg.describable_flags, 16.0);
        assert_eq!(agg.described_flags, 16.0);
    }

    #[test]
    fn aggregate_counts_suspicious_status_separately_from_no_tier() {
        let rows = vec![
            row("clean", 10, Some(100.0), "ok"),
            row("phantom", 40, Some(100.0), "suspicious"),
            row("nothing", 0, None, "no-tier"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.suspicious_count, 1);
        assert_eq!(agg.no_tier_count, 1);
    }

    #[test]
    fn aggregate_counts_verbatim_separately_and_it_is_not_gated_by_construction() {
        let mut verbatim_row = row("mystery", 0, None, "verbatim");
        verbatim_row.verbatim = true;
        let rows = vec![row("clean", 10, Some(100.0), "ok"), verbatim_row];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.verbatim_count, 1);
        // `Aggregate` simply has no field a gate could accidentally key
        // on beyond the three documented ones; this test exists so a
        // future reader sees `verbatim_count` is computed and populated,
        // not forgotten — the *not gated* half is enforced by
        // `xtask/src/main.rs` never comparing it, covered by reading that
        // function, not a unit test over a private struct.
    }

    /// [M-16]'s enumeration column: a man-shaped root is a *subset* of
    /// verbatim (git's subcommands are both), but not every verbatim root
    /// is man-shaped (some tools produce output the grammar just can't
    /// use, with no man banner in sight) — so the two counts must move
    /// independently, and `man_shaped_count` must never be gated (this is
    /// a brand-new measurement with no baseline, per the task).
    #[test]
    fn aggregate_counts_man_shaped_separately_from_plain_verbatim() {
        let mut man_shaped_row = row("git-bisect", 0, None, "verbatim");
        man_shaped_row.verbatim = true;
        man_shaped_row.man_shaped = true;
        let mut plain_verbatim_row = row("mystery", 0, None, "verbatim");
        plain_verbatim_row.verbatim = true;
        let rows = vec![
            row("clean", 10, Some(100.0), "ok"),
            man_shaped_row,
            plain_verbatim_row,
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.verbatim_count, 2);
        assert_eq!(agg.man_shaped_count, 1);
    }

    /// [M-15]'s own measure: a tool at status `ok` with zero flags at all
    /// (the shape 378 of 1,895 `ok` tools had fleet-wide before the usage-
    /// synopsis flag grammar). A `low-confidence` or `no-tier` tool with
    /// zero flags must not count — only `ok` ones do, since those are the
    /// ones a reader would otherwise trust as "nothing more to find here."
    #[test]
    fn aggregate_counts_ok_tools_with_zero_flags() {
        let rows = vec![
            row("git-like", 0, None, "ok"),
            row("has-flags", 10, Some(90.0), "ok"),
            row("weak", 0, None, "low-confidence"),
            row("nothing", 0, None, "no-tier"),
        ];
        let agg = compute_aggregate(&rows);
        assert_eq!(agg.zero_flag_ok_count, 1);
    }

    #[test]
    fn framework_counts_aggregate_by_name_ignoring_method() {
        let mut a = row("gh", 10, Some(90.0), "ok");
        a.framework = "cobra (artifact)".to_string();
        let mut b = row("docker", 20, Some(80.0), "ok");
        b.framework = "cobra (artifact)".to_string();
        let mut c = row("tar", 5, Some(70.0), "ok");
        c.framework = "GNU argp/getopt_long (help-text)".to_string();
        let mut d = row("weird", 0, None, "no-tier");
        d.framework = "—".to_string();
        let agg = compute_aggregate(&[a, b, c, d]);
        assert_eq!(agg.framework_counts.get("cobra"), Some(&2));
        assert_eq!(agg.framework_counts.get("GNU argp/getopt_long"), Some(&1));
        assert_eq!(agg.framework_detected_count, 3);
    }

    #[test]
    fn framework_name_only_strips_the_method_suffix() {
        assert_eq!(
            framework_name_only("clap (v3/v4) (artifact)"),
            Some("clap (v3/v4)")
        );
        assert_eq!(framework_name_only("—"), None);
    }

    #[test]
    fn truncate_col_pads_nothing_leaves_short_strings_alone() {
        assert_eq!(truncate_col("git", 24), "git");
    }

    #[test]
    fn truncate_col_shortens_long_names_with_an_ellipsis_marker() {
        let long = "UnicodeNameMappingGenerator-18";
        let truncated = truncate_col(long, 24);
        assert_eq!(truncated.chars().count(), 24);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn text_table_columns_stay_aligned_despite_a_very_long_tool_name() {
        let rows = vec![
            row(
                "aarch64-linux-gnu-cpp-13-extremely-long-name",
                5,
                Some(100.0),
                "ok",
            ),
            row("git", 5, Some(100.0), "ok"),
        ];
        let agg = compute_aggregate(&rows);
        let table = render_text(&rows, &agg);
        let lines: Vec<&str> = table
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .collect();
        // Every data/header row must be exactly the same length up to the
        // status column's start — i.e. every fixed-width column lines up
        // regardless of how long any one tool's name was. Measured in
        // *characters*, not bytes: the framework column's `—` fallback and
        // a truncated tool name's `…` marker are both multi-byte UTF-8, so
        // a byte offset would (and, before this fix, did) disagree between
        // rows with different multi-byte-character counts even though the
        // actual rendered alignment is fine.
        let status_col_start = |line: &str| -> usize {
            // Two spaces precede the status column in the format string.
            match line.rfind("  ") {
                Some(byte_idx) => line[..byte_idx].chars().count() + 2,
                None => line.chars().count(),
            }
        };
        let widths: Vec<usize> = lines.iter().map(|l| status_col_start(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "column widths were not aligned: {widths:?}\n{table}"
        );
    }

    #[test]
    fn markdown_format_produces_a_gfm_table_and_footer() {
        let rows = vec![row("git", 10, Some(90.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.starts_with("| tool |"));
        assert!(md.contains("|---|"));
        assert!(md.contains("| git |"));
        assert!(md.contains("**Aggregate:**"));
        assert!(md.contains("**Framework detection:**"));
    }

    /// "Surfacing unidentified tools for audit": the top-unidentified list
    /// Ranked by how many flag *descriptions* are missing, not by flag
    /// count and not by percentage alone: a tool with 150 flags at 80% has
    /// more missing documentation behind it than one with 3 flags at 0%.
    /// Tools that parsed cleanly are excluded entirely, since a work queue
    /// of finished work is not a work queue.
    #[test]
    fn worst_parsed_ranks_by_missing_descriptions() {
        let rows = vec![
            row("perfect", 500, Some(100.0), "ok"),    // nothing missing
            row("tiny-but-awful", 3, Some(0.0), "ok"), // 3 missing
            row("big-and-ok", 150, Some(80.0), "ok"),  // 30 missing
            row("mid", 40, Some(50.0), "ok"),          // 20 missing
        ];
        let worst = worst_parsed(&rows);
        let names: Vec<&str> = worst.iter().map(|r| r.tool.as_str()).collect();
        assert_eq!(
            names,
            vec!["big-and-ok", "mid", "tiny-but-awful"],
            "expected ranking by missing descriptions, cleanly-parsed excluded: {names:?}"
        );
    }

    #[test]
    fn worst_parsed_is_capped() {
        let rows: Vec<Row> = (0..(WORST_PARSED_LIMIT + 10))
            .map(|i| row(&format!("tool{i}"), i + 10, Some(10.0), "ok"))
            .collect();
        assert_eq!(worst_parsed(&rows).len(), WORST_PARSED_LIMIT);
    }

    /// Nothing to report when every tool parsed cleanly. The section
    /// disappears rather than printing an empty heading.
    #[test]
    fn worst_parsed_lines_text_is_empty_when_everything_parsed_cleanly() {
        let rows = vec![row("git", 10, Some(100.0), "ok")];
        assert!(worst_parsed_lines_text(&worst_parsed(&rows)).is_empty());
    }

    #[test]
    fn render_text_includes_the_worst_parsed_audit_section() {
        let rows = vec![row("half-parsed", 42, Some(50.0), "ok")];
        let agg = compute_aggregate(&rows);
        let table = render_text(&rows, &agg);
        assert!(table.contains("# worst-parsed"));
        assert!(table.contains("half-parsed"));
    }

    #[test]
    fn render_markdown_includes_the_worst_parsed_audit_section() {
        let rows = vec![row("half-parsed", 42, Some(50.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.contains("**Worst-parsed tools**"));
        assert!(md.contains("| half-parsed |"));
    }

    #[test]
    fn shards_partition_the_tool_list_exactly_once_each() {
        let tools: Vec<String> = (0..20).map(|i| format!("tool{i:02}")).collect();
        let total = 4;
        let mut seen: Vec<String> = Vec::new();
        for index in 0..total {
            seen.extend(select_shard(tools.clone(), index, total));
        }
        seen.sort();
        // Every tool appears in exactly one shard: none dropped, none
        // counted twice. A sharded scoreboard that silently loses tools
        // would understate coverage without looking wrong.
        assert_eq!(seen, tools);
    }

    #[test]
    fn shards_are_a_stride_not_a_contiguous_block() {
        let tools: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
        assert_eq!(select_shard(tools, 0, 3), vec!["t0", "t3"]);
    }

    #[test]
    fn unique_executables_on_path_finds_something_real() {
        // `sh` is present on every POSIX system this test would run on;
        // this is a sanity check that PATH scanning works at all, not an
        // exhaustive test of the harness (that's what running it for real
        // and inspecting the checked-in scoreboard is for).
        let tools = unique_executables_on_path();
        assert!(tools.iter().any(|t| t == "sh"));
    }

    /// `run_over` (the `--tools` path CI uses) scans exactly the given
    /// list, deduplicated — not every executable on `PATH` — so the
    /// aggregate's `total` is deterministic regardless of what else
    /// happens to be installed on the machine running it.
    #[test]
    fn run_over_scans_exactly_the_given_tools() {
        let (table, aggregate) = run_over(
            vec![
                "sh".to_string(),
                "sh".to_string(), // duplicate, must be deduped
                "true".to_string(),
            ],
            None,
            false,
            ScoreFormat::Text,
        );
        assert_eq!(aggregate.total, 2);
        assert!(table.contains("sh"));
        assert!(table.contains("true"));
    }

    #[test]
    fn run_over_markdown_format_produces_a_table() {
        let (table, _aggregate) =
            run_over(vec!["sh".to_string()], None, false, ScoreFormat::Markdown);
        assert!(table.starts_with("| tool |"));
    }

    /// **The round trip this whole `#fp` footer exists for, on a synthetic
    /// tree — never a host binary.** An earlier version of this test drove
    /// it from a real `grep --help` probe and asserted "at least one flag
    /// has a description," which is a fact about the host's `grep`
    /// (GNU grep's `--help` documents its options; BSD grep's, on macOS,
    /// is a bare usage synopsis with none) — exactly the class of failure
    /// AGENTS.md §4 warns about ("macOS breaks in ways Linux CI cannot
    /// see") and a real red `test (macos-latest)` job on this branch. A
    /// hand-built [`mandible_core::CommandNode`] carrying a described flag,
    /// a flag with choices and a `value_name`, and one subcommand makes the
    /// description-carrying case true by construction, on every platform,
    /// with no process spawned at all.
    #[test]
    fn fingerprint_footer_round_trips_a_synthetic_tree() {
        use mandible_core::{Choice, CommandNode, Entity, Provenance, Source, Text, ValueKind};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_spelled(
            Some('v'),
            Some("verbose".to_string()),
            false,
            false,
            Provenance::single(Source::HelpText),
        );
        flag.description = Some(Text::sanitize("increase verbosity"));
        flag.choices = vec![Choice::bare("low"), Choice::bare("high")];
        flag.value_name = Some("LEVEL".to_string());
        flag.value_kind = ValueKind::Required;
        root.entities.push(flag);
        root.subcommands.push(CommandNode::new(
            "child",
            Provenance::single(Source::HelpText),
        ));

        let mut r = row("demo", 1, Some(100.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("demo")
            .expect("demo fingerprint present in the #fp footer");
        assert_eq!(fp.flags.len(), 1);
        let flag = fp.flags.values().next().unwrap();
        assert!(flag.has_description, "description presence round-trips");
        assert!(
            flag.description_hash.is_some(),
            "description hash round-trips"
        );
        assert!(flag.choices_hash.is_some(), "choices hash round-trips");
        assert_eq!(flag.value_name.as_deref(), Some("LEVEL"));
        assert_eq!(fp.subcommands.len(), 1);
        assert!(fp.subcommands.contains("child"));
    }

    /// A real-binary smoke check (spec §3.1: "at least one test exercising
    /// real argv construction," not just the parser behind it), but —
    /// unlike the synthetic test above — asserting only what is true of
    /// *any* host's `grep`: that the round trip through
    /// [`fingerprint_lines`]/[`crate::transition::parse_scoreboard`] loses
    /// nothing, whatever `grep --help` on this machine actually said. Never
    /// a claim about grep's own content (that's the synthetic test's job;
    /// this one would stay green against BSD grep's flagless usage synopsis
    /// just as it does against GNU grep's described option table).
    #[test]
    fn fingerprint_footer_round_trips_whatever_a_real_grep_produced() {
        let live = score_one("grep");
        let rows = vec![live];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("grep")
            .expect("a #fp line is emitted unconditionally, even for an empty fingerprint");

        let live_fingerprint = &rows[0].fingerprint;
        assert_eq!(
            fp.flags.len(),
            live_fingerprint.flags.len(),
            "flag count must round-trip losslessly regardless of what this host's grep documents"
        );
        assert_eq!(fp.subcommands.len(), live_fingerprint.subcommands.len());
        for (id, live_flag) in &live_fingerprint.flags {
            let parsed_flag = fp.flags.get(id).unwrap_or_else(|| {
                panic!("flag {id:?} present before rendering must survive the round trip")
            });
            assert_eq!(parsed_flag.has_description, live_flag.has_description);
            assert_eq!(parsed_flag.description_hash, live_flag.description_hash);
            assert_eq!(parsed_flag.choices_hash, live_flag.choices_hash);
            assert_eq!(parsed_flag.value_name, live_flag.value_name);
        }
    }

    /// **The awk regression, reproduced.** PR #22's real finding: `awk`'s
    /// `-L` flag has `value_name` `"fatal|invalid|no-ext"` — free-form text
    /// lifted verbatim from `awk --help`, not something this codebase
    /// invents. `fingerprint_lines`'s flag-list separator is also `|`, so
    /// pre-fix (`fp_escape` only scrubbing tab/newline) the rendered `#fp`
    /// line contains three unescaped pipes where only one flag-list
    /// separator was intended; `transition::parse_fingerprint_line` splits
    /// on every `|` it sees, so `"invalid"` and `"no-ext"` become their own
    /// bogus flag entries with no `=`, `split_once('=')` returns `None`, and
    /// the `?` on that line discards the *entire* `#fp awk` line — every
    /// flag on it, not just `-L`. `awk`/`gawk`/`nawk` silently vanish from
    /// every field-level `sweep-diff` comparison. This test drives the exact
    /// shape through the real pipeline (`build_fingerprint` ->
    /// `fingerprint_lines` -> `transition::parse_scoreboard`) and asserts
    /// the line survives and the value_name comes back byte-for-byte.
    #[test]
    fn fingerprint_footer_round_trips_a_value_name_containing_the_flag_list_separator() {
        use mandible_core::{CommandNode, Entity, Provenance, Source};

        let mut root = CommandNode::new("awk", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_short('L', Provenance::single(Source::HelpText));
        flag.value_name = Some("fatal|invalid|no-ext".to_string());
        root.entities.push(flag);

        let mut r = row("awk", 1, Some(0.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed.fingerprints.get("awk").expect(
            "pre-fix this whole line is dropped by parse_fingerprint_line \
             because value_name's unescaped `|`s are mistaken for extra \
             flag-list entries",
        );
        assert_eq!(fp.flags.len(), 1, "the only flag on the line must survive");
        let flag = fp.flags.values().next().unwrap();
        assert_eq!(
            flag.value_name.as_deref(),
            Some("fatal|invalid|no-ext"),
            "value_name must round-trip byte-for-byte, not be mangled or dropped"
        );
    }

    /// **Every separator the `#fp` wire format uses, in one sweep**, plus
    /// two defensive cases beyond `value_name`: a subcommand name carrying
    /// the subcommand-list separator (`,`), and a flag long spelling
    /// carrying the flag-list separator (`|`) — a badly-parsed flag can, in
    /// principle, carry anything, and the escaping scheme is supposed to be
    /// blind to *which* piece of text needs it, not special-cased to
    /// `value_name` alone. Each value_name below embeds exactly one
    /// character the wire format would otherwise misread as structure:
    /// `,` (subcommand-list sep), `=` (id/fields sep), `:` (intra-entry
    /// sep), a literal tab (top-level field sep), and a literal backslash
    /// (the escape character itself, which must round-trip too).
    #[test]
    fn fingerprint_footer_round_trips_every_separator_character() {
        use mandible_core::{CommandNode, Entity, Provenance, Source};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));

        let mut comma_flag = Entity::flag_long("comma-value", Provenance::single(Source::HelpText));
        comma_flag.value_name = Some("a,b".to_string());
        root.entities.push(comma_flag);

        let mut equals_flag =
            Entity::flag_long("equals-value", Provenance::single(Source::HelpText));
        equals_flag.value_name = Some("a=b".to_string());
        root.entities.push(equals_flag);

        let mut colon_flag = Entity::flag_long("colon-value", Provenance::single(Source::HelpText));
        colon_flag.value_name = Some("a:b".to_string());
        root.entities.push(colon_flag);

        let mut tab_flag = Entity::flag_long("tab-value", Provenance::single(Source::HelpText));
        tab_flag.value_name = Some("a\tb".to_string());
        root.entities.push(tab_flag);

        let mut backslash_flag =
            Entity::flag_long("backslash-value", Provenance::single(Source::HelpText));
        backslash_flag.value_name = Some("a\\b".to_string());
        root.entities.push(backslash_flag);

        // Defensive: a flag whose own long spelling (not just its
        // value_name) carries the flag-list separator.
        let pipe_id_flag = Entity::flag_long("weird|name", Provenance::single(Source::HelpText));
        root.entities.push(pipe_id_flag);

        // Defensive: a subcommand name carrying the subcommand-list
        // separator.
        root.subcommands.push(CommandNode::new(
            "sub,with,comma",
            Provenance::single(Source::HelpText),
        ));

        let mut r = row("demo", 1, Some(0.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("demo")
            .expect("the #fp line must survive with every flag intact");
        assert_eq!(fp.flags.len(), 6, "no flag entry may be lost or merged");

        assert_eq!(
            fp.flags
                .get("(root)::Flag::--comma-value")
                .and_then(|f| f.value_name.clone()),
            Some("a,b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--equals-value")
                .and_then(|f| f.value_name.clone()),
            Some("a=b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--colon-value")
                .and_then(|f| f.value_name.clone()),
            Some("a:b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--tab-value")
                .and_then(|f| f.value_name.clone()),
            Some("a\tb".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--backslash-value")
                .and_then(|f| f.value_name.clone()),
            Some("a\\b".to_string())
        );
        assert!(
            fp.flags.contains_key("(root)::Flag::--weird|name"),
            "a flag id carrying the flag-list separator must survive under its own key"
        );

        assert_eq!(fp.subcommands.len(), 1);
        assert!(
            fp.subcommands.contains("sub,with,comma"),
            "a subcommand name carrying the subcommand-list separator must round-trip whole"
        );
    }

    /// **The positive-signal proof this task exists for.** A node carrying
    /// one entity of every `EntityKind` — a flag, a positional, a modifier,
    /// and an env-var item — all with the *same* bare spelling (`"x"`),
    /// must fingerprint as four distinct entries, not collapse into one:
    /// `entity_identity`'s `EntityKind` tag is what tells `ar`'s `x`
    /// modifier apart from a hypothetical `x` flag on the same node, and is
    /// exactly what the pre-generalization fingerprint (flags only, no kind
    /// tag) could never have expressed even by accident, since it only ever
    /// saw the flag.
    #[test]
    fn every_entity_kind_fingerprints_as_a_distinct_entry() {
        use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));
        root.entities.push(Entity::flag_short(
            'x',
            Provenance::single(Source::HelpText),
        ));
        root.entities.push(Entity::positional(
            "x",
            Provenance::single(Source::HelpText),
        ));
        root.entities
            .push(Entity::modifier('x', Provenance::single(Source::HelpText)));
        root.entities.push(Entity::env_var_item(
            "x",
            Provenance::single(Source::HelpText),
        ));

        let fp = build_fingerprint(Some(&root));
        assert_eq!(
            fp.flags.len(),
            4,
            "four different EntityKinds sharing one bare spelling must not collide: {:?}",
            fp.flags.keys().collect::<Vec<_>>()
        );
        assert!(fp.flags.contains_key("(root)::Flag::-x"));
        assert!(fp.flags.contains_key("(root)::Positional::x"));
        assert!(fp.flags.contains_key("(root)::Modifier::x"));
        assert!(fp.flags.contains_key("(root)::EnvVar::x"));

        // Round-trips through the real wire format too, not just the
        // in-memory ToolFingerprint.
        let mut r = row("demo", 4, Some(0.0), "ok");
        r.fingerprint = fp;
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);
        assert!(
            text.contains("#fp2 "),
            "the emitted footer must use the v2 line prefix"
        );
        let parsed = crate::transition::parse_scoreboard(&text);
        let round_tripped = parsed
            .fingerprints
            .get("demo")
            .expect("demo fingerprint present in the #fp2 footer");
        assert_eq!(round_tripped.flags.len(), 4);
        for kind in [
            EntityKind::Flag,
            EntityKind::Positional,
            EntityKind::Modifier,
            EntityKind::EnvVar,
        ] {
            let id = format!(
                "(root)::{kind:?}::{}",
                if kind == EntityKind::Flag { "-x" } else { "x" }
            );
            assert!(
                round_tripped.flags.contains_key(&id),
                "{id} must survive the round trip"
            );
        }
    }

    // `structure_sanity`'s own unit tests (fabricated names, empty nodes,
    // the root-name exclusion, `heading_attested` provenance, a clean
    // tree) now live in `status.rs`'s test module, alongside the function
    // itself — see that module's doc comment for why it moved.
}
