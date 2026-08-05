//! Tier F: user overrides. **Batch 3** (spec roadmap phase 5, alongside
//! Tier D).
//!
//! Spec §7 Tier F: `~/.config/mantui/overrides/<tool>.toml`, merged with
//! `Authority { structural: 255, prose: 255 }`. Binding policy: overrides
//! are user-local and **never vendored into this repository** — this is
//! what actually enforces the project's no-per-tool-patches invariant
//! (spec §1). The pipeline must never depend on one existing.
//!
//! Left unimplemented in this batch; the module already exists so batch 3
//! slots in without a restructure.
