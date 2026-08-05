//! Tier B: engineered `--help` grammar parser. **Batch 2.**
//!
//! Spec §7 Tier B: a `winnow`-based grammar for `Usage:` lines and
//! layout-driven `Options:`/`Flags:`/`Commands:` section parsing, recursing
//! per-node under lazy extraction, reading stdout *and* stderr regardless
//! of exit code (measured: `openssl --help` writes only to stderr with
//! exit 0; `ip --help` exits 255 with stderr-only output — spec §7, [M-8]),
//! and attaching a `confidence: f32` derived from how much of the output
//! the grammar consumed.
//!
//! Left unimplemented in this batch so the crate's module layout and
//! feature-flag shape (`help-text` in `Cargo.toml`) are already correct for
//! batch 2 to fill in without a restructure.
