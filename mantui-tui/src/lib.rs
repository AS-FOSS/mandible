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
//! **Search:** `App` owns a `mantui_search::SearchIndex` (spec §10):
//! commands and flags are both indexed, ranked with `nucleo`'s fuzzy
//! scoring plus an exact-name-prefix boost, and driven via
//! `App::tick_search` from the caller's event-loop poll timeout rather
//! than blocking a keystroke handler. `tree::flatten` itself does no text
//! matching — it takes a precomputed set of matching command paths (a
//! flag match contributes its parent command's path, since flags aren't
//! tree rows) and handles the hierarchy-preserving expand/hide logic.

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
