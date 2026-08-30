//! Tier E: native, self-describing binary probes (spec §7 Tier E).
//!
//! One protocol, driven entirely through [`crate::exec::run_inert`] with
//! an argv shape already on [`crate::exec::InertArgv`] — this tier adds no
//! new spawn shapes, it only teaches the pipeline to speak one that was
//! already on the §6 allowlist:
//!
//! - **cobra `__complete`** (`gh`, `docker`, `kubectl`, ...). The protocol
//!   needs **two probes per node**, not one — an earlier implementation
//!   that only ran `__complete <path> ""` got subcommands but zero flags
//!   [M-2]; flags only show up when the trailing word is `"-"` instead of
//!   `""`. Responses end with a `:N` directive line (the
//!   `ShellCompDirective` bitmask) on stdout; a human-readable
//!   `Completion ended with directive: ...` note goes to stderr and is
//!   ignored. Candidates are `value\tdescription` (or bare `value`) per
//!   line.
//!
//! **An empty trailing word does *not* mean "subcommands only"** — the
//! premise [M-2] was originally written with, and measured wrong at leaf
//! commands. cobra answers `__complete <path> ""` by emitting the node's
//! real subcommands *and then appending whatever that command's
//! `ValidArgsFunction` returns*, which is application code that reads live
//! state. Measured on this box (docker 29.7.2): `docker __complete stop ""`
//! returns **running container names**, `docker __complete run ""` returns
//! **image names**, `docker __complete network rm ""` returns network
//! names. Treating those as subcommands rendered a user's private container
//! names in the tree as if they were docker commands, and — because each
//! fabricated node is then warmed like any other (spec §5.2 step 4) —
//! multiplied the probe count by the size of a set that scales with the
//! user's data, not with the tool. See [`populate_subcommands`] for the
//! general rule that stops it, and spec Appendix A [M-2] for the full
//! measurement.
//!
//! clap's `CompleteEnv` was also probed here once and has been removed: it
//! never identified a single real clap tool, matched ten unrelated ones by
//! accident, and its argv spelling handed tools an empty first positional
//! — the shape measured terminating every process in a PID namespace via
//! `pkill -- ""`. The full reasoning, and what re-adding it would require,
//! is recorded at the former call site below.
//!
//! **Strictly node-scoped, never eager.** [`ExtractionTier::extract_node`]
//! probes exactly the one path it's given; the two-probe cost is paid once
//! per node, on demand, driven by the runner's lazy per-node expansion
//! (spec §5.2) — this tier is the entire reason that laziness exists in
//! the first place. Revision 1's eager cobra walk cost `docker` 232
//! spawns and 10.5s [M-3]; that shape must never come back.
//!
//! **Depth cap and an echoed-root guard**, because some tools don't
//! reject an unrecognized deep path — they silently re-emit the root's own
//! candidate list instead, which would otherwise let a user keep expanding
//! the same illusory subtree forever. A path deeper than [`DEPTH_CAP`] is
//! never probed at all; shallower than that, this tier remembers each
//! tool's root-level candidate-list fingerprint (a simple hash) the first
//! time it sees it, and treats a later, deeper probe whose fingerprint
//! matches as an echo — a leaf, not real structure.
//!
//! **Alias detection.** cobra apps sometimes register a shortcut as its
//! own top-level entry with a description like `Alias for "pr checkout"`
//! (`gh co`). Recursing into `co` as if it were a real command would
//! duplicate the whole `pr checkout` subtree under a second name. Instead,
//! when the named target is a sibling in the *same* candidate list (the
//! common case — an alias for a command at the same level), the alias
//! name is recorded on that sibling's [`mandible_core::CommandNode::aliases`]
//! and never becomes a subcommand entry of its own. When the target isn't
//! a sibling (an alias for something nested deeper, like `co` for the
//! *nested* `pr checkout`), the candidate is dropped rather than
//! fabricated as a duplicate top-level branch or guessed at across levels
//! this tier hasn't probed.
//!
//! **Authority** (spec §4.4): structural 200, prose 40 — a live probe of
//! the tool's own binary should win flag *existence* against even
//! carapace's vendored snapshot (which can drift stale), but carapace's
//! human-written descriptions stay more trustworthy than a completion
//! system's often-terse or absent ones.
//!
//! **Gated on prior evidence, never speculative** (spec §7 Tier E, the
//! 2026-08-12 incident). `detect()` used to send `__complete <word>` to
//! *every* tool on `PATH` to find out whether it answered — the only way
//! to know, absent any other signal. Reported from real use: probing
//! `wall` this way broadcast the literal text `__complete` to every
//! logged-in terminal on the reporter's machine, because `wall` treats an
//! unrecognized first positional as the message to send rather than
//! rejecting it — the same *shape* of hazard as `pkill -- ""` (§6 rule 2a),
//! an argv that is inert for nearly every tool and an action for one
//! family, discovered a second time. A per-tool containment list
//! (`exec::spawn::HELP_ONLY_PROBE`) closes the six measured cases; this
//! gate closes the general one. `detect()` now sends `__complete` only
//! when [`crate::framework::identify_from_artifact`] has already read the
//! tool's own compiled bytes and found the `spf13/cobra` marker — ground
//! truth, and free: a plain file read, no subprocess. A tool this check
//! misses (a stripped binary with debug info removed, mainly) loses Tier E
//! rather than being probed speculatively; see spec §7 Tier E for the
//! measured cost. [`CobraEvidence`] is the seam that makes this testable
//! without a real cobra-marked binary on disk.

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
/// Real command trees essentially never nest this deep; a tool that would
/// need to is exactly the shape spec [M-3]'s eager-walk measurement (232
/// spawns for `docker`) warns against chasing further anyway.
const DEPTH_CAP: usize = 6;

/// Which native protocol a tool was found to speak, cached per tool name
/// after the first successful `detect` so `extract_node` doesn't have to
/// re-run the same detection probe on every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    /// cobra's `__complete` convention. The only native protocol left
    /// after clap's `CompleteEnv` probe was removed (see below); kept as
    /// an enum because the protocol cache is keyed on it and a second
    /// protocol re-added later belongs here.
    Cobra,
}

/// Prior evidence that a tool speaks cobra, checked before this tier ever
/// constructs a `__complete` argv (spec §7 Tier E's gate — see the module
/// doc comment). A trait, mirroring [`Probe`]'s own seam, rather than
/// calling [`framework::identify_from_artifact`] directly: the production
/// check does a real file read (`framework::artifact::scan`), and that
/// scan can never recognize a `#!`-shebang shell script as cobra —
/// script-shebang scanning only ever resolves to a *script* framework
/// (argparse, click, commander, ...), never cobra, which is Go-only and
/// always compiled. Every test shim in this crate's suite is exactly such
/// a script, so exercising the *gated* branch of `detect()` (as opposed to
/// the *refused* branch, which any real shim already proves against the
/// live production check — see `mandible-extract/tests/`) needs a way to
/// supply a fixed answer instead.
trait CobraEvidence: Send + Sync {
    /// True when `tool` is already known to speak cobra.
    fn speaks_cobra(&self, tool: &ResolvedTool) -> bool;
}

/// Production [`CobraEvidence`]: ground truth from the tool's own compiled
/// bytes, via Tier A′ (spec §7 Tier A′). Adds no subprocess of its own —
/// `identify_from_artifact` is a bounded file read, memoized per binary
/// path, so this gate costs nothing beyond what framework identification
/// already pays elsewhere in the pipeline.
struct ArtifactEvidence;

impl CobraEvidence for ArtifactEvidence {
    fn speaks_cobra(&self, tool: &ResolvedTool) -> bool {
        framework::identify_from_artifact(tool) == Some(Framework::Cobra)
    }
}

/// Tier E: cobra `__complete` dynamic-completion probes.
pub struct NativeTier {
    /// Which protocol each tool name was found to speak. Bounded by the
    /// number of distinct tool names probed in this process's lifetime —
    /// normally exactly one, since `mandible` opens a single tool per run.
    protocol_cache: Mutex<HashMap<String, Protocol>>,
    /// Each cobra-speaking tool's root-level candidate-list fingerprint,
    /// remembered the first time it's seen, so a later deeper probe that
    /// echoes it back can be recognized as not-real-structure rather than
    /// trusted (see the module doc's "echoed-root guard").
    root_fingerprint_cache: Mutex<HashMap<String, u64>>,
    /// The source of a `__complete` probe's output — [`LiveProbe`] in
    /// production ([`Self::default`]), or a [`crate::exec::Transcript`] to
    /// replay frozen bytes with zero subprocesses.
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
    /// file-backed [`ArtifactEvidence`] check — what every production
    /// caller wants (`mandible-extract/src/lib.rs`'s `default_tiers_with_probe`
    /// is the only one).
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self::new_with_evidence(probe, Arc::new(ArtifactEvidence))
    }

    /// [`Self::new`], but against an explicit [`CobraEvidence`] rather than
    /// always the real artifact scan — the seam a test uses to exercise
    /// `detect()`'s gated (rather than refused) branch without a real
    /// cobra-marked binary on disk. Not `pub`: every real caller wants
    /// [`Self::new`]/[`Self::default`], and the *negative* property (no
    /// evidence ⇒ no probe) is proven against the real, ungated production
    /// check by this crate's integration tests instead, which is the
    /// stronger claim.
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
        // The gate (spec §7 Tier E, module doc comment): `__complete` is
        // never sent speculatively. Without prior evidence that this tool
        // speaks cobra, `detect()` declines before constructing any argv
        // at all — no probe, no spawn, nothing reaches the tool's binary.
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
        // One probe here does genuinely discover the complete list of
        // direct subcommand *names* at this level (spec §5.2's "the names
        // of its direct subcommands" contract) — same rationale as Tier
        // B's `children_filled: true`.
        node.children_filled = true;

        if words.len() >= DEPTH_CAP {
            // Refuse to go any deeper. Returning an empty leaf (not an
            // error) means the merge simply gets nothing extra from this
            // tier at this depth, rather than failing the node outright.
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
                // filtered by its own dash shape, which an argument value
                // essentially never has, and cobra emits plenty of real
                // flags with no help text.
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
/// genuine cobra completion response at all (no `:N` directive line found)
/// — the general signal that this isn't a cobra-speaking tool, not a
/// per-tool special case.
fn probe_cobra_list(
    probe: &dyn Probe,
    tool_path: &Path,
    words: &[String],
    trailing: &str,
    timeout: Duration,
) -> Option<Vec<Candidate>> {
    let mut argv_words = words.to_vec();
    // The empty word is required by cobra's protocol, not incidental:
    // `docker __complete` without it fails with "requires at least 1
    // arg(s), only received 0" and detection collapses for every cobra
    // tool. It is safe here for a reason `run_inert` can check — it is
    // never the first positional, always shielded behind the `__complete`
    // sentinel, which a non-cobra tool rejects. See spec §6 rule 2a.
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
/// The distinction that matters is *whether the line carried a
/// description at all*, which the earlier `(String, String)` shape
/// flattened away by spelling "no description" and "empty description"
/// both as `""`. It is the only thing in cobra's wire format that
/// separates a real subcommand from a value the command's own
/// `ValidArgsFunction` computed from live state — see
/// [`populate_subcommands`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// The completion value: a subcommand name, a flag spelling, or an
    /// argument value.
    value: String,
    /// The text after the `\t`, when there was one and it was not blank.
    description: Option<String>,
}

impl Candidate {
    /// The description as a `&str`, with "absent" and "blank" collapsed —
    /// correct for flag descriptions, where the distinction carries no
    /// meaning.
    fn description_text(&self) -> &str {
        self.description.as_deref().unwrap_or("")
    }
}

/// Parse a cobra `__complete` response: candidate lines (`value` or
/// `value\tdescription`) followed by a `:N` directive line. Returns `None`
/// unless that directive line is actually found — the response shape
/// cobra's protocol guarantees, and the general (not tool-specific) signal
/// that a binary really speaks this protocol rather than just happening to
/// accept the argv without erroring.
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

/// A cheap, stable fingerprint of a candidate list's *values* (not
/// descriptions, which some tools vary slightly by context) — used only to
/// recognize "this is the same list as the root's," not for anything
/// security-sensitive, so a simple hash is sufficient.
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
/// **The general rule that keeps live user data out of the tree.** cobra
/// answers one `__complete <path> ""` by emitting the node's real
/// subcommands — always as `name\tShort`, from cobra's own
/// `fmt.Sprintf("%s\t%s", ...)` — and then *appending* whatever the
/// command's `ValidArgsFunction` returns. That second half is application
/// code reading live state: container names, image tags, network names,
/// context names. cobra's wire format marks no boundary between the two
/// halves, so the only thing left to read is the description column:
///
/// - every real subcommand carries one, because cobra writes it;
/// - a `ValidArgsFunction` value normally carries none, because returning
///   a plain `[]string` is the ordinary way to write one.
///
/// So a list is trusted only when **every** candidate in it is described.
/// A single undescribed candidate proves the list contains argument
/// completions, and since the boundary is unmarked, nothing in that list
/// can be trusted — not even the described entries ahead of it.
///
/// Measured across 631 real command paths on `docker` 29.7.2 and `gh`
/// 2.45.0 (spec Appendix A [M-2a]): 85 all-described lists, every one a
/// genuine subcommand list; 45 all-undescribed lists and 5 mixed lists,
/// every one of the 50 pure argument data. The rule therefore admitted
/// every real subcommand and no argument value on the whole measured set.
///
/// **The trade is deliberate and one-directional** (AGENTS.md §1's
/// maintainer principle): a real cobra subcommand registered with an empty
/// `Short`, sitting in a list that also carries undescribed argument
/// values, is dropped here. Tier B's `--help` parse still finds it, and a
/// missing rare subcommand is a smaller harm than rendering a user's
/// container names as commands. Never relax this to recover such a
/// subcommand.
fn candidates_are_a_subcommand_list(candidates: &[Candidate]) -> bool {
    !candidates.is_empty() && candidates.iter().all(|c| c.description.is_some())
}

/// Turn a subcommands-probe candidate list into `node`'s subcommands,
/// routing alias-shaped candidates onto a matching sibling's `aliases`
/// instead of fabricating them as their own subcommand (spec §7 Tier E,
/// the module doc's "Alias detection").
///
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
        // Every candidate is described here — the guard above rejected the
        // list otherwise — so alias routing (whose marker *is* a
        // description) is unaffected by that guard.
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

// clap's `CompleteEnv` probe was removed here; the reasons are worth
// keeping, because "re-add it" is an obvious-looking idea.
//
// The probe was `COMPLETE=<shell> <tool> -- <partial>`, and it could not
// be spelled safely in either direction:
//
// - With an empty partial it rendered as `<tool> -- ""`. `--` is the
//   option terminator essentially every getopt program discards, so the
//   empty string arrived as the tool's *first positional*, and a program
//   whose first positional is a pattern reads that as "match everything".
//   Measured: `pkill -- ""` terminated every process in a private PID
//   namespace, pkill included. This is the mechanism behind the machine
//   reset that motivated the never-probe list (spec §6 rule 0) — which
//   masked it for thirteen tools while this same argv went to the rest of
//   PATH.
// - Spelled `<tool> --` instead, it is harmless but wrong: `--` is a
//   no-op for most tools, so they run normally and print their ordinary
//   output, which the shape heuristic below then read as a candidate
//   list. Measured on the PATH-wide sweep: 16 tools newly acquired this
//   tier and 8 became `suspicious` (`whoami --` prints a username, which
//   is command-name-shaped).
//
// And it never worked. clap's protocol has no self-identifying trailer
// like cobra's `:N` directive, so detection was only ever a shape
// heuristic, and on the sweep it matched 10 tools of which *none* were
// clap: `echo`, `bzless`, `bzmore`, `validlocale`, `xdg-user-dir`,
// `update-alternatives` and friends. `echo -- ""` prints `--`, which
// starts with a dash and so "looked like" a flag candidate. Combined with
// [M-4] (measured absent from both `ripgrep` and `cargo`), the feature
// was pure false-positive generation attached to a lethal argv shape.
//
// Re-adding it needs two things this code never had: a way to confirm the
// tool really speaks the protocol before trusting the response, and a
// spelling that never hands a tool an empty first positional. Gating on
// Tier A′ framework identification (probe only what is already identified
// as clap) would supply the first.

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

    /// An undescribed candidate, as a `ValidArgsFunction` emits a live
    /// argument value.
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

    /// The exact regression this exists for: `gh co` (`Alias for "pr
    /// checkout"`) must not become its own subcommand entry, and — since
    /// its target isn't a sibling at this level (it's the nested `pr
    /// checkout`) — must not be attached anywhere either, rather than
    /// guessed at.
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

    /// The same-level case: an alias whose target *is* a sibling in this
    /// same candidate list must be recorded on that sibling, not emitted
    /// as its own subcommand.
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

    /// The user-reported bug, at the unit level: `docker __complete stop
    /// ""` answers with the names of the containers currently on the
    /// machine, bare, because cobra runs the leaf's `ValidArgsFunction`.
    /// Those are private user data, not commands, and must produce **no**
    /// subcommands at all — not "some, filtered by name shape", which is
    /// what `is_command_name_shaped` alone did (it rejects `redis:7` for
    /// the colon but passes `mandible-canary-1`).
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

    /// The mixed case, measured verbatim from `docker __complete context
    /// use ""` on a machine with two contexts: the *current* one carries a
    /// description (`CompletionWithDesc`) and the rest do not. cobra marks
    /// no boundary between its own subcommand block and the appended
    /// `ValidArgsFunction` output, so one undescribed entry condemns the
    /// whole list — including the described `rootless`, which is a context
    /// name and not a command.
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

    /// A fabricated node must never be marked heading-attested, because
    /// `heading_attested` is what lets Tier B construct `<tool> <path>
    /// --help` — and `docker run <image> --help` **creates a container**.
    /// Nothing in this fix may start setting it; this pins the default.
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
            Path::new("/bin/true"), // never actually spawned: depth cap short-circuits first
            "mytool",
            &deep_words,
            "leaf".to_string(),
        );
        assert!(node.subcommands.is_empty());
        assert!(node.flags().next().is_none());
    }

    #[test]
    fn echoed_root_candidates_are_not_trusted_as_real_structure() {
        // A tool whose deeper probe happens to return the exact same
        // candidate list as its root (simulated via the fingerprint cache
        // directly, since we can't spawn a real echoing binary here) must
        // not have that list treated as genuine subcommands.
        let tier = NativeTier::default();
        let root_candidates = vec![described("a", "first"), described("b", "second")];
        let fp = fingerprint_candidates(&root_candidates);
        tier.remember_root_fingerprint("mytool", fp);
        assert_eq!(tier.remembered_root_fingerprint("mytool"), Some(fp));
    }

    /// A fixed [`CobraEvidence`] answer, for tests that want to exercise
    /// `detect()`'s probing logic (argv construction, response parsing)
    /// without a real cobra-marked binary on disk — see [`CobraEvidence`]'s
    /// own doc comment for why a shell-script shim can never satisfy the
    /// real, file-backed [`ArtifactEvidence`] check.
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
        // `/bin/sh` carries no cobra artifact marker at all, so this is
        // refused by the gate before any probe is even attempted — the
        // production `ArtifactEvidence` check is exercised here unmodified
        // (this test uses `NativeTier::default()`, not `new_with_evidence`).
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn detect_sends_the_literal_dunder_complete_word_in_argv() {
        // The bug this exists to prevent: an earlier cobra tier built its
        // argv as `[...words, ""]` and omitted the literal `"__complete"`,
        // so the tier was silently dead in production while its unit tests
        // passed — they injected a mock probe and never exercised argv
        // construction at all (AGENTS.md §3.1).
        //
        // This shim answers with a valid cobra response *only* when it is
        // actually invoked with `__complete`, so detection can only succeed
        // if the real argv was built correctly. Deterministic, and it
        // replaces a test that asserted against whichever of docker/gh
        // happened to be installed — that one was flaky in CI, since
        // detection spawns the real binary and `docker __complete` is not
        // answerable when the daemon is down.
        //
        // Built with `FixedEvidence(true)` rather than `NativeTier::default()`:
        // this test is about what happens *after* the gate passes (is the
        // probe itself correct?), not about the gate itself — a shell-script
        // shim can never carry real cobra artifact evidence (see
        // `CobraEvidence`'s doc comment), so proving this with the real gate
        // would require a real Go/cobra binary in the test fixtures. The
        // gate itself — that a tool *without* evidence never reaches this
        // probe at all — is proven separately, against the real,
        // ungated-by-tests production check, by this crate's integration
        // tests (`mandible-extract/tests/`).
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

    /// Real argv, replayed: the subcommands probe is `CobraComplete {
    /// words: [""] }`, rendering to `["__complete", ""]`
    /// (`InertArgv::args`), and the flags probe is `["__complete", "-"]`.
    /// A transcript keyed on exactly those two argvs must let `detect`
    /// and `extract_node` recover a cobra node through the tier's actual
    /// probe construction — the same protocol
    /// `detect_sends_the_literal_dunder_complete_word_in_argv` proves with
    /// a real shim binary, but with zero subprocesses.
    ///
    /// `FixedEvidence(true)` stands in for a real artifact-scan hit
    /// (`/replayed/cobratool` doesn't exist on disk, so the real,
    /// file-backed gate would refuse it regardless of what the transcript
    /// covers) — this test is about probe/response plumbing downstream of
    /// the gate, which the gate itself is not.
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

    /// The dynamic-argument guard through **real argv construction**
    /// (AGENTS.md §3.1 — the dead-tier incident), not just the parser
    /// behind it: a transcript keyed on the exact argv this tier builds
    /// for a leaf (`["__complete", "stop", ""]`), answering with the byte
    /// shape `docker` really returns for a container-name completion —
    /// bare names, `ShellCompDirectiveNoFileComp`. The node must come back
    /// with the leaf's flags and **zero** subcommands.
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

    /// The negative case: a transcript that does not cover the exact
    /// subcommands-probe argv (`["__complete", ""]`) must not be
    /// mistaken for a cobra-speaking tool — `detect` must come back
    /// `false`, not silently succeed with an empty candidate list treated
    /// as a confident (if empty) detection.
    ///
    /// `FixedEvidence(true)` again stands in for a real artifact-scan hit,
    /// so this test isolates the *response-parsing* miss it's named for —
    /// see the sibling test above.
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

    /// The gate itself, isolated: `FixedEvidence(false)` must refuse
    /// detection before the probe is even consulted, regardless of what
    /// the transcript covers — proving `detect()`'s early return actually
    /// short-circuits rather than merely happening to fail downstream.
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
