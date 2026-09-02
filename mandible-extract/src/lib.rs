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
// `#[allow(unsafe_code)]` sites — `exec/spawn.rs`'s `pre_exec` + `setsid`
// call (spec §6 rule 6) and `exec/containment.rs`'s fd reconstruction
// across `unshare` + re-exec. Every other item still carries
// `#![forbid(unsafe_code)]`; `deny` here lets those two exceptions exist
// without disabling the lint elsewhere in this crate.
#![deny(unsafe_code)]
// Size ceilings; thresholds live in workspace `clippy.toml`.
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
/// driven by [`exec::LiveProbe`]. See [`default_tiers_with_probe`] for the
/// replay-seam variant driving the same tiers from frozen bytes instead.
pub fn default_tiers() -> Vec<Box<dyn ExtractionTier>> {
    default_tiers_with_probe(Arc::new(exec::LiveProbe))
}

/// [`default_tiers`], but every probing tier is built against `probe`
/// instead of always [`exec::LiveProbe`]. Tier B leads (dispatched on the
/// framework identified by Tier A′, spec §7), costing 1-2 spawns but
/// existing for every tool; Tier C generates and parses a completion
/// script; Tier E speaks a tool's own dynamic completion protocol; Tier F
/// is a local file read, attempted last, never probing anything. Tier D
/// (man pages) remains unimplemented. Whichever features are enabled
/// contributes its tier; conflicts resolve by [`mandible_core::Authority`],
/// not attempt order.
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
