//! Tier E: native, self-describing binary probes (spec §7 Tier E).
//!
//! One protocol, driven entirely through [`crate::exec::run_inert`] with
//! an argv shape already on [`crate::exec::InertArgv`]:
//!
//! - **cobra `__complete`** (`gh`, `docker`, `kubectl`, ...). Needs two
//!   probes per node, not one: flags only show up when the trailing word
//!   is `"-"` instead of `""` [M-2]. Responses end with a `:N` directive
//!   line on stdout; candidates are `value\tdescription` (or bare `value`)
//!   per line.
//!
//! **An empty trailing word does not mean "subcommands only".** cobra
//! answers `__complete <path> ""` by emitting the node's real subcommands
//! and then appending whatever that command's `ValidArgsFunction` returns
//! — live application state (container names, image tags, ...). See
//! [`populate_subcommands`] for the rule that stops it, and spec Appendix
//! A [M-2a] for the measurement.
//!
//! clap's `CompleteEnv` was also probed here once and has been removed: it
//! never identified a real clap tool, matched unrelated ones by accident,
//! and its argv spelling handed tools an empty first positional — the
//! shape measured terminating every process via `pkill -- ""` [M-4]. Full
//! reasoning is at the former call site below.
//!
//! **Strictly node-scoped, never eager.** [`ExtractionTier::extract_node`]
//! probes exactly the one path it's given; the two-probe cost is paid once
//! per node, on demand, driven by the runner's lazy per-node expansion
//! (spec §5.2) — this tier is the reason that laziness exists at all
//! [M-3].
//!
//! **Depth cap and an echoed-root guard**, because some tools silently
//! re-emit the root's own candidate list instead of rejecting an
//! unrecognized deep path. A path deeper than [`DEPTH_CAP`] is never
//! probed; shallower than that, this tier remembers each tool's
//! root-level candidate-list fingerprint the first time it's seen and
//! treats a later, deeper probe whose fingerprint matches as an echo.
//!
//! **Alias detection.** cobra apps sometimes register a shortcut as its
//! own top-level entry (`Alias for "pr checkout"`, `gh co`). When the
//! named target is a sibling in the same candidate list, the alias name
//! is recorded on that sibling's [`mandible_core::CommandNode::aliases`]
//! rather than becoming a subcommand of its own. When the target isn't a
//! sibling, the candidate is dropped rather than fabricated or guessed
//! across levels this tier hasn't probed.
//!
//! **Authority** (spec §4.4): structural 200, prose 40 — a live probe
//! should win flag existence against carapace's vendored snapshot, but
//! carapace's human-written descriptions stay more trustworthy.
//!
//! **Gated on prior evidence, never speculative** (spec §7 Tier E).
//! `detect()` used to send `__complete <word>` to every tool on `PATH`
//! speculatively; measured broadcasting the literal text to every
//! terminal via `wall`, the same shape of hazard as `pkill -- ""` (§6
//! rule 2a) discovered a second time. `detect()` now sends `__complete`
//! only when [`crate::framework::identify_from_artifact`] has already
//! found the `spf13/cobra` marker in the tool's own compiled bytes —
//! ground truth, free (a memoized file read, no subprocess).
//! [`CobraEvidence`] is the seam that makes this testable without a real
//! cobra-marked binary on disk.

use crate::errors::ExtractError;
use crate::exec::{InertArgv, LiveProbe, Probe};
use crate::framework::{self, Framework};
use crate::resolve::ResolvedTool;
use crate::tier::{ExtractionTier, NodeHints};
use mandible_core::{
    is_command_name_shaped, Authority, CommandNode, Entity, Provenance, Source, Text,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wall-clock cap for a `detect` probe (spec §6 rule 4).
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Wall-clock cap for an `extract_node` probe (spec §6 rule 4).
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(10);

/// Never probe a path deeper than this many segments below the tool root.
/// Real command trees essentially never nest this deep [M-3].
const DEPTH_CAP: usize = 6;

/// Which native protocol a tool was found to speak, cached per tool name
/// after the first successful `detect` so `extract_node` doesn't have to
/// re-run the same detection probe on every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    /// cobra's `__complete` convention. The only native protocol left
    /// after clap's `CompleteEnv` probe was removed; kept as an enum
    /// because the protocol cache is keyed on it.
    Cobra,
}

/// Prior evidence that a tool speaks cobra, checked before this tier ever
/// constructs a `__complete` argv (spec §7 Tier E's gate). A trait,
/// mirroring [`Probe`]'s own seam, rather than calling
/// [`framework::identify_from_artifact`] directly: the production check
/// scans real bytes and can never recognize a shebang shell script as
/// cobra (Go-only, always compiled), so every test shim needs a way to
/// supply a fixed answer instead.
trait CobraEvidence: Send + Sync {
    /// True when `tool` is already known to speak cobra.
    fn speaks_cobra(&self, tool: &ResolvedTool) -> bool;
}

/// Production [`CobraEvidence`]: ground truth from the tool's own compiled
/// bytes, via Tier A′. Adds no subprocess — `identify_from_artifact` is a
/// bounded, memoized file read.
struct ArtifactEvidence;

impl CobraEvidence for ArtifactEvidence {
    fn speaks_cobra(&self, tool: &ResolvedTool) -> bool {
        framework::identify_from_artifact(tool) == Some(Framework::Cobra)
    }
}

/// Tier E: cobra `__complete` dynamic-completion probes.
pub struct NativeTier {
    /// Which protocol each tool name was found to speak. Bounded by the
    /// number of distinct tool names probed in this process's lifetime.
    protocol_cache: Mutex<HashMap<String, Protocol>>,
    /// Each cobra-speaking tool's root-level candidate-list fingerprint,
    /// remembered the first time it's seen (see module doc's "echoed-root
    /// guard").
    root_fingerprint_cache: Mutex<HashMap<String, u64>>,
    /// The source of a `__complete` probe's output — [`LiveProbe`] in
    /// production, or a [`crate::exec::Transcript`] to replay frozen bytes.
    probe: Arc<dyn Probe>,
    /// The gate `detect()` checks before ever sending `__complete` — see
    /// [`CobraEvidence`].
    evidence: Arc<dyn CobraEvidence>,
}

impl Default for NativeTier {
    fn default() -> Self {
        Self::new(Arc::new(LiveProbe))
    }
}

impl NativeTier {
    /// Build this tier against an explicit probe, gated by the real,
    /// file-backed [`ArtifactEvidence`] check.
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self::new_with_evidence(probe, Arc::new(ArtifactEvidence))
    }

    /// [`Self::new`], but against an explicit [`CobraEvidence`] — the seam
    /// a test uses to exercise `detect()`'s gated branch without a real
    /// cobra-marked binary on disk. Not `pub`: the negative property (no
    /// evidence ⇒ no probe) is proven against the real, ungated production
    /// check by this crate's integration tests instead.
    fn new_with_evidence(probe: Arc<dyn Probe>, evidence: Arc<dyn CobraEvidence>) -> Self {
        Self {
            protocol_cache: Mutex::new(HashMap::new()),
            root_fingerprint_cache: Mutex::new(HashMap::new()),
            probe,
            evidence,
        }
    }
}

impl ExtractionTier for NativeTier {
    fn name(&self) -> &'static str {
        "native"
    }

    fn authority(&self) -> Authority {
        Source::NativeDynamic {
            protocol: String::new(),
        }
        .authority()
    }

    fn detect(&self, tool: &ResolvedTool) -> bool {
        let Some(tool_path) = &tool.path else {
            return false;
        };
        if self.cached_protocol(&tool.name).is_some() {
            return true;
        }
        // The gate (spec §7 Tier E): `__complete` is never sent
        // speculatively. Without prior evidence, decline before
        // constructing any argv at all.
        if !self.evidence.speaks_cobra(tool) {
            return false;
        }
        if probe_cobra_list(self.probe.as_ref(), tool_path, &[], "", DETECT_TIMEOUT).is_some() {
            self.set_protocol(&tool.name, Protocol::Cobra);
            return true;
        }
        false
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
        _hints: NodeHints,
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        let words: Vec<String> = path.iter().skip(1).cloned().collect();
        let name = path.last().cloned().unwrap_or_else(|| tool.name.clone());

        match self.cached_protocol(&tool.name) {
            Some(Protocol::Cobra) => {
                Ok(self.extract_cobra_node(tool_path, &tool.name, &words, name))
            }
            None => Err(ExtractError::Other(
                "no native protocol detected for this tool".to_string(),
            )),
        }
    }

    fn is_incremental(&self) -> bool {
        true
    }
}

impl NativeTier {
    fn cached_protocol(&self, tool_name: &str) -> Option<Protocol> {
        self.protocol_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tool_name)
            .copied()
    }

    fn set_protocol(&self, tool_name: &str, protocol: Protocol) {
        self.protocol_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tool_name.to_string(), protocol);
    }

    fn remembered_root_fingerprint(&self, tool_name: &str) -> Option<u64> {
        self.root_fingerprint_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tool_name)
            .copied()
    }

    fn remember_root_fingerprint(&self, tool_name: &str, fingerprint: u64) {
        self.root_fingerprint_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tool_name.to_string())
            .or_insert(fingerprint);
    }

    fn extract_cobra_node(
        &self,
        tool_path: &Path,
        tool_name: &str,
        words: &[String],
        name: String,
    ) -> CommandNode {
        let provenance = Provenance::single(Source::NativeDynamic {
            protocol: "cobra-dunder-complete".to_string(),
        });
        let mut node = CommandNode::new(name, provenance.clone());
        // One probe genuinely discovers the complete list of direct
        // subcommand names at this level (spec §5.2).
        node.children_filled = true;

        if words.len() >= DEPTH_CAP {
            // An empty leaf, not an error: the merge gets nothing extra
            // from this tier at this depth, rather than the node failing.
            return node;
        }

        if let Some(candidates) =
            probe_cobra_list(self.probe.as_ref(), tool_path, words, "", EXTRACT_TIMEOUT)
        {
            let fingerprint = fingerprint_candidates(&candidates);
            let is_root = words.is_empty();
            if is_root {
                self.remember_root_fingerprint(tool_name, fingerprint);
            }
            let looks_echoed =
                !is_root && self.remembered_root_fingerprint(tool_name) == Some(fingerprint);
            if !looks_echoed {
                populate_subcommands(&mut node, candidates, &provenance);
            }
        }

        if let Some(candidates) =
            probe_cobra_list(self.probe.as_ref(), tool_path, words, "-", EXTRACT_TIMEOUT)
        {
            for candidate in candidates {
                // No description gate here: a flag candidate is already
                // filtered by its own dash shape, and cobra emits plenty
                // of real flags with no help text.
                if let Some(flag) =
                    flag_from_candidate(&candidate.value, candidate.description_text(), &provenance)
                {
                    node.entities.push(flag);
                }
            }
        }

        node
    }
}

/// Run `<tool> <words...> <trailing>` as `InertArgv::CobraComplete` and
/// parse the response, returning `None` if the output doesn't look like a
/// genuine cobra completion response (no `:N` directive line found).
fn probe_cobra_list(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    trailing: &str,
    timeout: Duration,
) -> Option<Vec<Candidate>> {
    let mut argv_words = words.to_vec();
    // The empty word is required by cobra's protocol, not incidental.
    // Safe here: never the first positional, always shielded behind the
    // `__complete` sentinel, which a non-cobra tool rejects. Spec §6 rule 2a.
    argv_words.push(trailing.to_string());
    let out = probe
        .run(
            tool_path,
            &InertArgv::CobraComplete { words: argv_words },
            timeout,
        )
        .ok()?;
    parse_cobra_response(&out.stdout)
}

/// One line of a cobra `__complete` response.
///
/// The distinction that matters is whether the line carried a description
/// at all — the only thing in cobra's wire format that separates a real
/// subcommand from a `ValidArgsFunction` value. See [`populate_subcommands`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// The completion value: a subcommand name, a flag spelling, or an
    /// argument value.
    value: String,
    /// The text after the `\t`, when there was one and it was not blank.
    description: Option<String>,
}

impl Candidate {
    /// The description as a `&str`, with "absent" and "blank" collapsed.
    fn description_text(&self) -> &str {
        self.description.as_deref().unwrap_or("")
    }
}

/// Parse a cobra `__complete` response: candidate lines (`value` or
/// `value\tdescription`) followed by a `:N` directive line. Returns `None`
/// unless that directive line is actually found.
fn parse_cobra_response(stdout: &[u8]) -> Option<Vec<Candidate>> {
    let text = String::from_utf8_lossy(stdout);
    let mut candidates = Vec::new();
    let mut saw_directive = false;
    for line in text.lines() {
        if is_directive_line(line) {
            saw_directive = true;
            break;
        }
        if line.is_empty() {
            continue;
        }
        let (value, description) = match line.split_once('\t') {
            // A tab with nothing (or only blanks) after it carries no more
            // information than no tab at all, so both collapse to `None`.
            Some((value, description)) if !description.trim().is_empty() => {
                (value, Some(description.to_string()))
            }
            Some((value, _)) => (value, None),
            None => (line, None),
        };
        candidates.push(Candidate {
            value: value.to_string(),
            description,
        });
    }
    saw_directive.then_some(candidates)
}

/// True for a cobra `ShellCompDirective` trailer line: `:` followed by one
/// or more ASCII digits and nothing else.
fn is_directive_line(line: &str) -> bool {
    line.strip_prefix(':')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// A cheap, stable fingerprint of a candidate list's values (not
/// descriptions), used only to recognize "same list as the root's" — not
/// security-sensitive, so a simple hash suffices.
fn fingerprint_candidates(candidates: &[Candidate]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for candidate in candidates {
        candidate.value.hash(&mut hasher);
    }
    hasher.finish()
}

/// `Alias for "target"` — cobra's own convention for a registered
/// shortcut command (`gh co` → `Alias for "pr checkout"`). Returns the
/// target's last path segment (`"checkout"` from `"pr checkout"`), which
/// is what a same-level sibling's own name would be.
fn alias_target_last_segment(description: &str) -> Option<&str> {
    let inner = description
        .strip_prefix("Alias for \"")?
        .strip_suffix('"')?;
    inner.split_whitespace().next_back()
}

/// True when a candidate list may be read as a subcommand list at all.
///
/// cobra answers `__complete <path> ""` by emitting the node's real
/// subcommands (always `name\tShort`) and then appending whatever the
/// command's `ValidArgsFunction` returns — live application state. The
/// wire format marks no boundary between the two halves, but every real
/// subcommand carries a description and a `ValidArgsFunction` value
/// normally does not. So a list is trusted only when every candidate in
/// it is described; a single undescribed candidate condemns the whole
/// list, including the described entries. Measured admitting every real
/// subcommand and no argument value across 631 real command paths on
/// `docker`/`gh` (spec Appendix A [M-2a]).
///
/// The trade is deliberate and one-directional: a real subcommand with an
/// empty `Short`, sitting alongside undescribed values, is dropped here.
/// Tier B's `--help` parse still finds it. Never relax this.
fn candidates_are_a_subcommand_list(candidates: &[Candidate]) -> bool {
    !candidates.is_empty() && candidates.iter().all(|c| c.description.is_some())
}

/// Turn a subcommands-probe candidate list into `node`'s subcommands,
/// routing alias-shaped candidates onto a matching sibling's `aliases`
/// instead of fabricating them as their own subcommand (spec §7 Tier E).
/// Refuses the whole list unless [`candidates_are_a_subcommand_list`]
/// accepts it.
fn populate_subcommands(
    node: &mut CommandNode,
    candidates: Vec<Candidate>,
    provenance: &Provenance,
) {
    if !candidates_are_a_subcommand_list(&candidates) {
        return;
    }
    let mut aliases: Vec<(String, String)> = Vec::new();
    for candidate in candidates {
        // Every candidate is described here (the guard above rejected the
        // list otherwise), so alias routing is unaffected.
        let Candidate { value, description } = candidate;
        let description = description.unwrap_or_default();
        if let Some(target) = alias_target_last_segment(&description) {
            aliases.push((value, target.to_string()));
            continue;
        }
        if !is_command_name_shaped(&value) {
            continue;
        }
        let summary = non_empty_text(&description);
        let mut child = CommandNode::new(value, provenance.clone());
        child.summary = summary;
        child.children_filled = false;
        node.subcommands.push(child);
    }
    for (alias_name, target_name) in aliases {
        if let Some(sibling) = node.subcommands.iter_mut().find(|c| c.name == target_name) {
            if !sibling.aliases.contains(&alias_name) {
                sibling.aliases.push(alias_name);
            }
        }
        // No matching sibling (the target is nested deeper than this
        // level, e.g. `co` for the nested `pr checkout`): dropped rather
        // than guessed at across a level this tier hasn't probed.
    }
}

/// A flag candidate from a cobra `"-"` probe is already a single bare
/// spelling (`-a` or `--all-tags`, never a combined `-a, --all-tags`
/// spec), so this needs none of Tier B's flag-spec grammar — just which
/// dash shape it is.
fn flag_from_candidate(value: &str, description: &str, provenance: &Provenance) -> Option<Entity> {
    let trimmed = value.trim();
    let (short, long) = if let Some(long) = trimmed.strip_prefix("--") {
        if long.is_empty() {
            return None;
        }
        (None, Some(long.to_string()))
    } else {
        let rest = trimmed.strip_prefix('-')?;
        let mut chars = rest.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None; // more than one char after a single dash
        }
        (Some(c), None)
    };
    let mut flag = Entity::flag_spelled(short, long, false, false, provenance.clone());
    flag.description = non_empty_text(description);
    Some(flag)
}

fn non_empty_text(s: &str) -> Option<Text> {
    let t = s.trim();
    (!t.is_empty()).then(|| Text::sanitize(t))
}

// clap's `CompleteEnv` probe was removed here; "re-add it" is an
// obvious-looking idea, so the reasons are worth keeping.
//
// The probe was `COMPLETE=<shell> <tool> -- <partial>`, and it could not
// be spelled safely: an empty partial renders `<tool> -- ""`, handing the
// tool an empty first positional — the same shape that terminated every
// process via `pkill -- ""` (spec §6 rule 0). Spelled `<tool> --` instead,
// it's harmless but wrong: most tools just print ordinary output, which a
// shape heuristic then misreads as a candidate list.
//
// It never worked either: clap's protocol has no self-identifying trailer
// like cobra's `:N` directive, so detection was only ever a shape
// heuristic, and it matched several non-clap tools on a PATH-wide sweep
// while never actually identifying a real clap tool [M-4].
//
// Re-adding it needs a way to confirm the tool really speaks the protocol
// before trusting the response, and a spelling that never hands a tool an
// empty first positional. Gating on Tier A′ framework identification would
// supply the first.

#[cfg(test)]
mod tests {
    use super::*;

    /// A described candidate, as cobra emits a real subcommand.
    fn described(value: &str, description: &str) -> Candidate {
        Candidate {
            value: value.to_string(),
            description: Some(description.to_string()),
        }
    }

    /// An undescribed candidate, as a `ValidArgsFunction` emits.
    fn bare(value: &str) -> Candidate {
        Candidate {
            value: value.to_string(),
            description: None,
        }
    }

    #[test]
    fn parses_a_real_cobra_response_shape() {
        let raw = b"add\tAdd file contents to the index\nrebase\tReapply commits\n:4\nCompletion ended with directive: ShellCompDirectiveNoFileComp\n";
        let parsed = parse_cobra_response(raw).expect("should recognize the directive");
        assert_eq!(
            parsed,
            vec![
                described("add", "Add file contents to the index"),
                described("rebase", "Reapply commits"),
            ]
        );
    }

    #[test]
    fn rejects_output_with_no_directive_line() {
        // A tool that doesn't understand __complete at all and just
        // printed ordinary help text or an error.
        let raw = b"error: unknown command\nusage: mytool [options]\n";
        assert!(parse_cobra_response(raw).is_none());
    }

    #[test]
    fn bare_value_with_no_tab_has_no_description() {
        let raw = b"solo\n:0\n";
        let parsed = parse_cobra_response(raw).unwrap();
        assert_eq!(parsed, vec![bare("solo")]);
    }

    /// A tab with nothing after it says no more than no tab at all, and
    /// must not be allowed to pass as "described" — otherwise the
    /// subcommand-list rule below could be satisfied by whitespace.
    #[test]
    fn a_trailing_tab_with_no_text_is_not_a_description() {
        let parsed = parse_cobra_response(b"solo\t\nspaced\t   \n:0\n").unwrap();
        assert_eq!(parsed, vec![bare("solo"), bare("spaced")]);
    }

    #[test]
    fn is_directive_line_matches_colon_digits_only() {
        assert!(is_directive_line(":4"));
        assert!(is_directive_line(":0"));
        assert!(!is_directive_line(":"));
        assert!(!is_directive_line(":4x"));
        assert!(!is_directive_line("value:4"));
    }

    #[test]
    fn flag_from_candidate_recognizes_long_and_short() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let long = flag_from_candidate("--all-tags", "Download all tagged images", &prov).unwrap();
        assert_eq!(long.long(), Some("all-tags"));
        assert_eq!(long.short(), None);

        let short = flag_from_candidate("-a", "Download all tagged images", &prov).unwrap();
        assert_eq!(short.short(), Some('a'));
        assert_eq!(short.long(), None);
    }

    #[test]
    fn flag_from_candidate_rejects_non_flag_shapes() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        assert!(flag_from_candidate("build", "Build an image", &prov).is_none());
        assert!(flag_from_candidate("-ab", "not a single short flag", &prov).is_none());
        assert!(flag_from_candidate("--", "bare dashes", &prov).is_none());
    }

    #[test]
    fn alias_target_last_segment_extracts_the_final_word() {
        assert_eq!(
            alias_target_last_segment("Alias for \"pr checkout\""),
            Some("checkout")
        );
        assert_eq!(
            alias_target_last_segment("Alias for \"status\""),
            Some("status")
        );
        assert_eq!(alias_target_last_segment("Not an alias at all"), None);
    }

    /// `gh co` (`Alias for "pr checkout"`) must not become its own
    /// subcommand entry, and since its target isn't a sibling at this
    /// level, must not be attached anywhere either.
    #[test]
    fn alias_for_a_non_sibling_target_is_dropped_not_fabricated() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("gh", prov.clone());
        populate_subcommands(
            &mut node,
            vec![
                described("pr", "Work with pull requests"),
                described("co", "Alias for \"pr checkout\""),
            ],
            &prov,
        );
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["pr"]);
        assert!(node.subcommands[0].aliases.is_empty());
    }

    /// An alias whose target is a sibling in the same candidate list must
    /// be recorded on that sibling, not emitted as its own subcommand.
    #[test]
    fn alias_for_a_sibling_target_is_recorded_not_duplicated() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("tool", prov.clone());
        populate_subcommands(
            &mut node,
            vec![
                described("remove", "Remove a thing"),
                described("rm", "Alias for \"remove\""),
            ],
            &prov,
        );
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["remove"]);
        assert_eq!(node.subcommands[0].aliases, vec!["rm".to_string()]);
    }

    // --- the dynamic-argument guard (spec Appendix A [M-2a]) ---

    /// `docker __complete stop ""` answers with running container names,
    /// bare, because cobra runs the leaf's `ValidArgsFunction`. Those must
    /// produce no subcommands at all — not "some, filtered by name shape".
    #[test]
    fn a_leaf_answering_with_bare_dynamic_values_yields_no_subcommands() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("stop", prov.clone());
        populate_subcommands(
            &mut node,
            vec![
                bare("mandible-canary-1"),
                bare("mandible-canary-2"),
                bare("adoring_hopper"),
            ],
            &prov,
        );
        assert!(
            node.subcommands.is_empty(),
            "container names must never become commands: {:?}",
            node.subcommands.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// The mixed case, from `docker __complete context use ""`: one
    /// undescribed entry condemns the whole list, including the described
    /// entries.
    #[test]
    fn a_mixed_list_contributes_nothing_not_even_its_described_entries() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("use", prov.clone());
        populate_subcommands(
            &mut node,
            vec![described("rootless", "current"), bare("default")],
            &prov,
        );
        assert!(node.subcommands.is_empty(), "{:?}", node.subcommands);
    }

    /// The other half of the trade: a fully-described list is still taken
    /// in full. Verbatim from `docker __complete container ""`.
    #[test]
    fn a_fully_described_list_still_becomes_subcommands() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("container", prov.clone());
        populate_subcommands(
            &mut node,
            vec![
                described(
                    "attach",
                    "Attach local standard input, output, and error streams",
                ),
                described("commit", "Create a new image from a container's changes"),
                described("ls", "List containers"),
            ],
            &prov,
        );
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["attach", "commit", "ls"]);
        assert!(node.subcommands.iter().all(|c| c.summary.is_some()));
    }

    /// A fabricated node must never be marked heading-attested: it gates
    /// Tier B's `<tool> <path> --help`, and `docker run <image> --help`
    /// creates a container.
    #[test]
    fn completion_derived_children_are_never_heading_attested() {
        let prov = Provenance::single(Source::NativeDynamic {
            protocol: "test".to_string(),
        });
        let mut node = CommandNode::new("docker", prov.clone());
        populate_subcommands(
            &mut node,
            vec![described("run", "Create and run a new container")],
            &prov,
        );
        assert_eq!(node.subcommands.len(), 1);
        assert!(!node.subcommands[0].heading_attested);
    }

    #[test]
    fn subcommand_list_predicate_matches_the_measured_shapes() {
        // All described: docker's real subcommand lists, gh's whole tree.
        assert!(candidates_are_a_subcommand_list(&[
            described("pr", "Work with pull requests"),
            described("repo", "Manage repositories"),
        ]));
        // All bare: `docker __complete rm ""`, `docker __complete run ""`.
        assert!(!candidates_are_a_subcommand_list(&[
            bare("mandible-canary-1"),
            bare("hello-world:latest"),
        ]));
        // Mixed: `docker __complete context use ""`.
        assert!(!candidates_are_a_subcommand_list(&[
            described("rootless", "current"),
            bare("default"),
        ]));
        // Empty is not a subcommand list either — a real leaf, and there
        // is nothing to take from it.
        assert!(!candidates_are_a_subcommand_list(&[]));
    }

    #[test]
    fn depth_cap_stops_probing_without_erroring() {
        let tier = NativeTier::default();
        tier.set_protocol("mytool", Protocol::Cobra);
        let deep_words: Vec<String> = (0..DEPTH_CAP).map(|i| format!("level{i}")).collect();
        let node = tier.extract_cobra_node(
            Path::new("/bin/true"), // never spawned: depth cap short-circuits first
            "mytool",
            &deep_words,
            "leaf".to_string(),
        );
        assert!(node.subcommands.is_empty());
        assert!(node.flags().next().is_none());
    }

    #[test]
    fn echoed_root_candidates_are_not_trusted_as_real_structure() {
        // Simulated via the fingerprint cache directly, since we can't
        // spawn a real echoing binary here.
        let tier = NativeTier::default();
        let root_candidates = vec![described("a", "first"), described("b", "second")];
        let fp = fingerprint_candidates(&root_candidates);
        tier.remember_root_fingerprint("mytool", fp);
        assert_eq!(tier.remembered_root_fingerprint("mytool"), Some(fp));
    }

    /// A fixed [`CobraEvidence`] answer, for tests exercising `detect()`'s
    /// probing logic without a real cobra-marked binary on disk (see
    /// [`CobraEvidence`]).
    struct FixedEvidence(bool);
    impl CobraEvidence for FixedEvidence {
        fn speaks_cobra(&self, _tool: &ResolvedTool) -> bool {
            self.0
        }
    }

    #[test]
    fn detect_false_for_a_tool_that_understands_neither_protocol() {
        let tier = NativeTier::default();
        let tool = ResolvedTool {
            name: "sh".to_string(),
            path: Some(Path::new("/bin/sh").to_path_buf()),
            version: None,
        };
        // `/bin/sh` carries no cobra artifact marker, so this is refused
        // by the gate before any probe is attempted (production
        // `ArtifactEvidence`, via `NativeTier::default()`).
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn detect_sends_the_literal_dunder_complete_word_in_argv() {
        // This shim answers with a valid cobra response only when
        // invoked with `__complete`, so detection can only succeed if the
        // real argv was built correctly (AGENTS.md §3.1).
        //
        // Built with `FixedEvidence(true)`: this test is about what
        // happens after the gate passes, not the gate itself, which is
        // proven separately against the real production check.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cobrashim.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\ncase \"$1\" in\n  __complete) printf 'build\\tbuild the thing\\n:0\\n' ;;\n  *) echo 'no' ;;\nesac\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let tier =
            NativeTier::new_with_evidence(Arc::new(LiveProbe), Arc::new(FixedEvidence(true)));
        let tool = ResolvedTool {
            name: "cobrashim".to_string(),
            path: Some(path),
            version: None,
        };
        assert!(
            tier.detect(&tool),
            "cobra detection must send the literal `__complete` word"
        );
    }

    // --- the replay seam: real-argv tests against a `Transcript` ---

    fn exec_output(stdout: &str) -> crate::exec::ExecOutput {
        crate::exec::ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    /// Real argv, replayed: the subcommands probe renders to
    /// `["__complete", ""]`, the flags probe to `["__complete", "-"]`. A
    /// transcript keyed on exactly those two argvs must let `detect` and
    /// `extract_node` recover a cobra node through the tier's actual probe
    /// construction, with zero subprocesses.
    ///
    /// `FixedEvidence(true)` stands in for a real artifact-scan hit
    /// (`/replayed/cobratool` doesn't exist on disk) — this test is about
    /// probe/response plumbing downstream of the gate, not the gate itself.
    #[test]
    fn extract_node_replays_cobra_candidates_from_a_transcript_keyed_on_the_real_argv() {
        let transcript = crate::exec::Transcript::new([
            (
                vec!["__complete".to_string(), String::new()],
                exec_output("build\tBuild the thing\n:0\n"),
            ),
            (
                vec!["__complete".to_string(), "-".to_string()],
                exec_output("--all\tAll of it\n:0\n"),
            ),
        ]);
        let tier =
            NativeTier::new_with_evidence(Arc::new(transcript), Arc::new(FixedEvidence(true)));
        let tool = ResolvedTool {
            name: "cobratool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/cobratool")),
            version: None,
        };
        assert!(
            tier.detect(&tool),
            "transcript covers the real subcommands-probe argv"
        );
        let node = tier
            .extract_node(
                &tool,
                &["cobratool".to_string()],
                NodeHints {
                    heading_attested: true,
                },
            )
            .expect("detect having succeeded, extract_node must too");
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["build"], "{names:?}");
        assert!(node.flags().any(|f| f.long() == Some("all")));
    }

    /// The dynamic-argument guard through real argv construction (AGENTS.md
    /// §3.1), not just the parser behind it: a transcript keyed on the
    /// exact argv this tier builds for a leaf, answering with the byte
    /// shape `docker` really returns for a container-name completion. The
    /// node must come back with the leaf's flags and zero subcommands.
    #[test]
    fn extract_node_takes_no_subcommands_from_a_real_argv_leaf_returning_bare_names() {
        let transcript = crate::exec::Transcript::new([
            (
                vec!["__complete".to_string(), String::new()],
                exec_output("stop\tStop one or more running containers\n:4\n"),
            ),
            (
                vec!["__complete".to_string(), "stop".to_string(), String::new()],
                exec_output("mandible-canary-1\nmandible-canary-2\nadoring_hopper\n:4\n"),
            ),
            (
                vec![
                    "__complete".to_string(),
                    "stop".to_string(),
                    "-".to_string(),
                ],
                exec_output("--time\tSeconds to wait before killing\n:4\n"),
            ),
        ]);
        let tier =
            NativeTier::new_with_evidence(Arc::new(transcript), Arc::new(FixedEvidence(true)));
        let tool = ResolvedTool {
            name: "cobratool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/cobratool")),
            version: None,
        };
        assert!(tier.detect(&tool));
        let node = tier
            .extract_node(
                &tool,
                &["cobratool".to_string(), "stop".to_string()],
                NodeHints {
                    heading_attested: true,
                },
            )
            .expect("detect having succeeded, extract_node must too");
        assert!(
            node.subcommands.is_empty(),
            "container names reached the tree through the real argv path: {:?}",
            node.subcommands.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(
            node.flags().any(|f| f.long() == Some("time")),
            "the flags probe must keep working at a leaf"
        );
    }

    /// A transcript not covering the exact subcommands-probe argv must
    /// not be mistaken for a cobra-speaking tool — `detect` must come
    /// back `false`, not silently succeed with an empty detection.
    #[test]
    fn detect_is_false_against_a_transcript_missing_the_real_argv() {
        let transcript = crate::exec::Transcript::new([(
            // Deliberately the wrong argv: a bare `__complete` with no
            // trailing word at all, which this tier never sends (spec §6
            // rule 2a requires the trailing word be present, even empty).
            vec!["__complete".to_string()],
            exec_output("build\tBuild the thing\n:0\n"),
        )]);
        let tier =
            NativeTier::new_with_evidence(Arc::new(transcript), Arc::new(FixedEvidence(true)));
        let tool = ResolvedTool {
            name: "cobratool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/cobratool")),
            version: None,
        };
        assert!(
            !tier.detect(&tool),
            "a transcript miss must not be mistaken for a successful cobra detection"
        );
    }

    /// `FixedEvidence(false)` must refuse detection before the probe is
    /// consulted, regardless of what the transcript covers.
    #[test]
    fn detect_is_false_when_evidence_says_no_even_if_the_transcript_would_answer() {
        let transcript = crate::exec::Transcript::new([
            (
                vec!["__complete".to_string(), String::new()],
                exec_output("build\tBuild the thing\n:0\n"),
            ),
            (
                vec!["__complete".to_string(), "-".to_string()],
                exec_output("--all\tAll of it\n:0\n"),
            ),
        ]);
        let tier =
            NativeTier::new_with_evidence(Arc::new(transcript), Arc::new(FixedEvidence(false)));
        let tool = ResolvedTool {
            name: "cobratool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/cobratool")),
            version: None,
        };
        assert!(
            !tier.detect(&tool),
            "no cobra evidence must refuse detection even when the transcript covers a valid response"
        );
    }
}
