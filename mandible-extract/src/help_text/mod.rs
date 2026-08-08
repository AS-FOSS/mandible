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

mod grammar;
mod profile;
mod sections;

use crate::errors::ExtractError;
use crate::exec::{run_inert, InertArgv};
use crate::framework::{self, Framework};
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use mandible_core::{Authority, CommandNode, Provenance, Source, Text};
use std::path::Path;
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
#[derive(Debug, Default)]
pub struct HelpTextTier;

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
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        let words: Vec<String> = path.iter().skip(1).cloned().collect();
        let raw = probe_help_text(tool_path, &words)?;
        let node_name = path.last().cloned().unwrap_or_else(|| tool.name.clone());
        // Detection order per spec §7 Tier A′, never double-probing: the
        // free artifact scan first (memoized per binary path — see
        // `framework::identify_from_artifact`'s doc comment — so probing
        // every node of a deep tree over the same binary costs nothing
        // extra here), and only on a miss, the help-text signature against
        // the text this call already fetched above for its own parsing.
        let detected = framework::identify_from_artifact(tool)
            .or_else(|| framework::identify_from_help_text(&raw));
        Ok(build_node(&node_name, &raw, detected))
    }

    fn is_incremental(&self) -> bool {
        true
    }
}

/// Run `<tool> <words...> --help`, falling back to `-h` if that produced
/// no output at all on either stream, and return whichever stream had
/// content (stdout preferred when both are non-empty — spec §7 Tier B,
/// measured against real tools in [M-8]).
fn probe_help_text(tool_path: &Path, words: &[String]) -> Result<String, ExtractError> {
    let long = run_inert(
        tool_path,
        &InertArgv::HelpLongForPath {
            words: words.to_vec(),
        },
        EXTRACT_TIMEOUT,
    )?;
    if !long.stdout.is_empty() || !long.stderr.is_empty() {
        return Ok(pick_stream(&long.stdout, &long.stderr));
    }

    let short = run_inert(
        tool_path,
        &InertArgv::HelpShortForPath {
            words: words.to_vec(),
        },
        EXTRACT_TIMEOUT,
    )?;
    Ok(pick_stream(&short.stdout, &short.stderr))
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
/// [`run_inert`] unchanged: `kill --help` is no safer because a human asked
/// for it interactively.
pub fn raw_help(tool: &ResolvedTool, path: &[String]) -> Result<Vec<Text>, ExtractError> {
    let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
    let words: Vec<String> = path.iter().skip(1).cloned().collect();
    let raw = probe_help_text(tool_path, &words)?;
    Ok(raw
        .lines()
        .take(MAX_UNPARSED_LINES)
        .map(Text::sanitize)
        .collect())
}

/// Prefer stdout when both streams are non-empty (spec §7 Tier B).
fn pick_stream(stdout: &[u8], stderr: &[u8]) -> String {
    if !stdout.is_empty() {
        String::from_utf8_lossy(stdout).into_owned()
    } else {
        String::from_utf8_lossy(stderr).into_owned()
    }
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
fn build_node(name: &str, raw: &str, framework: Option<Framework>) -> CommandNode {
    let fw_profile = framework.map(profile::profile);
    let parsed = sections::parse_with_profile(raw, fw_profile.as_ref());
    let detected_framework = framework.map(|f| f.name().to_string());

    let structurally_plausible =
        !parsed.flags.is_empty() || !parsed.subcommands.is_empty() || !parsed.usage.is_empty();

    if !structurally_plausible {
        let provenance = Provenance::with_confidence(Source::HelpText, 0.0);
        let mut node = CommandNode::new(name, provenance);
        node.unparsed = raw
            .lines()
            .take(MAX_UNPARSED_LINES)
            .map(Text::sanitize)
            .collect();
        node.detected_framework = detected_framework;
        // Same rationale as the structurally-plausible path below: one
        // probe did complete and did discover this level's (empty, as it
        // turns out) children list.
        node.children_filled = true;
        return node;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_tool;

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/help_text/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    /// `raw_help` is a second public entry point into the exec boundary,
    /// reached by a key press rather than by the extraction pipeline. The
    /// never-probe refusal (spec §6 rule 0) has to hold on it too: asking
    /// interactively is not a reason `pkill something --help` becomes safe,
    /// and it was an interactive `mandible pkill` that froze a machine.
    #[test]
    fn raw_help_refuses_a_never_probe_tool() {
        let mut tool = resolve_tool("pkill");
        tool.path = Some(std::path::PathBuf::from("/usr/bin/pkill"));
        let err = raw_help(&tool, &["pkill".to_string()]).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ExtractError::Exec(crate::exec::ExecError::RefusedUnsafeTool { .. })
            ),
            "expected a refusal, got {err:?}"
        );
    }

    #[test]
    fn build_node_from_tar_fixture_has_flags_and_confidence() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, None);
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
        let unidentified = build_node("tar", &raw, None);
        let identified = build_node("tar", &raw, Some(Framework::GnuArgp));
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
        let node = build_node("mystery", raw, None);
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
        let node = build_node("tar", &raw, None);
        assert!(node.unparsed.is_empty());
    }

    #[test]
    fn detected_framework_is_recorded_on_the_node() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw, Some(Framework::GnuArgp));
        assert_eq!(
            node.detected_framework.as_deref(),
            Some(Framework::GnuArgp.name())
        );
        let unidentified = build_node("tar", &raw, None);
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
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::GnuArgp)));
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
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::ClapV3V4)));
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
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Argparse)));
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

    /// Argparse's own dedicated-scan regression: an ordinary positional
    /// argument list (no `add_subparsers()` at all) must never be promoted
    /// to fake subcommands just because it sits under `positional
    /// arguments:` — the exact fabrication [M-10] is about.
    #[test]
    fn argparse_profile_does_not_fabricate_subcommands_from_plain_positionals() {
        let raw = "usage: tool [-h] path\n\npositional arguments:\n  path        the file to process\n\noptions:\n  -h, --help  show this help message and exit\n";
        let parsed =
            sections::parse_with_profile(raw, Some(&profile::profile(Framework::Argparse)));
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
            sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Busybox)));
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
        let parsed = sections::parse_with_profile(raw, Some(&profile::profile(Framework::Busybox)));
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
        let parsed = sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Cobra)));
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
        let parsed = sections::parse_with_profile(&raw, Some(&profile::profile(Framework::Click)));
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
        let tier = HelpTextTier;
        let tool = resolve_tool("sh");
        assert!(tier.detect(&tool));
    }

    #[test]
    fn detect_false_for_unresolved_tool() {
        let tier = HelpTextTier;
        let tool = resolve_tool("definitely-not-a-real-tool-xyz");
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn extract_node_against_real_tar_binary() {
        let tier = HelpTextTier;
        let tool = resolve_tool("tar");
        if tool.path.is_none() {
            return; // environment without tar; nothing to verify
        }
        let node = tier.extract_node(&tool, &["tar".to_string()]).unwrap();
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
        let tier = HelpTextTier;
        let tool = resolve_tool("zoxide");
        if tool.path.is_none() {
            return;
        }
        let node = tier.extract_node(&tool, &["zoxide".to_string()]).unwrap();
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
        let tier = HelpTextTier;
        let tool = resolve_tool("gh");
        if tool.path.is_none() {
            return;
        }
        let node = tier.extract_node(&tool, &["gh".to_string()]).unwrap();
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
        let tier = HelpTextTier;
        let path = format!(
            "{}/tests/fixtures/help_text/argparse_demo.py",
            env!("CARGO_MANIFEST_DIR")
        );
        let tool = resolve_tool(&path);
        assert!(tool.path.is_some(), "fixture script must be executable");
        let node = tier
            .extract_node(&tool, &["argparse_demo.py".to_string()])
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
        let tier = HelpTextTier;
        let path = format!(
            "{}/tests/fixtures/help_text/click_demo.py",
            env!("CARGO_MANIFEST_DIR")
        );
        let tool = resolve_tool(&path);
        assert!(tool.path.is_some(), "fixture script must be executable");
        let node = tier
            .extract_node(&tool, &["click_demo.py".to_string()])
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
        let tier = HelpTextTier;
        let tool = resolve_tool("ip");
        if tool.path.is_none() {
            return;
        }
        let node = tier.extract_node(&tool, &["ip".to_string()]).unwrap();
        assert!(
            !node.usage.is_empty() || !node.flags.is_empty() || !node.subcommands.is_empty(),
            "expected ip's stderr-only help to produce *something*"
        );
    }

    #[test]
    fn extract_node_against_stderr_only_openssl_binary() {
        let tier = HelpTextTier;
        let tool = resolve_tool("openssl");
        if tool.path.is_none() {
            return;
        }
        let node = tier.extract_node(&tool, &["openssl".to_string()]).unwrap();
        assert!(
            !node.usage.is_empty() || !node.flags.is_empty() || !node.subcommands.is_empty(),
            "expected openssl's stderr-only help to produce *something*"
        );
    }
}
