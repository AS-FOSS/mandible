# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [0.1.7]

### Fixed

- **Probe sandbox paths no longer leak into documentation.** Every probe runs
  with `HOME`, `TMPDIR` and the writable `XDG_*` variables redirected at a
  scratch directory, so a tool printing a `$HOME`-derived default printed
  *ours*: `docker --help` reported its config location as
  `/tmp/mandible-exec-L3saJ8/.docker`, a directory deleted moments later that
  never existed for the reader, with nothing marking it as anything but
  docker's own text.

  Scratch paths are now masked back to the variable that stood in for them —
  `(default "$HOME/.docker")` — at the exec boundary, so every tier,
  `--doctor` and the verbatim view get the corrected text. Deliberately not the
  reader's real home directory: the tool never told us that, it is how man
  pages write such defaults anyway, and it keeps captured fixtures independent
  of the machine that captured them.

  A tool that wraps a scratch path across two lines still cannot be matched;
  the scratch prefix is now short to make that rarer.

- **The flag table no longer goes ragged as the terminal narrows.** The
  description column was capped at 45% of the pane, and any row too wide for
  the cap started its description at its own width instead — so the "shared"
  column was a target most rows missed. In a 90-column terminal `docker`'s
  global flags rendered descriptions at three different columns in one list,
  and `--log-level string` lost the gap between spelling and value, running
  them together as a single token.

  The column is now invariant for the list. A spelling too wide for it hangs
  its description onto the next line rather than pushing the column right for
  itself alone, and an outlier spelling is excluded from the measurement
  instead of being clamped to a column it would still miss.

  Below the width where a table can leave prose a readable amount of room, the
  list now stacks — spelling on one line, description indented underneath —
  the way every narrow-terminal help renderer does. Previously a 90-column
  pane broke `--platform`'s six-word description across six lines, one of them
  truncated mid-word.

### Changed

- **Each redirected variable gets its own scratch subdirectory.** They all
  pointed at one directory, which is a filesystem shape no real machine has — a
  probe writing `$XDG_CACHE_HOME/x` and reading `$HOME/x` saw the same file —
  so every tool was probed against an environment that cannot occur. It also
  left the path leak above unfixable, since a path under the shared directory
  could have come from any of seven variables.

### Internal

- `scripts/pty_screenshot.py` is back. It renders the TUI through a real
  pseudo-terminal in an environment that has no tty, and it found both
  rendering defects above — the `TestBackend` suite was green throughout. See
  `AGENTS.md` §3.2; it is a debugging tool, not part of CI.

## [0.1.6]

### Fixed

Three defects in re-extract (`r`), reported in
[#6](https://github.com/sadigaxund/mandible/issues/6). The last two shared a
cause: refresh replaced the whole `App` and then skipped both things the
event loop does for a freshly built one.

- **Holding `r` no longer piles up re-extractions.** `pipeline::load` runs a
  full extraction on the UI thread, so on a slow tool the screen is frozen
  for seconds while key auto-repeat keeps filling the input buffer. Every
  one of those buffered events was typed blind at a frozen screen, and each
  replayed into another complete re-extraction. Input that arrives during
  the block is now discarded.

  Refresh also left the previous whole-tree warming cascade running against
  the tree it had just discarded, so N presses left N overlapping walks
  competing for the pool. `Warmer::reset` abandons them by generation
  rather than by rebuilding the pool, because dropping a
  `rayon::ThreadPool` waits for its running jobs and would freeze the UI
  for exactly as long as the work being abandoned.

  The same counter that bounds warming (`MAX_WARMED_NODES`) was monotonic
  across refreshes, so each `r` spent from a budget that never refilled
  until background warming silently stopped working for the rest of the
  session. It resets with the generation.

- **The detail pane is no longer empty after a re-extract.** The root fill
  is what starts the cascade that walks the tree, and it was submitted once
  at startup and never re-queued. Afterwards the tree existed with nothing
  filling it, and expand was the only path that still triggered extraction,
  which is why pressing Enter appeared to fix it.

- **`r` is shown in the footer.** It was bound and listed in the `?`
  overlay from the first release but never on screen, so it could only be
  found by someone who already knew it was there.

### Changed

- **Re-extract keeps your place.** It previously rebuilt the `App` from
  scratch, discarding every expanded node, the selection, the scroll
  position, the search filter and the view mode. That is a poor trade for a
  key you press *because* you want to keep looking at what you are looking
  at. The selection is restored by path rather than row index, since a
  re-extract can change how many rows precede it.

- **The footer always keeps `? help`, not only `^C quit`.** A narrow
  terminal hides most of the row, which is exactly where a reader needs to
  be told the full list exists. It also means adding a hint can no longer
  quietly push `?` off the end, which is how `r` stayed invisible.

## [0.1.5]

### Added

- **`t` shows a node's raw `--help` output instead of the parse.** The
  staged degradation in spec §7 labels a node it could not parse, and a
  thin parse carries a low-confidence warning, but neither covers a
  grammar that misreads a layout and produces a *plausible* tree: from the
  outside that is indistinguishable from a correct one. Every fabricated
  subcommand regression this project has had (apt-get's description
  paragraph, git bisect's man-page prose) was caught by a human reading
  the tool's real output beside ours, and `t` puts that check one key away
  on every node rather than reserving it for whoever runs the coverage
  harness.

  It re-probes on demand instead of retaining raw text for every node,
  which would cost megabytes across a warmed tree to serve one node at a
  time and would show what the tool said at startup rather than what it
  says now. Refusals are rendered, not swallowed: pressing `t` on a
  never-probe tool says so, because a blank pane is also what a tool that
  prints nothing looks like.

### Changed

- README leads with evidence rather than reference material: the coverage
  and safety sections now come before the key list, and the key list itself
  is prose covering only what a reader cannot guess. An exhaustive key
  table duplicated the `?` overlay and the footer, which are both authoritative
  and neither of which can drift out of date.

### Fixed

- `Tab` was missing from the footer despite being bound since the first
  release. It is how the detail pane is reached at all, so long flag lists
  were unscrollable for anyone who had not read the `?` overlay.

## [0.1.4]

### Fixed — safety

- **mandible no longer runs programs whose purpose is to kill processes.**
  `mandible pkill` froze a user's machine badly enough to require a reset.
  `kill`, `pkill`, `killall`, `killall5`, `skill`, `xkill`, `fuser` and the
  system-state commands `halt`, `poweroff`, `reboot`, `shutdown`,
  `telinit`, `init` are refused before anything is spawned, under every
  argument shape.

  `--help` being harmless on one machine's build is not sufficient. The
  shapes spec §6 rule 2 permits include `<tool> <word> --help`, and for
  these programs the first positional is a **target, not a subcommand**:
  `killall foo --help` kills everything named `foo`. Any parser change that
  starts emitting subcommands for one of them turns a flag list into a
  process massacre — and the whole-tree background warmer would do it
  without being asked.

  The check lives in the single chokepoint every tier goes through, so
  nothing can reach one by another route, including the coverage harness
  that sweeps the whole `PATH`. A test asserts a shim named `pkill` is
  never executed under any allowed argv.

  This is a **safety** rule, deliberately distinct from §1's ban on
  per-tool parsing knowledge: it is closed, and keyed on what a program
  *does* rather than on how its output is formatted.

### Fixed

- **`mandible git` reported "low confidence: 0% parsed" on every node.** The
  number was accurate and the conclusion was wrong: `git clone --help`
  renders GIT-CLONE(1), 405 lines of man page, and the man-page guard
  correctly refuses to mine roff prose for structure — so the node degrades
  to verbatim with confidence 0.0 by construction. That is the designed
  fallback, and the pane already says `unparsed — showing raw --help
  output`. The caveat now skips verbatim nodes, and still fires for
  genuinely poor parses (`find` 11%, `ip` 9%).

## [0.1.3]

### Fixed

- **Version-manager shims resolve their toolchain again.** `mandible cargo`
  showed "rustup could not choose a version of cargo to run" instead of
  cargo's help: `cargo` is usually a rustup shim, and shims resolve the
  program they stand in for through `$HOME`, which spec §6 redirects to a
  scratch directory so a probe can never write into the real one. The
  redirect is right; it just made a whole class of developer tooling
  unusable — the same applied to pyenv, nvm, rbenv, asdf, sdkman and volta.

  `RUSTUP_HOME`, `CARGO_HOME`, `PYENV_ROOT`, `NVM_DIR`, `RBENV_ROOT`,
  `ASDF_DIR`, `SDKMAN_DIR` and `VOLTA_HOME` now pass through while `HOME`
  itself stays redirected. Each names a *toolchain* directory rather than
  the user's home, so the blast radius is far narrower than what the rule
  protects.

      cargo    0 flags  ->  12 nodes, 13 flags, 100% described
      rustc             ->  29 flags
      rustup            ->   4 flags

### Changed

- `authors` now names the maintainer rather than a placeholder.

### Documentation

- **spec §7 reconciles the framework-detection claim with what was built.**
  It advertised 71% from three fingerprints; the implementation identifies
  ~17%. Both are right and measure different things — [M-12] measured
  *recall* from deliberately crude patterns, while the implementation chose
  narrow, high-precision markers, because a *wrong* framework silently
  applies the wrong grammar whereas an unidentified one falls back to the
  general engine and is honestly marked low-confidence. Also records why
  the gap should not simply be closed: a `getopt_long` fingerprint would
  parse nothing better while lifting those tools out of the low-confidence
  cap, improving a metric by the thing it exists to detect.
- **spec §6 records the measured cause of lost coverage-sweep shards.** The
  timeout kills a probe's process *group*, which a child calling `setsid`
  leaves — `chromedriver`, `vimtutor`, `ghci` and `syscount.bt` were named
  by instrumenting each probe on both sides. `--help` is not what those
  programs do when they don't recognise it. Full containment needs
  OS-level sandboxing; `#![forbid(unsafe_code)]` rules out the cheaper
  partial mitigation, since `PR_SET_PDEATHSIG` requires `pre_exec`.

## [0.1.2]

A pass over the detail pane so it reads as documentation rather than as
flat output, plus the universality work that makes it hold up in terminals
that can't do Unicode or colour.

### Changed — the detail pane

- **Flag descriptions share one column.** They were indented by each
  flag's *own* width, so every row started its text somewhere different
  and the block read as ragged prose. Aligned columns are what make a
  parameter list read as a table, and a parameter table is what an options
  list is.
- **Value placeholders get their own column.** `--env` and `list` answer
  different questions, and run together as `--env list` they read as one
  token. The column collapses entirely when nothing in a list takes a
  value.
- **Section headings carry a rule** — `FLAGS ─────`. A bold word above body
  text is two lines of similar weight with nothing to anchor a boundary to.
- **The usage line is a signature block, and no longer says the tool's name
  twice.** Raw usage strings usually carry both a `Usage:` label and the
  tool's name, and the renderer prepended the name again, producing
  `tar Usage: tar [OPTION...]`.
- **Flag groups keep the tool's own ordering.** They were sorted
  alphabetically, so `tar` opened on "Archive format selection" rather than
  "Main operation mode". The sequencing is editorial and was being
  overwritten.
- **A flag's permitted values are shown** — `--format` accepts exactly
  `gnu, oldgnu, pax, posix, ustar, v7`. Extracted since 0.1.0 and never
  displayed.
- **Padding**, so text isn't flush against the border.

### Changed — provenance

- **The per-command footer is gone.** It read `help-text · structure ✓ ·
  prose ✓ · framework: cobra` under every command, and every part of it was
  identical on every node of every tool measured. The framework is a
  property of the *tool* and now sits in the tree pane's title; the source
  list moved to the status row, beside the pane it describes.
- **Low confidence is surfaced** — the one sanctioned exception to
  single-accent in spec §9.2, which had never been implemented. `find`
  scores 0.11 and `ip` 0.09, meaning the grammar recognised almost nothing;
  both used to report `structure ✓ · prose ✓` like everything else.

### Changed — controls

- **Controls sit on the left of the status row, provenance on the right**,
  no longer muted, with a left margin.
- **The help overlay is grouped** into MOVE / SEARCH / ACTIONS, padded, and
  its key column aligned. Two entries were stale.
- **Status messages expire.** `y` set the footer to `copied: <command>` and
  nothing ever cleared it, so one copy removed the keybinding hints for the
  rest of the session.

### Added — works where it's needed

- **An ASCII fallback for every glyph**, chosen from the locale, with
  `MANDIBLE_ASCII=1` to force it. Chevrons, borders, `›`, `✓`, `…` and the
  footer arrows were drawn unconditionally and are tofu in a non-UTF-8
  locale — the common case inside a minimal container, and one of the
  environments this tool is most often reached for. Enforced by a test that
  renders a full frame and asserts no cell contains a non-ASCII symbol.
- **`TERM=dumb` disables colour**, as does a non-terminal stdout — piping
  used to write SGR escapes into the file.
- **spec §9.2 records the rendering policy**: which techniques are used,
  which are refused, and the two properties that decide it — whether the
  capability can be *detected* (colour depth can; a font cannot, which
  rules out Nerd Fonts permanently) and whether it degrades to *less
  pretty* or to *meaningless*.
- **`mandible mandible` animates its wordmark.** One pass, redrawn in place
  so it stays in scrollback, skipped when piped or when the terminal can't
  draw block elements.

### Fixed

- **The detail pane scrolled past the end of its content**, so holding `↓`
  on a short description pushed the text off the top into blank space.
- **Name-mode search matches command names only.** It also matched flag
  spellings, so searching `run` in `docker` returned `ps` — `--no-trunc`
  contains "run". Correct, and indistinguishable from a broken filter.
  Flags are searched in the other mode, now labelled `everything`.
- **Value placeholders are no longer italic**, a modifier many terminals
  ignore and which leaves rendering artefacts where it is honoured.

### Changed — CI

- The PATH sweep runs in 16 shards and logs each tool on both sides of its
  probe, so a shard killed by the runner names the tool that started and
  never finished. Release assets now include version-less copies, so
  `/releases/latest/download/mandible-<target>.tar.gz` keeps working across
  releases.
- `cargo-deny` checks advisories, licences, bans and sources.

## [0.1.1]

Parser and UI fixes found by using 0.1.0 on real tools. Three tools that
returned nothing useful now return their real structure, and the search
box behaves the way its results imply.

**Tools fixed** — `apt-get` 1 node → 18 (its real subcommands, with
descriptions), `busybox` 1 → 271 applets, `openssl`'s 151 genuine commands
no longer flagged as suspect.

### Fixed

- **Name-mode search now filters literally.** It used subsequence matching,
  so searching `run` in `docker` returned most of the tree — `--no-trunc`
  contains r…u…n in order, and a matching flag surfaces its parent command.
  Every result was correct and none of them looked it. The two modes are
  now genuinely different: `names` is a literal substring match over names
  and flag spellings, `names+text` is the fuzzy index over descriptions
  too, where `gco` still finds `checkout`.
- **A row surfaced by a flag now says which flag.** Even literally,
  `docker ps` matches `run` via `--no-trunc`. That is right, and it was
  invisible, which is indistinguishable from a broken filter. Such rows
  show `via --no-trunc` in place of their summary: during a search, "why am
  I looking at this" is more useful than the description.
- **The status bar no longer lies while typing.** It always read
  `… q quit`, but `q` types the letter q in the search box — a user who
  wants out, hammers `q`, and watches `qqqq` appear in the filter has been
  told by the footer that this should have worked. While searching it now
  reads `type to filter   ↑↓ move   Enter/Esc leave search   / names↔text
  ^C quit`, and names `Ctrl-C`, the one escape that works from anywhere.
- **A long row name no longer runs into its summary.** Padding collapsed to
  zero when a name exceeded the shared summary column, producing
  `dselect-upgradeFollow dselect…`.
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

- **Supply-chain and licence checking** via `cargo-deny` in CI
  (`deny.toml`). Two policy calls are recorded there with their reasoning:
  unmaintained crates fail the build only when depended on *directly*
  (`paste` reaches us through `ratatui` with no safe upgrade, and a
  permanently red build over someone else's decision trains everyone to
  ignore the check), and MPL-2.0 is allowed and named as the exception it
  is rather than waved through under a "permissive only" heading that would
  not have been true.
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
