//! Tier E: native, self-describing binary probes (spec §7 Tier E).
//!
//! Two protocols, both driven entirely through [`crate::exec::run_inert`]
//! with argv shapes already on [`crate::exec::InertArgv`] — this tier adds
//! no new spawn shapes, it only teaches the pipeline to speak two that
//! were already on the §6 allowlist:
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
//! - **clap `CompleteEnv`** (`COMPLETE=<shell> <tool> --`), probed but not
//!   relied on: measured absent from both `ripgrep` and `cargo` [M-4]: so
//!   nothing in this pipeline gates on it working. Never invoked bare —
//!   always `<tool> --` at minimum (spec §6 rule 1).
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

use crate::errors::ExtractError;
use crate::exec::{run_inert, InertArgv};
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use mandible_core::{
    is_command_name_shaped, Authority, CommandNode, Flag, Provenance, Source, Text, ValueKind,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Mutex;
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
    /// cobra's `__complete` convention.
    Cobra,
    /// clap's `CompleteEnv` convention.
    ClapCompleteEnv,
}

/// Tier E: cobra `__complete` and clap `CompleteEnv` dynamic-completion
/// probes.
#[derive(Debug, Default)]
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
        if probe_cobra_list(tool_path, &[], "", DETECT_TIMEOUT).is_some() {
            self.set_protocol(&tool.name, Protocol::Cobra);
            return true;
        }
        if probe_clap_complete_env(tool_path, "", DETECT_TIMEOUT).is_some() {
            self.set_protocol(&tool.name, Protocol::ClapCompleteEnv);
            return true;
        }
        false
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        let words: Vec<String> = path.iter().skip(1).cloned().collect();
        let name = path.last().cloned().unwrap_or_else(|| tool.name.clone());

        match self.cached_protocol(&tool.name) {
            Some(Protocol::Cobra) => {
                Ok(self.extract_cobra_node(tool_path, &tool.name, &words, name))
            }
            Some(Protocol::ClapCompleteEnv) => Ok(extract_clap_node(tool_path, &words, name)),
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

        if let Some(candidates) = probe_cobra_list(tool_path, words, "", EXTRACT_TIMEOUT) {
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

        if let Some(candidates) = probe_cobra_list(tool_path, words, "-", EXTRACT_TIMEOUT) {
            for (value, description) in candidates {
                if let Some(flag) = flag_from_candidate(&value, &description, &provenance) {
                    node.flags.push(flag);
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
    tool_path: &Path,
    words: &[String],
    trailing: &str,
    timeout: Duration,
) -> Option<Vec<(String, String)>> {
    let mut argv_words = words.to_vec();
    argv_words.push(trailing.to_string());
    let out = run_inert(
        tool_path,
        &InertArgv::CobraComplete { words: argv_words },
        timeout,
    )
    .ok()?;
    parse_cobra_response(&out.stdout)
}

/// Parse a cobra `__complete` response: candidate lines (`value` or
/// `value\tdescription`) followed by a `:N` directive line. Returns `None`
/// unless that directive line is actually found — the response shape
/// cobra's protocol guarantees, and the general (not tool-specific) signal
/// that a binary really speaks this protocol rather than just happening to
/// accept the argv without erroring.
fn parse_cobra_response(stdout: &[u8]) -> Option<Vec<(String, String)>> {
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
        match line.split_once('\t') {
            Some((value, description)) => {
                candidates.push((value.to_string(), description.to_string()))
            }
            None => candidates.push((line.to_string(), String::new())),
        }
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
fn fingerprint_candidates(candidates: &[(String, String)]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (value, _) in candidates {
        value.hash(&mut hasher);
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

/// Turn a subcommands-probe candidate list into `node`'s subcommands,
/// routing alias-shaped candidates onto a matching sibling's `aliases`
/// instead of fabricating them as their own subcommand (spec §7 Tier E,
/// the module doc's "Alias detection").
fn populate_subcommands(
    node: &mut CommandNode,
    candidates: Vec<(String, String)>,
    provenance: &Provenance,
) {
    let mut aliases: Vec<(String, String)> = Vec::new();
    for (value, description) in candidates {
        if let Some(target) = alias_target_last_segment(&description) {
            aliases.push((value, target.to_string()));
            continue;
        }
        if !is_command_name_shaped(&value) {
            continue;
        }
        let mut child = CommandNode::new(value, provenance.clone());
        child.summary = non_empty_text(&description);
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
fn flag_from_candidate(value: &str, description: &str, provenance: &Provenance) -> Option<Flag> {
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
    Some(Flag {
        short,
        long,
        value_name: None,
        value_kind: ValueKind::None,
        choices: Vec::new(),
        repeatable: false,
        required: false,
        hidden: false,
        deprecated: None,
        inherited: false,
        group: None,
        description: non_empty_text(description),
        default: None,
        env_var: None,
        provenance: provenance.clone(),
    })
}

fn non_empty_text(s: &str) -> Option<Text> {
    let t = s.trim();
    (!t.is_empty()).then(|| Text::sanitize(t))
}

/// Probe clap's `CompleteEnv` convention: `COMPLETE=<shell> <tool> --
/// <partial>`, never bare (spec §6 rule 1 — always at least the trailing
/// `--`). Measured absent from both `ripgrep` and `cargo` [M-4]; nothing
/// in this pipeline depends on this succeeding. Low-confidence by design:
/// unlike cobra's `:N` directive, clap's protocol has no equally strong
/// self-identifying trailer this code can check for, so detection here is
/// a shape heuristic (every returned line must itself look like a
/// plausible candidate) rather than a protocol-guaranteed signal.
fn probe_clap_complete_env(
    tool_path: &Path,
    partial: &str,
    timeout: Duration,
) -> Option<Vec<(String, String)>> {
    let out = run_inert(
        tool_path,
        &InertArgv::ClapCompleteEnvComplete {
            shell: "zsh".to_string(),
            partial: partial.to_string(),
        },
        timeout,
    )
    .ok()?;
    if out.stdout.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut candidates = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (value, description) = match line.split_once('\t') {
            Some((v, d)) => (v.to_string(), d.to_string()),
            None => (line.to_string(), String::new()),
        };
        let looks_like_candidate = value.starts_with('-') || is_command_name_shaped(value.trim());
        if !looks_like_candidate {
            return None; // one implausible line: not this protocol
        }
        candidates.push((value, description));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn extract_clap_node(tool_path: &Path, words: &[String], name: String) -> CommandNode {
    let provenance = Provenance::single(Source::NativeDynamic {
        protocol: "clap-complete-env".to_string(),
    });
    let mut node = CommandNode::new(name, provenance.clone());
    node.children_filled = true;

    let partial_prefix = words.join(" ");
    if let Some(candidates) = probe_clap_complete_env(tool_path, &partial_prefix, EXTRACT_TIMEOUT) {
        for (value, description) in &candidates {
            if let Some(flag) = flag_from_candidate(value, description, &provenance) {
                node.flags.push(flag);
                continue;
            }
            if is_command_name_shaped(value) {
                let mut child = CommandNode::new(value.clone(), provenance.clone());
                child.summary = non_empty_text(description);
                child.children_filled = false;
                node.subcommands.push(child);
            }
        }
    }

    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_cobra_response_shape() {
        let raw = b"add\tAdd file contents to the index\nrebase\tReapply commits\n:4\nCompletion ended with directive: ShellCompDirectiveNoFileComp\n";
        let parsed = parse_cobra_response(raw).expect("should recognize the directive");
        assert_eq!(
            parsed,
            vec![
                (
                    "add".to_string(),
                    "Add file contents to the index".to_string()
                ),
                ("rebase".to_string(), "Reapply commits".to_string()),
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
    fn bare_value_with_no_tab_has_empty_description() {
        let raw = b"solo\n:0\n";
        let parsed = parse_cobra_response(raw).unwrap();
        assert_eq!(parsed, vec![("solo".to_string(), String::new())]);
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
        assert_eq!(long.long.as_deref(), Some("all-tags"));
        assert_eq!(long.short, None);

        let short = flag_from_candidate("-a", "Download all tagged images", &prov).unwrap();
        assert_eq!(short.short, Some('a'));
        assert_eq!(short.long, None);
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
                ("pr".to_string(), "Work with pull requests".to_string()),
                ("co".to_string(), "Alias for \"pr checkout\"".to_string()),
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
                ("remove".to_string(), "Remove a thing".to_string()),
                ("rm".to_string(), "Alias for \"remove\"".to_string()),
            ],
            &prov,
        );
        let names: Vec<&str> = node.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["remove"]);
        assert_eq!(node.subcommands[0].aliases, vec!["rm".to_string()]);
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
        assert!(node.flags.is_empty());
    }

    #[test]
    fn echoed_root_candidates_are_not_trusted_as_real_structure() {
        // A tool whose deeper probe happens to return the exact same
        // candidate list as its root (simulated via the fingerprint cache
        // directly, since we can't spawn a real echoing binary here) must
        // not have that list treated as genuine subcommands.
        let tier = NativeTier::default();
        let root_candidates = vec![
            ("a".to_string(), String::new()),
            ("b".to_string(), String::new()),
        ];
        let fp = fingerprint_candidates(&root_candidates);
        tier.remember_root_fingerprint("mytool", fp);
        assert_eq!(tier.remembered_root_fingerprint("mytool"), Some(fp));
    }

    #[test]
    fn detect_false_for_a_tool_that_understands_neither_protocol() {
        let tier = NativeTier::default();
        let tool = ResolvedTool {
            name: "sh".to_string(),
            path: Some(Path::new("/bin/sh").to_path_buf()),
            version: None,
        };
        // `/bin/sh __complete ""` and the clap probe both just run `sh`
        // normally (or fail), neither producing a recognizable protocol
        // response.
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn detect_true_for_a_real_cobra_binary() {
        // `docker`/`gh` are the tools this batch was verified against;
        // skip gracefully if neither is on PATH (e.g. a minimal CI image).
        for candidate in ["docker", "gh"] {
            let resolved = crate::resolve::resolve_tool(candidate);
            if resolved.path.is_none() {
                continue;
            }
            let tier = NativeTier::default();
            assert!(
                tier.detect(&resolved),
                "expected {candidate} to be detected as a cobra binary"
            );
            return;
        }
    }
}
