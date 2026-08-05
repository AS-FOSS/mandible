# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [Unreleased]

### Added

- Workspace skeleton for all crates described in spec.md §8:
  `mantui-core`, `mantui-extract`, `mantui-cache`, `mantui-search`,
  `mantui-tui`, `mantui`, `xtask`.
- `mantui-core`: the shared intermediate representation — `CommandNode`,
  `Flag`, `Positional`, `Example`, `ValueKind`; `Text` with the
  `Text::sanitize` IR boundary; per-item `Provenance`/`Source`/`Authority`;
  `NodeRef`/`FlagKey` addressing; two-axis merge with alias pairing.
- `mantui-extract`: the `ExtractionTier` trait, the extraction `Runner`,
  the `exec/` execution-safety module (spec §6), and **Tier A**
  (`known_specs`) backed by a byte-offset-indexed vendored carapace-spec
  catalog. Tiers B, C, D (feature-gated, off by default), E, and F are
  stubbed for later batches.
- `mantui-cache`: on-disk, gzip-compressed, one-file-per-tool cache with
  file-identity keying and negative-result caching (spec §11).
- `mantui-tui`: the full tree/detail/search/status/help TUI (spec §2, §9),
  responsive layout, mouse support, `y` clipboard copy (OS clipboard with
  OSC-52 fallback), and the border-integrity regression test suite.
- `mantui` binary: `mantui <tool>`, `--refresh`, `--doctor <tool>`, and a
  graceful non-tty failure path.
- Packaging skeleton: `LICENSE` (MIT), `NOTICE` (carapace-bin attribution),
  `README.md`, `CONTRIBUTING.md`, this file, and a CI workflow running
  `fmt`, `clippy -D warnings`, and `test`.

### Known gaps (see spec.md §12 roadmap)

- Tiers B (help-text grammar), C (completion-script parsing), D (man
  pages), E (native probes), and F (user overrides) are not implemented.
- Lazy/incremental per-node extraction is not implemented; Tier A is
  non-incremental so this doesn't block usefulness today, but a future
  incremental tier will need the runner extended per spec §5.2.
- `mantui-search` is a stub; the TUI's search bar does local substring
  filtering rather than the `nucleo`-backed, flag-aware, ranked search
  described in spec §10.
- The coverage harness (`cargo xtask coverage`, spec §13.1) is not
  implemented — it depends on Tier B existing to produce a meaningful
  scoreboard.
