//! `mandible-core`: the shared intermediate representation (IR) every mandible
//! extraction tier normalizes into, and the merge logic that combines
//! several tiers' output for the same node into one.
//!
//! See spec §4. This crate has no knowledge of any specific tool, tier, or
//! terminal library — it is pure data model plus the sanitization and merge
//! rules that keep that data model trustworthy.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audit;
pub mod config;
mod entity;
mod merge;
mod node;
mod noderef;
pub mod notice;
mod provenance;
mod snapshot;
mod text;

pub use entity::{Dashes, Entity, EntityKind, Spelling};
pub use merge::{
    merge_entity_lists, merge_nodes, merge_subcommand_lists, pair_aliases, MergeError,
};
pub use node::{is_command_name_shaped, CommandNode, Confession, Example, ValueKind};
pub use noderef::{resolve, resolve_flag, resolve_mut, FlagKey, NodeRef};
pub use provenance::{Authority, Axis, ManFormat, Provenance, Source};
pub use snapshot::{
    to_snapshot, ConfessionSnapshot, EnvVarSnapshot, ExampleSnapshot, FlagSnapshot,
    ModifierSnapshot, NodeSnapshot, PositionalSnapshot, ProvenanceSnapshot,
};
pub use text::{strip_escapes, Text, MAX_TEXT_CHARS};
