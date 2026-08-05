# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [Unreleased]

### Added (batch 2)

- **Tier B** (`mantui-extract::help_text`): a `winnow`-based flag-spec
  grammar plus a layout-driven, content-shaped (not heading-text-keyed)
  section scanner for `--help`/`-h` output. Reads stdout and stderr
  without requiring exit 0 (spec [M-8]); recovers a same-indent "word
  grid" shape for tools like `openssl` that use no indentation at all.
  Preserves section headings as `Flag::group`/`CommandNode::group`.
- Lazy, node-at-a-time extraction (spec §5.2 steps 3-4):
  `Runner::fill_node` re-probes incremental tiers for one node on
  expand; `mantui-tui`'s `App` tracks pending fills and renders a
  spinner row; the binary's `background.rs` speculatively warms one
  level of a node's children on a bounded `rayon` pool after a
  user-triggered fill, cancelled on quit.
- Real `nucleo`-backed search (spec §10, `mantui-search`): commands and
  flags are both independently indexed and addressable via `NodeRef`,
  ranked with fuzzy score plus an exact-name-prefix boost, driven from
  the event loop's poll timeout. `mantui-tui`'s tree filtering now takes
  a precomputed matching-path set rather than doing its own text search.
- The extraction coverage harness (`cargo xtask coverage`, spec §13.1):
  scans every executable on `PATH`, runs the full pipeline against each
  in parallel, and emits a scoreboard (checked in as
  `coverage-scoreboard.txt`) with a `--check` regression mode.
- `mantui-cache`: cache keys now include `SOURCE_FINGERPRINT`, a
  build-time hash of `mantui-core/src` + `mantui-extract/src`
  (`mantui-cache/build.rs`), and the vendored catalog's commit — fixes a
  real bug where a stale cache entry from before a parser fix kept being
  served indefinitely after upgrading.
- `Text::sanitize_markdown`: normalizes carapace's markdown-flavored
  `description`/`documentation` fields (`[label](uri)` links including
  custom schemes, inline code, bold/emphasis) without being a general
  markdown parser; `Text::sanitize` itself now unwraps hard-wrapped
  source paragraphs before a later render-time re-wrap.

### Fixed (batch 2)

- A `&str[..6]` slice in the Tier B usage-line scanner panicked on any
  `--help` output with a multi-byte character positioned so byte offset
  6 fell inside it — found by the coverage harness's first real run
  against ~900 real system binaries, not by synthetic tests. Now
  compares raw ASCII bytes via `[u8]::get`, which is bounds-checked and
  needs no UTF-8 boundary at all.
- Raw markdown markup (`](man://gittutorial/7)`-style) leaking into the
  detail pane, and hard-wrapped source prose re-wrapping raggedly at the
  pane's actual width — both only visible with real catalog data, found
  via a pty-based manual verification harness (`scripts/pty_screenshot.py`)
  rather than the synthetic `TestBackend` fixtures.
- `Up`/`Down` were dead keys while the search box had focus; they now
  move the tree selection live while typing continues.

### Added (batch 1)

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

- Tiers C (completion-script parsing), D (man pages, deferred
  entirely), E (native probes), and F (user overrides) are not
  implemented.
- Search result selection scrolls the detail pane to the matched
  command but not to the specific matched flag within it.
- The coverage harness's `--check` regression gate is not wired into CI
  (`.github/workflows/ci.yml`) because the harness scans every
  executable on `PATH`, which is environment-dependent; the checked-in
  baseline was generated in this batch's sandbox, not on the actual CI
  runner image.
