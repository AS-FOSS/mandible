# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [Unreleased]

### Fixed (batch 3)

- **Tier B invented subcommands** [M-10]: wrapped description continuation
  lines and `--format=`-style enum value lists were misread as bare-word
  command entries — `tar` gained 39 phantom subcommands, `dd` 40,
  `less`/`zstdless` 65. Implements spec §7 Tier B's four binding rules:
  a bare-word block only becomes subcommands under a recognized heading
  (or a chain started by one); a candidate name must match
  `^[a-z][a-z0-9_.-]*$`; an unrecognized block nested under a flag
  becomes that flag's `choices` instead; layout alone is never
  sufficient evidence. Root cause of the underlying parsing bug: block
  boundaries and entry-vs-continuation splitting were indent-floor
  heuristics that broke whenever a block mixed two entry depths (a
  short+long flag and a long-only flag in the same block, a real and
  common shape). Zero subcommands now for tar, dd, less, sed, and
  find/bfs, regression-tested against real fixtures for all five.
- **`--help` probe containment** [M-11], generalized beyond CWD: every
  probe now runs with its CWD, `HOME`, `TMPDIR`, and the writable XDG
  base-directory variables all pointed at one scratch directory created
  fresh per invocation and removed on drop — not just CWD, which
  `mysql_secure_installation`'s `.my.cnf` write (an empty root password)
  already showed wasn't sufficient. Portable regression test proves a
  probe cannot write into the real `$HOME`.
- **Quadratic parse time and unbounded entry recovery**: the coverage
  harness found a tool (`instmodsh`, a Perl REPL that ignores `--help`
  entirely and free-runs printing its own banner) that took over two
  minutes and produced 58,663 duplicate-name "subcommands" from one
  probe. Two bugs, both fixed: an O(n) prose-bound scan was being
  called from inside a loop condition instead of once before it; and
  recovered entries had no cap or deduplication, so tens of thousands
  of duplicates all reached the merge step. Now completes in ~10s
  (bounded by the ordinary exec timeout) with 3 clean nodes.

### Added (batch 3)

- **Structure-sanity coverage column** (spec §13.1): the scoreboard now
  carries a count of descendant nodes whose name fails
  `mantui_core::is_command_name_shaped` or that carry nothing at all (no
  flags, no children, no summary) — the shape a mis-parsed fragment
  takes even when its name happens to look valid. Any non-zero count
  marks the tool `suspicious`, checked before `%described` (which the
  Tier B phantom-subcommand bug proved can stay at 100% while a tree is
  fabricated) and gated in `--check` exactly like `no_tier_count`.
- **Tree row alignment and truncation** (spec §9.1): the summary column
  is now computed once over the whole flattened row set (not the
  viewport, which would jump while scrolling); truncation breaks at a
  word boundary with `…` instead of a hard character cut; the name
  column never yields to make room for a summary.
- **The styling contract** (spec §9.2), new `mantui-tui::style` module:
  `DarkGray` instead of `Modifier::DIM` for muted text; every style
  degrades under `NO_COLOR`; search-matched characters are underlined
  within a row's name (via a new `mantui_search::match_indices`,
  independent of the ranking match against a command's or flag's full
  haystack).
- **Detail pane rewrite**: a flag's description continuation now
  hang-indents under the description column instead of restarting at
  column 0; group headings (`GLOBAL OPTIONS:`, `Main operation mode:`)
  are normalized (trailing colon stripped, casing normalized) so the
  same logical group renders identically regardless of source; a flag
  line is three distinctly styled spans (spelling: accent; value
  placeholder: muted italic; description: default) instead of one
  undifferentiated run; deprecated flags get a `(deprecated)` tag.
  Selecting a flag via search now scrolls the detail pane to that exact
  flag's line — closing the batch-2 known gap below.

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
- The coverage harness's `--check` regression gate is not wired into CI
  (`.github/workflows/ci.yml`) because the harness scans every
  executable on `PATH`, which is environment-dependent; the checked-in
  baseline was generated in this batch's sandbox, not on the actual CI
  runner image.
- Probe containment (spec §6 rule 8) redirects CWD/HOME/TMPDIR/XDG_*
  per invocation, but cannot be a complete guarantee: a tool that bakes
  an absolute write path into itself, independent of any of these
  variables, sits outside what an environment/CWD redirect can reach.
  Full containment needs OS-level sandboxing (namespaces/seccomp).
- The coverage scoreboard's structure-sanity column currently flags 25
  real tools as `suspicious` (see `coverage-scoreboard.txt`) that
  weren't individually investigated this batch — the column is new and
  doing its job, but each flagged tool is an open item for a future
  pass, not a confirmed regression.
