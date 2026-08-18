//! The per-tool sweep transition report (WS2 part 1): a semantic diff
//! between two independently-generated [`crate::coverage::ScoreFormat::Text`]
//! scoreboards.
//!
//! **Why this exists at all.** Two grammar fixes on this branch shipped
//! regressions that the aggregate `%flags_text` gate and the 4-fixture
//! corpus both stayed green through: one lost 228 flags across 72 tools,
//! another lost 6 on `lsof` and 34 across four tools nobody had looked at.
//! Both were caught only by a human running a full sweep before and after
//! and diffing per tool by hand. `compute_aggregate`'s own doc comment
//! explains why the fleet-wide ratio can't see this: it's a flag-weighted
//! average, so a regression on four tools out of a couple thousand moves it
//! by hundredths of a percent — invisible in the aggregate, glaring in a
//! per-tool diff. This module automates exactly the manual step that has
//! actually caught every regression so far, never a raw text diff of the
//! two scoreboard files (`corpus.rs`'s own report is the model: "a 1,000+
//! line YAML diff is unreviewable... instead of lines, report what changed
//! semantically").
//!
//! **Losses, not net (measured, not a preference).** A change that adds
//! 2,000 flags and loses 6 is a regression on those 6 tools; summing gains
//! and losses into one signed number would hide exactly the losses that
//! caught the two real regressions above. Gains and losses are therefore
//! always reported as two separate totals, never netted — see
//! [`FlagDelta`] and [`render_markdown`].
//!
//! **Non-blocking by design (maintainer decision D4).** This ships as a
//! loud report, promoted to a real gate only after a burn-in period. Unlike
//! `coverage --check` or `corpus`'s hard gate, nothing in this module
//! returns a failure signal — `cargo xtask sweep-diff` always exits `0`
//! (barring an I/O error reading its inputs). The CLI layer
//! (`xtask/src/main.rs`) enforces this by construction: there is no
//! `--check`/`--gate` flag here at all, so there's nothing to wire to a
//! nonzero exit by accident.
//!
//! **Truncated tool names are a real hazard, not a hypothetical one.**
//! `coverage::truncate_col` elides any tool name over
//! [`crate::coverage::TOOL_COL_WIDTH`] characters with a single `…`
//! marker, so two differently-named tools can render to the *same*
//! truncated string (`aarch64-linux-gnu-cpp-13-extremely-long-name` and a
//! same-length sibling both truncate to `aarch64-linux-gnu-cpp-1…`).
//! Joining two scoreboards on tool name naively — `HashMap<String, Row>`,
//! last-write-wins — turns that collision into a silent cross-product: one
//! tool's "before" gets diffed against a different tool's "after". Every
//! truncated row (detected in [`parse_scoreboard`], not guessed at) is
//! therefore dropped from the comparison entirely rather than joined, and
//! the count dropped is a headline number in the report — see
//! [`ParsedScoreboard::truncated_dropped`] and
//! [`render_markdown`]/[`render_text`].

use crate::coverage::{
    BUNDLE_COL_WIDTH, EXISTENCE_COL_WIDTH, FLAGS_COL_WIDTH, FRAMEWORK_COL_WIDTH, MAN_COL_WIDTH,
    MISATTR_COL_WIDTH, MS_COL_WIDTH, NODES_COL_WIDTH, PCT_COL_WIDTH, SUSPECT_COL_WIDTH,
    TIER_COL_WIDTH, TOOL_COL_WIDTH,
};
use std::collections::BTreeMap;

/// The single-probe extraction timeout (`mandible_extract::help_text::mod`'s
/// and `native::mod`'s own private `EXTRACT_TIMEOUT`, `Duration::from_secs(10)`).
/// Duplicated here, not imported: `xtask` has no path to that private
/// constant, and this crate's own hard boundary (`xtask` may not depend on
/// `mandible-extract` beyond its public API, and the parallel work on
/// `mandible-extract/src/help_text/` in flight on this branch is explicitly
/// off limits for this task) means duplicating one well-known, stable
/// number is the honest choice over reaching for it. If it ever changes,
/// AGENTS.md's environment-facts discipline applies: re-measure and update
/// both places in the same commit.
const EXTRACT_TIMEOUT_MS: u128 = 10_000;

/// True when `ms` is close enough to [`EXTRACT_TIMEOUT_MS`] that a status
/// derived from it is a statement about the machine, not the parser (spec
/// §13.1b rule 3; maintainer decision D4).
///
/// **Why a lower bound only, no upper bound.** The measured incident this
/// rule exists for (AGENTS.md, `waagent2.0`) is not a single-probe tool:
/// `score_one` recurses into every discovered subcommand, each under its
/// own `EXTRACT_TIMEOUT`, so a tool's total `ms` legitimately exceeds one
/// cap's worth of wall time by design — `waagent2.0` measured 41.9s and
/// 21.4s across two runs of *identical code*, both multiples of the 10s
/// single-probe cap. A symmetric "within 2x" band (5s–20s) would have
/// missed the very case that motivated this rule. A total that has reached
/// or passed half the single-probe cap is already timeout-adjacent — and
/// more total time near/over the cap is more evidence of timeout pressure
/// on some probe inside that tool's tree, never less — so the honest bound
/// is "at least half the cap", open-ended above it.
fn near_timeout_cap(ms: u128) -> bool {
    ms.saturating_mul(2) >= EXTRACT_TIMEOUT_MS
}

/// One data row parsed back out of a rendered
/// [`crate::coverage::ScoreFormat::Text`] scoreboard.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRow {
    pub tool: String,
    pub tiers: String,
    pub framework: String,
    pub nodes: usize,
    pub flags: usize,
    pub pct_flags_with_text: Option<f64>,
    pub ms: u128,
    pub suspicious_nodes: usize,
    pub man_shaped: bool,
    /// `None` on a scoreboard rendered before the misattribution detector
    /// existed (no `misattr` column at all) — see [`has_misattr_column`].
    /// Distinct from `Some(0)` ("column present, zero suspects"), so a
    /// reader can tell "not measured yet" from "measured, clean".
    pub misattribution_suspect_count: Option<usize>,
    /// `None` on a scoreboard rendered before the existence detector
    /// existed (no `exist` column at all) — see [`has_existence_column`].
    /// Same `None`-vs-`Some(0)` distinction as
    /// `misattribution_suspect_count` above.
    pub existence_fabrication_count: Option<usize>,
    /// `None` on a scoreboard rendered before the bundled-short-flag
    /// detector existed (no `bundle` column at all) — see
    /// [`has_bundle_column`]. Same `None`-vs-`Some(0)` distinction as the
    /// two counts above.
    pub bundle_collapse_count: Option<usize>,
    pub status: String,
}

impl ParsedRow {
    fn near_cap(&self) -> bool {
        near_timeout_cap(self.ms)
    }
}

/// The result of parsing one scoreboard: every clean data row, keyed by
/// tool name for the join in [`diff`], plus counts of what had to be
/// dropped and why — both surfaced in the report rather than silently
/// swallowed (this module's own doc comment on the truncation hazard).
#[derive(Debug, Default)]
pub struct ParsedScoreboard {
    pub rows: BTreeMap<String, ParsedRow>,
    /// Rows dropped because the tool-name column was truncated
    /// (`coverage::truncate_col`'s `…` marker) — never joined, since a
    /// truncated name can collide with a different tool's truncated name.
    pub truncated_dropped: usize,
    /// Rows dropped because a numeric field didn't parse (a hand-edited or
    /// corrupted scoreboard, or a row whose content overflowed its nominal
    /// column width and desynced every fixed-offset field after it — see
    /// this module's doc comment). Never expected on a scoreboard this
    /// binary itself produced; tracked so a malformed input fails visibly
    /// small rather than silently large.
    pub unparseable_dropped: usize,
    /// Every tool's field-level fingerprint, parsed from the scoreboard's
    /// `#fp` footer lines (`coverage::fingerprint_lines`'s own doc comment
    /// has the line shape). **Absent for a scoreboard rendered before this
    /// footer existed** — a tool missing from this map (as opposed to
    /// present with an empty [`ParsedFingerprint`]) means "not measured,"
    /// mirrored in [`diff`] by skipping field-level comparison for that
    /// tool entirely rather than reporting a false wholesale removal of
    /// every flag it has.
    pub fingerprints: BTreeMap<String, ParsedFingerprint>,
}

/// One flag's field-level fingerprint, read back from a `#fp` line —
/// [`crate::coverage`]'s `FlagFingerprint`, parsed rather than shared
/// directly: this module never depends on `mandible_core`/`mandible_extract`
/// tree types, only on the already-rendered text (this module's own doc
/// comment on why `sweep-diff` reads two rendered scoreboards, never talks
/// to the extraction pipeline itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFlagFingerprint {
    pub has_description: bool,
    pub description_hash: Option<u64>,
    pub choices_hash: Option<u64>,
    pub value_name: Option<String>,
}

/// One tool's field-level fingerprint, read back from its `#fp` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFingerprint {
    pub flags: BTreeMap<String, ParsedFlagFingerprint>,
    pub subcommands: std::collections::BTreeSet<String>,
}

/// The fingerprint [`diff`] substitutes for a matched tool's *missing* side
/// when the *other* side does carry a `#fp` entry for it — a `'static`
/// empty value (both collection types have a `const fn new()`) so it can be
/// borrowed at the same lifetime as the real, scoreboard-owned fingerprints
/// [`field_diff`] otherwise compares.
///
/// **Why "missing on one side only" is read as "empty" rather than
/// "unmeasured."** `coverage::fingerprint_lines` now emits a `#fp` line for
/// *every* row, including an empty one — the fix for the defect where a
/// tool that lost every flag produced a line on the "before" side and none
/// on the "after" side, which then reported as field-diff-unmeasured
/// instead of reporting the full flag loss it actually was. With that fix,
/// a per-tool line is absent on exactly one side only in the mixed-vintage
/// case (one scoreboard predates the `#fp` footer entirely, the other
/// doesn't) — and even there, the honest reading of "we hold no record of
/// this side's flags" is "empty," which correctly reports every flag the
/// present side has as added (if the earlier side is the one missing) or
/// removed (if the later side is). The genuinely unmeasurable case — both
/// sides missing — is handled separately in [`diff`] and keeps the
/// `field_diff_unmeasured` wording.
static EMPTY_FINGERPRINT: ParsedFingerprint = ParsedFingerprint {
    flags: BTreeMap::new(),
    subcommands: std::collections::BTreeSet::new(),
};

/// Parse one `#fp <tool>\t<subs>\t<flags>` line's content (the part after
/// `"#fp "`) into `(tool, fingerprint)`, or `None` if it's malformed —
/// treated exactly like [`LineResult::Unparseable`] by the caller: skipped,
/// never panicked on, since a `#fp` line only ever exists on a scoreboard
/// this binary itself wrote.
fn parse_fingerprint_line(rest: &str) -> Option<(String, ParsedFingerprint)> {
    let mut top = rest.splitn(3, FP_FIELD_SEP);
    let tool = top.next()?.to_string();
    let subs_s = top.next().unwrap_or("");
    let flags_s = top.next().unwrap_or("");

    let mut fp = ParsedFingerprint::default();
    if !subs_s.is_empty() {
        for s in subs_s.split(',') {
            if !s.is_empty() {
                fp.subcommands.insert(s.to_string());
            }
        }
    }
    if !flags_s.is_empty() {
        for entry in flags_s.split('|') {
            let (id, rest) = entry.split_once('=')?;
            // `splitn(4, ':')` so a `value_name` that itself contains a
            // colon (free-form text lifted from real `--help` output, only
            // `\t`/`\n` are escaped — see `coverage::fp_escape`) lands whole
            // in the final piece instead of being truncated at its first
            // colon.
            let mut fields = rest.splitn(4, ':');
            let has_description = fields.next()? == "1";
            let description_hash = match fields.next()? {
                "-" => None,
                h => u64::from_str_radix(h, 16).ok(),
            };
            let choices_hash = match fields.next()? {
                "-" => None,
                h => u64::from_str_radix(h, 16).ok(),
            };
            let value_name = match fields.next()? {
                "-" => None,
                v => Some(v.to_string()),
            };
            fp.flags.insert(
                id.to_string(),
                ParsedFlagFingerprint {
                    has_description,
                    description_hash,
                    choices_hash,
                    value_name,
                },
            );
        }
    }
    Some((tool, fp))
}

/// The literal tab [`coverage::fingerprint_lines`] separates a `#fp` line's
/// three top-level fields with — duplicated from `coverage::FP_FIELD_SEP`
/// (private to that module) for the same reason [`EXTRACT_TIMEOUT_MS`] is
/// duplicated rather than imported: a single well-known, stable character,
/// re-measured in the same commit as the other side if it ever changes.
const FP_FIELD_SEP: char = '\t';

/// True when `header` is (or resembles) a scoreboard's own header line,
/// used only to decide whether it carries the `misattr` column added after
/// the misattribution detector shipped — every scoreboard from before that
/// (this task found four real ones, captured during earlier work on this
/// branch) has ten columns instead of eleven (or twelve, with `exist` too)
/// and needs a different offset for the trailing `status` column. See
/// [`row_offsets`].
fn has_misattr_column(header: &str) -> bool {
    header.contains("misattr")
}

/// Same idea as [`has_misattr_column`], for the `exist` column
/// ([`crate::existence`], this task) appended after `misattr` — every
/// scoreboard from before this task has eleven columns (or ten, with no
/// `misattr` either) and needs the shorter offset for `status`.
fn has_existence_column(header: &str) -> bool {
    header.contains("exist")
}

/// Same idea again, for the `bundle` column ([`crate::bundling`]) appended
/// after `exist` — every scoreboard from before it has twelve columns or
/// fewer and needs the shorter offset for `status`.
fn has_bundle_column(header: &str) -> bool {
    header.contains("bundle")
}

/// The exact character offsets [`crate::coverage::render_text`] writes each
/// column at, derived from the same width constants that function uses —
/// never a second, hand-copied set of numbers (this module's doc comment).
/// The three `with_*` flags select among the four layouts a real,
/// checked-in scoreboard can have — ten columns (no detector existed yet),
/// eleven (`misattr`), twelve (`+ exist`), thirteen (`+ bundle`) — since
/// each detector only ever *appended* a column rather than resizing an
/// existing one, every column up through `man` shares identical offsets
/// regardless. The optional three are laid out as a chain, each starting
/// where the last present one ended, so a header missing an earlier one
/// (which no scoreboard this binary ever wrote can be — the columns
/// shipped in that order) still yields self-consistent offsets rather than
/// an assertion, since this function's only job is to read whatever header
/// string it's given.
struct RowOffsets {
    tool: (usize, usize),
    tier: (usize, usize),
    framework: (usize, usize),
    nodes: (usize, usize),
    flags: (usize, usize),
    pct: (usize, usize),
    ms: (usize, usize),
    suspect: (usize, usize),
    man: (usize, usize),
    misattr: Option<(usize, usize)>,
    existence: Option<(usize, usize)>,
    bundle: Option<(usize, usize)>,
    status_start: usize,
}

fn row_offsets(with_misattr: bool, with_existence: bool, with_bundle: bool) -> RowOffsets {
    let tool = (0, TOOL_COL_WIDTH);
    let tier = (tool.1 + 1, tool.1 + 1 + TIER_COL_WIDTH);
    let framework = (tier.1 + 1, tier.1 + 1 + FRAMEWORK_COL_WIDTH);
    let nodes = (framework.1 + 1, framework.1 + 1 + NODES_COL_WIDTH);
    let flags = (nodes.1, nodes.1 + FLAGS_COL_WIDTH);
    let pct = (flags.1, flags.1 + PCT_COL_WIDTH);
    let ms = (pct.1, pct.1 + MS_COL_WIDTH);
    let suspect = (ms.1, ms.1 + SUSPECT_COL_WIDTH);
    let man = (suspect.1, suspect.1 + MAN_COL_WIDTH);
    let mut end = man.1;
    let mut append = |present: bool, width: usize| -> Option<(usize, usize)> {
        if !present {
            return None;
        }
        let range = (end, end + width);
        end = range.1;
        Some(range)
    };
    let misattr = append(with_misattr, MISATTR_COL_WIDTH);
    let existence = append(with_existence, EXISTENCE_COL_WIDTH);
    let bundle = append(with_bundle, BUNDLE_COL_WIDTH);
    let status_start = end + 2;
    RowOffsets {
        tool,
        tier,
        framework,
        nodes,
        flags,
        pct,
        ms,
        suspect,
        man,
        misattr,
        existence,
        bundle,
        status_start,
    }
}

/// Slice `chars[start..end]` as a trimmed `String`, or `None` if the line
/// is too short to contain that field at all (a corrupt/truncated line).
fn slice(chars: &[char], range: (usize, usize)) -> Option<String> {
    let (start, end) = range;
    if chars.len() < end {
        return None;
    }
    Some(
        chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

/// Parse one data line into a [`ParsedRow`], or a reason it was dropped.
///
/// The success variant is boxed because the other two carry nothing at all,
/// and every added scoreboard column grows `ParsedRow` a little further past
/// the point where the whole enum costs a rejected line as much memory as an
/// accepted one (`clippy::large_enum_variant`, which the `bundle` column
/// tipped over).
enum LineResult {
    Row(Box<ParsedRow>),
    Truncated,
    Unparseable,
}

fn parse_line(line: &str, offsets: &RowOffsets) -> LineResult {
    let chars: Vec<char> = line.chars().collect();

    let Some(tool) = slice(&chars, offsets.tool) else {
        return LineResult::Unparseable;
    };
    // Detection mirrors `coverage::truncate_col` exactly: it never leaves a
    // string shorter than `width` chars once it truncates, and it only
    // ever appends `…` in that path. A tool name that happens to be
    // naturally exactly `TOOL_COL_WIDTH` chars long *and* end in a literal
    // `…` is the one false positive this shares with the renderer itself;
    // accepted for the same reason `truncate_col`'s own doc comment
    // accepts it (real tool names are overwhelmingly ASCII).
    if tool.chars().count() == TOOL_COL_WIDTH && tool.ends_with('…') {
        return LineResult::Truncated;
    }
    if tool.is_empty() {
        return LineResult::Unparseable;
    }

    let Some(tiers) = slice(&chars, offsets.tier) else {
        return LineResult::Unparseable;
    };
    let Some(framework) = slice(&chars, offsets.framework) else {
        return LineResult::Unparseable;
    };
    let Some(nodes_s) = slice(&chars, offsets.nodes) else {
        return LineResult::Unparseable;
    };
    let Some(flags_s) = slice(&chars, offsets.flags) else {
        return LineResult::Unparseable;
    };
    let Some(pct_s) = slice(&chars, offsets.pct) else {
        return LineResult::Unparseable;
    };
    let Some(ms_s) = slice(&chars, offsets.ms) else {
        return LineResult::Unparseable;
    };
    let Some(suspect_s) = slice(&chars, offsets.suspect) else {
        return LineResult::Unparseable;
    };
    let Some(man_s) = slice(&chars, offsets.man) else {
        return LineResult::Unparseable;
    };
    let misattribution_suspect_count = match offsets.misattr {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    let existence_fabrication_count = match offsets.existence {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    let bundle_collapse_count = match offsets.bundle {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    if chars.len() < offsets.status_start {
        return LineResult::Unparseable;
    }
    let status: String = chars[offsets.status_start..].iter().collect::<String>();
    let status = status.trim().to_string();
    if status.is_empty() {
        return LineResult::Unparseable;
    }

    let (Ok(nodes), Ok(flags), Ok(ms), Ok(suspicious_nodes)) = (
        nodes_s.parse::<usize>(),
        flags_s.parse::<usize>(),
        ms_s.parse::<u128>(),
        suspect_s.parse::<usize>(),
    ) else {
        return LineResult::Unparseable;
    };
    let pct_flags_with_text = pct_s
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|_| pct_s != "—" && pct_s != "-");
    let man_shaped = man_s == "yes";

    LineResult::Row(Box::new(ParsedRow {
        tool,
        tiers,
        framework,
        nodes,
        flags,
        pct_flags_with_text,
        ms,
        suspicious_nodes,
        man_shaped,
        misattribution_suspect_count,
        existence_fabrication_count,
        bundle_collapse_count,
        status,
    }))
}

/// Parse a rendered [`crate::coverage::ScoreFormat::Text`] scoreboard back
/// into rows. Every `#`-prefixed line (the aggregate footer and every
/// informational section after it — `render_text`'s own convention) and
/// every blank line is skipped; the header line is used only to detect
/// [`has_misattr_column`] and is never itself parsed as data.
pub fn parse_scoreboard(text: &str) -> ParsedScoreboard {
    let mut out = ParsedScoreboard::default();
    let mut lines = text.lines();
    let Some(header) = lines.find(|l| !l.trim().is_empty()) else {
        return out;
    };
    let offsets = row_offsets(
        has_misattr_column(header),
        has_existence_column(header),
        has_bundle_column(header),
    );

    // Two passes over the same remaining lines: the first (data rows) stops
    // at the first `#`-prefixed line exactly as before; the second (the
    // `#fp` fingerprint footer, added by this task) scans every remaining
    // line regardless, since fingerprint lines live *after* every other
    // footer section (`coverage::render_text`'s emission order) and would
    // never be reached by a loop that breaks on the first `#`. `Lines` is
    // `Clone` (a cheap cursor over the same borrowed `&str`), so this costs
    // no extra allocation or re-reading of the file.
    let footer = lines.clone();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        // Every footer section (`# aggregate: ...`, `# accuracy: ...`,
        // `# framework-detection: ...`, `# worst-parsed ...`, `# ... N.
        // ...`, `# misattribution-suspects ...`) starts with `#` — the one
        // and only marker `render_text` ever uses for non-data lines
        // (`aggregate_footer_line`, `accuracy_unmeasured_line`,
        // `framework_summary_lines`, `worst_parsed_lines_text`,
        // `misattribution_sample_lines_text`, grepped). Once one appears,
        // every remaining line is footer, so stop scanning for data rows
        // entirely rather than re-testing each one.
        if line.starts_with('#') {
            break;
        }
        match parse_line(line, &offsets) {
            LineResult::Row(row) => {
                out.rows.insert(row.tool.clone(), *row);
            }
            LineResult::Truncated => out.truncated_dropped += 1,
            LineResult::Unparseable => out.unparseable_dropped += 1,
        }
    }
    for line in footer {
        if let Some(rest) = line.strip_prefix("#fp ") {
            if let Some((tool, fp)) = parse_fingerprint_line(rest) {
                out.fingerprints.insert(tool, fp);
            }
        }
    }
    out
}

/// One matched tool's flag-count comparison. Kept as a signed delta
/// alongside both raw counts — never reduced to a single "net" number, per
/// this module's doc comment on why netting hides exactly the losses that
/// caught real regressions on this branch.
struct FlagDelta<'a> {
    tool: &'a str,
    before: usize,
    after: usize,
}

impl FlagDelta<'_> {
    fn delta(&self) -> i64 {
        self.after as i64 - self.before as i64
    }
}

/// One matched tool's status change.
struct StatusTransition<'a> {
    tool: &'a str,
    before: &'a str,
    after: &'a str,
}

/// One matched tool's field-level diff (WS2 part 2) — the granularity a
/// bare flag-count delta cannot see. Every list is a set of stable flag
/// identities or subcommand paths ([`crate::coverage::flag_identity`]),
/// never a count, per this module's requirement to report *what* changed,
/// not just *how many* — a count here would rebuild exactly the blind spot
/// this task exists to close.
struct FieldDiff<'a> {
    tool: &'a str,
    flags_added: Vec<&'a str>,
    flags_removed: Vec<&'a str>,
    /// Flags present on both sides whose description's presence or hash
    /// differs — catches both "text deleted" (`has_description` flips) and
    /// "text changed to something else" (hash differs, presence unchanged).
    description_changed: Vec<&'a str>,
    /// Flags present on both sides whose choices-list hash differs —
    /// catches both an added/fabricated choices list and a removed one
    /// (`None` on one side, `Some` on the other hashes as unequal).
    choices_changed: Vec<&'a str>,
    /// Flags present on both sides whose `value_name` text differs.
    value_name_changed: Vec<&'a str>,
    subcommands_added: Vec<&'a str>,
    subcommands_removed: Vec<&'a str>,
    tier_changed: Option<(&'a str, &'a str)>,
    framework_changed: Option<(&'a str, &'a str)>,
}

impl FieldDiff<'_> {
    /// True if this tool has at least one field-level change — the
    /// predicate that decides whether it earns a row in the report at all
    /// ([`diff`] only ever constructs a `FieldDiff` when this would be
    /// true, but kept as a named method rather than inlined so the "what
    /// counts as changed" list has exactly one definition).
    fn is_empty(&self) -> bool {
        self.flags_added.is_empty()
            && self.flags_removed.is_empty()
            && self.description_changed.is_empty()
            && self.choices_changed.is_empty()
            && self.value_name_changed.is_empty()
            && self.subcommands_added.is_empty()
            && self.subcommands_removed.is_empty()
            && self.tier_changed.is_none()
            && self.framework_changed.is_none()
    }
}

/// The full computed diff between two scoreboards, ready to render in
/// either format — computed once, rendered by [`render_text`] or
/// [`render_markdown`] so the two formats can never disagree about what
/// changed, only how it's displayed.
pub struct Transition<'a> {
    before: &'a ParsedScoreboard,
    after: &'a ParsedScoreboard,
    appeared: Vec<&'a str>,
    disappeared: Vec<&'a str>,
    near_cap: Vec<&'a str>,
    status_transitions: Vec<StatusTransition<'a>>,
    flag_gains: Vec<FlagDelta<'a>>,
    flag_losses: Vec<FlagDelta<'a>>,
    /// Per-tool field-level diffs — only tools with at least one change
    /// ([`FieldDiff::is_empty`] false), sorted by tool name. Empty (not
    /// absent) when neither side's scoreboard carries a `#fp` footer at
    /// all, or when every matched tool's fingerprint is identical.
    field_diffs: Vec<FieldDiff<'a>>,
    /// Tools present, matched, and outside the near-cap exclusion, but
    /// whose fingerprint could not be compared because at least one side's
    /// scoreboard predates the `#fp` footer (`ParsedScoreboard::fingerprints`'s
    /// doc comment) — reported so "no field-level changes" is never
    /// confused with "field-level comparison wasn't possible."
    field_diff_unmeasured: usize,
}

impl Transition<'_> {
    /// **The identical/changed determination `sweep-diff` reports.** A run
    /// is only "identical" when *nothing* changed across every dimension
    /// this module measures — appearances, disappearances, status,
    /// flag-count, and now field-level content — not merely when the
    /// coarser dimensions stayed flat. This is exactly the gap PR #14 fell
    /// through: `pngfix`'s and `pod2man`'s flag *counts* were unchanged (a
    /// description going empty doesn't remove the flag, and a fabricated
    /// choices list doesn't add one), so a determination based on counts
    /// alone would still call that run identical. Non-blocking either way
    /// (maintainer decision D4, this module's own doc comment) — this
    /// governs what the report *says*, never the exit code.
    pub fn is_identical(&self) -> bool {
        self.appeared.is_empty()
            && self.disappeared.is_empty()
            && self.status_transitions.is_empty()
            && self.flag_gains.is_empty()
            && self.flag_losses.is_empty()
            && self.field_diffs.is_empty()
    }
}

/// Compute the transition between two parsed scoreboards.
///
/// Tools whose `ms` is [`near_timeout_cap`] on *either* side are excluded
/// from status transitions and flag deltas entirely (spec §13.1b rule 3;
/// maintainer decision D4) and reported only in their own section — a
/// status or count derived under timeout pressure is a statement about the
/// machine that ran it, not the parser, and mixing it into the headline
/// numbers is exactly the `waagent2.0` false regression (AGENTS.md) this
/// rule exists to stop from recurring.
pub fn diff<'a>(before: &'a ParsedScoreboard, after: &'a ParsedScoreboard) -> Transition<'a> {
    let mut appeared = Vec::new();
    let mut disappeared = Vec::new();
    let mut near_cap = Vec::new();
    let mut status_transitions = Vec::new();
    let mut flag_gains = Vec::new();
    let mut flag_losses = Vec::new();
    let mut field_diffs = Vec::new();
    let mut field_diff_unmeasured = 0usize;

    for (tool, after_row) in &after.rows {
        let Some(before_row) = before.rows.get(tool) else {
            appeared.push(tool.as_str());
            continue;
        };
        if before_row.near_cap() || after_row.near_cap() {
            near_cap.push(tool.as_str());
            continue;
        }
        if before_row.status != after_row.status {
            status_transitions.push(StatusTransition {
                tool,
                before: &before_row.status,
                after: &after_row.status,
            });
        }
        if before_row.flags != after_row.flags {
            let d = FlagDelta {
                tool,
                before: before_row.flags,
                after: after_row.flags,
            };
            if d.delta() > 0 {
                flag_gains.push(d);
            } else {
                flag_losses.push(d);
            }
        }

        let tier_changed = (before_row.tiers != after_row.tiers)
            .then_some((before_row.tiers.as_str(), after_row.tiers.as_str()));
        let framework_changed = (before_row.framework != after_row.framework)
            .then_some((before_row.framework.as_str(), after_row.framework.as_str()));

        // Three states, not two (the defect this match used to have:
        // `coverage::fingerprint_lines` used to skip a row with no flags and
        // no subcommands, so a tool that lost every flag produced a line on
        // the "before" side and none on the "after" side, and fell into the
        // catch-all below — "unmeasured" — instead of reporting the total
        // loss it actually was). Now that every row gets a `#fp` line
        // unconditionally, a line is absent on *both* sides only for a
        // genuinely legacy scoreboard pair; absent on *one* side only means
        // "no record for this side," read as empty (`EMPTY_FINGERPRINT`'s
        // own doc comment) so the diff still reports the present side's
        // flags/subcommands as added or removed rather than staying silent.
        match (before.fingerprints.get(tool), after.fingerprints.get(tool)) {
            (None, None) => {
                // Neither side has a `#fp` entry for this tool — the
                // genuine legacy case (this scoreboard pair predates the
                // footer entirely, or — vanishingly rarely — this one row's
                // line failed to parse on both sides). Field-level
                // comparison is impossible, not "nothing changed"
                // (`ParsedScoreboard::fingerprints`'s doc comment). Still
                // surface a tier/framework change if one was found from the
                // ordinary columns, which every scoreboard shape carries.
                if tier_changed.is_some() || framework_changed.is_some() {
                    field_diffs.push(FieldDiff {
                        tool,
                        flags_added: Vec::new(),
                        flags_removed: Vec::new(),
                        description_changed: Vec::new(),
                        choices_changed: Vec::new(),
                        value_name_changed: Vec::new(),
                        subcommands_added: Vec::new(),
                        subcommands_removed: Vec::new(),
                        tier_changed,
                        framework_changed,
                    });
                } else {
                    field_diff_unmeasured += 1;
                }
            }
            (bfp, afp) => {
                // At least one side has a real entry — diff it against the
                // other side's entry, or against `EMPTY_FINGERPRINT` when
                // the other side has none. Covers both the ordinary
                // both-measured case and the deletion/mixed-vintage case.
                let bfp = bfp.unwrap_or(&EMPTY_FINGERPRINT);
                let afp = afp.unwrap_or(&EMPTY_FINGERPRINT);
                let fd = field_diff(tool, bfp, afp, tier_changed, framework_changed);
                if !fd.is_empty() {
                    field_diffs.push(fd);
                }
            }
        }
    }
    for tool in before.rows.keys() {
        if !after.rows.contains_key(tool) {
            disappeared.push(tool.as_str());
        }
    }

    appeared.sort_unstable();
    disappeared.sort_unstable();
    near_cap.sort_unstable();
    // Losses first within their own list, worst (most flags lost) first —
    // "the bar is losses, not net" extends to ranking: the tool that lost
    // the most is the one worth looking at first.
    flag_losses.sort_by_key(|d| (d.delta(), d.tool.to_string()));
    flag_gains.sort_by_key(|d| (std::cmp::Reverse(d.delta()), d.tool.to_string()));
    status_transitions.sort_by_key(|t| t.tool.to_string());
    field_diffs.sort_by_key(|d| d.tool.to_string());

    Transition {
        before,
        after,
        appeared,
        disappeared,
        near_cap,
        status_transitions,
        flag_gains,
        flag_losses,
        field_diffs,
        field_diff_unmeasured,
    }
}

/// Compute one matched tool's [`FieldDiff`] from its before/after
/// fingerprints — pure set/map comparison, no I/O, no knowledge of what a
/// flag or subcommand *means*, only whether the same identity's recorded
/// fields match (this module's `no per-tool logic` invariant: nothing here
/// keys off a tool name).
fn field_diff<'a>(
    tool: &'a str,
    before: &'a ParsedFingerprint,
    after: &'a ParsedFingerprint,
    tier_changed: Option<(&'a str, &'a str)>,
    framework_changed: Option<(&'a str, &'a str)>,
) -> FieldDiff<'a> {
    let mut flags_added = Vec::new();
    let mut flags_removed = Vec::new();
    let mut description_changed = Vec::new();
    let mut choices_changed = Vec::new();
    let mut value_name_changed = Vec::new();

    for (id, after_f) in &after.flags {
        match before.flags.get(id) {
            None => flags_added.push(id.as_str()),
            Some(before_f) => {
                if before_f.has_description != after_f.has_description
                    || before_f.description_hash != after_f.description_hash
                {
                    description_changed.push(id.as_str());
                }
                if before_f.choices_hash != after_f.choices_hash {
                    choices_changed.push(id.as_str());
                }
                if before_f.value_name != after_f.value_name {
                    value_name_changed.push(id.as_str());
                }
            }
        }
    }
    for id in before.flags.keys() {
        if !after.flags.contains_key(id) {
            flags_removed.push(id.as_str());
        }
    }

    let subcommands_added = after
        .subcommands
        .iter()
        .filter(|s| !before.subcommands.contains(*s))
        .map(String::as_str)
        .collect();
    let subcommands_removed = before
        .subcommands
        .iter()
        .filter(|s| !after.subcommands.contains(*s))
        .map(String::as_str)
        .collect();

    flags_added.sort_unstable();
    flags_removed.sort_unstable();
    description_changed.sort_unstable();
    choices_changed.sort_unstable();
    value_name_changed.sort_unstable();

    FieldDiff {
        tool,
        flags_added,
        flags_removed,
        description_changed,
        choices_changed,
        value_name_changed,
        subcommands_added,
        subcommands_removed,
        tier_changed,
        framework_changed,
    }
}

/// Cap on how many rows a table shows before folding the rest behind a
/// count — same reasoning and same order of magnitude as
/// `coverage::WORST_PARSED_LIMIT`: a full-`PATH` sweep runs a couple
/// thousand tools, and a report nobody can scan is a report nobody reads.
const TABLE_ROW_LIMIT: usize = 40;

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Render [`Transition`] as GitHub-flavored markdown for
/// `$GITHUB_STEP_SUMMARY` — the format this whole module's doc comment
/// insists on over a raw scoreboard-file diff.
pub fn render_markdown(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str("## Sweep transition report\n\n");
    out.push_str(
        "A semantic per-tool diff between two scoreboards — the check that has actually \
         caught every regression on this branch so far, run by hand until now. **Non-blocking**: \
         this never fails a run (maintainer decision D4); it is a loud report during burn-in, \
         promoted to a gate later.\n\n",
    );
    out.push_str(&format!(
        "**Overall: {}.** This now accounts for field-level content (per-flag \
         description/choices/value_name), not just tool appearances, status and flag counts — a \
         run that only edits a description's text no longer reports as identical.\n\n",
        if t.is_identical() {
            "IDENTICAL"
        } else {
            "CHANGED"
        },
    ));
    out.push_str(&format!(
        "**{before_total} → {after_total} tools.** {matched} matched, {appeared} appeared, \
         {disappeared} disappeared, {near_cap} excluded (near the {cap}s timeout cap).\n\n",
        before_total = t.before.rows.len(),
        after_total = t.after.rows.len(),
        matched = t.after.rows.len() - t.appeared.len(),
        appeared = t.appeared.len(),
        disappeared = t.disappeared.len(),
        near_cap = t.near_cap.len(),
        cap = EXTRACT_TIMEOUT_MS / 1000,
    ));
    if t.before.truncated_dropped > 0 || t.after.truncated_dropped > 0 {
        out.push_str(&format!(
            "> [!NOTE]\n> {before_trunc} tool name(s) truncated in the \"before\" scoreboard and \
             {after_trunc} in \"after\" were dropped from this diff entirely — a truncated name \
             (`coverage::truncate_col`'s `…` marker) can collide with a different tool's \
             truncated name, and joining on it would silently corrupt the comparison. See the raw \
             scoreboard files for full names.\n\n",
            before_trunc = t.before.truncated_dropped,
            after_trunc = t.after.truncated_dropped,
        ));
    }
    if t.before.unparseable_dropped > 0 || t.after.unparseable_dropped > 0 {
        out.push_str(&format!(
            "> [!NOTE]\n> {before_bad} row(s) in \"before\" and {after_bad} in \"after\" did not \
             parse as a scoreboard data row and were skipped.\n\n",
            before_bad = t.before.unparseable_dropped,
            after_bad = t.after.unparseable_dropped,
        ));
    }

    out.push_str("### Status transitions\n\n");
    if t.status_transitions.is_empty() {
        out.push_str("No matched tool (outside the near-cap exclusion) changed status.\n\n");
    } else {
        out.push_str(&format!(
            "**{} tool(s) changed status** (near-cap tools excluded — see below):\n\n",
            t.status_transitions.len()
        ));
        out.push_str("| tool | before | after |\n|---|---|---|\n");
        for row in t.status_transitions.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                escape_md(row.tool),
                escape_md(row.before),
                escape_md(row.after),
            ));
        }
        if t.status_transitions.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.status_transitions.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }

    let total_lost: i64 = t.flag_losses.iter().map(|d| -d.delta()).sum();
    out.push_str("### Flag-count losses (the bar — never netted against gains)\n\n");
    if t.flag_losses.is_empty() {
        out.push_str("No matched tool lost flags.\n\n");
    } else {
        out.push_str(&format!(
            "**{total_lost} flag(s) lost across {n} tool(s).** A gain elsewhere never offsets \
             this — see this module's doc comment.\n\n",
            n = t.flag_losses.len(),
        ));
        out.push_str("| tool | before | after | lost |\n|---|---|---|---|\n");
        for d in t.flag_losses.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_md(d.tool),
                d.before,
                d.after,
                -d.delta(),
            ));
        }
        if t.flag_losses.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.flag_losses.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }

    let total_gained: i64 = t.flag_gains.iter().map(|d| d.delta()).sum();
    out.push_str("### Flag-count gains\n\n");
    if t.flag_gains.is_empty() {
        out.push_str("No matched tool gained flags.\n\n");
    } else {
        out.push_str(&format!(
            "**{total_gained} flag(s) gained across {n} tool(s).**\n\n",
            n = t.flag_gains.len(),
        ));
        out.push_str("| tool | before | after | gained |\n|---|---|---|---|\n");
        for d in t.flag_gains.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_md(d.tool),
                d.before,
                d.after,
                d.delta(),
            ));
        }
        if t.flag_gains.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.flag_gains.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }

    out.push_str("### Field-level changes\n\n");
    if t.field_diffs.is_empty() {
        out.push_str(
            "No matched tool's flag set, per-flag description/choices/value_name, \
             subcommand set, tier, or framework changed.\n\n",
        );
    } else {
        out.push_str(&format!(
            "**{} tool(s) changed at field granularity** (adds/removes/changes, never just a \
             count — see this module's doc comment):\n\n",
            t.field_diffs.len(),
        ));
        for fd in t.field_diffs.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!("- **{}**", escape_md(fd.tool)));
            let mut parts = Vec::new();
            if !fd.flags_added.is_empty() {
                parts.push(format!("flags added: {}", capped_join(&fd.flags_added)));
            }
            if !fd.flags_removed.is_empty() {
                parts.push(format!("flags removed: {}", capped_join(&fd.flags_removed)));
            }
            if !fd.description_changed.is_empty() {
                parts.push(format!(
                    "description changed: {}",
                    capped_join(&fd.description_changed)
                ));
            }
            if !fd.choices_changed.is_empty() {
                parts.push(format!(
                    "choices changed: {}",
                    capped_join(&fd.choices_changed)
                ));
            }
            if !fd.value_name_changed.is_empty() {
                parts.push(format!(
                    "value_name changed: {}",
                    capped_join(&fd.value_name_changed)
                ));
            }
            if !fd.subcommands_added.is_empty() {
                parts.push(format!(
                    "subcommands added: {}",
                    capped_join(&fd.subcommands_added)
                ));
            }
            if !fd.subcommands_removed.is_empty() {
                parts.push(format!(
                    "subcommands removed: {}",
                    capped_join(&fd.subcommands_removed)
                ));
            }
            if let Some((b, a)) = fd.tier_changed {
                parts.push(format!("tier: {} -> {}", escape_md(b), escape_md(a)));
            }
            if let Some((b, a)) = fd.framework_changed {
                parts.push(format!("framework: {} -> {}", escape_md(b), escape_md(a)));
            }
            out.push_str(&format!(" — {}\n", parts.join("; ")));
        }
        if t.field_diffs.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.field_diffs.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    if t.field_diff_unmeasured > 0 {
        out.push_str(&format!(
            "> [!NOTE]\n> {} matched tool(s) could not be compared at field granularity — \
             neither scoreboard carries a `#fp` fingerprint entry for them, meaning this pair \
             predates the fingerprint footer entirely (a scoreboard that does carry it emits an \
             entry for every tool, including ones with no flags and no subcommands). Not counted \
             as \"no field-level changes.\"\n\n",
            t.field_diff_unmeasured,
        ));
    }

    if !t.appeared.is_empty() || !t.disappeared.is_empty() {
        out.push_str("### Appeared / disappeared\n\n");
        if !t.appeared.is_empty() {
            out.push_str(&format!(
                "**Appeared ({}):** {}\n\n",
                t.appeared.len(),
                escape_md(&capped_join(&t.appeared)),
            ));
        }
        if !t.disappeared.is_empty() {
            out.push_str(&format!(
                "**Disappeared ({}):** {}\n\n",
                t.disappeared.len(),
                escape_md(&capped_join(&t.disappeared)),
            ));
        }
    }

    if !t.near_cap.is_empty() {
        out.push_str(&format!(
            "### Excluded — near the {}s timeout cap\n\n",
            EXTRACT_TIMEOUT_MS / 1000
        ));
        out.push_str(
            "Elapsed time on at least one side was at or past half the single-probe extract \
             cap (spec §13.1b rule 3) — a status or flag count here may reflect machine load, \
             not a parser change. Reported for visibility, excluded from every number above. \
             See this module's doc comment (`near_timeout_cap`) for why the bound is one-sided.\n\n",
        );
        out.push_str("| tool | before status | before ms | after status | after ms |\n|---|---|---|---|---|\n");
        for tool in t.near_cap.iter().take(TABLE_ROW_LIMIT) {
            let b = t.before.rows.get(*tool);
            let a = t.after.rows.get(*tool);
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                escape_md(tool),
                b.map(|r| r.status.as_str()).unwrap_or("—"),
                b.map(|r| r.ms.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                a.map(|r| r.status.as_str()).unwrap_or("—"),
                a.map(|r| r.ms.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ));
        }
        if t.near_cap.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.near_cap.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }

    out
}

/// Cap on inline names before folding into a count-only summary — mirrors
/// `corpus::MARKDOWN_NAME_CAP`'s reasoning at the same scale for a tool
/// list this size.
const NAME_CAP: usize = 15;

fn capped_join(names: &[&str]) -> String {
    if names.len() <= NAME_CAP {
        names.join(", ")
    } else {
        format!(
            "{}, +{} more",
            names[..NAME_CAP].join(", "),
            names.len() - NAME_CAP
        )
    }
}

/// Plain-text rendering of [`Transition`], for a terminal or a plain log —
/// same content as [`render_markdown`], no GFM syntax. Mirrors
/// `coverage::render_text`/`render_markdown`'s own dual-format convention.
pub fn render_text(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "overall: {}\n",
        if t.is_identical() {
            "IDENTICAL"
        } else {
            "CHANGED"
        },
    ));
    out.push_str(&format!(
        "sweep transition: {before_total} -> {after_total} tools, {matched} matched, {appeared} appeared, {disappeared} disappeared, {near_cap} excluded (near {cap}s cap)\n",
        before_total = t.before.rows.len(),
        after_total = t.after.rows.len(),
        matched = t.after.rows.len() - t.appeared.len(),
        appeared = t.appeared.len(),
        disappeared = t.disappeared.len(),
        near_cap = t.near_cap.len(),
        cap = EXTRACT_TIMEOUT_MS / 1000,
    ));
    if t.before.truncated_dropped > 0 || t.after.truncated_dropped > 0 {
        out.push_str(&format!(
            "# dropped (truncated tool name): {} before, {} after — never joined, see doc comment\n",
            t.before.truncated_dropped, t.after.truncated_dropped
        ));
    }
    if t.before.unparseable_dropped > 0 || t.after.unparseable_dropped > 0 {
        out.push_str(&format!(
            "# dropped (unparseable row): {} before, {} after\n",
            t.before.unparseable_dropped, t.after.unparseable_dropped
        ));
    }
    out.push('\n');

    out.push_str("# status transitions\n");
    if t.status_transitions.is_empty() {
        out.push_str("(none)\n");
    } else {
        for row in &t.status_transitions {
            out.push_str(&format!(
                "  {}: {} -> {}\n",
                row.tool, row.before, row.after
            ));
        }
    }
    out.push('\n');

    let total_lost: i64 = t.flag_losses.iter().map(|d| -d.delta()).sum();
    out.push_str(&format!(
        "# flag-count losses (the bar — never netted): {total_lost} lost across {} tool(s)\n",
        t.flag_losses.len()
    ));
    for d in &t.flag_losses {
        out.push_str(&format!(
            "  {}: {} -> {} ({})\n",
            d.tool,
            d.before,
            d.after,
            d.delta()
        ));
    }
    out.push('\n');

    let total_gained: i64 = t.flag_gains.iter().map(|d| d.delta()).sum();
    out.push_str(&format!(
        "# flag-count gains: {total_gained} gained across {} tool(s)\n",
        t.flag_gains.len()
    ));
    for d in &t.flag_gains {
        out.push_str(&format!(
            "  {}: {} -> {} (+{})\n",
            d.tool,
            d.before,
            d.after,
            d.delta()
        ));
    }
    out.push('\n');

    out.push_str(&format!(
        "# field-level changes: {} tool(s) (adds/removes/changes, never just a count)\n",
        t.field_diffs.len()
    ));
    for fd in &t.field_diffs {
        let mut parts = Vec::new();
        if !fd.flags_added.is_empty() {
            parts.push(format!("flags added: {}", fd.flags_added.join(", ")));
        }
        if !fd.flags_removed.is_empty() {
            parts.push(format!("flags removed: {}", fd.flags_removed.join(", ")));
        }
        if !fd.description_changed.is_empty() {
            parts.push(format!(
                "description changed: {}",
                fd.description_changed.join(", ")
            ));
        }
        if !fd.choices_changed.is_empty() {
            parts.push(format!(
                "choices changed: {}",
                fd.choices_changed.join(", ")
            ));
        }
        if !fd.value_name_changed.is_empty() {
            parts.push(format!(
                "value_name changed: {}",
                fd.value_name_changed.join(", ")
            ));
        }
        if !fd.subcommands_added.is_empty() {
            parts.push(format!(
                "subcommands added: {}",
                fd.subcommands_added.join(", ")
            ));
        }
        if !fd.subcommands_removed.is_empty() {
            parts.push(format!(
                "subcommands removed: {}",
                fd.subcommands_removed.join(", ")
            ));
        }
        if let Some((b, a)) = fd.tier_changed {
            parts.push(format!("tier: {b} -> {a}"));
        }
        if let Some((b, a)) = fd.framework_changed {
            parts.push(format!("framework: {b} -> {a}"));
        }
        out.push_str(&format!("  {}: {}\n", fd.tool, parts.join("; ")));
    }
    if t.field_diff_unmeasured > 0 {
        out.push_str(&format!(
            "# field-level comparison unavailable for {} matched tool(s) — neither scoreboard carries a #fp entry for them (this pair predates the fingerprint footer entirely)\n",
            t.field_diff_unmeasured
        ));
    }
    out.push('\n');

    out.push_str(&format!(
        "# appeared ({}): {}\n",
        t.appeared.len(),
        t.appeared.join(", ")
    ));
    out.push_str(&format!(
        "# disappeared ({}): {}\n",
        t.disappeared.len(),
        t.disappeared.join(", ")
    ));
    out.push('\n');

    out.push_str(&format!(
        "# excluded, near {}s timeout cap ({})\n",
        EXTRACT_TIMEOUT_MS / 1000,
        t.near_cap.len()
    ));
    for tool in &t.near_cap {
        let b = t.before.rows.get(*tool);
        let a = t.after.rows.get(*tool);
        out.push_str(&format!(
            "  {}: {}@{}ms -> {}@{}ms\n",
            tool,
            b.map(|r| r.status.as_str()).unwrap_or("—"),
            b.map(|r| r.ms).unwrap_or(0),
            a.map(|r| r.status.as_str()).unwrap_or("—"),
            a.map(|r| r.ms).unwrap_or(0),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{run_over, ScoreFormat};

    /// Round-trips a scoreboard this binary just rendered itself — the
    /// sanity floor: if the renderer and parser ever drift, this is the
    /// test that catches it before a real sweep does.
    ///
    /// Checks `status`/`existence_fabrication_count`/
    /// `misattribution_suspect_count` explicitly, not just row presence:
    /// a fixed-offset desync (e.g. adding `exist` — [`crate::existence`],
    /// this task — without teaching this module its width) still leaves
    /// the `tool` column, and therefore the row key, intact, so a presence-
    /// only check would have stayed green through exactly that bug. This
    /// version would not have.
    #[test]
    fn parses_a_freshly_rendered_scoreboard_back_out() {
        let (table, _agg) = run_over(
            vec!["sh".to_string(), "true".to_string()],
            None,
            false,
            ScoreFormat::Text,
        );
        let parsed = parse_scoreboard(&table);
        assert_eq!(parsed.truncated_dropped, 0);
        assert_eq!(parsed.unparseable_dropped, 0);
        for tool in ["sh", "true"] {
            let row = parsed
                .rows
                .get(tool)
                .unwrap_or_else(|| panic!("{tool} row parsed"));
            assert_eq!(row.misattribution_suspect_count, Some(0));
            assert_eq!(row.existence_fabrication_count, Some(0));
            assert!(
                row.status
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase()),
                "status field looks corrupted (fixed-offset desync?): {:?}",
                row.status
            );
        }
    }

    fn sample_text(rows: &[&str]) -> String {
        let mut out = String::from(
            "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr  status\n",
        );
        for r in rows {
            out.push_str(r);
            out.push('\n');
        }
        out.push_str("# aggregate: pct_flags_with_text=90.00 no_tier_count=0 total=1\n");
        out
    }

    /// A hand-built row at the current (with `misattr`) column widths,
    /// matching exactly what `render_text` would produce for these values.
    fn row_line(tool: &str, status: &str, flags: usize, ms: u128) -> String {
        format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}",
            tool, "help", "—", 1, flags, "100%", ms, 0, "-", 0, status,
        )
    }

    #[test]
    fn parses_a_hand_built_row_at_current_widths() {
        let text = sample_text(&[&row_line("git", "ok", 34, 120)]);
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.ms, 120);
        assert_eq!(row.status, "ok");
        assert_eq!(row.misattribution_suspect_count, Some(0));
    }

    /// A hand-built row at the current (`misattr` + `exist`) column widths —
    /// the shape every scoreboard this task's own `cargo xtask coverage`
    /// run produces.
    fn row_line_with_existence(
        tool: &str,
        status: &str,
        flags: usize,
        ms: u128,
        existence_fabrication_count: usize,
    ) -> String {
        format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}{:>6}  {}",
            tool, "help", "—", 1, flags, "100%", ms, 0, "-", 0, existence_fabrication_count, status,
        )
    }

    #[test]
    fn parses_a_hand_built_row_with_the_existence_column() {
        let header = "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr exist  status\n";
        let row = row_line_with_existence("git", "ok", 34, 120, 2);
        let text = format!(
            "{header}{row}\n# aggregate: pct_flags_with_text=90.00 no_tier_count=0 total=1\n"
        );
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.status, "ok");
        assert_eq!(row.misattribution_suspect_count, Some(0));
        assert_eq!(row.existence_fabrication_count, Some(2));
    }

    /// A scoreboard from before the misattribution detector existed has no
    /// `misattr` column at all (and therefore no `exist` column either,
    /// since `exist` was only ever appended after `misattr`) — the four
    /// real scratch scoreboards used to verify this module during
    /// development are exactly this shape.
    #[test]
    fn parses_a_legacy_row_with_no_misattr_column() {
        let header = "tool                     tier(s)            framework                    nodes   flags   %described     ms suspect   man  status\n";
        let row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}  {}\n",
            "git", "help", "—", 1, 34, "100%", 120, 0, "-", "ok",
        );
        let text =
            format!("{header}{row}# aggregate: pct_described=90.00 no_tier_count=0 total=1\n");
        let parsed = parse_scoreboard(&text);
        let row = parsed.rows.get("git").expect("git row parsed");
        assert_eq!(row.flags, 34);
        assert_eq!(row.misattribution_suspect_count, None);
        assert_eq!(row.existence_fabrication_count, None);
    }

    /// The exact hazard this module's doc comment describes: a truncated
    /// tool name must never be joined, because two different real names can
    /// truncate to the same string.
    #[test]
    fn truncated_tool_names_are_dropped_not_joined() {
        let long_name = "a".repeat(TOOL_COL_WIDTH + 5);
        let truncated = format!(
            "{}…",
            long_name
                .chars()
                .take(TOOL_COL_WIDTH - 1)
                .collect::<String>()
        );
        let text = sample_text(&[&row_line(&truncated, "ok", 5, 50)]);
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.truncated_dropped, 1);
        assert!(parsed.rows.is_empty());
    }

    /// A short tool name that merely happens to be padded to the column
    /// width is not truncated and must parse normally.
    #[test]
    fn short_tool_names_are_not_mistaken_for_truncated() {
        let text = sample_text(&[&row_line("git", "ok", 5, 50)]);
        let parsed = parse_scoreboard(&text);
        assert_eq!(parsed.truncated_dropped, 0);
        assert!(parsed.rows.contains_key("git"));
    }

    fn scoreboard(rows: Vec<(&str, &str, usize, u128)>) -> ParsedScoreboard {
        let lines: Vec<String> = rows
            .iter()
            .map(|(tool, status, flags, ms)| row_line(tool, status, *flags, *ms))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        parse_scoreboard(&sample_text(&refs))
    }

    #[test]
    fn diff_reports_status_transitions() {
        let before = scoreboard(vec![("foo", "low-confidence", 5, 50)]);
        let after = scoreboard(vec![("foo", "ok", 5, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.status_transitions.len(), 1);
        assert_eq!(t.status_transitions[0].before, "low-confidence");
        assert_eq!(t.status_transitions[0].after, "ok");
    }

    /// The central rule this module exists to enforce: a tool that gains
    /// 2,000 flags elsewhere never cancels out a tool that lost 6 — losses
    /// and gains are reported as two independent totals.
    #[test]
    fn gains_and_losses_are_never_netted() {
        let before = scoreboard(vec![("big", "ok", 100, 50), ("lsof", "ok", 100, 50)]);
        let after = scoreboard(vec![("big", "ok", 2100, 50), ("lsof", "ok", 94, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.flag_gains.len(), 1);
        assert_eq!(t.flag_gains[0].delta(), 2000);
        assert_eq!(t.flag_losses.len(), 1);
        assert_eq!(t.flag_losses[0].delta(), -6);
        let md = render_markdown(&t);
        assert!(md.contains("6 flag(s) lost across 1 tool(s)"));
        assert!(md.contains("2000 flag(s) gained across 1 tool(s)"));
    }

    #[test]
    fn appeared_and_disappeared_tools_are_reported() {
        let before = scoreboard(vec![("old", "ok", 5, 50)]);
        let after = scoreboard(vec![("new", "ok", 5, 50)]);
        let t = diff(&before, &after);
        assert_eq!(t.appeared, vec!["new"]);
        assert_eq!(t.disappeared, vec!["old"]);
    }

    /// The timeout-bucketing proof: a tool whose elapsed time crosses half
    /// the single-probe cap must be excluded from status transitions and
    /// flag deltas, landing only in its own section, exactly as `waagent2.0`
    /// (AGENTS.md) should have been treated instead of read as a real
    /// regression.
    #[test]
    fn near_cap_tools_are_excluded_from_every_gated_dimension() {
        let before = scoreboard(vec![("waagent2.0", "ok", 40, 8_000)]);
        let after = scoreboard(vec![("waagent2.0", "verbatim", 0, 6_000)]);
        let t = diff(&before, &after);
        assert!(t.status_transitions.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.flag_gains.is_empty());
        assert_eq!(t.near_cap, vec!["waagent2.0"]);
        let md = render_markdown(&t);
        assert!(md.contains("Excluded — near the 10s timeout cap"));
        assert!(md.contains("waagent2.0"));
    }

    #[test]
    fn ms_well_under_the_cap_is_not_bucketed_as_near_cap() {
        assert!(!near_timeout_cap(500));
        assert!(!near_timeout_cap(4_999));
    }

    #[test]
    fn ms_at_or_past_half_the_cap_is_near_cap() {
        assert!(near_timeout_cap(5_000));
        assert!(near_timeout_cap(9_999));
        assert!(near_timeout_cap(10_000));
        // Well past the cap — the multi-probe `waagent2.0` shape — still
        // counts, per `near_timeout_cap`'s own doc comment on why there's
        // no upper bound.
        assert!(near_timeout_cap(41_900));
    }

    #[test]
    fn render_text_produces_a_structured_non_diff_report() {
        let before = scoreboard(vec![("foo", "ok", 5, 50)]);
        let after = scoreboard(vec![("foo", "suspicious", 5, 50)]);
        let t = diff(&before, &after);
        let text = render_text(&t);
        assert!(text.contains("foo: ok -> suspicious"));
    }

    /// Attach `#fp` fingerprints to a scoreboard already built by
    /// [`scoreboard`] — the counts/status/tiers/framework columns and the
    /// field-level fingerprint are independent inputs to [`diff`], and a
    /// test that wants to hold the former fixed while varying only the
    /// latter (exactly PR #14's shape: flag counts and status untouched,
    /// only field content changed) needs to set both.
    fn scoreboard_with_fp(
        rows: Vec<(&str, &str, usize, u128)>,
        fps: Vec<(&str, ParsedFingerprint)>,
    ) -> ParsedScoreboard {
        let mut sb = scoreboard(rows);
        for (tool, fp) in fps {
            sb.fingerprints.insert(tool.to_string(), fp);
        }
        sb
    }

    fn flag_fp(
        has_description: bool,
        description_hash: Option<u64>,
        choices_hash: Option<u64>,
        value_name: Option<&str>,
    ) -> ParsedFlagFingerprint {
        ParsedFlagFingerprint {
            has_description,
            description_hash,
            choices_hash,
            value_name: value_name.map(str::to_string),
        }
    }

    /// **The exact PR #14 shape, description half**: `--strip`'s
    /// description was deleted while every count-based column (flags,
    /// status, tiers, framework) stayed put. Proves the new field-level
    /// dimension catches it *and*, by asserting every pre-existing
    /// dimension is empty, documents precisely what the old
    /// count/status-only comparison had to work with — nothing. See this
    /// module's own doc comment and the CHANGELOG entry on the detector
    /// that first shipped this exact regression.
    #[test]
    fn field_diff_catches_a_description_only_change() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--strip".to_string(),
            flag_fp(true, Some(111), None, None),
        );
        let after_fp = ParsedFingerprint {
            flags: {
                let mut m = BTreeMap::new();
                m.insert(
                    "(root)::--strip".to_string(),
                    flag_fp(false, None, None, None),
                );
                m
            },
            subcommands: Default::default(),
        };

        let before = scoreboard_with_fp(vec![("pngfix", "ok", 3, 20)], vec![("pngfix", before_fp)]);
        let after = scoreboard_with_fp(vec![("pngfix", "ok", 3, 20)], vec![("pngfix", after_fp)]);

        let t = diff(&before, &after);

        // Every dimension the pre-existing comparison had: all quiet.
        assert!(t.status_transitions.is_empty());
        assert!(t.flag_gains.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.appeared.is_empty() && t.disappeared.is_empty());

        // The new field-level dimension: caught.
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pngfix");
        assert_eq!(
            t.field_diffs[0].description_changed,
            vec!["(root)::--strip"]
        );
        assert!(t.field_diffs[0].choices_changed.is_empty());
        assert!(
            !t.is_identical(),
            "a deleted description must not report as an identical run"
        );
    }

    /// **The exact PR #14 shape, choices half**: `--guesswork` had a
    /// fabricated choices list attached while flag counts and status stayed
    /// put — the other half of the same regression.
    #[test]
    fn field_diff_catches_a_choices_only_change() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        let mut after_fp = ParsedFingerprint::default();
        after_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(1), Some(999), None),
        );

        let before =
            scoreboard_with_fp(vec![("pod2man", "ok", 3, 20)], vec![("pod2man", before_fp)]);
        let after = scoreboard_with_fp(vec![("pod2man", "ok", 3, 20)], vec![("pod2man", after_fp)]);

        let t = diff(&before, &after);

        assert!(t.status_transitions.is_empty());
        assert!(t.flag_gains.is_empty());
        assert!(t.flag_losses.is_empty());
        assert!(t.appeared.is_empty() && t.disappeared.is_empty());

        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pod2man");
        assert_eq!(
            t.field_diffs[0].choices_changed,
            vec!["(root)::--guesswork"]
        );
        assert!(t.field_diffs[0].description_changed.is_empty());
        assert!(
            !t.is_identical(),
            "a fabricated choices list must not report as an identical run"
        );
    }

    /// A scoreboard from before this task (no `#fp` footer at all) must
    /// still load — `ParsedScoreboard::fingerprints` stays empty, and
    /// [`diff`] reports the affected tools as field-diff-unmeasured rather
    /// than silently claiming "no field-level changes" for data it never
    /// saw.
    #[test]
    fn legacy_scoreboards_with_no_fp_footer_report_unmeasured_not_identical_fields() {
        let before = scoreboard(vec![("git", "ok", 34, 120)]);
        let after = scoreboard(vec![("git", "ok", 34, 120)]);
        assert!(before.fingerprints.is_empty());
        let t = diff(&before, &after);
        assert!(t.field_diffs.is_empty());
        assert_eq!(t.field_diff_unmeasured, 1);
        // Every other dimension is genuinely unchanged here, so the overall
        // determination still reads identical — this test is only about
        // the unmeasured counter, not about forcing non-identical when
        // nothing else moved either.
        assert!(t.is_identical());
    }

    /// **The follow-up defect, direction 1**: a tool that had flags on the
    /// "before" side and loses every one of them must be reported as every
    /// flag removed, not as field-diff-unmeasured. `coverage::fingerprint_lines`
    /// used to skip emitting a `#fp` line for a row with no flags and no
    /// subcommands, so the "after" side (now empty) had no line at all and
    /// this fell into the unmeasured bucket instead — the field-level
    /// section going silent on exactly the case it exists to catch. See
    /// this test's sibling below (`without the fix...`) for the
    /// commit-then-attack proof this test was written to fail against.
    #[test]
    fn a_tool_that_loses_every_flag_is_reported_removed_not_unmeasured() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--strip".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        before_fp.flags.insert(
            "(root)::--guesswork".to_string(),
            flag_fp(true, Some(2), None, None),
        );

        let before = scoreboard_with_fp(vec![("pngfix", "ok", 2, 20)], vec![("pngfix", before_fp)]);
        // The "after" side carries *no* `#fp` entry for this tool at all —
        // exactly the shape `coverage::fingerprint_lines`'s pre-fix
        // skip-if-empty bug produced for a tool that lost every flag: the
        // row has no flags and no subcommands left, so the line was
        // dropped entirely rather than written as an empty one. Built with
        // plain `scoreboard` (no `#fp` population), not `scoreboard_with_fp`
        // with an explicit empty entry — the whole point of this test is
        // the *absent* entry, not a present-but-empty one.
        let after = scoreboard(vec![("pngfix", "ok", 0, 20)]);

        let t = diff(&before, &after);

        assert_eq!(
            t.field_diff_unmeasured, 0,
            "a missing entry on only one side must be read as empty, never as unmeasured"
        );
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tool, "pngfix");
        assert_eq!(
            t.field_diffs[0].flags_removed,
            vec!["(root)::--guesswork", "(root)::--strip"]
        );
        assert!(t.field_diffs[0].flags_added.is_empty());
        assert!(!t.is_identical());
    }

    /// **The follow-up defect, direction 2**: a flagless, subcommandless
    /// tool present (with an empty fingerprint) on both sides must be
    /// measured-with-no-changes — absent from `field_diffs` entirely — not
    /// counted as unmeasured. This is the common case on a real sweep
    /// (verbatim tools, zero-flag `ok` tools), and conflating "measured
    /// clean" with "not measured" was the other half of the same defect.
    #[test]
    fn a_flagless_tool_present_on_both_sides_is_measured_clean_not_unmeasured() {
        let before = scoreboard_with_fp(
            vec![("true", "ok", 0, 5)],
            vec![("true", ParsedFingerprint::default())],
        );
        let after = scoreboard_with_fp(
            vec![("true", "ok", 0, 5)],
            vec![("true", ParsedFingerprint::default())],
        );

        let t = diff(&before, &after);

        assert_eq!(
            t.field_diff_unmeasured, 0,
            "a present, empty fingerprint on both sides is measured, not unmeasured"
        );
        assert!(
            t.field_diffs.is_empty(),
            "no change to report for a flagless tool whose fingerprint didn't move"
        );
        assert!(t.is_identical());
    }

    /// Adds/removes/changes are reported as the actual flag identities and
    /// subcommand paths, never folded into a bare count — the requirement
    /// this whole diff exists to satisfy.
    #[test]
    fn field_diff_reports_flag_and_subcommand_adds_and_removes_by_name() {
        let mut before_fp = ParsedFingerprint::default();
        before_fp.flags.insert(
            "(root)::--old".to_string(),
            flag_fp(true, Some(1), None, None),
        );
        before_fp.subcommands.insert("old-sub".to_string());

        let mut after_fp = ParsedFingerprint::default();
        after_fp.flags.insert(
            "(root)::--new".to_string(),
            flag_fp(true, Some(2), None, None),
        );
        after_fp.subcommands.insert("new-sub".to_string());

        let before = scoreboard_with_fp(vec![("t", "ok", 1, 10)], vec![("t", before_fp)]);
        let after = scoreboard_with_fp(vec![("t", "ok", 1, 10)], vec![("t", after_fp)]);

        let t = diff(&before, &after);
        assert_eq!(t.field_diffs.len(), 1);
        let fd = &t.field_diffs[0];
        assert_eq!(fd.flags_added, vec!["(root)::--new"]);
        assert_eq!(fd.flags_removed, vec!["(root)::--old"]);
        assert_eq!(fd.subcommands_added, vec!["new-sub"]);
        assert_eq!(fd.subcommands_removed, vec!["old-sub"]);
    }

    /// A tier or framework change on an otherwise field-identical tool is
    /// still surfaced — the field-level diff isn't only about flags.
    #[test]
    fn tier_and_framework_changes_are_reported_per_tool() {
        let text = "tool                     tier(s)            framework                    nodes   flags   %flags_text     ms suspect   man  misattr  status\n";
        let before_row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}\n",
            "t", "help", "clap (v3/v4) (artifact)", 1, 1, "100%", 10, 0, "-", 0, "ok",
        );
        let after_row = format!(
            "{:<24} {:<18} {:<26} {:>7}{:>8}{:>13}{:>7}{:>8}{:>6}{:>9}  {}\n",
            "t", "help+native", "cobra (artifact)", 1, 1, "100%", 10, 0, "-", 0, "ok",
        );
        let before = parse_scoreboard(&format!(
            "{text}{before_row}# aggregate: pct_flags_with_text=100.00 no_tier_count=0 total=1\n"
        ));
        let after = parse_scoreboard(&format!(
            "{text}{after_row}# aggregate: pct_flags_with_text=100.00 no_tier_count=0 total=1\n"
        ));
        let t = diff(&before, &after);
        assert_eq!(t.field_diffs.len(), 1);
        assert_eq!(t.field_diffs[0].tier_changed, Some(("help", "help+native")));
        assert_eq!(
            t.field_diffs[0].framework_changed,
            Some(("clap (v3/v4) (artifact)", "cobra (artifact)"))
        );
        assert!(!t.is_identical());
    }

    /// End-to-end: [`crate::coverage::run_over`]'s own `ScoreFormat::Text`
    /// rendering carries a `#fp` footer that [`parse_scoreboard`] reads back
    /// — the round trip [`field_diff_catches_a_description_only_change`]
    /// and its sibling above don't exercise, since those build
    /// `ParsedFingerprint` by hand. `grep` is assumed present (every Linux
    /// dev/CI box has it) and its `--help` reliably documents at least one
    /// flag.
    #[test]
    fn fingerprint_footer_round_trips_through_render_and_parse() {
        let (table, _agg) = run_over(vec!["grep".to_string()], None, false, ScoreFormat::Text);
        let parsed = parse_scoreboard(&table);
        let fp = parsed
            .fingerprints
            .get("grep")
            .expect("grep fingerprint present in the #fp footer");
        assert!(
            !fp.flags.is_empty(),
            "grep --help should yield at least one flag"
        );
        assert!(
            fp.flags.values().any(|f| f.has_description),
            "at least one of grep's flags should carry a description"
        );
    }
}
