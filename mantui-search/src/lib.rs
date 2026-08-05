//! `mantui-search`: fuzzy search over commands and flags.
//!
//! **Batch 2.** Spec §10 requires a `nucleo`-backed index where flags are
//! first-class entries (not folded into their parent command's haystack)
//! and filtering preserves tree hierarchy. Out of scope for this batch;
//! this crate exists so the workspace's dependency graph and packaging
//! shape are correct from day one, and so batch 2 does not require a
//! workspace restructure.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

// Batch 2: nucleo-backed index over `NodeRef`s (commands and flags),
// hierarchy-preserving filtering, ranking that boosts prefix name matches.
// See spec §10.

/// Placeholder marker so this crate has at least one public item until
/// batch 2 implements the real index. Always `true`.
pub const UNIMPLEMENTED: bool = true;
