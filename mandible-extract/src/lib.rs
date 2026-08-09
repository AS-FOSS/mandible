//! `mandible-extract`: the tiered CLI extraction pipeline.
//!
//! See spec §5-§7. Every tier normalizes into [`mandible_core::CommandNode`]
//! via the [`ExtractionTier`] trait; the [`Runner`] drives the configured
//! tiers and merges their output with [`mandible_core::merge_nodes`].
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
pub use resolve::{resolve_tool, ResolvedTool};
pub use runner::{ExtractionResult, FillResult, Runner, TierStatus};
pub use tier::ExtractionTier;

/// Build the default set of tiers: B (`help_text`), C (`completion_script`),
/// E (`native`), F (`overrides`), in cost-attempt order (spec §7). Revision
/// 3 deleted Tier A (the vendored catalog, spec §7 "Tier A — REMOVED"): Tier
/// B now leads, dispatched on the framework identified by Tier A′ (spec §7
/// Tier A′), and costs 1-2 spawns but exists for every tool everywhere;
/// Tier C generates and parses a completion script; Tier E speaks a tool's
/// own dynamic completion protocol; Tier F is a local file read, attempted
/// last since most tools have no override at all. Tier D (man pages)
/// remains unimplemented (deferred entirely, per spec's roadmap). Whichever
/// features are enabled contributes its tier; conflict resolution between
/// tiers (when more than one contributes the same node) is by
/// [`mandible_core::Authority`], not attempt order.
// `vec![]` can't express the cfg-gated pushes below (each tier only
// exists to push when its feature is enabled).
#[allow(clippy::vec_init_then_push)]
pub fn default_tiers() -> Vec<Box<dyn ExtractionTier>> {
    #[allow(unused_mut)]
    let mut tiers: Vec<Box<dyn ExtractionTier>> = Vec::new();
    #[cfg(feature = "help-text")]
    tiers.push(Box::new(help_text::HelpTextTier));
    #[cfg(feature = "completion-script")]
    tiers.push(Box::new(completion_script::CompletionScriptTier));
    #[cfg(feature = "native")]
    tiers.push(Box::new(native::NativeTier::default()));
    tiers.push(Box::new(overrides::OverridesTier));
    tiers
}
