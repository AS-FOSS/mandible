//! `mantui-tui`: the `ratatui`-based terminal UI (spec §2, §9).
//!
//! This crate is split so its core logic is testable without a real tty:
//! [`app`] holds all mutable state as plain data (no rendering, no
//! terminal I/O), [`tree`] flattens the command tree into visible rows,
//! [`event`] translates crossterm events into `App` mutations, and
//! [`render`] draws an `App` into any `ratatui` backend — including
//! `TestBackend`, which is how this crate's rendering is verified (this
//! sandboxed environment has no tty to run the real thing against).
//!
//! **Search note:** the search bar in this batch does simple, local,
//! case-insensitive substring filtering over command names/summaries
//! (`tree::flatten`'s `filter` parameter). The `nucleo`-backed, flag-aware,
//! ranked search described in spec §10 is roadmap phase 3 and depends on
//! `mantui-search`, which is a stub in this batch.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod clipboard;
pub mod event;
pub mod layout;
pub mod render;
pub mod sanitize;
pub mod terminal;
pub mod tree;

pub use app::{App, Effect, Focus};
