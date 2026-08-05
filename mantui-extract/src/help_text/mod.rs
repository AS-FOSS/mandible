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

mod grammar;
mod sections;

use crate::errors::ExtractError;
use crate::exec::{run_inert, InertArgv};
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use mantui_core::{Authority, CommandNode, Provenance, Source, Text};
use std::path::Path;
use std::time::Duration;

/// Wall-clock cap for an `extract_node` probe (spec §6 rule 4).
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(10);

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
        Ok(build_node(&node_name, &raw))
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

/// Prefer stdout when both streams are non-empty (spec §7 Tier B).
fn pick_stream(stdout: &[u8], stderr: &[u8]) -> String {
    if !stdout.is_empty() {
        String::from_utf8_lossy(stdout).into_owned()
    } else {
        String::from_utf8_lossy(stderr).into_owned()
    }
}

fn build_node(name: &str, raw: &str) -> CommandNode {
    let parsed = sections::parse(raw);
    let provenance = Provenance::with_confidence(Source::HelpText, parsed.confidence);

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
        // Tier B recovers direct subcommand *names* (as stubs) but does
        // not recurse into them itself — lazy expansion is the runner's
        // job (spec §5.2) — so this node's own children are never
        // "known-complete" from Tier B's point of view.
        children_filled: false,
        group: None,
        provenance,
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

    #[test]
    fn build_node_from_tar_fixture_has_flags_and_confidence() {
        let raw = fixture("tar_help.stdout");
        let node = build_node("tar", &raw);
        assert_eq!(node.name, "tar");
        assert!(!node.flags.is_empty());
        assert!(node.provenance.confidence.unwrap() > 0.0);
        assert!(!node.children_filled);
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
