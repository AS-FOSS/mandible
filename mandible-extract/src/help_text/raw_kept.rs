//! The verbatim view over probes a fill already ran (spec §11).

use super::{raw_help_with_probe, LiveProbe};
use crate::errors::ExtractError;
use crate::exec::{KeptProbes, Probe, Recorder, Replay};
use crate::resolve::ResolvedTool;
use crate::tier::NodeHints;
use mandible_core::Text;

/// [`super::raw_help`] over probes already kept, running nothing.
///
/// `kept` lists the logs to answer from, the node's own first. The
/// not-attested fallback reads the root's log, so callers add the logs of
/// the node's ancestors after it. `None` means a probe the view needs was
/// not kept, and the caller fetches instead.
pub fn raw_help_kept(
    kept: &[&KeptProbes],
    tool: &ResolvedTool,
    path: &[String],
    hints: NodeHints,
) -> Option<Result<(Vec<Text>, String), ExtractError>> {
    let replay = Replay::new(kept);
    let shown = raw_help_with_probe(&replay, tool, path, hints);
    (!replay.missed()).then_some(shown)
}

/// Run the probes the verbatim view needs for `path` and keep them.
pub fn probe_raw(tool: &ResolvedTool, path: &[String], hints: NodeHints) -> KeptProbes {
    crate::exec::within_read(|| probe_raw_with(&LiveProbe, tool, path, hints)).0
}

/// [`probe_raw`] against an explicit [`Probe`].
pub fn probe_raw_with(
    probe: &dyn Probe,
    tool: &ResolvedTool,
    path: &[String],
    hints: NodeHints,
) -> KeptProbes {
    let recorder = Recorder::new(probe);
    let _ = raw_help_with_probe(&recorder, tool, path, hints);
    recorder.finish()
}
