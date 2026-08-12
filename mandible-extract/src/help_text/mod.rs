//! Tier B: engineered `--help` grammar parser (spec §7 Tier B).
//!
//! Attempted second (after the free, zero-spawn Tier A) because it costs
//! 1-2 spawns per node and is the only source that exists for every tool
//! everywhere. Reads stdout *and* stderr and does not require exit 0 —
//! `openssl --help` writes only to stderr with exit 0, `ip --help` writes
//! only to stderr with exit 255 (spec [M-8]) — preferring stdout when both
//! are non-empty. Recursion happens per-node, lazily, through the runner
//! (spec §5.2/batch 3's lazy runner); this tier itself only ever parses
//! the one node it's asked for.
//!
//! **Dispatch on framework (spec §7 Tier A′ + Tier B, batch 6 part 4).**
//! Before parsing, this tier tries to identify which framework generated
//! the text, in the order spec §7 Tier A′ prescribes, *without ever
//! double-probing*: [`framework::identify_from_artifact`] first (free —
//! it scans the already-resolved binary's own bytes, no process spawn),
//! and only on a miss, [`framework::identify_from_help_text`] against the
//! `--help` text this tier already fetched for its own parsing. The
//! result selects a [`profile::FrameworkProfile`] that
//! [`sections::parse_with_profile`] consults; `None` (unidentified) runs
//! the identical generic engine this parser always used, just called with
//! no profile. See [`build_node`] for the three-level staged degradation
//! this produces (spec §7 Tier B).

pub mod confession;
mod grammar;
mod profile;
mod sections;

/// Re-exported for the coverage harness (spec §13.1, [M-16]): the pure
/// man-page-banner check this tier already uses at [`build_node`]'s
/// verbatim-degradation step (`sections::parse_with_profile`), exposed so
/// `xtask` can ask "was this root's captured `--help` output a rendered
/// man page?" without a second copy of the rule and without re-probing the
/// tool — see [`sections::is_man_page_banner`]'s own doc comment.
pub use sections::is_man_page_banner;

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
        // `--help` is universal in the sense that it's the only thing
        // every tool has everywhere (spec §3); the only real
        // precondition is that the tool resolved to an executable at
        // all. We don't spend a probe here confirming `--help` actually
        // produces useful output — that's what extract_node finds out,
        // and a failure there is recorded per-node without invalidating
        // the tier (spec §5.3).
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
            // This probe *is* the root: remember its text as the baseline
            // the check below compares every subcommand probe against.
            // Always overwritten, never only-if-absent, so a refresh (`r`)
            // re-baselines rather than comparing against a stale probe.
            if let Ok(mut cache) = self.root_text.lock() {
                cache.insert(tool_path.clone(), Arc::from(raw.as_str()));
            }
        } else if let Ok(cache) = self.root_text.lock() {
            // The self-similar-fan-out hazard, found live against
            // `mandible systemctl` (spec [M-19]): GNU getopt permutes
            // `--help` to the front of argv regardless of what precedes
            // it, and `systemctl` never validates a verb before dispatch,
            // so `systemctl <anything...> --help` prints the tool's *own
            // root help*, byte-identical, no matter how many words or
            // what they are — confirmed at the shell directly, five words
            // deep. The generic grammar below has no way to know that;
            // it reads the root's own "Commands:" section a second time
            // and reports it as *this* subcommand's children, which are
            // the same subcommand names all over again. The background
            // warmer (`mandible/src/background.rs`) cascades every
            // discovered child unconditionally with no cycle detection —
            // that is deliberately this crate's job, not the extractor's,
            // per this module's own doc comment ("recursion happens
            // per-node, lazily, through the runner") — so left unchecked
            // this is not merely wrong data, it is unbounded (18^depth)
            // recursive re-probing capped only by `MAX_WARMED_NODES`
            // (4096), and reaching that cap means thousands of concurrent
            // `systemctl` spawns contending for the CPU with the UI
            // thread's own 100ms poll loop — measured starving it for
            // 45+ seconds on a 4-core machine, which is what a user
            // reported as the TUI simply freezing.
            //
            // Keyed on an observable property of the *output* — identical
            // bytes to the root — never on the tool's name, the same
            // discipline [M-16] used for man-page detection: this is not
            // "if tool == systemctl", it is "if probing this subcommand
            // produced nothing beyond a second copy of the root", which
            // generalizes to any multi-call binary or getopt permutation
            // that behaves the same way. Degrading to verbatim (spec §7
            // Tier B step 3) is the existing, honest response to "this
            // probe told us nothing new": the raw bytes are still shown
            // to the reader via `t` exactly as returned, but nothing is
            // reported as this node's *own* structure, so the cascade
            // that reads `subcommands` finds none and stops rather than
            // re-probing the same 18 names one level deeper forever.
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

        // Detection order per spec §7 Tier A′, never double-probing: the
        // free artifact scan first (memoized per binary path — see
        // `framework::identify_from_artifact`'s doc comment — so probing
        // every node of a deep tree over the same binary costs nothing
        // extra here), and only on a miss, the help-text signature against
        // the text this call already fetched above for its own parsing.
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

/// Run `<tool> <words...> --help`, falling back to `-h` if that produced no
/// output at all on either stream (the long-standing fallback; unconditional
/// on `words`'/`hints`' provenance, unchanged by [M-16]'s addition below),
/// and return whichever stream had content (stdout preferred when both are
/// non-empty — spec §7 Tier B, measured against real tools in [M-8]).
///
/// **The positional probe itself is gated on provenance.** `words`
/// non-empty means this is a subcommand path, not the root, and its last
/// element is a word some earlier extraction produced — either the tool's
/// own attested command table, or, if a grammar bug ever mis-reads a
/// layout, a fabrication. Spec §6 rule 0's closing paragraph names this as
/// the general form of the never-probe list's hazard: a fabricated word
/// becoming argv for *any* tool, not just the thirteen named there. So
/// **no probe of any kind is sent** — not `--help`, not the `-h` fallback
/// below it, not [M-16] sub-case (a)'s `-h` fallback further down — unless
/// [`NodeHints::heading_attested`] is true, exactly the same bit the
/// sub-case (a) fallback already checked before this function grew a gate
/// of its own. See [`probe_help_text_reporting_flag`] for where the check
/// actually lives; a non-attested node returns [`ExtractError::Other`]
/// instead of spawning anything, so the runner records a per-node,
/// per-tier failure (spec §5.3) rather than the tree silently gaining an
/// empty-but-successful node.
///
/// **[M-16] sub-case (a):** when `--help` instead produced real output that
/// is a *rendered man page* — `git commit --help` execs `man` and renders
/// `GIT-COMMIT(1)` rather than printing help — also fall back to `-h`,
/// which for 21 of git's 22 subcommands prints an ordinary option table the
/// generic grammar already handles. Two guards, both required:
///
/// - **`words` must be non-empty.** This is the boundary between sub-case
///   (a) (implemented here) and sub-case (b) (a tool's own root `--help`
///   being man-shaped — six named binaries, `byobu`/`byobu-screen`/
///   `byobu-tmux`/`git-receive-pack`/`git-upload-archive`/`git-upload-pack`,
///   **not** implemented here). [M-16] measured the six but the maintainer's
///   ruling is that an unmeasured argv broadening is a no; a man-shaped
///   *root* must keep degrading to verbatim exactly as it does today, so
///   this fallback never fires for the root regardless of anything else.
/// - **`hints.heading_attested` must be true.** No longer checked
///   explicitly at this fallback's own call site: the function-level gate
///   above already refused a non-attested `words` before any probe ran, so
///   by the time execution reaches this fallback, `words` non-empty
///   implies attested. This is the same bit [M-16] originally added this
///   fallback's own check for, before the wider gate existed.
///
/// The `-h` response is validated with [`looks_like_help_output`] before
/// being trusted (D1.3.1): a tool that *acts* on an argument it doesn't
/// recognise instead of printing help may still print *something*, and a
/// never-probe tool's `-h` is refused outright by [`crate::exec::run_inert`]
/// (spec §6 rule 0) regardless of any of the above — that refusal is
/// swallowed here exactly like a validation failure, not propagated as a
/// fatal error, so the node still degrades to verbatim.
/// Run `<tool> [<path>...] --help`, falling back to `-h`, also reporting
/// **which flag actually produced the text** — `"--help"` or `"-h"`.
///
/// The verbatim view (`t`) needs this to label itself honestly. Since
/// [M-16] sub-case (a) a subcommand's text may come from either probe, and
/// a pane that hardcodes "the tool's own `--help` output" while showing
/// `-h` output states something untrue about where the bytes came from —
/// in the one view whose entire job is showing the reader exactly what we
/// were given. It shipped that way briefly.
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
        // `-h` was refused (a never-probe tool, spec §6 rule 0), errored,
        // timed out, or didn't validate as real help output: keep the man
        // page text, so this node degrades to verbatim exactly as it did
        // before this fallback existed.
    }

    Ok((long_text, "--help"))
}

/// [`probe_help_text_reporting_flag`], further resolved against the tool's
/// own truncation-confession convention (spec §6 rule 2b, `help_text::
/// confession`): if that probe's text confesses to being an incomplete
/// document and names a word this tier knows how to follow
/// (`confession::expandable`), this issues exactly **one** additional
/// probe with the advertised argv (`InertArgv::HelpExpand`) and returns
/// *that* document instead.
///
/// **Never chained.** The expanded document's own text is returned as-is
/// — this function does not call itself, and does not run
/// `confession::detect_directives` a second time on the expanded text — so
/// a confession printed inside an expanded document (verification's "no
/// chaining" case) is structurally never looked at, not merely
/// discouraged by convention.
///
/// **Interaction with spec §6 rule 0 and the attestation gate.** The
/// `InertArgv::HelpExpand` probe below goes through the same
/// `probe.run` → `run_inert` chokepoint as every other shape, so a
/// never-probe tool (`pkill`, `shutdown`, ...) refuses it exactly as it
/// refuses every non-`--help` shape — `run_inert`'s own check
/// (`argv.args() != ["--help"]`) rejects `HelpExpand`'s
/// `[..words, "--help", word]` unconditionally, with no special case
/// needed here. Attestation (`hints.heading_attested`) is not
/// re-checked for `word` itself: the word is not a subcommand name a
/// grammar guessed at, it is a word the tool's *own already-probed and
/// already-trusted* output printed, so it is attested by construction —
/// the same way the root path itself is exempt from the gate (spec §6
/// rule 0's closing paragraph; see the amendment in spec.md §6 rule 2b).
///
/// Returns the confession info alongside the text/flag regardless of
/// whether following it succeeded, so a caller (`extract_node`,
/// `raw_help_with_probe`) can record it on the node — `None` only when no
/// confession was printed at all.
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
            Some(Confession {
                word: directives[0].word.clone(),
                flag: directives[0].flag.to_string(),
                followed: false,
            }),
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
            Some(Confession {
                word: chosen.word.clone(),
                flag: chosen.flag.to_string(),
                followed: true,
            }),
        )),
        // The follow-up probe failed, timed out, was refused (rule 0), or
        // came back empty on both streams: keep the original, truncated
        // text — the status ladder caps this at `incomplete` rather than
        // reporting a confident `ok` on the document we already know is
        // incomplete.
        _ => Ok((
            text,
            flag.to_string(),
            Some(Confession {
                word: chosen.word.clone(),
                flag: chosen.flag.to_string(),
                followed: false,
            }),
        )),
    }
}

/// D1.3.1: is `text` plausibly real help output, rather than a tool having
/// *acted* on an argument it didn't recognise (and printed nothing
/// help-shaped as a side effect) or having rendered a man page under a
/// different guise?
///
/// Reuses [`sections::parse_with_profile`] — the exact structural-
/// plausibility test [`build_node`] already applies to every probe result
/// (spec §7 Tier B step 3: "flags, subcommands, or a usage line") — rather
/// than inventing a second heuristic. This transitively rejects a man page
/// too: `parse_with_profile` checks [`sections::is_man_page_banner`] first
/// and returns an empty parse for one, so a `-h` response that renders a
/// *different* man page fails this check the same way an empty or
/// unstructured response does.
fn looks_like_help_output(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // `tool_name: None` — this check only cares whether *any* structure
    // came back, and the usage-block scanner's tool-name discriminator
    // only affects how many entries a usage block joins into, never
    // whether `usage` ends up empty (see `parse_with_profile`'s doc
    // comment). No tool name is in scope at this call site anyway.
    let parsed = sections::parse_with_profile(text, None, None);
    !parsed.flags.is_empty() || !parsed.subcommands.is_empty() || !parsed.usage.is_empty()
}

/// Fetch one node's raw `--help` output verbatim, sanitized one [`Text`]
/// per line and bounded the same way [`CommandNode::unparsed`] is.
///
/// This exists for the TUI's verbatim view (`t`), which answers a question
/// no confidence score can: *is this parse actually right?* A tool whose
/// grammar produced a confident, well-formed, wrong tree looks identical to
/// one that produced a correct tree. Showing the author's own bytes next to
/// our reading of them lets the person at the keyboard settle it in a
/// second, and every degradation this crate already performs — verbatim
/// nodes, low-confidence caps — is an admission that our reading is
/// sometimes worth checking.
///
/// Deliberately **re-probes** rather than reading a retained copy. Keeping
/// every node's raw text alive costs megabytes on a warmed tree (`git`
/// alone reaches the 4096-node warm ceiling) to serve one node at a time,
/// and a retained copy also ages: it would show what the tool said at
/// startup, not what it says now, which is the same staleness argument that
/// removed the cache in spec §11. One probe is ~10-30ms against a binary
/// the warmer has almost certainly already faulted in.
///
/// Refusal for a never-probe tool (spec §6 rule 0) propagates from
/// [`crate::exec::run_inert`] unchanged: `kill --help` is no safer because
/// a human asked for it interactively.
///
/// A thin [`LiveProbe`] wrapper over [`raw_help_with_probe`] — every real
/// caller (the TUI's verbatim view) wants the live tool, so this keeps
/// their call site unchanged while still funneling through the same
/// probe-taking function a replay caller would use.
pub fn raw_help(
    tool: &ResolvedTool,
    path: &[String],
    hints: NodeHints,
) -> Result<(Vec<Text>, String), ExtractError> {
    raw_help_with_probe(&LiveProbe, tool, path, hints)
}

/// [`raw_help`], but against an explicit [`Probe`] rather than always the
/// live one — the seam a future corpus runner or test can use to check the
/// TUI's verbatim view against frozen bytes with zero subprocesses.
///
/// **`hints` must describe the same node the tree was built from.** This
/// function exists so a reader can check our parse against the author's own
/// bytes; that only works if it fetches *the document we actually parsed*.
/// When [M-16] sub-case (a) fires, the parse came from `-h` and not from
/// the man page `--help` returned — so a verbatim view that re-probed with
/// different hints would show a different document than the tree came from,
/// and silently answer the wrong question. It shipped that way briefly:
/// `heading_attested` was hardcoded `false` here on the reasoning that this
/// call site could not verify attestation, which is true of the *function*
/// but not of its caller — the TUI resolves the `CommandNode` at `path`
/// before requesting raw help, and that node carries the bit.
///
/// **Confession-aware** (spec §6 rule 2b), same as the extraction path:
/// when the fetched text confesses to being incomplete and names a
/// followable word, this shows the *expanded* document instead, and the
/// returned argv string reflects that (`"--help all"`, not `"--help"`) —
/// spec's own example of what the pane must keep honest: "the verbatim
/// pane already names its own argv... make sure that keeps working and
/// shows the expanded form when expansion happened."
///
/// **Rewritten for the raw pane's two remaining defects** (the doc comment
/// above predates both fixes):
///
/// - **Defect 1 (alignment).** Lines are now built with [`Text::
///   sanitize_preserving_layout`], not [`Text::sanitize`] — see that
///   function's own doc comment for exactly what it neutralizes (terminal
///   control sequences) versus what it preserves (everything else,
///   including indentation and internal column gaps). `Text::sanitize`
///   remains exactly what it always was, unused on this path now, still
///   used everywhere parsing happens (`build_node`, `verbatim_node`).
/// - **Defect 2 (one stream only).** [`pick_stream`] merges stdout/stderr
///   into whichever one the *parser* should read — correct for parsing,
///   wrong for a pane whose job is showing what the parser had to choose
///   between (`openssl cmp --help`: diagnostics on stderr, usage on
///   stdout). This function therefore probes through
///   [`raw_probe_streams_confession_aware`], a **separate, display-only**
///   probing pipeline that keeps both streams apart instead of merging
///   them early, and hands them to [`format_streams`] to render — labelled
///   when both carry content, unlabelled (today's shape) when only one
///   does. `pick_stream` itself, and the parsing path's own probing
///   functions (`probe_help_text_reporting_flag`,
///   `probe_help_text_confession_aware`) that call it, are untouched: this
///   is deliberate duplication of a handful of `probe.run` calls rather
///   than threading a new return shape through code `extract_node` depends
///   on, so there is no way this change can alter what any tier parses.
/// - **Defect 3 (illegible refusal).** When the attestation gate (spec §6
///   rule 0's closing paragraph) refuses this node specifically because its
///   name isn't `heading_attested`, [`not_attested_fallback`] explains that
///   plainly and shows the tool's own root `--help` instead of nothing —
///   see that function's doc comment. The gate itself is untouched; only
///   what the pane says about hitting it changed.
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

/// Choose which stream the *parser* reads (spec §7 Tier B).
///
/// **Parsing-path only.** The raw display path (`raw_help*`, this module's
/// display-only probing functions below) keeps both streams apart instead
/// — see [`RawStreams`] and [`raw_help_with_probe`]'s doc comment.
///
/// "Non-empty" alone is the wrong test: `openssl cmp --help` prints two
/// `CMP info: ...` diagnostic lines to **stdout** and its entire ~60-line
/// help (`Usage: cmp [options]`, `Valid options are:`, the flag list) to
/// **stderr**. The old rule ("stdout if non-empty, else stderr") handed the
/// parser two banner lines and threw the actual document away — reachable
/// by roughly 150 openssl subcommands in the same shape. The truth table,
/// using [`looks_like_help_output`] (D1.3.1) to judge each stream on its
/// own:
///
/// | stdout empty? | stderr empty? | stdout help-shaped? | stderr help-shaped? | picks |
/// |---|---|---|---|---|
/// | yes | yes | – | – | stdout (empty; nothing to pick) |
/// | yes | no  | – | – | stderr — only stream available |
/// | no  | yes | – | – | stdout — only stream available |
/// | no  | no  | yes | any | stdout — already correct, unchanged |
/// | no  | no  | no  | yes | stderr — **the bug fix**: the merely non-empty stream loses to the help-shaped one |
/// | no  | no  | no  | no  | stdout — neither is recognizably help; keep today's default rather than invent a new failure mode |
///
/// Both-help-shaped is folded into the first "yes" row above: when stdout
/// is help-shaped it always wins, deliberately, per the same row — stdout
/// is the conventional stream for a well-behaved tool's `--help`, so ties
/// break toward it.
fn pick_stream(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout_text = String::from_utf8_lossy(stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(stderr).into_owned();

    if stdout_text.is_empty() {
        return stderr_text;
    }
    if stderr_text.is_empty() {
        return stdout_text;
    }

    // Both non-empty: only the case where stdout looks *nothing* like help
    // and stderr does hands the document to stderr. Every other
    // combination — including both being help-shaped — keeps stdout.
    if !looks_like_help_output(&stdout_text) && looks_like_help_output(&stderr_text) {
        stderr_text
    } else {
        stdout_text
    }
}

// --- raw pane display path: defects 1-3 ---
//
// Everything below is used only by [`raw_help`]/[`raw_help_with_probe`],
// never by [`HelpTextTier::extract_node`] or anything the parser depends
// on. It exists apart from `probe_help_text_reporting_flag`/
// `probe_help_text_confession_aware` above by design (see
// `raw_help_with_probe`'s doc comment): the parser-freeze constraint this
// crate operates under is easiest to keep literal by never letting the
// display path share a return type, and therefore a call site, with code
// `extract_node` calls.

/// Both raw streams from a single probe result, kept apart rather than
/// merged through [`pick_stream`] — the raw pane's whole reason to exist is
/// showing which bytes came from where. Real specimen: `openssl cmp --help`
/// prints two `CMP info: ...` diagnostic lines on stderr *and* its usage on
/// stdout; `pick_stream` (correctly, for parsing) picks one, and a reader
/// checking the parser's work needs to see both.
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

    /// The same selection [`pick_stream`] performs, used internally by this
    /// display path only to decide *plausibility* — the man-page-banner
    /// check, [`looks_like_help_output`], and truncation-confession
    /// detection all need one string to test, the same way the parsing
    /// path does — never to decide what gets *shown*. [`format_streams`]
    /// is what renders both streams for the reader.
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
    /// This node's name did not come from a recognized heading (spec §6
    /// rule 0's closing paragraph): refused before any probe was sent, the
    /// same gate [`probe_help_text_reporting_flag`] applies on the parsing
    /// path, checked here independently rather than shared (see this
    /// section's own doc comment on why).
    NotAttested,
}

/// The display path's base probe sequence — mirrors
/// [`probe_help_text_reporting_flag`]'s attestation gate, long-probe/
/// short-fallback, and [M-16] sub-case (a)'s man-page fallback exactly,
/// but returns both streams instead of one merged string, and returns the
/// attestation refusal as a value ([`RawProbeOutcome::NotAttested`]) rather
/// than an [`ExtractError`], so [`raw_help_with_probe`] can show a
/// fallback instead of just failing (defect 3).
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

/// Render a fetched document's lines for the raw pane: one [`Text::
/// sanitize_preserving_layout`] entry per line (defect 1), both streams
/// shown and clearly labelled when both carry content (defect 2) —
/// unlabelled, today's shape, when only one does, so the overwhelming
/// majority of tools (a single stream) see no cosmetic change at all.
/// Bounded to [`MAX_UNPARSED_LINES`], same cap [`CommandNode::unparsed`]
/// uses, so a pathological tool can't blow up the pane.
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

/// Defect 3: the attestation gate (spec §6 rule 0's closing paragraph)
/// refused to probe this node because its name did not come from a
/// recognized `--help` heading — typically a name a native/cobra artifact
/// scan produced instead. **Fixing that gate is out of scope here**
/// (spec's own instruction: it changes what the coverage audit measures,
/// and is deliberately scheduled after it) — this only makes hitting the
/// gate legible instead of a bare, jargon-heavy error string, and shows
/// the one document that's always safe to fetch in its place: the tool's
/// own root `--help`, unconditionally exempt from this exact gate by
/// construction (`words.is_empty()` in [`raw_probe_streams`]) — never a
/// second, potentially-fabricated word.
fn not_attested_fallback(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
) -> (Vec<Text>, String) {
    let attempted = words.join(" ");
    let mut lines = vec![Text::sanitize_preserving_layout(&format!(
        "mandible could not verify \"{attempted}\" as a real subcommand name: it came from a \
         source the probe-safety gate does not accept (a native/cobra artifact scan, not a \
         --help heading), so it was never sent as an argument. This is a known limitation of \
         the gate, not something already worked around."
    ))];

    // `words` is `&[]` here, so `heading_attested`'s value is irrelevant —
    // `raw_probe_streams`'s gate only ever checks it when `words` is
    // non-empty. `true` is chosen only to read honestly at the call site
    // (the root probe genuinely is attested, by construction).
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
                "Showing the tool's own root --help instead, labelled below:",
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
/// spec §7 Tier B's three degradation levels:
///
/// 1. **Framework identified** (`framework.is_some()`): dispatch through
///    that framework's [`profile::FrameworkProfile`], at normal (uncapped)
///    confidence.
/// 2. **Unidentified**: the same shared engine with no profile — pure
///    layout heuristics — with confidence capped to `0.5` so the
///    scoreboard and TUI both show it as a guess rather than a trusted
///    parse.
/// 3. **Structurally implausible** (no flags, no subcommands, *and* no
///    usage line recovered, regardless of level 1 vs. 2 above): give up on
///    structure entirely and carry the raw text verbatim in
///    [`CommandNode::unparsed`] instead, at `confidence: 0.0` — spec §7
///    Tier B step 3, "never fabricate, degrade to verbatim".
///
/// `name` is this node's own name (`"rebase"` for `git rebase`); `tool_name`
/// is the root tool's name (`"git"` for both `git --help` and `git rebase
/// --help`) — usage synopses conventionally repeat the *root* invocation,
/// not the immediate node's name, so `tool_name` (not `name`) is what
/// `sections::parse_with_profile`'s usage-block scanner is given as the
/// "starts a new entry" discriminator.
fn build_node(name: &str, raw: &str, framework: Option<Framework>, tool_name: &str) -> CommandNode {
    let fw_profile = framework.map(profile::profile);
    let parsed = sections::parse_with_profile(raw, fw_profile.as_ref(), Some(tool_name));
    let detected_framework = framework.map(|f| f.name().to_string());

    let structurally_plausible =
        !parsed.flags.is_empty() || !parsed.subcommands.is_empty() || !parsed.usage.is_empty();

    if !structurally_plausible {
        return verbatim_node(name, raw, detected_framework);
    }

    // Level 2 (unidentified) is capped low; level 1 (identified) is not
    // (spec §7 Tier B: "Framework identified → its grammar, high
    // confidence" vs. "Unidentified → ... marked low-confidence").
    const UNIDENTIFIED_CONFIDENCE_CAP: f32 = 0.5;
    let confidence = if framework.is_some() {
        parsed.confidence
    } else {
        parsed.confidence.min(UNIDENTIFIED_CONFIDENCE_CAP)
    };
    let provenance = Provenance::with_confidence(Source::HelpText, confidence);

    CommandNode {
        name: name.to_string(),
        aliases: Vec::new(),
        summary: None,
        description: parsed.description.as_deref().map(Text::sanitize),
        usage: parsed.usage.iter().map(|s| Text::sanitize(s)).collect(),
        flags: parsed.flags,
        positionals: parsed.positionals,
        subcommands: parsed.subcommands,
        examples: Vec::new(),
        hidden: false,
        deprecated: None,
        // A single probe of this node genuinely does discover its
        // complete direct-children *list* (spec §5.2: "the names of its
        // direct subcommands" — one level, not recursive) — whatever the
        // "Commands:"-shaped section names, or an empty list for a
        // flags-only leaf like `tar`. That list's accuracy is exactly
        // what `confidence` already communicates; `children_filled`
        // itself is about *this level* being known, not about the
        // subcommands' own children (which stay `children_filled: false`
        // stubs until each is, in turn, expanded and probed — that's the
        // lazy per-node expansion spec §5.2 describes, driven by the
        // runner, not by recursing here).
        children_filled: true,
        group: None,
        unparsed: Vec::new(),
        detected_framework,
        provenance,
        // This node is the probed node itself (the root `--help` was run
        // against, or a subcommand's own node once *its* `--help` is
        // probed in turn) — never a bare-word entry recovered from a
        // listing under some other node's heading, so there is no heading
        // to attest to. `emit_subcommands`/`process_word_grid` are what
        // set this `true`, for the stub entries `parsed.subcommands`
        // already carries into this node's `subcommands` list above.
        heading_attested: false,
        // Set by the caller (`HelpTextTier::extract_node`), which is the
        // only place with the confession-aware probe result this function
        // doesn't see — see `build_node`'s own callers.
        confession: None,
    }
}

/// Give up on structure entirely and carry `raw` verbatim in
/// [`CommandNode::unparsed`] instead, at `confidence: 0.0` (spec §7 Tier B
/// step 3, "never fabricate, degrade to verbatim").
///
/// Two callers reach this, both "this probe has nothing to report as this
/// node's own structure": [`build_node`], when the generic parse found no
/// flags, no subcommands and no usage line at all; and [`HelpTextTier::
/// extract_node`]'s self-similar-fan-out guard, when the parse found
/// plenty of *structure* but it demonstrably belongs to the tool's root,
/// not to this subcommand (see that call site for [M-19]'s `systemctl`
/// finding). Sharing this function rather than duplicating the empty-node
/// construction keeps both places honest about what `children_filled:
/// true` with an empty `subcommands` means: a probe genuinely completed
/// and genuinely found nothing to attribute to this level, which is what
/// stops the background warmer (`mandible/src/background.rs`) from
/// cascading any further past this node.
fn verbatim_node(name: &str, raw: &str, detected_framework: Option<String>) -> CommandNode {
    let provenance = Provenance::with_confidence(Source::HelpText, 0.0);
    let mut node = CommandNode::new(name, provenance);
    node.unparsed = raw
        .lines()
        .take(MAX_UNPARSED_LINES)
        .map(Text::sanitize)
        .collect();
    node.detected_framework = detected_framework;
    // Same rationale as the structurally-plausible path in `build_node`:
    // one probe did complete and did discover this level's (empty, as it
    // turns out) children list.
    node.children_filled = true;
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_tool;

    /// Every test in this module extracts a node whose path came from a
    /// real, structurally-known source (a real binary's actual root, or a
    /// path handed in directly by the test) — never an invented word — so
    /// `heading_attested: true` is the honest hint throughout, matching
    /// what `Runner::extract_full_for` passes for a root in production.
    const ATTESTED: NodeHints = NodeHints {
        heading_attested: true,
    };

    fn fixture(name: &str) -> String {
        // `tar --help` and `git --help`'s captures now live once, as the
        // corpus regression fixtures (`corpus/tar/<version>/help.txt`,
        // `corpus/git/<version>/help.txt` — see corpus/README.md), rather
        // than duplicated here byte-for-byte. A version bump means
        // updating the path below, same as the corpus fixture's own
        // directory would need renaming.
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

    /// `raw_help` is a second public entry point into the exec boundary,
    /// reached by a key press rather than by the extraction pipeline, so
    /// spec §6 rule 0 has to hold on it too. Asking interactively is not a
    /// reason `pkill something --help` becomes safe.
    ///
    /// The root shape is now permitted — `pkill --help` is measured
    /// harmless and is the whole point of the verbatim view — so what this
    /// pins is the boundary between the two.
    #[test]
    fn raw_help_allows_the_root_but_refuses_a_deeper_path_for_a_help_only_tool() {
        // A shim named `pkill`, so the refusal is exercised by file name
        // without this test depending on the real binary's behaviour.
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

        // "ghost" stands in for a name a native/cobra artifact scan
        // produced that never appeared under any recognized heading in
        // this shim's own `--help` text above.
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
        // The one document that's always safe to fetch — the tool's own
        // root `--help` — must actually appear, not just be described.
        assert!(joined.contains("Usage: shimtool [COMMAND]"), "{joined}");
        assert!(flag.contains("not heading-attested"), "{flag:?}");
    }

    #[test]
    fn build_node_from_tar_fixture_has_flags_and_confidence() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, None, "tar");
        assert_eq!(node.name, "tar");
        assert!(!node.flags.is_empty());
        assert!(node.provenance.confidence.unwrap() > 0.0);
        // A single probe genuinely discovers the complete direct-children
        // list for this level (spec §5.2) — tar has none, which is itself
        // a known-complete fact, not an unknown.
        assert!(node.children_filled);
    }

    // --- batch 6 part 4: staged degradation (spec §7 Tier B) ---

    /// Level 2 vs. level 1 (spec §7 Tier B): the exact same text parses to
    /// the exact same underlying confidence either way (tar's structure is
    /// what it is regardless of whether we know its framework), but an
    /// *unidentified* parse must be capped to signal "this is a guess",
    /// while an identified one is not.
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

    /// Level 3 (spec §7 Tier B step 3): a probe that recovers no flags, no
    /// subcommands, and no usage line must degrade to verbatim rather than
    /// present an empty-but-structured node as if it were a real (if
    /// sparse) parse — confidence `0.0`, non-empty `unparsed`, and no
    /// fabricated structure anywhere.
    #[test]
    fn structurally_implausible_output_degrades_to_verbatim() {
        let raw = "This tool prints only a friendly banner and nothing else.\nGoodbye.\n";
        let node = build_node("mystery", raw, None, "mystery");
        assert_eq!(node.provenance.confidence, Some(0.0));
        assert!(!node.unparsed.is_empty());
        assert_eq!(node.unparsed.len(), 2);
        assert_eq!(node.unparsed[0].as_str(), raw.lines().next().unwrap());
        assert!(node.flags.is_empty());
        assert!(node.subcommands.is_empty());
        assert!(node.usage.is_empty());
        assert!(node.description.is_none());
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

    // --- batch 6 part 4: the five priority frameworks (spec §7 Tier B) ---
    // Each gets a deterministic, fixture-based parse-level assertion here,
    // plus (where a real binary of that framework happens to be present
    // in this environment) a real-argv test further down exercising the
    // *actual* detection-plus-extraction pipeline end to end (AGENTS.md
    // §3.1: a mocked probe hid a dead cobra tier before; these must go
    // through `HelpTextTier::extract_node` for real).

    /// GnuArgp, [M-12]'s single largest fingerprint (15.5%): the exact
    /// regression this batch's profile mechanism exists to make
    /// structural rather than incidental — [M-10]'s tar/dd/sed/find/less
    /// phantom-subcommand bug, for a tool now positively identified as
    /// having no subcommand concept at all.
    #[test]
    fn gnu_argp_profile_forces_zero_subcommands_on_real_tar_output() {
        let raw = fixture("tar_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::GnuArgp)), None);
        assert!(parsed.subcommands.is_empty());
        let create = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("create"));
        assert!(create.is_some(), "expected --create to still be recovered");
    }

    /// ClapV3V4, [M-12]'s second-largest fingerprint (24.6%). Captured from
    /// the real `cargo --help` (a genuine `clap_builder`-linked binary —
    /// deliberately not `ripgrep`, which depends on clap but hand-rolls its
    /// own `--help` formatter [M-13] and would misrepresent this as a
    /// tool-specific fixture rather than a framework one).
    #[test]
    fn clap_v3v4_profile_recovers_cargo_commands_and_flags() {
        let raw = fixture("cargo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::ClapV3V4)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        // Deliberately only asserting names with no inline `, alias`
        // (clap's `visible_alias` rendering, e.g. "build, b") — entries
        // shaped that way are currently dropped as not name-shaped rather
        // than fabricated (a known, honest gap, not a silent wrong
        // answer), which is out of this batch's scope to fix.
        for want in ["clean", "new", "init", "add", "remove", "install"] {
            assert!(names.contains(&want), "{names:?}");
        }
        let long_flags: Vec<&str> = parsed
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        for want in ["version", "locked", "offline", "frozen", "help"] {
            assert!(long_flags.contains(&want), "{long_flags:?}");
        }
    }

    /// Argparse: the dedicated `scan_argparse_subparsers` path (see
    /// `sections`'s doc comment on it), captured from a real, runnable
    /// Python script's actual `--help` output
    /// (`tests/fixtures/help_text/argparse_demo.py`).
    #[test]
    fn argparse_profile_recovers_subparsers_not_plain_positionals() {
        let raw = fixture("argparse_demo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Argparse)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["init", "build", "run"], "{names:?}");
        let long_flags: Vec<&str> = parsed
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        assert!(long_flags.contains(&"verbose"));
        assert!(long_flags.contains(&"config"));
    }

    /// The same recovery when the tool styles the heading itself.
    ///
    /// `add_subparsers(title="commands")` is the ordinary way an argparse
    /// tool names that block, and the dedicated scan used to be gated on
    /// the heading reading `"positional arguments"`, so a styled heading
    /// skipped it entirely and the whole command tree collapsed to a
    /// single node. Captured from a real runnable script
    /// (`tests/fixtures/help_text/argparse_titled_demo.py`) rather than
    /// hand-written, so it stays honest about what argparse emits.
    #[test]
    fn argparse_profile_recovers_subparsers_under_a_styled_heading() {
        let raw = fixture("argparse_titled_demo_help.stdout");
        let parsed =
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Argparse)), None);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["init", "build", "run"], "{names:?}");
    }

    /// A flush-left command table is recognized (`dnf` 4 prints its whole
    /// command list at column 0, under a heading also at column 0), and
    /// the `--help` engine's indent rule cannot see that on its own.
    #[test]
    fn a_command_table_at_its_headings_own_indent_is_recovered() {
        let raw = "usage: dnf [options] COMMAND\n\nList of Main Commands:\n\nalias                     List or create command aliases\nautoremove                remove all unneeded packages\ncheck                     check for problems in the packagedb\n\nGeneral DNF options:\n  -v, --verbose         verbose operation\n";
        let parsed = sections::parse(raw);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alias", "autoremove", "check"], "{names:?}");
    }

    /// ...but a flush-left table of *settings* must not become commands,
    /// even when one of its rows happens to contain the word "command".
    ///
    /// At a shared indent nothing structurally separates a heading from a
    /// row, so every row is a candidate heading for those beneath it, and
    /// `mentions_commands_word` splits on non-alphanumerics — so
    /// `init-command` qualifies as mentioning "command". Measured against
    /// the real thing: `mysqlslap --help` ends with a table of config
    /// variables and their defaults, and this shape fabricated 28
    /// subcommands out of MySQL settings before the guard landed. [M-10],
    /// reached by a new route.
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

    /// Argparse's own dedicated-scan regression: an ordinary positional
    /// argument list (no `add_subparsers()` at all) must never be promoted
    /// to fake subcommands just because it sits under `positional
    /// arguments:` — the exact fabrication [M-10] is about.
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

    /// Busybox (spec issue #1): the dedicated `scan_comma_separated_commands`
    /// path, captured from this machine's real `busybox --help` output
    /// (`tests/fixtures/help_text/busybox_help.stdout`). Before this fix
    /// the framework fingerprinted correctly but yielded zero subcommands
    /// — the applet list is one flat, tab-indented, comma-separated run
    /// under `"Currently defined functions:"`, a shape the generic
    /// per-line bare-block engine cannot express at all.
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
        // `[` and `[[` are real busybox applets but fail the name-shape
        // test (spec §7 Tier B rule 3) — dropped, not fabricated into
        // something they're not. This just documents that the count
        // reflects real recovery, not an accidental swallow of the whole
        // block as one entry.
        assert!(names.len() > 250, "got {} applets: {names:?}", names.len());
    }

    /// Busybox's own dedicated-scan regression, mirroring argparse's above:
    /// the comma-separated scan must never fire outside a recognized
    /// command heading, even for a framework whose profile enables it —
    /// the heading-gate check in `parse_with_profile` is what prevents an
    /// unrelated comma-separated list elsewhere in a busybox-identified
    /// tool's output from being read as commands.
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
        let long_flags: Vec<&str> = parsed
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        assert!(long_flags.contains(&"verbose"));
        assert!(long_flags.contains(&"help"));
    }

    /// End-to-end identification: the click demo script's own bytes carry
    /// `import click` after a `python3` shebang, so Tier A′ step 1
    /// (artifact scanning, no probe) must find `Framework::Click` without
    /// ever needing to fall back to the help-text signature.
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
        assert!(!node.flags.is_empty());

        // The GNU-argp assertions below only apply to *GNU* tar. macOS
        // ships bsdtar, which is a different program that happens to have
        // the same name and legitimately carries no argp fingerprint —
        // asserting one there tests the runner's OS, not this code. CI
        // caught exactly that on `macos-latest`.
        if node.detected_framework.as_deref() != Some(Framework::GnuArgp.name()) {
            return;
        }

        // Real argv, real binary, real detection (AGENTS.md §3.1): tar's
        // own binary doesn't embed the GNU argp marker itself (it's
        // dynamically linked, the phrase lives in libc), so reaching this
        // point at all proves extract_node's help-text-signature fallback
        // ran against the text it fetched — the wiring, not a mock.
        assert!(
            node.subcommands.is_empty(),
            "GnuArgp must never carry subcommands: {:?}",
            node.subcommands.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// Real argv, real binary (AGENTS.md §3.1): `zoxide`, a real
    /// `clap_builder`-linked binary with both real flags and real
    /// subcommands, identified via Tier A′ step 1 (artifact scanning), no
    /// `--help` signature fallback needed. Deliberately not `cargo`
    /// (resolves to a `rustup` proxy on this machine that reads
    /// `$HOME/.rustup` to pick a toolchain, which fails under spec §6
    /// rule 8's mandatory per-probe scratch `HOME`: `error: rustup could
    /// not choose a version of cargo to run` — a real consequence of the
    /// sandboxing being correct, not a bug here, discovered by this test
    /// originally targeting `cargo`) and not `mandible` itself (this
    /// crate's own `framework::artifact::BINARY_MARKERS` table embeds the
    /// literal bytes `b"spf13/cobra"` as a scan pattern, and since
    /// `mandible` statically links this very crate, scanning mandible's
    /// own binary for that pattern finds *itself* — a real, if amusing,
    /// self-reference bug also discovered while writing this test).
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
        assert!(!node.flags.is_empty());
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in ["add", "edit", "import", "init", "query", "remove"] {
            assert!(names.contains(&want), "{names:?}");
        }
    }

    /// Real argv, real binary (AGENTS.md §3.1): `gh`'s own binary embeds
    /// `spf13/cobra` hundreds of times [M-13], identified via artifact
    /// scanning alone.
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

        // This test's unique job is proving the *wiring*: real argv, a real
        // spawned process, and detection running against the bytes that
        // came back (AGENTS.md §3.1 — a tier once shipped dead because its
        // tests mocked the probe). Whether the click grammar recovers the
        // right subcommands is covered deterministically, and on every
        // platform, by `click_profile_recovers_commands_and_flags` against
        // a captured fixture. Asserting grammar details here too added no
        // coverage and made the test depend on which click version the
        // runner's Python happens to have.
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
            !node.usage.is_empty() || !node.flags.is_empty() || !node.subcommands.is_empty(),
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
            !node.usage.is_empty() || !node.flags.is_empty() || !node.subcommands.is_empty(),
            "expected openssl's stderr-only help to produce *something*"
        );
    }

    // --- the replay seam: real-argv tests against a `Transcript` ---
    //
    // These drive `HelpTextTier` through its real `extract_node` — not
    // `build_node` directly — against a `Transcript` keyed on exactly the
    // argv the tier's own probe construction produces, with zero
    // subprocesses. This is the test class AGENTS.md §3.1 and spec §13.3
    // exist for: a tier that built the wrong argv (as a since-fixed cobra
    // tier once did, undetected because its tests mocked the probe and
    // skipped argv construction) would miss here instead of passing.

    fn exec_output(stdout: &str) -> crate::exec::ExecOutput {
        crate::exec::ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    /// Real argv, replayed: the root probe is `InertArgv::HelpLongForPath
    /// { words: [] }`, which renders to exactly `["--help"]`
    /// (`InertArgv::args`). A transcript keyed on that argv, holding the
    /// real captured `tar --help` fixture, must let `extract_node` produce
    /// the same structural result as the direct `build_node` unit test
    /// above — but reached through the tier's actual probe construction,
    /// not by handing the parser text directly.
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
        assert!(!node.flags.is_empty());
    }

    /// The self-similar-fan-out hazard [M-19] found live against `mandible
    /// systemctl`: a subcommand probe that comes back byte-identical to the
    /// root's own `--help` text must not be read as "this subcommand has
    /// the same children as the root" — that reading is what turned a
    /// harmless 18-way command list into an unbounded recursive re-probe
    /// (18, then 18², then 18³…) that starved the UI thread for 45+ seconds
    /// on real hardware before this fix.
    ///
    /// Reproduced here exactly as `systemctl` does it: one transcript entry
    /// for the root argv and a *second* entry, for a subcommand's argv,
    /// holding the identical text — standing in for a tool whose own getopt
    /// permutes `--help` to the front regardless of what precedes it. Two
    /// calls through the *same* tier instance, root first, so the cache
    /// this fix adds is populated exactly as production does it (root
    /// always fills before any child, spec §5.2 step 4's cascade order).
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

    /// A subcommand's probe that happens to share *some* text with the root
    /// (a common preamble, say) but is not byte-identical must be parsed
    /// normally — the guard is keyed on exact equality, not similarity, so
    /// it never swallows a real subcommand's real, distinct structure.
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
        let flag_names: Vec<&str> = child
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        assert!(
            flag_names.contains(&"force"),
            "a genuinely distinct subcommand's own flags must still parse: {flag_names:?}"
        );
    }

    /// The negative case: a transcript that does *not* contain the argv
    /// this tier actually sends must miss loudly, naming the argv it was
    /// asked for — never fall through to an empty, confidently-wrong
    /// success. If a tier's argv construction ever drifts from what this
    /// test expects, this is the assertion that catches it.
    #[test]
    fn extract_node_against_a_transcript_missing_the_argv_is_a_named_miss() {
        // Deliberately keyed on a *different* argv than the tier will ever
        // send at the root (`["commit", "--help"]` instead of `["--help"]`)
        // — simulating exactly the class of bug this seam exists to catch.
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

    /// The real-world specimen behind [`pick_stream`]'s fix, replayed
    /// verbatim (bytes measured by hand from `openssl cmp --help` on a real
    /// machine): two `CMP info: ...` diagnostic lines land on **stdout**,
    /// and the tool's entire help — `Usage: cmp [options]`, `Valid options
    /// are:`, the flag list — lands on **stderr**. Before the fix,
    /// `pick_stream` preferred the merely non-empty stdout and handed the
    /// parser two banner lines instead of the document, so this node came
    /// back with zero flags; openssl has ~150 subcommands in this exact
    /// shape. Driven through the tier's real `extract_node`, not
    /// `build_node` directly, so it also proves argv construction
    /// (AGENTS.md §3.1).
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

        assert!(
            !node.usage.is_empty() || !node.flags.is_empty(),
            "expected the parser to read stderr's help-shaped document \
             instead of stdout's two diagnostic lines; got usage={:?} flags={:?}",
            node.usage,
            node.flags
        );
        // openssl spells its long options single-dash (`-cmd val`), so the
        // generic grammar reads them as a short flag plus a value name
        // (`short: 'c'`, `value_name: "md"`) rather than a `--long` — that
        // is a grammar detail unrelated to this fix. What this assertion
        // pins is that the six real flags from stderr's document parsed at
        // all (the pre-fix bug produced zero: the two stdout diagnostic
        // lines contain no flag syntax whatsoever), identified by
        // descriptions that only the real document has.
        assert!(
            node.flags.len() >= 5,
            "expected the ~6 real flags from stderr's document, not the \
             empty parse the two stdout diagnostic lines would produce: {:?}",
            node.flags
        );
        assert!(
            node.flags.iter().any(|f| f
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("CMP request to send"))),
            "expected the `-cmd` flag's real description from stderr's \
             document to have parsed: {:?}",
            node.flags
        );
    }
}
