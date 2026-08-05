//! `mantui-extract`: the tiered CLI extraction pipeline.
//!
//! See spec §5-§7. Every tier normalizes into [`mantui_core::CommandNode`]
//! via the [`ExtractionTier`] trait; the [`Runner`] drives the configured
//! tiers and merges their output with [`mantui_core::merge_nodes`].
//!
//! `exec/` is the only module in this crate — and the only module in the
//! whole workspace — permitted to use `std::process`. See `exec`'s
//! documentation and `tests/no_process_outside_exec.rs`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod exec;

mod errors;
mod resolve;
mod runner;
mod tier;

#[cfg(feature = "known-specs")]
pub mod known_specs;

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
pub use resolve::{resolve_tool, ResolvedTool};
pub use runner::{ExtractionResult, Runner, TierStatus};
pub use tier::ExtractionTier;

/// Build the default set of tiers for this batch: Tier A (`known_specs`)
/// then Tier B (`help_text`), in cost-attempt order (spec §7) — Tier A is
/// zero-spawn and covers the catalog; Tier B costs 1-2 spawns but exists
/// for every tool everywhere. Whichever of the two features are enabled
/// contributes its tier; conflict resolution between them (when both
/// contribute the same node) is by [`mantui_core::Authority`], not attempt
/// order.
// `vec![]` can't express the cfg-gated pushes below (each tier only
// exists to push when its feature is enabled).
#[allow(clippy::vec_init_then_push)]
pub fn default_tiers() -> Vec<Box<dyn ExtractionTier>> {
    #[allow(unused_mut)]
    let mut tiers: Vec<Box<dyn ExtractionTier>> = Vec::new();
    #[cfg(feature = "known-specs")]
    tiers.push(Box::new(known_specs::CarapaceTier));
    #[cfg(feature = "help-text")]
    tiers.push(Box::new(help_text::HelpTextTier));
    tiers
}
