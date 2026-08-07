# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [Unreleased]

### Fixed

- **openssl: 151 real command nodes wrongly marked `suspicious`.** The
  coverage harness's structure-sanity check (spec §13.1) flagged any node
  with no flags, no children, and no summary — a good fabrication signal
  in general, but openssl's `--help` genuinely lists 152 bare command
  names with no per-entry description, so every real node looked
  identical to a fabricated one. Adds `CommandNode::heading_attested`,
  positive evidence set only where the parser already requires a
  recognized command heading before creating a subcommand node; emptiness
  alone is no longer suspicious once that evidence is present. tar's
  phantom-subcommand shape ([M-10]) stays caught. openssl: 151 suspect →
  0.
- **apt-get: `name - description` subcommand lists not parsed.** Under an
  already-recognized command heading, ` - ` (space-dash-space) is now
  accepted as an entry separator alongside the existing 2+-space column
  gap, so apt-get's `"update - Retrieve new lists of packages"`-style
  listing recovers real subcommands with descriptions instead of zero. A
  bare ` - ` outside a recognized heading still can't manufacture
  anything — the [M-10] regression this column-gap rule exists for stays
  green. apt-get: 1 node (root only) → 18 (root + 17 real subcommands).
- **busybox: applet list not parsed.** `busybox --help` lists every
  applet tab-indented and comma-separated on a handful of wrapped lines
  under `"Currently defined functions:"` — a shape the shared bare-block
  engine (one entry per line) can't express. Adds a busybox-scoped
  `FrameworkProfile::comma_separated_command_list` and a dedicated scan,
  following the same pattern as argparse's subparser scan: framework-
  specific code, not a knob that loosens the shared engine for everyone.
  busybox: 1 node (root only, empty tree) → 271 (root + 270 real
  applets). Closes the framework-matrix's informational-only marker for
  busybox — it's a real gate again.

### Added

- **Top unidentified tools by flag count.** The coverage scoreboard's
  footer (text and markdown) now lists the ~25 tools with no detected
  framework, ranked by flag count — a work queue for the next framework
  fingerprint, not just a detection-rate percentage. Not gated.

## [0.1.0]

First release.

**What it is.** `mandible <tool>` opens a full-screen, explorable tree of
every command, subcommand, and flag a tool has — with descriptions — plus a
search bar over all of it. A reference browser, not a command builder: the
job ends when you've found the flag and can `y`-copy its exact spelling.

**How it works on tools it has never seen.** No per-tool logic, anywhere.
Help text isn't written by hand, it's *generated*, and only a small closed
set of generators exists — so mandible identifies the **framework** behind a
tool's output (clap v2/v3/v4, cobra, argparse, click, urfave/cli, Go's
stdlib `flag`, GNU argp/getopt_long, docopt, commander, yargs, oclif,
picocli, System.CommandLine, Symfony Console, OptionParser/Thor, busybox,
BSD-terse) and applies that framework's grammar. Identification is
artifact-first — `spf13/cobra` appears 583× in `docker`'s own bytes, which
is ground truth rather than a guess about section headings — with a
help-text signature as fallback.

Fixing the argparse grammar improves every Python CLI ever written.

**When it can't parse something, it says so.** A tool matching no known
grammar is rendered verbatim: the author's own text, untouched, labelled
`unparsed`. Structure is never invented — a user cannot tell fabricated
structure is wrong, which makes it worse than no structure at all.

**Performance.** Startup does no extraction: the UI is on screen
immediately and the tree fills in behind it on a bounded background pool,
showing `⋯ loading` where it hasn't arrived. No cache, deliberately — a
cache cannot see `docker` gaining a plugin or `git` gaining an alias from
`~/.gitconfig`, and being confidently stale is worse than being fast.

**Measured, not asserted.** `cargo xtask coverage` runs the pipeline against
every executable on `PATH` and scores it, including a structure-sanity
column that catches fabricated output — because `%described` alone once
reported a tool as `ok` at 100% while 39 of its 40 nodes were invented. A
CI workflow reports framework support on every run.

**Safety.** Extraction runs real tools, so it's fenced: an allowlist of
inert argv forms, `std::process` confined to one audited module and
enforced by a test, and every probe's CWD, `HOME`, `TMPDIR` and `XDG_*`
pointed at a per-invocation scratch directory. That last one isn't
paranoia — `mysql_secure_installation --help` was measured writing a
`.my.cnf` with an empty root password.

Linux and macOS. MSRV 1.88.

Known gaps are tracked as issues; busybox's applet list is
[#1](https://github.com/sadigaxund/mandible/issues/1).

<details>
<summary>Development history</summary>

## [Unreleased]

### Added (batch 6 part 6, framework-support workflow)

- **`.github/workflows/frameworks.yml`** (spec §13.1a): a framework matrix
  job (one representative tool per supported framework, asserting Tier A′
  detects the expected framework and Tier B extracts a non-trivial tree)
  and a PATH-sweep job (the coverage harness over the runner's own ~1,500
  executables). Both render markdown into `$GITHUB_STEP_SUMMARY`, so the
  supported-framework picture is on the run's summary page rather than
  buried in logs. The matrix logic lives in `scripts/framework_matrix.sh`
  so it runs identically on a laptop.
- Frameworks with no cheaply-installable representative (picocli, oclif)
  are reported as **skipped rows**, not omitted — an unmeasured framework
  should look unmeasured.

### Removed (batch 6 part 5)

- **`coverage-scoreboard.txt` is no longer checked in.** A full-`PATH`
  scoreboard is a snapshot of one developer's installed tools, so it can
  never be a portable baseline — spec §13.1a already said CI is "the
  natural home for the §13.1 scoreboard once it stops depending on
  whatever happens to be installed on a developer's laptop". The checked-in
  regression gate stays `coverage-scoreboard.ci.txt` over a fixed tool
  list; the broad measurement is the PATH sweep in
  `.github/workflows/frameworks.yml`, where the inventory is at least
  reproducible. `cargo xtask coverage` still writes the file locally; it is
  now gitignored.

### Added (batch 6 part 5, coverage harness)

- **Raw `described_flags`/`total_flags` in the aggregate footer**, so a
  scoreboard produced in shards can be merged exactly. Recomputing the
  aggregate from the per-row `%described` column cannot be exact — that
  column is rounded to whole percent — and a gated baseline must not be
  approximate.

- **`framework` column** in the scoreboard: detected framework plus how it
  was detected (`artifact` / `help-text`), with an aggregate
  framework-detection rate and per-framework counts in the footer.
- **`verbatim` status** for tools that degraded to spec §7 Tier B step 3.
  Reported but deliberately **not gated**: a correct new grammar can
  legitimately move a tool from fabricated structure to honest verbatim,
  and gating it would block exactly that improvement. `%described`,
  `no-tier`, and `suspicious` remain the gate.
- **`--format markdown`** output mode, consumed by the part 6 workflow.
- **Fixed column alignment.** Long names (`aarch64-linux-gnu-cpp-13`,
  `UnicodeNameMappingGenerator-18`) previously shoved every column to
  their right out of alignment for that row; columns are now fixed-width
  and truncated with an `…` marker.

### Fixed

- **Flags running straight into the `Usage:` line were silently
  swallowed.** The usage block consumed every indented line that followed
  it, so a tool that lists its flags immediately under `Usage:` with no
  blank separator and no `Options:` heading contributed *zero* flags —
  `curl --help` is the clearest case — while still reporting status `ok`,
  since nothing was fabricated for the structure-sanity check to catch.
  A usage continuation is an alternative invocation form and never begins
  with a dash, so a line that reads as a flag entry now ends the usage
  block. The curl fixture had been checked in but never asserted on,
  which is how this survived; it has a regression test now.

### Added (batch 6 part 4, per-framework Tier B grammars)

- **Framework dispatch wired into Tier B.** `HelpTextTier::extract_node`
  now identifies the framework behind a tool's `--help` text (spec §7
  Tier A′ — free artifact scan first, help-text signature fallback only
  on a miss, never double-probing) and dispatches parsing through a new
  `FrameworkProfile` (`mandible-extract/src/help_text/profile.rs`): a
  small per-framework data table (recognized command headings, whether
  the framework has a subcommand concept at all) that the *shared*
  section/layout engine consults, rather than one parser per framework.
  `GnuArgp`, `ClapV3V4`, `Argparse`, `Cobra`, and `Click` (spec §7 Tier
  B's priority order) have real, fixture-tested grammars — including a
  small dedicated scan for argparse's structurally distinct
  `add_subparsers()` layout, which a data table can't express — and the
  rest are documented as plausible-but-coarse. Adding a framework is one
  `match` arm plus one fingerprint, never a tool-name branch.
- **Staged degradation, three levels** (spec §7 Tier B): framework
  identified → normal confidence; unidentified → the same generic engine,
  capped to ≤0.5 confidence; structurally implausible (no flags, no
  subcommands, no usage) → a new `CommandNode::unparsed: Vec<Text>` field
  carries the tool's raw `--help` text verbatim at `confidence: 0.0`,
  instead of inventing structure. `mandible-tui`'s detail pane renders an
  `unparsed` node as a preformatted, unwrapped block (clipped rather than
  reflowed) with a `framework:` line added to the provenance footer.
- Two real parsing bugs found via fixture-testing against captured real
  `--help` output (not synthetic fixtures): gh's own `HELP TOPICS` group
  was being swept into subcommands by the engine's sticky same-indent
  chain (fixed with a new `non_command_heading_markers` profile field);
  and a `"name:"` trailing-colon convention (also from real `gh --help`)
  was silently rejecting every one of gh's subcommand names from the
  shape check (fixed generically in `emit_subcommands`, not gh-specific).

### Added (batch 6 part 3, framework identification — undocumented at the
time, recorded here for a complete changelog)

- **Tier A′**: `mandible-extract::framework` identifies the generator
  behind a tool's `--help` output — artifact byte-scanning first (ground
  truth, no spawn), a `--help`-text signature fallback second — across 18
  frameworks. `mandible --doctor` reports the detected framework.

### Removed (batch 6 part 1, spec revision 3)

- **Tier A, the vendored carapace-spec catalog.** Removed
  `mandible-extract/src/known_specs/`, `vendor/carapace-specs.json` (11 MB),
  `scripts/vendor_carapace_specs.py`, `mandible-extract/build.rs` (the
  byte-offset catalog index), the `known-specs` feature, and the carapace
  attribution in `NOTICE`. Reasoning recorded in spec.md §7 ("Tier A —
  REMOVED"): a per-tool catalog is per-tool knowledge merely relocated into
  data (spec §1), it cannot stay current with the tool actually installed,
  and it cost 11 MB of a 16 MB binary to raise flag-description coverage
  from a measured 87% to 99.5% on the ~250 tools it happened to contain.
  Replaced by framework identification and per-framework `--help` grammars
  (batch 6 parts 3-4). Also fixed a related bug: `mandible/src/doctor.rs`
  and `pipeline.rs` called `known_specs::catalog_meta()` with no `#[cfg]`
  gate, so the `known-specs` feature was never actually optional despite
  being documented as one.

### Added (batch 5, packaging)

- **`cargo-deb`/`cargo-generate-rpm` metadata** in `mandible/Cargo.toml`
  (`[package.metadata.deb]`/`[package.metadata.generate-rpm]`), following
  each tool's documented schema. Neither tool is installed in this build
  environment (no network to fetch them), so this metadata is unverified
  by an actual `cargo deb`/`cargo generate-rpm` run — see the report for
  what's verified vs. not.
- **`packaging/mandible.1`**: a real, hand-written man page for the
  `mandible` binary itself (options, key bindings, environment variables,
  file locations including the macOS/Linux path difference, exit status).
  Validated with both `mandoc -Tlint` (zero warnings) and `groff -man`
  (renders cleanly).
- **Shell completions**, generated at build time from the same `clap`
  command definition the binary parses against (`mandible/build.rs`,
  `include!`s `src/cli.rs` — the same pattern ripgrep/fd use, since a
  build script is a separate compilation with no other way to reach the
  `Cli` type), so they cannot drift from the real CLI surface. Packaging
  metadata installs them to standard paths via a glob into the
  build-script `OUT_DIR`. Also added a `mandible --completions <shell>`
  runtime flag (bash/zsh/fish/elvish/powershell) for non-packaged
  installs.
- README: a real terminal screenshot (`scripts/pty_screenshot.py` against
  the actual release binary, not fabricated), install instructions
  covering both `cargo install` and packaged installs, and an honest
  coverage section that deliberately does not quote a specific tool
  count or percentage — the extraction pipeline is expected to change
  again soon, and a number that's about to be wrong is worse than no
  number; points at `cargo xtask coverage` / `mandible --doctor <tool>`
  for current, real figures instead.
- Confirmed default features still build with no network access and no C
  toolchain (`cc`/`bindgen` stay behind the opt-in `manpage` feature,
  unchanged by this batch).

### Added (batch 5)

- **macOS support, audited and wired into CI.** `.github/workflows/ci.yml`'s
  `fmt`/`clippy`/`test`/`msrv` jobs now run as a matrix over
  `ubuntu-latest` and `macos-latest` (a real native Apple Silicon runner,
  not a cross-compile). Audited every `cfg(unix)` site
  (`mandible-extract/src/exec/spawn.rs`'s process-group spawn/kill,
  `mandible-extract/src/resolve.rs` and `xtask/src/coverage.rs`'s
  executable-bit checks, `mandible-cache/src/key.rs`'s mtime/inode cache
  key) and confirmed each uses only POSIX-standard APIs
  (`std::os::unix::fs::MetadataExt`, `std::os::unix::process::CommandExt`,
  `nix`'s `signal`/`process` features) with no Linux-only assumption
  (no `/proc`, no GNU-specific behavior) — this sandbox is Linux-only, so
  macOS behavior is reasoned-about from source, not run. README documents
  supported platforms honestly, including that cache/config paths differ
  by OS (`~/Library/Caches/mandible` etc. on macOS, via the `directories`
  crate) — this was already true before this batch, just not stated.
- The coverage harness's `--tools` fixed-list job stays Ubuntu-only,
  deliberately not matrixed: its pinned tool list reflects
  `ubuntu-latest`'s specific preinstalled inventory, and running the same
  list on macOS would fail on inventory differences alone, not a real
  regression.

### Changed (batch 5)

- **Project renamed from `mantui` to `mandible`.** Canonical repository is
  now `https://github.com/sadigaxund/mandible`. All 7 crates, the binary
  (`mantui` → `mandible`), user-facing paths (`~/.cache/mantui/` →
  `~/.cache/mandible/`, `~/.config/mantui/overrides/` →
  `~/.config/mandible/overrides/`), and every doc/prose reference were
  renamed. No migration shim: nothing had shipped under the old name, so
  there is no stale on-disk state to migrate.

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
  `mandible_core::is_command_name_shaped` or that carry nothing at all (no
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
- **The styling contract** (spec §9.2), new `mandible-tui::style` module:
  `DarkGray` instead of `Modifier::DIM` for muted text; every style
  degrades under `NO_COLOR`; search-matched characters are underlined
  within a row's name (via a new `mandible_search::match_indices`,
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

- **Tier B** (`mandible-extract::help_text`): a `winnow`-based flag-spec
  grammar plus a layout-driven, content-shaped (not heading-text-keyed)
  section scanner for `--help`/`-h` output. Reads stdout and stderr
  without requiring exit 0 (spec [M-8]); recovers a same-indent "word
  grid" shape for tools like `openssl` that use no indentation at all.
  Preserves section headings as `Flag::group`/`CommandNode::group`.
- Lazy, node-at-a-time extraction (spec §5.2 steps 3-4):
  `Runner::fill_node` re-probes incremental tiers for one node on
  expand; `mandible-tui`'s `App` tracks pending fills and renders a
  spinner row; the binary's `background.rs` speculatively warms one
  level of a node's children on a bounded `rayon` pool after a
  user-triggered fill, cancelled on quit.
- Real `nucleo`-backed search (spec §10, `mandible-search`): commands and
  flags are both independently indexed and addressable via `NodeRef`,
  ranked with fuzzy score plus an exact-name-prefix boost, driven from
  the event loop's poll timeout. `mandible-tui`'s tree filtering now takes
  a precomputed matching-path set rather than doing its own text search.
- The extraction coverage harness (`cargo xtask coverage`, spec §13.1):
  scans every executable on `PATH`, runs the full pipeline against each
  in parallel, and emits a scoreboard (checked in as
  `coverage-scoreboard.txt`) with a `--check` regression mode.
- `mandible-cache`: cache keys now include `SOURCE_FINGERPRINT`, a
  build-time hash of `mandible-core/src` + `mandible-extract/src`
  (`mandible-cache/build.rs`), and the vendored catalog's commit — fixes a
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
  `mandible-core`, `mandible-extract`, `mandible-cache`, `mandible-search`,
  `mandible-tui`, `mandible`, `xtask`.
- `mandible-core`: the shared intermediate representation — `CommandNode`,
  `Flag`, `Positional`, `Example`, `ValueKind`; `Text` with the
  `Text::sanitize` IR boundary; per-item `Provenance`/`Source`/`Authority`;
  `NodeRef`/`FlagKey` addressing; two-axis merge with alias pairing.
- `mandible-extract`: the `ExtractionTier` trait, the extraction `Runner`,
  the `exec/` execution-safety module (spec §6), and **Tier A**
  (`known_specs`) backed by a byte-offset-indexed vendored carapace-spec
  catalog. Tiers B, C, D (feature-gated, off by default), E, and F are
  stubbed for later batches.
- `mandible-cache`: on-disk, gzip-compressed, one-file-per-tool cache with
  file-identity keying and negative-result caching (spec §11).
- `mandible-tui`: the full tree/detail/search/status/help TUI (spec §2, §9),
  responsive layout, mouse support, `y` clipboard copy (OS clipboard with
  OSC-52 fallback), and the border-integrity regression test suite.
- `mandible` binary: `mandible <tool>`, `--refresh`, `--doctor <tool>`, and a
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

</details>
