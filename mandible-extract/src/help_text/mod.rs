//! Tier B: engineered `--help` grammar parser (spec §7 Tier B).
//!
//! Attempted second, after Tier A. Reads stdout *and* stderr and does not
//! require exit 0 — `openssl --help` exits 0 on stderr only, `ip --help`
//! exits 255 on stderr only (spec M-8). When both streams carry bytes, the
//! one that looks like help wins, not the merely non-empty one; see
//! [`pick_stream`] and docs/shapes.md S-091.
//!
//! Before parsing, dispatches on framework (spec §7 Tier A′ + Tier B):
//! [`framework::identify_from_artifact`] first, then on a miss
//! [`framework::identify_from_help_text`] against the fetched text. The
//! result selects a [`profile::FrameworkProfile`] for
//! [`sections::parse_with_profile`]; `None` runs the generic engine with no
//! profile. See [`build_node`] for the staged degradation this produces.

pub mod confession;
mod grammar;
mod profile;
mod sections;

/// Re-exported for the coverage harness (spec §13.1, M-16): lets `xtask`
/// ask whether a root's captured `--help` was a rendered man page without a
/// second copy of the rule. See docs/shapes.md S-066.
pub use grammar::parse_bundled_shorts;
pub use sections::is_man_page_banner;

/// Re-exported for `xtask/src/alternation.rs`'s `brace-alternation-flag`
/// detector: the same rule this grammar uses for `{-i|--input}`/`[-c|-C]`.
/// A detector and its fix must agree character-for-character on the
/// defect, or the oracle drifts out of sync (200/656 fleet-wide
/// fabrications, spec §13.1c K2). See docs/shapes.md S-084.
pub use grammar::{parse_flag_alternation, FlagAlternation};

/// Re-exported for `xtask/src/misattribution.rs`: the same drift hazard as
/// [`pick_stream`] above — a second copy of "what is a flag" or "what is a
/// bare cell" would let the oracle stop measuring what this splitter
/// actually does. `fields_in_line` itself stays unshared; see the comment
/// above `sections::is_flag_shaped` in `sections/layout.rs`.
pub use sections::{
    cells, first_word, is_flag_shaped, is_value_placeholder_only, MIN_COLUMN_GAP_SPACES,
    MIN_COLUMN_RECURRENCE,
};

/// Re-exported for `xtask/src/existence.rs`: binutils `ar`'s modifier
/// suffix (`m[ab]` names command `m`) must be stripped the same way here
/// and by the oracle, or five real `ar` commands report as invented. See
/// docs/shapes.md S-059.
pub use sections::strip_optional_modifier_suffix;

/// Re-exported for `xtask/src/existence.rs`'s positional-operand check: the
/// oracle must agree on which lines are a synopsis before it can attest an
/// operand's position. Includes the fprintf-idiom and unlabelled-synopsis
/// entry points (PR #32/#33) — without them the oracle reported 94 real
/// operands as invented fleet-wide. See docs/shapes.md S-001.
///
/// The oracle deliberately does *not* borrow the block-continuation rule
/// (how far the synopsis runs past the marker) — its own wider read can
/// only attest more, never less, so that difference is safe.
pub use sections::{
    looks_like_unlabeled_synopsis_line, starts_with_name_prefixed_usage, starts_with_or_marker,
    starts_with_tool_name, starts_with_usage_prefix,
};

/// Re-exported for `xtask/src/existence.rs`: LVM's bare invocation line
/// (`vgextend VG PV ...`, no bracket notation) opens a usage block only on
/// the following line reading as a bracket flag row: the oracle must open
/// on the same evidence or the whole `vg*`/`lv*`/`pv*` family's operands
/// (29 tools, fleet-wide) go invisible to it. See docs/shapes.md S-005.
pub use grammar::looks_like_bracket_flag_row;

/// Re-exported for `xtask/src/existence.rs`'s `option_list_slot`: the same
/// option-list-placeholder vocabulary `sections::extract_positionals`
/// excludes by, layered on top of (never replacing) the oracle's own shape
/// rule — needed because `gh`'s `<command> <subcommand> [flags]` puts the
/// real placeholder last, which a shape-only rule reads backwards.
pub use sections::is_option_list_placeholder;

use crate::errors::ExtractError;
use crate::exec::{ExecOutput, InertArgv, LiveProbe, Probe};
use crate::framework::{self, Framework};
use crate::resolve::ResolvedTool;
use crate::tier::{ExtractionTier, NodeHints};
use mandible_core::{Authority, CommandNode, Confession, Provenance, Source, Text};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wall-clock cap for an `extract_node` probe (spec §6 rule 4).
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(10);

/// Hard cap on lines kept for level-3 verbatim degradation (spec §7 Tier B
/// step 3). Mirrors `sections::MAX_RECOVERED_ENTRIES`'s reasoning: a
/// degenerate free-running tool (the same shape `sections`'s own
/// `repeated_identical_banner_does_not_explode_into_duplicate_subcommands`
/// regression test guards against) must not turn "render the raw help
/// text verbatim" into an unbounded `Vec<Text>` the TUI then has to hold
/// and scroll.
const MAX_UNPARSED_LINES: usize = 4096;

/// Tier B: parses `<tool> [<path>...] --help` (falling back to `-h`) via a
/// layout-driven section parser and a small `winnow` flag-spec grammar.
pub struct HelpTextTier {
    /// The source of a `--help`/`-h` probe's output — [`LiveProbe`] in
    /// production ([`Self::default`]), or a [`crate::exec::Transcript`] to
    /// replay frozen bytes through this exact parsing pipeline with zero
    /// subprocesses (the corpus regression harness's seam).
    probe: Arc<dyn Probe>,
    /// Each tool's root `--help` text, keyed by its resolved binary path,
    /// remembered the first time [`Self::extract_node`] probes the root —
    /// see that method's doc comment for why: it is the baseline a later
    /// subcommand probe is checked against to catch the self-similar-
    /// fan-out hazard.
    ///
    /// A `Mutex` because `fill_node` is called concurrently from the
    /// background warm pool (`mandible/src/background.rs`) as well as the
    /// UI thread; a plain `RefCell` would not be `Sync`. Lives for the
    /// tier's lifetime, which is the whole session — one `Runner` (and so
    /// one set of tiers) is built once per `mandible` invocation and
    /// targets exactly one tool for that invocation's lifetime, refresh
    /// (`r`) included, so the cache never needs to be evicted.
    root_text: Mutex<HashMap<PathBuf, Arc<str>>>,
}

impl Default for HelpTextTier {
    fn default() -> Self {
        Self::new(Arc::new(LiveProbe))
    }
}

impl HelpTextTier {
    /// Build this tier against an explicit probe. Production code wants
    /// [`Self::default`]; tests and the future corpus runner construct a
    /// [`crate::exec::Transcript`] here instead.
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self {
            probe,
            root_text: Mutex::new(HashMap::new()),
        }
    }
}

impl ExtractionTier for HelpTextTier {
    fn name(&self) -> &'static str {
        "help_text"
    }

    fn authority(&self) -> Authority {
        Source::HelpText.authority()
    }

    fn detect(&self, tool: &ResolvedTool) -> bool {
        // `--help` is universal (spec §3); only precondition is a
        // resolved executable. A failure to produce useful output is
        // recorded per-node later (spec §5.3), not here.
        tool.path.is_some()
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
        hints: NodeHints,
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        let words: Vec<String> = path.iter().skip(1).cloned().collect();
        let (raw, _argv_display, confession) =
            probe_help_text_confession_aware(self.probe.as_ref(), tool_path, &words, hints)?;
        let node_name = path.last().cloned().unwrap_or_else(|| tool.name.clone());

        if words.is_empty() {
            // This probe is the root: cache its text as the baseline
            // later subcommand probes compare against. Always
            // overwritten so a refresh re-baselines.
            if let Ok(mut cache) = self.root_text.lock() {
                cache.insert(tool_path.clone(), Arc::from(raw.as_str()));
            }
        } else if let Ok(cache) = self.root_text.lock() {
            // Self-similar fan-out (spec M-19): a subcommand probe that
            // returns bytes identical to the cached root text degrades to
            // verbatim with no children, rather than re-reading the
            // root's own command table as this subcommand's own.
            // Keyed on output equality, never on tool name (same
            // discipline as M-16's man-page check). See docs/shapes.md
            // S-079.
            if let Some(root_raw) = cache.get(tool_path) {
                if root_raw.as_ref() == raw.as_str() {
                    let detected_framework = framework::identify_from_artifact(tool)
                        .or_else(|| framework::identify_from_help_text(&raw))
                        .map(|f| f.name().to_string());
                    let mut node = verbatim_node(&node_name, &raw, detected_framework);
                    node.confession = confession;
                    return Ok(node);
                }
            }
        }

        // Detection order per spec §7 Tier A′: free artifact scan first
        // (memoized per binary path), text signature only on a miss.
        let detected = framework::identify_from_artifact(tool)
            .or_else(|| framework::identify_from_help_text(&raw));
        let mut node = build_node(&node_name, &raw, detected, &tool.name);
        node.confession = confession;
        Ok(node)
    }

    fn is_incremental(&self) -> bool {
        true
    }
}

/// Run `<tool> [<path>...] --help`, falling back to `-h` on empty output or
/// a rendered man page (spec M-16 sub-case (a) — `git commit --help` execs
/// `man`), and report which flag actually produced the text. Preferred
/// stream is whichever looks like help, stdout on a tie (spec §7 Tier B,
/// M-8). See docs/shapes.md S-066.
///
/// Gated on provenance: no probe is sent at all for a non-empty `words`
/// unless [`NodeHints::heading_attested`] is true (spec §6 rule 0) — a
/// non-attested node returns [`ExtractError::Other`] instead. The man-page
/// fallback never fires for the root (six root-level binaries stay
/// verbatim, S-066); the `-h` response is validated with
/// [`looks_like_help_output`] (D1.3.1) before being trusted.
fn probe_help_text_reporting_flag(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    hints: NodeHints,
) -> Result<(String, &'static str), ExtractError> {
    // Spec §6 rule 0's closing paragraph, closed: a subcommand word is
    // probed at all only when it is structurally attested. The root
    // (`words.is_empty()`) is always attested by construction — it is the
    // name the user typed, never a word any parser invented — so this
    // never blocks the ordinary `<tool> --help` root probe, only a deeper
    // path whose last word did not come from a recognized heading.
    if !words.is_empty() && !hints.heading_attested {
        return Err(ExtractError::Other(format!(
            "refusing to probe `{} --help`: {:?} is not heading_attested, so it may be a \
             fabricated subcommand rather than a real one (spec §6 rule 0)",
            words.join(" "),
            words.last().expect("words is non-empty in this branch"),
        )));
    }

    let long = probe.run(
        tool_path,
        &InertArgv::HelpLongForPath {
            words: words.to_vec(),
        },
        EXTRACT_TIMEOUT,
    )?;

    if long.stdout.is_empty() && long.stderr.is_empty() {
        let short = probe.run(
            tool_path,
            &InertArgv::HelpShortForPath {
                words: words.to_vec(),
            },
            EXTRACT_TIMEOUT,
        )?;
        return Ok((pick_stream(&short.stdout, &short.stderr), "-h"));
    }

    let long_text = pick_stream(&long.stdout, &long.stderr);

    // `hints.heading_attested` is no longer checked here explicitly: the
    // function-level gate above already returned early for a non-attested
    // `words`, so reaching this line with `words` non-empty means it's
    // attested by construction.
    if !words.is_empty() && sections::is_man_page_banner(&long_text) {
        if let Ok(short) = probe.run(
            tool_path,
            &InertArgv::HelpShortForPath {
                words: words.to_vec(),
            },
            EXTRACT_TIMEOUT,
        ) {
            let short_text = pick_stream(&short.stdout, &short.stderr);
            if looks_like_help_output(&short_text) {
                return Ok((short_text, "-h"));
            }
        }
        // `-h` refused, errored, timed out, or didn't validate: keep the
        // man page text, so this node degrades to verbatim.
    }

    Ok((long_text, "--help"))
}

/// [`probe_help_text_reporting_flag`], further resolved against the
/// truncation-confession convention (spec §6 rule 2b, `help_text::
/// confession`, docs/shapes.md S-080): if the text confesses and names a
/// followable word, issues exactly one more probe (`InertArgv::HelpExpand`)
/// and returns that document instead. Never chained — the expanded
/// document's own confessions are never re-scanned.
///
/// `word` needs no separate attestation check: it comes from the tool's
/// own already-trusted output, not a grammar guess, so it is attested by
/// construction (spec §6 rule 0). Returns confession info regardless of
/// whether following it succeeded; `None` only when nothing was printed.
fn probe_help_text_confession_aware(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    hints: NodeHints,
) -> Result<(String, String, Option<Confession>), ExtractError> {
    let (text, flag) = probe_help_text_reporting_flag(probe, tool_path, words, hints)?;
    let directives = confession::detect_directives(&text);
    if directives.is_empty() {
        return Ok((text, flag.to_string(), None));
    }

    let Some(chosen) = confession::expandable(&directives) else {
        // Detected, but no shape this tier follows yet (spec's scope
        // discipline — curl's own `--help category` is exactly this: a
        // menu of further probes, not a single complete document).
        return Ok((
            text,
            flag.to_string(),
            Some(Confession::new(
                directives[0].word.clone(),
                directives[0].flag.to_string(),
                false,
            )),
        ));
    };

    let expand_argv = InertArgv::HelpExpand {
        words: words.to_vec(),
        word: chosen.word.clone(),
    };
    match probe.run(tool_path, &expand_argv, EXTRACT_TIMEOUT) {
        Ok(out) if !out.stdout.is_empty() || !out.stderr.is_empty() => Ok((
            pick_stream(&out.stdout, &out.stderr),
            format!("{} {}", chosen.flag, chosen.word),
            Some(Confession::new(
                chosen.word.clone(),
                chosen.flag.to_string(),
                true,
            )),
        )),
        // Failed, timed out, refused, or empty: keep the truncated text,
        // capped at `incomplete` rather than a confident `ok`.
        _ => Ok((
            text,
            flag.to_string(),
            Some(Confession::new(
                chosen.word.clone(),
                chosen.flag.to_string(),
                false,
            )),
        )),
    }
}

/// D1.3.1: is `text` plausibly real help output, not a tool acting on an
/// unrecognized argument or rendering a man page under a different guise?
/// Reuses [`sections::parse_with_profile`] (spec §7 Tier B step 3), which
/// transitively rejects a man page via [`sections::is_man_page_banner`].
fn looks_like_help_output(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // `tool_name: None`: this check only cares whether any structure came
    // back, not how many entries a usage block joins into.
    let parsed = sections::parse_with_profile(text, None, None);
    !parsed.flags.is_empty()
        || !parsed.subcommands.is_empty()
        || !parsed.modifiers.is_empty()
        || !parsed.env_vars.is_empty()
        || !parsed.usage.is_empty()
}

/// Fetch one node's raw `--help` output verbatim, sanitized one [`Text`]
/// per line and bounded like [`CommandNode::unparsed`]. Serves the TUI's
/// verbatim view (`t`), letting a reader check a confident-but-wrong parse
/// against the author's own bytes.
///
/// Deliberately re-probes rather than reading a retained copy (same
/// staleness argument as spec §11's cache removal); refusal for a
/// never-probe tool (spec §6 rule 0) propagates unchanged.
///
/// Thin [`LiveProbe`] wrapper over [`raw_help_with_probe`].
pub fn raw_help(
    tool: &ResolvedTool,
    path: &[String],
    hints: NodeHints,
) -> Result<(Vec<Text>, String), ExtractError> {
    raw_help_with_probe(&LiveProbe, tool, path, hints)
}

/// [`raw_help`], but against an explicit [`Probe`] — the seam a corpus
/// runner or test uses to check the verbatim view against frozen bytes.
///
/// `hints` must describe the same node the tree was built from: when spec
/// M-16 sub-case (a) fires, the tree's parse came from `-h`, not the man
/// page `--help` returned, so re-probing with different hints would show
/// a different document than the tree came from.
///
/// Confession-aware (spec §6 rule 2b): shows the expanded document when
/// one exists, and the returned argv string reflects that.
///
/// Raw display keeps both streams separate rather than merging via
/// [`pick_stream`] (parsing-path only) — see [`RawStreams`] and
/// [`format_streams`], docs/shapes.md S-091. When the attestation gate
/// (spec §6 rule 0) refuses a node, [`not_attested_fallback`] shows the
/// tool's own root `--help` instead of nothing.
pub fn raw_help_with_probe(
    probe: &dyn Probe,
    tool: &ResolvedTool,
    path: &[String],
    hints: NodeHints,
) -> Result<(Vec<Text>, String), ExtractError> {
    let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
    let words: Vec<String> = path.iter().skip(1).cloned().collect();

    match raw_probe_streams_confession_aware(probe, tool_path, &words, hints)? {
        RawProbeOutcome::Streams(streams, flag) => Ok((format_streams(&streams), flag)),
        RawProbeOutcome::NotAttested => Ok(not_attested_fallback(probe, tool_path, &words)),
    }
}

/// Choose which stream the parser reads (spec §7 Tier B). Parsing-path
/// only — the raw display path keeps both streams apart, see
/// [`RawStreams`]. "Non-empty" alone is the wrong test (`openssl cmp
/// --help`: diagnostics on stdout, real help on stderr): each stream is
/// judged by [`looks_like_help_output`] (D1.3.1), preferring the
/// help-shaped one and defaulting to stdout on any tie or when neither
/// looks like help. See docs/shapes.md S-091.
///
/// Public so `xtask`'s anti-fabrication oracles share this exact decision
/// rather than re-deriving their own copy, which drifted before (200 of
/// 656 fleet-wide fabrications, S-091).
pub fn pick_stream(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout_text = String::from_utf8_lossy(stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(stderr).into_owned();

    if stdout_text.is_empty() {
        return stderr_text;
    }
    if stderr_text.is_empty() {
        return stdout_text;
    }

    // Both non-empty: only stdout-not-help/stderr-help hands it to
    // stderr. Every other combination, including both help-shaped, keeps
    // stdout.
    if !looks_like_help_output(&stdout_text) && looks_like_help_output(&stderr_text) {
        stderr_text
    } else {
        stdout_text
    }
}

// Everything below is used only by [`raw_help`]/[`raw_help_with_probe`],
// never by [`HelpTextTier::extract_node`] or anything the parser depends
// on, and never shares a return type with code `extract_node` calls.

/// Both raw streams from a single probe result, kept apart rather than
/// merged through [`pick_stream`]. See docs/shapes.md S-091.
struct RawStreams {
    stdout: String,
    stderr: String,
}

impl RawStreams {
    fn from_output(out: &ExecOutput) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }

    /// Same selection as [`pick_stream`], used only to decide plausibility
    /// (man-page check, confession detection), never what gets shown —
    /// [`format_streams`] renders both streams.
    fn merged(&self) -> String {
        pick_stream(self.stdout.as_bytes(), self.stderr.as_bytes())
    }
}

/// The result of the display-only probe sequence below: either both
/// streams from whichever probe answered, or a refusal specific to the
/// attestation gate (defect 3) that [`raw_help_with_probe`] handles
/// separately from any other [`ExtractError`].
enum RawProbeOutcome {
    Streams(RawStreams, String),
    /// Node name did not come from a recognized heading (spec §6 rule 0):
    /// refused before any probe, same gate as the parsing path.
    NotAttested,
}

/// Mirrors [`probe_help_text_reporting_flag`]'s attestation gate,
/// long/short fallback, and man-page fallback (spec M-16), but returns
/// both streams and the attestation refusal as a value rather than
/// failing, so [`raw_help_with_probe`] can show a fallback instead.
fn raw_probe_streams(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    hints: NodeHints,
) -> Result<RawProbeOutcome, ExtractError> {
    if !words.is_empty() && !hints.heading_attested {
        return Ok(RawProbeOutcome::NotAttested);
    }

    let long = probe.run(
        tool_path,
        &InertArgv::HelpLongForPath {
            words: words.to_vec(),
        },
        EXTRACT_TIMEOUT,
    )?;
    let long_streams = RawStreams::from_output(&long);
    if long_streams.is_empty() {
        let short = probe.run(
            tool_path,
            &InertArgv::HelpShortForPath {
                words: words.to_vec(),
            },
            EXTRACT_TIMEOUT,
        )?;
        return Ok(RawProbeOutcome::Streams(
            RawStreams::from_output(&short),
            "-h".to_string(),
        ));
    }

    if !words.is_empty() && sections::is_man_page_banner(&long_streams.merged()) {
        if let Ok(short) = probe.run(
            tool_path,
            &InertArgv::HelpShortForPath {
                words: words.to_vec(),
            },
            EXTRACT_TIMEOUT,
        ) {
            let short_streams = RawStreams::from_output(&short);
            if looks_like_help_output(&short_streams.merged()) {
                return Ok(RawProbeOutcome::Streams(short_streams, "-h".to_string()));
            }
        }
        // `-h` was refused, errored, timed out, or didn't validate: keep
        // the man page text, exactly as the parsing path does.
    }

    Ok(RawProbeOutcome::Streams(long_streams, "--help".to_string()))
}

/// [`raw_probe_streams`], further resolved against the truncation-
/// confession convention (spec §6 rule 2b) — mirrors
/// [`probe_help_text_confession_aware`]'s single, never-chained follow-up
/// probe, but keeps both streams of whichever document (original or
/// expanded) ends up being shown.
fn raw_probe_streams_confession_aware(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    hints: NodeHints,
) -> Result<RawProbeOutcome, ExtractError> {
    let outcome = raw_probe_streams(probe, tool_path, words, hints)?;
    let RawProbeOutcome::Streams(streams, flag) = outcome else {
        return Ok(outcome);
    };

    let merged = streams.merged();
    let directives = confession::detect_directives(&merged);
    if directives.is_empty() {
        return Ok(RawProbeOutcome::Streams(streams, flag));
    }
    let Some(chosen) = confession::expandable(&directives) else {
        return Ok(RawProbeOutcome::Streams(streams, flag));
    };

    let expand_argv = InertArgv::HelpExpand {
        words: words.to_vec(),
        word: chosen.word.clone(),
    };
    match probe.run(tool_path, &expand_argv, EXTRACT_TIMEOUT) {
        Ok(out) => {
            let expanded = RawStreams::from_output(&out);
            if expanded.is_empty() {
                // Follow-up came back empty on both streams: keep the
                // original, truncated document rather than show nothing.
                Ok(RawProbeOutcome::Streams(streams, flag))
            } else {
                Ok(RawProbeOutcome::Streams(
                    expanded,
                    format!("{} {}", chosen.flag, chosen.word),
                ))
            }
        }
        // The follow-up probe failed, timed out, or was refused (rule 0):
        // keep the original text.
        Err(_) => Ok(RawProbeOutcome::Streams(streams, flag)),
    }
}

/// Render a fetched document's lines for the raw pane: one line per
/// [`Text::sanitize_preserving_layout`] entry, both streams shown and
/// labelled when both carry content, unlabelled when only one does.
/// Bounded to [`MAX_UNPARSED_LINES`]. See docs/shapes.md S-091.
fn format_streams(streams: &RawStreams) -> Vec<Text> {
    let stdout_present = !streams.stdout.is_empty();
    let stderr_present = !streams.stderr.is_empty();

    let mut lines: Vec<Text> = Vec::new();
    if stdout_present && stderr_present {
        lines.push(Text::sanitize_preserving_layout("── stdout ──"));
        lines.extend(streams.stdout.lines().map(Text::sanitize_preserving_layout));
        lines.push(Text::sanitize_preserving_layout(""));
        lines.push(Text::sanitize_preserving_layout("── stderr ──"));
        lines.extend(streams.stderr.lines().map(Text::sanitize_preserving_layout));
    } else if stdout_present {
        lines.extend(streams.stdout.lines().map(Text::sanitize_preserving_layout));
    } else {
        lines.extend(streams.stderr.lines().map(Text::sanitize_preserving_layout));
    }

    lines.truncate(MAX_UNPARSED_LINES);
    lines
}

/// The attestation gate (spec §6 rule 0) refused to probe this node
/// because its name did not come from a recognized `--help` heading.
/// Fixing the gate is out of scope here; this only makes hitting it
/// legible and shows the one document always safe to fetch instead: the
/// tool's own root `--help`, exempt from this gate by construction.
fn not_attested_fallback(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
) -> (Vec<Text>, String) {
    let attempted = words.join(" ");
    let mut lines = vec![Text::sanitize_preserving_layout(&format!(
        "{prefix}{attempted}\" as a real subcommand name: it came from a \
         source the probe-safety gate does not accept as evidence a word is safe to run (a \
         native/cobra artifact scan, or a headingless invocation table's layout evidence — \
         neither is a recognized --help heading), so it was never sent as an argument. This is \
         a known limitation of the gate, not something already worked around.",
        prefix = mandible_core::notice::UNVERIFIED_SUBCOMMAND_NOTICE_PREFIX
    ))];

    // `words` is empty here, so `heading_attested`'s value doesn't matter
    // to the gate; `true` reads honestly since the root is attested by
    // construction.
    if let Ok(RawProbeOutcome::Streams(root_streams, _)) = raw_probe_streams_confession_aware(
        probe,
        tool_path,
        &[],
        NodeHints {
            heading_attested: true,
        },
    ) {
        if !root_streams.is_empty() {
            lines.push(Text::sanitize_preserving_layout(""));
            lines.push(Text::sanitize_preserving_layout(
                mandible_core::notice::ROOT_HELP_FALLBACK_LABEL,
            ));
            lines.push(Text::sanitize_preserving_layout(""));
            lines.extend(format_streams(&root_streams));
        }
    }

    lines.truncate(MAX_UNPARSED_LINES);
    (
        lines,
        "(name not heading-attested — showing root --help as a fallback)".to_string(),
    )
}

/// Build a [`CommandNode`] from one probe's raw `--help` text, staging
/// spec §7 Tier B's three degradation levels: (1) framework identified —
/// dispatch through its [`profile::FrameworkProfile`] at normal
/// confidence; (2) unidentified — same engine, no profile, confidence
/// capped to 0.5; (3) structurally implausible (no flags, subcommands, or
/// usage line either way) — degrade to verbatim at confidence 0.0, never
/// fabricate.
///
/// `tool_name` (the root's name) drives the usage-block "starts a new
/// entry" discriminator, not `name` — synopses repeat the root invocation.
fn build_node(name: &str, raw: &str, framework: Option<Framework>, tool_name: &str) -> CommandNode {
    let fw_profile = framework.map(profile::profile);
    let parsed = sections::parse_with_profile(raw, fw_profile.as_ref(), Some(tool_name));
    let detected_framework = framework.map(|f| f.name().to_string());

    // A modifier table or env-var section counts as recovered structure
    // too, or a document whose only structure is one would be thrown away.
    let structurally_plausible = !parsed.flags.is_empty()
        || !parsed.subcommands.is_empty()
        || !parsed.modifiers.is_empty()
        || !parsed.env_vars.is_empty()
        || !parsed.usage.is_empty();

    if !structurally_plausible {
        return verbatim_node(name, raw, detected_framework);
    }

    // Level 2 (unidentified) capped low; level 1 (identified) not (spec
    // §7 Tier B).
    const UNIDENTIFIED_CONFIDENCE_CAP: f32 = 0.5;
    let confidence = if framework.is_some() {
        parsed.confidence
    } else {
        parsed.confidence.min(UNIDENTIFIED_CONFIDENCE_CAP)
    };
    let provenance = Provenance::with_confidence(Source::HelpText, confidence);

    let mut node = CommandNode::new(name, provenance);
    node.description = parsed.description.as_deref().map(Text::sanitize);
    // A synopsis is the tool's own layout, not ours (spec §4.1, §9.3):
    // `Text::sanitize`'s whitespace collapse would erase real column
    // alignment, so usage entries use the layout-preserving sanitizer.
    node.usage = parsed
        .usage
        .iter()
        .map(|s| Text::sanitize_preserving_layout(s))
        .collect();
    node.set_flags(parsed.flags);
    node.set_positionals(parsed.positionals);
    node.set_modifiers(parsed.modifiers);
    node.set_env_vars(parsed.env_vars);
    node.subcommands = parsed.subcommands;
    // A single probe of this node genuinely does discover its complete
    // direct-children *list* (spec §5.2: "the names of its direct
    // subcommands" — one level, not recursive) — whatever the
    // "Commands:"-shaped section names, or an empty list for a flags-only
    // leaf like `tar`. That list's accuracy is exactly what `confidence`
    // already communicates; `children_filled` itself is about *this level*
    // being known, not about the subcommands' own children (which stay
    // `children_filled: false` stubs until each is, in turn, expanded and
    // probed — that's the lazy per-node expansion spec §5.2 describes,
    // driven by the runner, not by recursing here).
    node.children_filled = true;
    node.detected_framework = detected_framework;
    // `heading_attested`, `invocation_attested` and `confession` keep
    // `CommandNode::new`'s defaults. This node is the probed node itself
    // (the root `--help` was run against, or a subcommand's own node once
    // *its* `--help` is probed in turn) — never a bare-word entry
    // recovered from a listing under some other node's heading, so there
    // is no heading or invocation table to attest to.
    // `emit_subcommands`/`process_word_grid` are what set those `true`,
    // for the stub entries `parsed.subcommands` already carries into this
    // node's `subcommands` list above. `confession` is set by the caller
    // (`HelpTextTier::extract_node`), the only place with the
    // confession-aware probe result this function doesn't see.
    node
}

/// Give up on structure entirely and carry `raw` verbatim in
/// [`CommandNode::unparsed`] at `confidence: 0.0` (spec §7 Tier B step 3,
/// never fabricate, degrade to verbatim). Shared by [`build_node`] (no
/// structure at all) and the self-similar-fan-out guard (spec M-19,
/// docs/shapes.md S-079), so `children_filled: true` with empty
/// `subcommands` consistently means "probed, found nothing here".
///
/// Lines use [`Text::sanitize_preserving_layout`], not `Text::sanitize`
/// (which would collapse `ar`'s padded `m[ab]` column, see docs/shapes.md
/// S-059) — this is the author's own document, shown because mandible
/// couldn't read it.
fn verbatim_node(name: &str, raw: &str, detected_framework: Option<String>) -> CommandNode {
    let provenance = Provenance::with_confidence(Source::HelpText, 0.0);
    let mut node = CommandNode::new(name, provenance);
    node.unparsed = raw
        .lines()
        .take(MAX_UNPARSED_LINES)
        .map(Text::sanitize_preserving_layout)
        .collect();
    node.detected_framework = detected_framework;
    // One probe completed and discovered this level's (empty) children.
    node.children_filled = true;
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_tool;

    /// Every test path is a real, structurally-known source, never an
    /// invented word, so `heading_attested: true` is honest throughout.
    const ATTESTED: NodeHints = NodeHints {
        heading_attested: true,
    };

    fn fixture(name: &str) -> String {
        // tar/git captures live once as corpus regression fixtures (see
        // corpus/README.md), not duplicated here.
        let path = match name {
            "tar_help.stdout" => {
                format!("{}/../corpus/tar/1.35/help.txt", env!("CARGO_MANIFEST_DIR"))
            }
            "git_help.stdout" => format!(
                "{}/../corpus/git/2.43.0/help.txt",
                env!("CARGO_MANIFEST_DIR")
            ),
            _ => format!(
                "{}/tests/fixtures/help_text/{name}",
                env!("CARGO_MANIFEST_DIR")
            ),
        };
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
    }

    /// `raw_help` is a second entry point into the exec boundary; spec §6
    /// rule 0 must hold on it too. Root shape is permitted; this pins the
    /// boundary between root and a deeper unattested path.
    #[test]
    fn raw_help_allows_the_root_but_refuses_a_deeper_path_for_a_help_only_tool() {
        // A shim named `pkill` exercises the refusal by file name, not
        // the real binary's behavior.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkill");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'Usage: pkill [options] <pattern>'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut tool = resolve_tool("pkill");
        tool.path = Some(path);

        let root = raw_help(
            &tool,
            &["pkill".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect("`pkill --help` is the one permitted shape and must be shown");
        assert!(
            root.0.iter().any(|line| line.as_str().contains("Usage:")),
            "{root:?}"
        );

        let err = raw_help(
            &tool,
            &["pkill".to_string(), "something".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect_err("a positional path must still be refused");
        assert!(
            matches!(
                err,
                ExtractError::Exec(crate::exec::ExecError::RefusedUnsafeTool { .. })
            ),
            "expected a refusal, got {err:?}"
        );
    }

    /// Defect 3: a subcommand whose name is not `heading_attested` (spec §6
    /// rule 0's closing paragraph — e.g. a name a native/cobra artifact
    /// scan produced rather than a real `--help` heading) must not surface
    /// the old terse `ExtractError::Other` string as the pane's *entire*
    /// answer. The gate itself is untouched (still refused, still no probe
    /// sent for the unattested node — that assertion is what
    /// `raw_help_allows_the_root_but_refuses_a_deeper_path_for_a_help_only_tool`
    /// above already pins for the *other* refusal kind); what changed is
    /// that this specific refusal now resolves to `Ok` with an explanation
    /// plus the tool's own root text, not `Err`.
    #[test]
    fn raw_help_explains_a_not_attested_refusal_and_falls_back_to_root_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shimtool");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'Usage: shimtool [COMMAND]'\necho ''\necho 'Commands:'\necho '  clean   tidy up'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut tool = resolve_tool("shimtool");
        tool.path = Some(path);

        // "ghost" stands in for a name that never appeared under any
        // recognized heading in this shim's own `--help` text.
        let (lines, flag) = raw_help(
            &tool,
            &["shimtool".to_string(), "ghost".to_string()],
            NodeHints {
                heading_attested: false,
            },
        )
        .expect("a not-attested refusal must resolve to Ok with an explanation, not Err");

        let joined: String = lines
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("ghost"), "{joined}");
        assert!(
            joined.to_lowercase().contains("known limitation"),
            "{joined}"
        );
        // The root --help must actually appear, not just be described.
        assert!(joined.contains("Usage: shimtool [COMMAND]"), "{joined}");
        assert!(flag.contains("not heading-attested"), "{flag:?}");
    }

    #[test]
    fn build_node_from_tar_fixture_has_flags_and_confidence() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, None, "tar");
        assert_eq!(node.name, "tar");
        assert!(node.flags().next().is_some());
        assert!(node.provenance.confidence.unwrap() > 0.0);
        // tar has no children, which is known-complete (spec §5.2), not
        // unknown.
        assert!(node.children_filled);
    }

    // --- batch 6 part 4: staged degradation (spec §7 Tier B) ---

    /// Level 2 vs. level 1 (spec §7 Tier B): the same text, same
    /// underlying confidence, but unidentified must be capped and
    /// identified must not.
    #[test]
    fn unidentified_confidence_is_capped_but_identified_is_not() {
        let raw = fixture("tar_help.stdout");
        let unidentified = build_node("tar", &raw, None, "tar");
        let identified = build_node("tar", &raw, Some(Framework::GnuArgp), "tar");
        assert!(unidentified.provenance.confidence.unwrap() <= 0.5);
        assert!(
            identified.provenance.confidence.unwrap() > 0.5,
            "identified confidence was {:?}",
            identified.provenance.confidence
        );
    }

    /// A synopsis keeps the spacing the tool printed (spec §4.1). LVM
    /// pads a long-only option's column; `Text::sanitize` would flatten
    /// it to one space.
    #[test]
    fn usage_keeps_the_tools_own_column_spacing() {
        let raw = "Usage:  prog  [ -A|--autobackup y|n ] [    --reportformat basic|json ]\n\n\
                   Options:\n  -A, --autobackup y|n   back up\n";
        let node = build_node("prog", raw, None, "prog");
        assert_eq!(
            node.usage,
            vec![mandible_core::Text::sanitize_preserving_layout(
                "Usage:  prog  [ -A|--autobackup y|n ] [    --reportformat basic|json ]"
            )],
            "usage: {:?}",
            node.usage
        );
    }

    /// A `\t` in a synopsis is still expanded to 8-column stops, not
    /// passed through — ratatui gives a bare tab zero display width and
    /// would misalign the columns being preserved (pastebinit).
    #[test]
    fn usage_expands_tabs_rather_than_passing_them_through() {
        let raw = "Usage:\tprog [OPTION...]\n\nOptions:\n  -a, --all   everything\n";
        let node = build_node("prog", raw, None, "prog");
        assert_eq!(node.usage.len(), 1, "usage: {:?}", node.usage);
        assert!(!node.usage[0].as_str().contains('\t'));
        assert_eq!(node.usage[0].as_str(), "Usage:  prog [OPTION...]");
    }

    /// Level 3 (spec §7 Tier B step 3): no flags/subcommands/usage line
    /// degrades to verbatim — confidence 0.0, non-empty `unparsed`, no
    /// fabricated structure.
    #[test]
    fn structurally_implausible_output_degrades_to_verbatim() {
        let raw = "This tool prints only a friendly banner and nothing else.\nGoodbye.\n";
        let node = build_node("mystery", raw, None, "mystery");
        assert_eq!(node.provenance.confidence, Some(0.0));
        assert!(!node.unparsed.is_empty());
        assert_eq!(node.unparsed.len(), 2);
        assert_eq!(node.unparsed[0].as_str(), raw.lines().next().unwrap());
        assert!(node.flags().next().is_none());
        assert!(node.subcommands.is_empty());
        assert!(node.usage.is_empty());
        assert!(node.description.is_none());
    }

    /// Verbatim fallback reproduces the author's layout (spec §4.1):
    /// leading indentation and column padding survive into `unparsed`
    /// exactly as printed. See docs/shapes.md S-059.
    #[test]
    fn verbatim_fallback_preserves_the_authors_column_alignment() {
        let raw = " supported targets:\n  elf64-littleaarch64     elf64-bigaarch64\n\
                   \x20 elf32-littlearm         elf32-bigarm\n";
        let node = build_node("ar", raw, None, "ar");
        assert!(!node.unparsed.is_empty(), "expected the verbatim fallback");
        let rendered: Vec<&str> = node.unparsed.iter().map(Text::as_str).collect();
        assert_eq!(rendered, raw.lines().collect::<Vec<_>>(), "{rendered:?}");
        assert!(rendered[0].starts_with(' '), "leading indent: {rendered:?}");
        assert!(
            rendered[2].contains("elf32-littlearm         elf32"),
            "columns: {rendered:?}"
        );
    }

    /// A structurally *plausible* parse (at least one flag/subcommand/
    /// usage line) must never carry `unparsed`, identified or not — the
    /// two are mutually exclusive by construction.
    #[test]
    fn structurally_plausible_output_never_carries_unparsed() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, None, "tar");
        assert!(node.unparsed.is_empty());
    }

    #[test]
    fn detected_framework_is_recorded_on_the_node() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, Some(Framework::GnuArgp), "tar");
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::GnuArgp.name())
        );
        let unidentified = build_node("tar", &raw, None, "tar");
        assert_eq!(unidentified.detected_framework, None);
    }

    // The five priority frameworks (spec §7 Tier B): a fixture-based
    // parse-level assertion here, plus a real-argv test further down
    // where a real binary is present (AGENTS.md §3.1: a mocked probe hid
    // a dead cobra tier before).

    /// GnuArgp, M-12's largest fingerprint: M-10's phantom-subcommand bug,
    /// made structural via the profile mechanism. See docs/shapes.md
    /// S-013.
    #[test]
    fn gnu_argp_profile_forces_zero_subcommands_on_real_tar_output() {
        let raw = fixture("tar_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::GnuArgp)), None);
        assert!(parsed.subcommands.is_empty());
        let create = parsed.flags.iter().find(|f| f.long() == Some("create"));
        assert!(create.is_some(), "expected --create to still be recovered");
    }

    /// ClapV3V4, M-12's second-largest fingerprint. Captured from real
    /// `cargo --help`, not ripgrep (hand-rolled formatter, M-13).
    #[test]
    fn clap_v3v4_profile_recovers_cargo_commands_and_flags() {
        let raw = fixture("cargo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::ClapV3V4)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        // Only names with no inline `, alias` — clap's alias rendering is
        // currently dropped as not name-shaped, a known honest gap.
        for want in ["clean", "new", "init", "add", "remove", "install"] {
            assert!(names.contains(&want), "{names:?}");
        }
        let long_flags: Vec<&str> = parsed.flags.iter().filter_map(|f| f.long()).collect();
        for want in ["version", "locked", "offline", "frozen", "help"] {
            assert!(long_flags.contains(&want), "{long_flags:?}");
        }
    }

    /// Argparse: the dedicated subparsers scan, from a real runnable
    /// Python script's `--help` output. See docs/shapes.md S-073.
    #[test]
    fn argparse_profile_recovers_subparsers_not_plain_positionals() {
        let raw = fixture("argparse_demo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Argparse)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["init", "build", "run"], "{names:?}");
        let long_flags: Vec<&str> = parsed.flags.iter().filter_map(|f| f.long()).collect();
        assert!(long_flags.contains(&"verbose"));
        assert!(long_flags.contains(&"config"));
    }

    /// Same recovery when the tool styles the heading itself
    /// (`add_subparsers(title=...)`). See docs/shapes.md S-073.
    #[test]
    fn argparse_profile_recovers_subparsers_under_a_styled_heading() {
        let raw = fixture("argparse_titled_demo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Argparse)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["init", "build", "run"], "{names:?}");
    }

    /// A flush-left command table (dnf) is recognized. See docs/shapes.md
    /// S-050.
    #[test]
    fn a_command_table_at_its_headings_own_indent_is_recovered() {
        let raw = "usage: dnf [options] COMMAND\n\nList of Main Commands:\n\nalias                     List or create command aliases\nautoremove                remove all unneeded packages\ncheck                     check for problems in the packagedb\n\nGeneral DNF options:\n  -v, --verbose         verbose operation\n";
        let parsed = sections::parse(raw);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alias", "autoremove", "check"], "{names:?}");
    }

    /// A flush-left table of settings, not commands, must not become
    /// commands even though a row like `init-command` mentions the word.
    /// See docs/shapes.md S-092.
    #[test]
    fn a_flush_left_settings_table_is_not_promoted_to_subcommands() {
        let raw = "Usage: mysqlslap [OPTIONS]\n\nVariables (--variable-name=value)\nand boolean options {FALSE|TRUE}      Value (after reading options)\ncommit                                0\ninit-command                          (No default value)\niterations                            1\nno-drop                               FALSE\nport                                  3306\n";
        let parsed = sections::parse(raw);
        assert!(
            parsed.subcommands.is_empty(),
            "settings became subcommands: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// Plain positionals under "positional arguments:" (no
    /// add_subparsers()) must never fabricate subcommands. See
    /// docs/shapes.md S-073.
    #[test]
    fn argparse_profile_does_not_fabricate_subcommands_from_plain_positionals() {
        let raw = "usage: tool [-h] path\n\npositional arguments:\n  path        the file to process\n\noptions:\n  -h, --help  show this help message and exit\n";
        let parsed =
            sections::parse_with_profile(raw, Some(&profile::profile(Framework::Argparse)), None);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// Busybox's dedicated comma-separated scan, from this machine's real
    /// `busybox --help`. See docs/shapes.md S-093.
    #[test]
    fn busybox_profile_recovers_comma_separated_applets() {
        let raw = fixture("busybox_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Busybox)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        // Applets from the first, middle, and last wrapped line of the
        // real list, so this can't pass by only recovering one line's
        // worth of entries.
        for want in ["acpid", "adjtimex", "grep", "mount", "wget", "zcat"] {
            assert!(names.contains(&want), "expected {want:?} among {names:?}");
        }
        // `[` and `[[` fail the name-shape test (spec §7 Tier B rule 3),
        // dropped not fabricated.
        assert!(names.len() > 250, "got {} applets: {names:?}", names.len());
    }

    /// The comma-separated scan must never fire outside a recognized
    /// command heading, even with the profile enabled. See
    /// docs/shapes.md S-093.
    #[test]
    fn busybox_profile_does_not_fabricate_commands_outside_a_command_heading() {
        let raw = "Usage: widget [OPTIONS]\n\nSupported formats:\n\tjson, yaml, toml, xml\n";
        let parsed =
            sections::parse_with_profile(raw, Some(&profile::profile(Framework::Busybox)), None);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// Cobra, captured from real `gh --help` output. Exercises the
    /// trailing-colon name normalization (`"auth:        Authenticate..."`)
    /// and, just as importantly, a *negative* case: `HELP TOPICS` never
    /// says "command" or "commands" anywhere and must not become
    /// subcommands, even though it's laid out identically to the real
    /// command groups around it.
    #[test]
    fn cobra_profile_recovers_gh_commands_and_excludes_help_topics() {
        let raw = fixture("gh_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Cobra)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in ["auth", "pr", "co", "alias", "cache"] {
            assert!(names.contains(&want), "{names:?}");
        }
        for excluded in ["actions", "environment", "reference"] {
            assert!(
                !names.contains(&excluded),
                "HELP TOPICS entry {excluded:?} must not become a subcommand: {names:?}"
            );
        }
    }

    /// Click, captured from a real, runnable Python script's actual
    /// `--help` output (`tests/fixtures/help_text/click_demo.py`).
    #[test]
    fn click_profile_recovers_commands_and_flags() {
        let raw = fixture("click_demo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Click)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"build"), "{names:?}");
        assert!(names.contains(&"init"), "{names:?}");
        let long_flags: Vec<&str> = parsed.flags.iter().filter_map(|f| f.long()).collect();
        assert!(long_flags.contains(&"verbose"));
        assert!(long_flags.contains(&"help"));
    }

    /// Artifact scanning (Tier A′ step 1) finds `Framework::Click` from
    /// the script's own `import click` bytes, no probe needed.
    #[test]
    fn click_is_identified_from_the_script_artifact_itself() {
        let path = format!(
            "{}/tests/fixtures/help_text/click_demo.py",
            env!("CARGO_MANIFEST_DIR")
        );
        let detected = framework::identify_from_artifact(&ResolvedTool {
            name: "click_demo.py".to_string(),
            path: Some(std::path::PathBuf::from(path)),
            version: None,
        });
        assert_eq!(detected, Some(Framework::Click));
    }

    #[test]
    fn detect_true_for_resolved_tool() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("sh");
        assert!(tier.detect(&tool));
    }

    #[test]
    fn detect_false_for_unresolved_tool() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("definitely-not-a-real-tool-xyz");
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn extract_node_against_real_tar_binary() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("tar");
        if tool.path.is_none() {
            return; // environment without tar; nothing to verify
        }
        let node = tier
            .extract_node(&tool, &["tar".to_string()], ATTESTED)
            .unwrap();
        assert!(node.flags().next().is_some());

        // GNU-argp assertions below apply only to GNU tar; macOS ships
        // bsdtar, a different program with no argp fingerprint.
        if node.detected_framework.as_deref() != Some(Framework::GnuArgp.name()) {
            return;
        }

        assert!(
            node.subcommands.is_empty(),
            "GnuArgp must never carry subcommands: {:?}",
            node.subcommands.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// Real argv, real binary (AGENTS.md §3.1): `zoxide`, identified via
    /// artifact scanning. Not `cargo` (resolves to a rustup proxy that
    /// fails under spec §6 rule 8's scratch `HOME`) and not `mandible`
    /// itself (statically links this crate, so its own cobra marker scan
    /// finds itself).
    #[test]
    fn extract_node_against_real_zoxide_binary_identifies_clap() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("zoxide");
        if tool.path.is_none() {
            return;
        }
        let node = tier
            .extract_node(&tool, &["zoxide".to_string()], ATTESTED)
            .unwrap();
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::ClapV3V4.name())
        );
        assert!(node.flags().next().is_some());
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in ["add", "edit", "import", "init", "query", "remove"] {
            assert!(names.contains(&want), "{names:?}");
        }
    }

    /// Real argv, real binary: `gh` embeds `spf13/cobra` (M-13),
    /// identified via artifact scanning alone.
    #[test]
    fn extract_node_against_real_gh_binary_identifies_cobra() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("gh");
        if tool.path.is_none() {
            return;
        }
        let node = tier
            .extract_node(&tool, &["gh".to_string()], ATTESTED)
            .unwrap();
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::Cobra.name())
        );
        assert!(!node.subcommands.is_empty());
    }

    /// Real argv, real (script) binary: `argparse_demo.py` is a genuine
    /// executable Python script, run exactly the way `extract_node` runs
    /// any other tool.
    #[test]
    fn extract_node_against_real_argparse_script() {
        let tier = HelpTextTier::default();
        let path = format!(
            "{}/tests/fixtures/help_text/argparse_demo.py",
            env!("CARGO_MANIFEST_DIR")
        );
        let tool = resolve_tool(&path);
        assert!(tool.path.is_some(), "fixture script must be executable");
        let node = tier
            .extract_node(&tool, &["argparse_demo.py".to_string()], ATTESTED)
            .unwrap();
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::Argparse.name())
        );
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["init", "build", "run"], "{names:?}");
    }

    /// Real argv, real (script) binary: `click_demo.py`.
    #[test]
    fn extract_node_against_real_click_script() {
        let tier = HelpTextTier::default();
        let path = format!(
            "{}/tests/fixtures/help_text/click_demo.py",
            env!("CARGO_MANIFEST_DIR")
        );
        let tool = resolve_tool(&path);
        assert!(tool.path.is_some(), "fixture script must be executable");
        let node = tier
            .extract_node(&tool, &["click_demo.py".to_string()], ATTESTED)
            .unwrap();

        // Proves the wiring only (AGENTS.md §3.1); grammar details are
        // covered deterministically by the fixture-based test instead.
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::Click.name()),
            "click not detected — is click installed for this python3?"
        );
    }

    #[test]
    fn extract_node_against_stderr_only_ip_binary() {
        // Regression for spec [M-8]: `ip --help` writes only to stderr
        // and exits 255. A tier that required exit 0/stdout would
        // silently produce nothing here.
        let tier = HelpTextTier::default();
        let tool = resolve_tool("ip");
        if tool.path.is_none() {
            return;
        }
        let node = tier
            .extract_node(&tool, &["ip".to_string()], ATTESTED)
            .unwrap();
        assert!(
            !node.usage.is_empty() || node.flags().next().is_some() || !node.subcommands.is_empty(),
            "expected ip's stderr-only help to produce *something*"
        );
    }

    #[test]
    fn extract_node_against_stderr_only_openssl_binary() {
        let tier = HelpTextTier::default();
        let tool = resolve_tool("openssl");
        if tool.path.is_none() {
            return;
        }
        let node = tier
            .extract_node(&tool, &["openssl".to_string()], ATTESTED)
            .unwrap();
        assert!(
            !node.usage.is_empty() || node.flags().next().is_some() || !node.subcommands.is_empty(),
            "expected openssl's stderr-only help to produce *something*"
        );
    }

    // The replay seam: drives `HelpTextTier` through real `extract_node`
    // against a `Transcript` keyed on the tier's own probe argv, zero
    // subprocesses (AGENTS.md §3.1, spec §13.3).

    fn exec_output(stdout: &str) -> crate::exec::ExecOutput {
        crate::exec::ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    /// Real argv, replayed: the root probe renders to exactly `["--help"]`;
    /// a transcript keyed on that argv must let `extract_node` produce the
    /// same result as `build_node` directly.
    #[test]
    fn extract_node_replays_from_a_transcript_keyed_on_the_real_argv() {
        let raw = fixture("tar_help.stdout");
        let transcript =
            crate::exec::Transcript::new([(vec!["--help".to_string()], exec_output(&raw))]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "tar".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/tar")),
            version: None,
        };
        let node = tier
            .extract_node(&tool, &["tar".to_string()], ATTESTED)
            .expect("the transcript covers the exact argv this tier sends");
        assert_eq!(node.name, "tar");
        assert!(node.flags().next().is_some());
    }

    /// Self-similar fan-out (spec M-19, docs/shapes.md S-079): a
    /// subcommand probe byte-identical to the root's own text must not be
    /// read as sharing the root's children. Two calls through the same
    /// tier instance, root first, so the cache populates as production
    /// does (spec §5.2 step 4).
    #[test]
    fn a_subcommand_probe_identical_to_the_root_does_not_fan_out() {
        let raw = "usage: widget [options] COMMAND\n\nCommands:\n\n  preset-all   Do the preset-all thing\n  get-default  Get the default\n\nOptions:\n  -h, --help   Show this help\n";
        let transcript = crate::exec::Transcript::new([
            (vec!["--help".to_string()], exec_output(raw)),
            (
                vec!["preset-all".to_string(), "--help".to_string()],
                exec_output(raw),
            ),
        ]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "widget".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/widget")),
            version: None,
        };

        let root = tier
            .extract_node(&tool, &["widget".to_string()], ATTESTED)
            .expect("the transcript covers the root's argv");
        let root_names: Vec<&str> = root.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            root_names,
            vec!["preset-all", "get-default"],
            "sanity: the root itself must read the two real subcommands"
        );

        let child = tier
            .extract_node(
                &tool,
                &["widget".to_string(), "preset-all".to_string()],
                ATTESTED,
            )
            .expect("the transcript covers this subcommand's argv too");
        assert!(
            child.subcommands.is_empty(),
            "a probe identical to the root must not report the root's own \
             children as this subcommand's children — that is the cascade \
             that starved the UI thread: {:?}",
            child
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
        assert!(
            child.children_filled,
            "this level is still known-complete (empty), just not \
             re-probed forever"
        );
        assert!(
            !child.unparsed.is_empty(),
            "the raw text must still be available to the verbatim ('t') \
             view even though nothing was promoted to structure"
        );
    }

    /// A probe sharing some text with the root but not byte-identical
    /// must parse normally — the guard is keyed on exact equality only.
    #[test]
    fn a_subcommand_probe_merely_similar_to_the_root_is_parsed_normally() {
        let root_raw = "usage: widget [options] COMMAND\n\nCommands:\n\n  preset-all   Do the preset-all thing\n\nOptions:\n  -h, --help   Show this help\n";
        let child_raw = "usage: widget preset-all [options]\n\nOptions:\n  -h, --help   Show this help\n  -f, --force  Force it\n";
        let transcript = crate::exec::Transcript::new([
            (vec!["--help".to_string()], exec_output(root_raw)),
            (
                vec!["preset-all".to_string(), "--help".to_string()],
                exec_output(child_raw),
            ),
        ]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "widget".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/widget")),
            version: None,
        };

        tier.extract_node(&tool, &["widget".to_string()], ATTESTED)
            .expect("the transcript covers the root's argv");
        let child = tier
            .extract_node(
                &tool,
                &["widget".to_string(), "preset-all".to_string()],
                ATTESTED,
            )
            .expect("the transcript covers this subcommand's argv too");
        assert!(
            child.subcommands.is_empty(),
            "preset-all genuinely has none of its own"
        );
        let flag_names: Vec<&str> = child.flags().filter_map(|f| f.long()).collect();
        assert!(
            flag_names.contains(&"force"),
            "a genuinely distinct subcommand's own flags must still parse: {flag_names:?}"
        );
    }

    /// The self-similar-fan-out guard (M-19), the `llvm-ar` instance:
    /// every operation letter under `OPERATIONS:` answers `--help` with
    /// the root's own text, byte-identical (LLVM's `cl::opt` processes
    /// `--help` before acting on anything ahead of it). See docs/shapes.md
    /// S-079.
    #[test]
    fn an_operation_letter_probe_identical_to_the_root_does_not_fan_out() {
        let raw = "OVERVIEW: LLVM Archiver\n\nUSAGE: llvm-ar [options] [-]<operation>[modifiers] [relpos] [count] <archive> [files]\n\nOPERATIONS:\n  d - delete [files] from the archive\n  m - move [files] in the archive\n";
        let transcript = crate::exec::Transcript::new([
            (vec!["--help".to_string()], exec_output(raw)),
            (
                vec!["d".to_string(), "--help".to_string()],
                exec_output(raw),
            ),
        ]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "llvm-ar".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/llvm-ar")),
            version: None,
        };

        let root = tier
            .extract_node(&tool, &["llvm-ar".to_string()], ATTESTED)
            .expect("the transcript covers the root's argv");
        let root_names: Vec<&str> = root.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            root_names,
            vec!["d", "m"],
            "sanity: the root itself must read the two real operations"
        );

        let child = tier
            .extract_node(&tool, &["llvm-ar".to_string(), "d".to_string()], ATTESTED)
            .expect("the transcript covers this operation's argv too");
        assert!(
            child.subcommands.is_empty(),
            "an operation probe identical to the root must not report the \
             root's own OPERATIONS: table as this operation's children: {:?}",
            child
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
        assert!(
            child.children_filled,
            "this level is still known-complete (empty), just not \
             re-probed forever"
        );
    }

    /// A transcript missing the argv this tier actually sends must miss
    /// loudly, naming the argv it was asked for.
    #[test]
    fn extract_node_against_a_transcript_missing_the_argv_is_a_named_miss() {
        // Keyed on a different argv than the tier will ever send at the
        // root, to simulate the bug class this seam catches.
        let transcript = crate::exec::Transcript::new([(
            vec!["commit".to_string(), "--help".to_string()],
            exec_output("usage: tar [options]\n"),
        )]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "tar".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/tar")),
            version: None,
        };
        let err = tier
            .extract_node(&tool, &["tar".to_string()], ATTESTED)
            .expect_err("the transcript has no recording for the root's `--help` argv");
        match err {
            ExtractError::Exec(crate::exec::ExecError::TranscriptMiss { argv, .. }) => {
                assert_eq!(
                    argv,
                    vec!["--help".to_string()],
                    "must name the requested argv"
                );
            }
            other => panic!("expected a named TranscriptMiss, got {other:?}"),
        }
    }

    /// The real specimen behind [`pick_stream`]'s fix, replayed verbatim:
    /// diagnostics on stdout, real help on stderr. Driven through
    /// `extract_node`, not `build_node`, to also prove argv construction
    /// (AGENTS.md §3.1). See docs/shapes.md S-091.
    #[test]
    fn openssl_cmp_shape_prefers_the_help_shaped_stderr_over_diagnostic_stdout() {
        let stdout_diagnostics = "cmp_main:../apps/cmp.c:2832:CMP info: using section(s) 'cmp' of OpenSSL configuration file '/usr/lib/ssl/openssl.cnf'\ncmp_main:../apps/cmp.c:2840:CMP info: no [cmp] section found in config file '/usr/lib/ssl/openssl.cnf'; will thus use just [default] and unnamed section if present\n";
        let stderr_help = "Usage: cmp [options]\nValid options are:\n -help                  Display this summary\n -config val            Configuration file to use. \"\" = none. Default from env variable OPENSSL_CONF\n -section val           Section(s) in config file to get options from. \"\" = 'default'. Default 'cmp'\n -verbosity nonneg      Log level; 3=ERR, 4=WARN, 6=INFO, 7=DEBUG, 8=TRACE. Default 6 = INFO\n -cmd val               CMP request to send: ir/cr/kur/p10cr/rr/genm\n -infotype val          InfoType name for requesting specific info in genm, e.g. 'signKeyPairTypes'\n";

        let output = crate::exec::ExecOutput {
            stdout: stdout_diagnostics.as_bytes().to_vec(),
            stderr: stderr_help.as_bytes().to_vec(),
            exit_code: Some(0),
            timed_out: false,
        };
        let transcript =
            crate::exec::Transcript::new([(vec!["cmp".to_string(), "--help".to_string()], output)]);
        let tier = HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "openssl".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/openssl")),
            version: None,
        };

        let node = tier
            .extract_node(&tool, &["openssl".to_string(), "cmp".to_string()], ATTESTED)
            .expect("the transcript covers the exact argv this tier sends");

        let flags: Vec<_> = node.flags().collect();
        assert!(
            !node.usage.is_empty() || !flags.is_empty(),
            "expected the parser to read stderr's help-shaped document \
             instead of stdout's two diagnostic lines; got usage={:?} flags={:?}",
            node.usage,
            flags
        );
        // openssl's single-dash long options parse as short flag + value
        // name, a grammar detail unrelated to this fix; this pins that
        // stderr's real flags parsed at all (pre-fix bug produced zero).
        assert!(
            flags.len() >= 5,
            "expected the ~6 real flags from stderr's document, not the \
             empty parse the two stdout diagnostic lines would produce: {:?}",
            flags
        );
        assert!(
            flags.iter().any(|f| f
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("CMP request to send"))),
            "expected the `-cmd` flag's real description from stderr's \
             document to have parsed: {:?}",
            flags
        );
    }
}
