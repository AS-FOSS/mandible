//! Tier C: completion script structural parsing. **Batch 3** (spec roadmap
//! phase 4).
//!
//! Spec §7 Tier C: generate `<tool> completion zsh|bash` (never execute the
//! result — parsing only), then walk it as a real shell AST via
//! `brush-parser` (not `conch-parser`, which is unmaintained and emits a
//! future-incompat build warning — spec [M-9] — and not `yash-syntax`,
//! which is GPLv3). Prioritize zsh `_arguments` blocks (spelling +
//! description in one structure) over bash (spellings only, and often
//! computed at runtime).
//!
//! Left unimplemented in this batch; the `completion-script` feature flag
//! and this module already exist so batch 3 slots in without a
//! restructure.
