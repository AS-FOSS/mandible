//! `mandible-extract`: the tiered CLI extraction pipeline.
//!
//! See spec §5-§7. Every tier normalizes into [`mandible_core::CommandNode`]
//! via the [`ExtractionTier`] trait; the [`Runner`] drives the configured
//! tiers and merges their output with [`mandible_core::merge_nodes`].
//!
//! `exec/` is the only module in this crate — and the only module in the
//! whole workspace — permitted to use `std::process`. See `exec`'s
//! documentation and `tests/no_process_outside_exec.rs`.

// `deny`, not `forbid`: this crate carries exactly two scoped
// `#[allow(unsafe_code)]` sites. `exec/spawn.rs`'s probe-spawning function
// has one, for the `pre_exec` + `setsid` call that gives every probe a new
// *session* (not just a new process group) so a descendant cannot reopen
// the controlling terminal via `/dev/tty` (see that function's doc comment,
// and spec §6 rule 6). `exec/containment.rs` has the other, for
// reconstructing a `File` from a raw fd number a contained sweep inherits
// across `unshare` + re-exec (see that module's `write_scoreboard` and
// `secured_scoreboard_file` doc comments). Every other item in this crate —
// and every other crate in the workspace — still carries
// `#![forbid(unsafe_code)]`; `deny` here is what lets those two exceptions
// exist without silently disabling the lint everywhere else in this crate
// too.
#![deny(unsafe_code)]
// Size ceilings. Both lints are opt-in; the thresholds live in the
// workspace `clippy.toml`, and CI's `-D warnings` makes them gates.
#![warn(clippy::too_many_lines)]
#![warn(clippy::cognitive_complexity)]
#![warn(missing_docs)]

pub mod exec;

mod errors;
mod resolve;
mod runner;
mod tier;

pub mod framework;

#[cfg(feature = "help-text")]
pub mod help_text;

#[cfg(feature = "completion-script")]
pub mod completion_script;

#[cfg(feature = "manpage")]
pub mod manpage;

#[cfg(feature = "native")]
pub mod native;

pub mod overrides;

pub use errors::ExtractError;
pub use exec::is_help_only_probe;
pub use resolve::{
    discover_path_siblings, discover_path_siblings_in, resolve_tool, PathSibling, ResolvedTool,
};
pub use runner::{ExtractionResult, FillResult, Runner, TierStatus};
pub use tier::{ExtractionTier, NodeHints};

use std::sync::Arc;

/// Build the default set of tiers: B (`help_text`), C (`completion_script`),
/// E (`native`), F (`overrides`), in cost-attempt order (spec §7), each
/// probing tier driven by [`exec::LiveProbe`] — the real pipeline. See
/// [`default_tiers_with_probe`] for the replay-seam variant (batch:
/// rework/regression-net-and-recall) that lets a caller (a future corpus
/// runner) drive the same tiers from frozen bytes instead.
pub fn default_tiers() -> Vec<Box<dyn ExtractionTier>> {
    default_tiers_with_probe(Arc::new(exec::LiveProbe))
}

/// [`default_tiers`], but every probing tier is built against `probe`
/// instead of always [`exec::LiveProbe`]. Revision 3 deleted Tier A (the
/// vendored catalog, spec §7 "Tier A — REMOVED"): Tier B now leads,
/// dispatched on the framework identified by Tier A′ (spec §7 Tier A′), and
/// costs 1-2 spawns but exists for every tool everywhere; Tier C generates
/// and parses a completion script; Tier E speaks a tool's own dynamic
/// completion protocol; Tier F is a local file read, attempted last since
/// most tools have no override at all and never probes anything (spec §6
/// machinery doesn't apply to it, so it takes no probe). Tier D (man pages)
/// remains unimplemented (deferred entirely, per spec's roadmap). Whichever
/// features are enabled contributes its tier; conflict resolution between
/// tiers (when more than one contributes the same node) is by
/// [`mandible_core::Authority`], not attempt order.
// `vec![]` can't express the cfg-gated pushes below (each tier only
// exists to push when its feature is enabled).
#[allow(clippy::vec_init_then_push, clippy::redundant_clone, unused_variables)]
pub fn default_tiers_with_probe(probe: Arc<dyn exec::Probe>) -> Vec<Box<dyn ExtractionTier>> {
    #[allow(unused_mut)]
    let mut tiers: Vec<Box<dyn ExtractionTier>> = Vec::new();
    #[cfg(feature = "help-text")]
    tiers.push(Box::new(help_text::HelpTextTier::new(probe.clone())));
    #[cfg(feature = "completion-script")]
    tiers.push(Box::new(completion_script::CompletionScriptTier::new(
        probe.clone(),
    )));
    #[cfg(feature = "native")]
    tiers.push(Box::new(native::NativeTier::new(probe.clone())));
    tiers.push(Box::new(overrides::OverridesTier));
    tiers
}
