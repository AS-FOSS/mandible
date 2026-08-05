//! Tier D: man page structural extraction. **Deferred entirely** (spec
//! roadmap phase 5), behind the `manpage` feature flag, off by default.
//!
//! Spec §7 Tier D and §14: `libmandoc` is not a shipped library on Linux —
//! no `.so` or headers in a default install ([M-6]) — so this tier means
//! vendoring mandoc's ISC-licensed source and building it with `cc` plus
//! `bindgen` FFI bindings (`mparse_alloc` → `mparse_readfd` →
//! `mparse_result` → walk `mdoc_node()`/`man_node()`). That is the most
//! build-complex tier in the project, which is exactly why default features
//! must not require it (spec §15: "Default features must build with no
//! network and no C toolchain").
//!
//! Also unaddressed here: multi-page discovery (`git`'s structure spans
//! `git-commit.1`, `git-rebase.1`, etc., not `git.1`) via `MANPATH`/`man -k`
//! and the `<tool>-<sub>.N` convention (spec §7 Tier D, §16 risk 9).
//!
//! This module is intentionally empty. The `manpage` feature flag and
//! `cc`/`bindgen` optional dependencies are wired in `Cargo.toml` so a
//! future batch can implement this without touching the dependency graph,
//! but no vendoring or FFI work has been done.
