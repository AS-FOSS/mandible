# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to adhere to [Semantic Versioning](https://semver.org/)
once it reaches a published 0.1.0 release.

## [Unreleased]

### Changed

- 0.5.0 IR: flags, positionals, modifiers and environment variables become one kind-tagged `Entity` carrying every documented spelling in document order, replacing the parallel-vector schema whose `short`/`long` pair could hold only two of a row's spellings (ffplay documents `-h`, `-?`, `-help` and `--help` as a single row); this change migrates flags and positionals, unifying a node's per-kind vectors into one `entities` vector read through kind-filtered accessors (`flags()`, `positionals()`) and folding a positional's variadic `...` onto the `repeatable` a repeatable flag already used, leaving rendering, search and merge behaviour identical — every corpus fixture's tree is byte-for-byte unchanged — and makes the public IR types `#[non_exhaustive]` so the modifier and environment-variable stages can follow without another breaking change.

- The detail pane is now one scrollable document of sections — DESCRIPTION, USAGE, POSITIONALS, FLAGS, MODIFIERS, ENVIRONMENT, in that order and only when non-empty — with the entity count in each list section's header (`FLAGS (41)`), groups drawn as a full-width mixed-case divider rule whose rows stay at the section's own margin, inherited flags as the final such group rather than a section of their own, and a spelling column computed per section and fitted to the majority rather than the widest row, so an unusually wide entity hangs its description on the next line at a four-column indent instead of pushing every other description right; a wrapped entry stays one row for selection and scrolling, and a search that selects a flag near the end of a long list no longer scrolls past the end of the pane's content.

## [0.4.5] - 2026-08-28

### Added

- The detail pane's raw `--help` view (`t`) and USAGE-section synopsis lines now scroll horizontally with `h`/`l`/`←`/`→` instead of wrapping, with a `←`/`→` marker drawn in the pane's border when there is more content off that edge and, per line, a muted vim-style `>`/`<` drawn in the pane's padding gutter against the border beside exactly the lines clipped at that edge (the text keeps its full width; a line that fits gets no marker); the rest of the detail pane (description, flags) is unaffected and still wraps as before. Controlled by a new `[ui] horizontal_scroll` key in `~/.config/mandible/config.toml` (default `true`); setting it to `false` restores the previous wrapping behavior exactly.

### Fixed

- Quitting no longer leaves mouse-report fragments (`35;24;9M…`) in the shell: teardown disabled raw mode before turning off mouse capture, so motion during quit was reported into a cooked terminal — the order is now reversed and pending reports are drained while still raw.
- A probe whose per-invocation scratch directory (spec §6 rule 8) cannot be built is now refused with a named error instead of silently running against the inherited environment and working directory — the silent best-effort fallback was how a probed font-cache builder once wrote `fonts.scale` into the directory mandible was started from; the redirect is now all-or-nothing (a partially-built scratch, any failed subdirectory, also refuses) and the refusal surfaces in the detail pane and `--doctor` like every other named refusal.
- Pressing `t` to flip between the parsed view and the tool's own `--help` reset the detail pane to the top on every toggle, which broke the exact comparison the verbatim view exists for; the pane now carries the reader's place as a proportion of the view's extent and resolves it against the other view's height, surviving any number of consecutive toggles, while a selection change still starts at the top; the horizontal offset carries the same way, so comparing the wide region of a synopsis against the raw text no longer snaps back to column zero on every toggle.
- LVM's per-mode synopsis stanzas (`vgchange`, and the whole `lv*`/`vg*`/`pv*` family) repeat the tool's own name plus that mode's mode-selecting flag on the stanza's head line (`vgchange -a|--activate y|n|ay`, `vgchange --systemid String VG`), which the engine kept only as the stanza's `group` label and never itself parsed for a flag — `sections::recover_stanza_head_flag` now also reads that flag out (via the existing alias/value grammar, so `-a|--activate y|n|ay` still reads as one flag) while leaving the head line as `group` exactly as before; recovers `vgchange`'s `--activate`, `--refresh`, `--systemid`, `--lockstart`, `--lockstop` and `--locktype`, all previously absent from the tree.

## [0.4.4] - 2026-08-27

### Fixed

- The `mandible mandible` easter-egg banner was painted with xterm-256 index 173 (an orange), an odd, unexplained choice given the rest of the UI's single-cyan-accent contract; it now uses index 30, a dark cyan/teal chosen for balanced WCAG contrast against both a plain black and a plain white terminal background (~4.4:1 and ~4.8:1 respectively), and the SGR code is now a named `BANNER_COLOR` constant next to the banner's other constants (`LETTERS`, `TRAJECTORY`, `LETTER_DELAY`, `FPS`) instead of a string literal at the call site.
- GNU `find --help`'s "Tests"/"Actions"/"Normal options"/"Positional options" tables pack several `-flag [ARG]` entries onto one physical line with no per-flag description anywhere, a shape the generic single-column fallback had no model for; when no description gap was found it read the whole line as one flag's spec, and `find_placeholder_boundary_gap` misread `-size N[bcwkMG]`'s own bracketed unit suffix as a description boundary and handed `-wholename` the front of the next entries on the line (`-true -type [bcdpflsD] -uid N`) as a fabricated description — a new shape reader (`sections::block_is_packed_flag_rows`/`try_split_packed_row`/`emit_packed_flags`) recognizes the "several bare dash-entries per line, no description" shape directly and bypasses `parse_flag_spec`'s short/long ambiguity for it entirely, recovering `find` from 22 to 75 correctly-spelled flags with zero fabricated descriptions and, measured over all 2,301 frozen captures in `audit/queue-captures/`, zero flag-id changes on any other tool.
- `ip` and its iproute2 siblings write a whole `OPTIONS := { -V[ersion] | -s[tatistics] | ... }` alternation group as one BNF production, and only the first `|`-alternative on each physical line ever became its own flag — the rest sat unparsed inside that flag's `description`, and the group's own `where OBJECT := { ... }` production line leaked into the node's `description` field as if it were prose; `mandible-extract/src/help_text/sections.rs` gains a reader, `split_bnf_alternation_row`, that splits a flag-table row's top-level `|`-alternatives into separate flags (folding a bare short immediately followed by a bare long back into one flag with both spellings, `dcb`'s own convention) and a predicate, `looks_like_bnf_production_line`, that keeps a `:=` production out of the description — both gated on the row's own heading having been produced by `split_shared_heading_row`'s `:=`-operator clause (never a bare `|` alone), which a full sweep of the 2,301 frozen captures in `audit/queue-captures/` showed was necessary: an ungated version tore a real short/long pair apart on 8 unrelated tools (`btrfsck`, `dpkg`, `mkfs.btrfs`, `pvchange`, `sg_get_config`, `sg_write_x`, `update-java-alternatives`, `vgchange`) that spell one flag's aliases with `|` instead of a comma; with the gate, only the six iproute2-family tools change, recovering every clean `|`-alternation and `[`-bracket-alias-pair flag documentable this way (`dcb`'s whole `OPTIONS` block, `ip`'s `-s`/`-d`/`-0`/`-6`/`-B`/`-M`) while deliberately refusing rows that mix `|`-alternation with space-run-together flags (`rdma`, `devlink`) or glue a two-letter abbreviation directly onto a short flag with no separator (`ip`'s `-iec`/`-ts[hort]`), leaving those rows unchanged rather than guessing.
- LVM's own emitter writes one stanza's mode flags as a multi-line parenthesized alternation group (`vgchange`'s first stanza: `( -l|--logicalvolume Number, -p|--maxphysicalvolumes Number, ... --setautoactivation y|n )`, "for options listed in parentheses, any one is required, after which the others are optional") rather than the one-`[...]`-per-line bracket-row shape the unlabelled-synopsis entry point already recognized, so the block never opened at all; `grammar::looks_like_paren_alternation_open`/`paren_alternation_member_content` (a new, narrow row-level predicate pair, never widening `looks_like_bracket_flag_row`) plus depth-tracking in `sections::parse_body`'s usage-block loop now recognize the shape, including a blank line separating the group's own closing `)` from its stanza's trailing bracket-row flag list, still the *same* stanza — and reuse the existing `parse_flag_spec`/`take_rest_value_token` alias-vs-value grammar unchanged; a new `corpus/vgchange/2.03.16` fixture pins the shape (full field-level measurement, including a pre-existing, unrelated trailing-comma value-name defect this fix also happens to repair, is in the fix's PR description).

## [0.4.3] - 2026-08-26

### Fixed

- **The TUI footer's confidence badge read `low confidence: 0% parsed` for `ssh-keygen` while `--doctor` reported `100.0% flags with text` for the same document**, because `sections::compute_confidence` divided a one-row option-table sample (a wrapped usage-synopsis fragment misread as a lone flag row) instead of treating it as too small to rate; fixed by giving a one-row sample its own `0.5` fallback, independent of whether a usage line was found — a fleet scan of `audit/queue-captures/` shows this removes 23 fabricated badges (including `ssh-keygen`'s) and fabricates none, and confirms an earlier version of this fix (folding the one-row case into the zero-row fallback) would have fabricated 7 new badges on cleanly-parsed single-flag tools (`byobu-disable`, `byobu-enable`, `bzless`, `bzmore`, `debconf-apt-progress`, `validlocale`, `xdg-user-dir`); `find`/`ip`'s genuine low-confidence badges (real, larger samples) are untouched.
- A tool's own getopt-family "unrecognized option" complaint about the `--help` probe (e.g. `ssh-keygen`'s `unknown option -- -`, or `c_rehash`'s entire output, `Usage error; try -h.`) could become the DESCRIPTION mandible shows in the TUI; two new predicates in `mandible-extract/src/help_text/sections.rs`, `is_option_error_line`/`is_option_error_paragraph`, recognize the narrow, well-known shape and drop it, even when it is the tool's only leading paragraph — measured over the 2,301 frozen captures in `audit/queue-captures/`, 116 tools have their DESCRIPTION changed by this fix, with zero flag/subcommand/status regressions on a full-`PATH` sweep (full measurement and the deliberately-excluded broader shape's 52 tools are in the fix's PR description).
- Long unverified-subcommand notices now wrap inside the detail pane instead of being clipped in the raw-help view. ([#38](https://github.com/AS-FOSS/mandible/pull/38))
- `ip` and its iproute2 siblings (`bridge`, `dcb`, `devlink`, `rdma`, `vdpa`) write their `OPTIONS` heading and its first flag row on one physical line joined by a BNF `:=` operator (`OPTIONS := { -V[ersion] | -s[tatistics] | ... }`), which the plain-column-gap shape `split_shared_heading_row` already handled (`uconv`, `zipinfo`) never matched, so the whole line — including the first flag — was swallowed into the section heading; `split_shared_heading_row` in `mandible-extract/src/help_text/sections.rs` now also recognizes the BNF-operator shape, recovering `-V` as its own flag and giving every flag in the block a clean group label instead of the entire swallowed line — measured over the 2,301 frozen captures in `audit/queue-captures/`, 6 tools gain their first `OPTIONS` row with zero flag-count regressions on a full-`PATH` sweep (full measurement, including why the broader bracket-without-operator shape's extra 30 tools are excluded, is in the fix's PR description).
- An unlabelled synopsis (no `Usage:`/`usage:` anywhere) can write one stanza per operation mode or invocation form — a prose description, its own `<tool> <args>` head, then continuation rows — separated by blank lines, and the usage-block loop in `sections::parse_body` ended unconditionally on the first blank line, so only the first stanza was ever read; LVM's own emitter (`vgck`, `lvextend`, `lvresize`, `pvscan`, and the rest of the `lv*`/`vg*`/`pv*` family) is the specimen this was measured against (`vgck --updatemetadata`, a flag rather than a subcommand, was completely absent from the tree), but the same shape also hit `adduser` and `pydoc3` with their own unrelated help formatters; the loop now looks one line past a blank line for the same synopsis-head evidence that opens the block in the first place, admitting a further stanza only when it is unambiguous, never reopening on a later paragraph that merely happens to repeat the tool's name (`corepack`'s headingless-invocation-table rows and `pydoc3`'s own wrapped, multi-line descriptions are explicitly guarded against) — measured over the 2,301 frozen captures in `audit/queue-captures/`, this recovers `vgck`'s `--updatemetadata`, `pvscan`'s `--cache`/`--listvg`/`--listlvs`/`-a|--activate` family, `lvextend`'s `--usepolicies`, and `adduser`'s `--system`/`--group`, plus 2 real flags for `pydoc3`, with zero flags lost on a full-`PATH` sweep (a coarser raw flag-count dip on `lvextend`/`pvscan` is a pre-existing duplicate-flag-counting defect correcting itself, not a loss — the deduplicated per-flag fingerprint sets show a clean gain on both); `vgchange`'s own first stanza uses a parenthesized multi-line alternation group rather than `[`-brackets and never opens under the existing entry point at all, a distinct, separately-measured shape this fix does not attempt (full measurement in the fix's PR description).

### Thanks

- [@Daniele-Cangi](https://github.com/Daniele-Cangi) for the detail-pane notice wrapping fix ([#38](https://github.com/AS-FOSS/mandible/pull/38)) — mandible's first outside contribution.

## [0.4.2] - 2026-08-24

### Fixed

- **The existence-fabrication oracle could not see three synopsis-entry
  shapes the parser itself already recognized, and reported the operands
  it correctly recovered from them as invented.** Measurement fix, not a
  parser fix: `xtask/src/existence.rs`'s `synopsis_lines` learned the same
  three entry points `mandible-extract`'s help-text tier already opens a
  usage block on — the C `fprintf(stderr, "%s: Usage: ...", argv[0])`
  idiom (`nfsidmap: Usage: nfsidmap [-vh] ...`), an **unlabelled** synopsis
  with no `usage:` marker anywhere (`gh`'s bare `USAGE` heading followed by
  `gh <command> <subcommand> [flags]`), and LVM's bare own-name line whose
  notation sits on the *next* physical line instead (`vgextend VG PV ...`
  followed by `[ -A|--autobackup y|n ]`) — by re-exporting and reusing the
  parser's own predicates rather than restating them (`mandible-extract/
  src/help_text/mod.rs`'s re-export block). Also fixed `existence::
  option_list_slot`, whose position-only placeholder rule read a genuine
  leading operand as invented the moment a synopsis's real flag-list
  stand-in sat *last* rather than first (`gh`'s trailing `[flags]`): the
  rule is now vocabulary-first (the parser's own `OPTION_LIST_PLACEHOLDERS`
  five-word list, reused rather than restated), falling back to the
  position rule only when nothing on the line already matched it. On a
  full-`PATH` sweep this closed 103 of a 154-tool existence-fabrication
  count down to 52 — below the round's own 66-tool starting point, since
  14 tools flagged even before this round's parser work turned out to be
  the same instrument gap — with zero remaining tools newly fabricating
  relative to that starting point. `xtask detector self-check --detector
  existence` (9 hand-built cases; this family has no labelled member in
  the audit, so self-check is its only calibration evidence) and
  `xtask corpus` both stay green.

- **A short flag's abbreviation-continuation bracket (`ip --help`'s
  `-V[ersion]`, `-s[tatistics]`, `-f[amily]`, `-h[uman-readable]`, ...)
  was read as a fabricated optional value.** `-V[ersion]` came out as `-V`
  taking an optional value literally named `"ersion"` — a value `ip` does
  not document, on a flag that takes none. `grammar::
  strip_short_abbrev_suffix` recognizes the shape structurally (a bracket
  glued directly onto a short character, opening with an ASCII lowercase
  letter and containing nothing but lowercase letters and hyphens — every
  real optional-value convention this project has measured is upper/mixed
  case, angle-delimited, or carries its own `=`) and discards it, mirroring
  `sections::strip_optional_modifier_suffix`'s existing command-name
  convention (`m[ab]` names the command `m`) on the flag side. `-h`, `-f`,
  `-4`, `-l` and `-o` in `ip`'s own `OPTIONS := { ... }` line are now
  boolean, as documented. (`-V`/`-s`/`-d` themselves remain absent — a
  separate, already-tracked defect, `corpus/ip/audit-seed2`'s `[xfail]`:
  the block's own opening physical line is consumed as heading text and
  never scanned for entries.)

  `strip_short_abbrev_suffix` also drops stray trailing `}`/`)`/`]`
  glued directly onto the bracket it closes — `ip`'s own last alternative,
  `-c[olor]}`, carries the enclosing `OPTIONS := { ... }` group's own
  closing brace with nothing between it and the abbreviation bracket.
  Without this, stripping `[olor]` left the bare `}` to fall into the
  ordinary value grammar, and `-c` moved from a merely-doubtful
  `value_name: "olor"` (the pre-fix reading) to an outright
  `value_name: "}"`, `Required` — a fabrication in the exact flag this fix
  targets, caught during review rather than by the fleet sweep (which
  only diffs *changed* value names, and this one was already changing).
  `-c` is now boolean, exactly as `-a[ll]` one line above it in the same
  block.

- **A mandatory flag some tool's usage synopsis writes unbracketed
  (`ssh-keygen -D pkcs11`, `-M generate`, `-I certificate_identity -s
  ca_key`, `-F`/`-R`/`-r hostname`) had its own value silently dropped.**
  `extract_usage_flags`'s bare-token walk read each bare token in a
  synopsis line independently; a flag's own required value sitting right
  after it with no bracket group around either was an unrelated, second
  bare token that started with neither `-` nor anything else the scan
  read, so it was dropped and the flag came out looking like a boolean it
  isn't. The walk now looks one token ahead: a bare flag immediately
  followed by a bare, non-flag, non-parenthetical token treats that token
  as its value (never when the flag is itself a recognized bundle of
  single-character switches, never when what follows is another flag, and
  never when it opens a parenthetical aside — `iptables`' own `iptables -h
  (print this help information)` was measured attaching the fabricated
  value `"(print"` to `-h` before that last guard existed). `ssh-keygen`'s
  `-D`, `-F`, `-I`, `-M`, `-R`, `-r` and `-s` now all carry their real,
  documented value; a fleet sweep found the same recovery on 60+ other
  tools (`iptables`/`arptables`/`ip6tables`'s own `-D chain`/`-I
  chain`/`-P chain`, `winpr-hash`'s `-u <username>`, `vim`'s `-t tag`,
  `rpcinfo`'s `-T netid`, among others) with zero flags lost or gained
  anywhere in the same sweep.

- **`pastebinit` reporting flag types the tool does not document (`-l`
  carrying a fabricated value `"List"`, taken from the first word of its
  own description) is confirmed already fixed** by the general
  `find_sentence_start_gap` fix (`apt-ftparchive`'s `--md5 Control MD5
  generation` shape) — no code change was needed. `corpus/pastebinit/
  1.6.2` pins the fix as a regression fixture.

- **LVM's docopt bracket-group flag rows (`vgck`, `vgextend`, `vgrename`,
  and the whole `lv*`/`vg*`/`pv*` family) rendered `verbatim` with zero
  flags.** LVM's own help emitter writes a *bare* invocation line with no
  bracket notation of its own (`vgck` alone, or `vgextend VG PV ...`) and
  puts every flag on a continuation row shaped `[ -x|--long value ]`, plus
  a real `Common options for lvm:` heading whose rows are the identical
  shape, tab-indented under a heading written with two spaces. Three
  independent gaps combined to hide this entirely:

  1. The unlabelled-synopsis entry point required notation evidence
     (`[`, `<`, `{`) on the invocation line itself; LVM's evidence lives
     only on the continuation row beneath it.
  2. `grammar::looks_like_flag_start` (deliberately, per its own doc
     comment — widening it would end `lsof`'s usage-block continuation one
     line early and drop six flags documented only there) never recognized
     a `[...]` group as a flag-table row, so the headed `Common options
     for lvm:` block was invisible to the flags-table scanner.
  3. `sections::leading_whitespace` counted raw characters, so a single
     leading tab (LVM's own convention for its option rows) measured as
     narrower than the heading's two leading spaces — the "is this content
     indented more than its heading" gate failed structurally, independent
     of (2).

  Fixed with a new row-level predicate,
  `grammar::{looks_like_bracket_flag_row, bracket_flag_row_content}` —
  never folded into `looks_like_flag_start`, consulted only where a
  flag-table row or a synopsis-continuation row is recognized — plus a
  tab-stop-aware `leading_whitespace` (a leading tab now expands to the
  next multiple of 8 columns, matching every terminal's own convention;
  behaviour is byte-for-byte unchanged for the fleet's dominant
  space-only-indentation case). A guard in `bracket_flag_row_content`
  refuses a row whose apparent alias run doesn't actually end at its first
  whitespace gap (`ethtool --help`'s `[ --all-groups | --groups [eth-phy]
  ... ]` is an alternation between two *different* flags, not one flag
  with aliases and a value, and reading it the LVM way would fabricate
  `--all-groups`'s value from `--groups`'s operands while dropping
  `--groups` outright).

  Recovers all 19 flags for `vgck`, 30 for `vgextend`, 21 for `vgrename`,
  and — via the same general, shape-keyed fix, no tool name ever consulted
  — the entire `lv*`/`vg*`/`pv*` family (~1,400 flags fleet-wide). The tab-
  stop correction also fixed a pre-existing, unrelated defect: a
  deeply-tab-indented description continuation that happened to start with
  a dash after trimming (`sotruss`'s `-o, --output`, `unsquashfs`'s
  `-match`, `mksquashfs`'s compressor-option rows) was previously read as
  a new, fabricated flag entry rather than a continuation of the row
  above it, because its raw tab count fell inside
  `scan_flags_block::ENTRY_INDENT_TOLERANCE`; the real terminal-column
  count does not.

- **A flag row that separates its spec from its description with a colon
  instead of whitespace or `=` had the colon (and, when glued, part of the
  spec itself) read as a fabricated required value.** `sg_emc_trespass
  --help` writes `-d : output debug` (spaced) and `-hr: Set Honor
  Reservation bit` / `-V: print version string then exit` (glued, no space
  at all). With no colon rule, the whole row fell into
  `grammar::parse_flag_spec`, whose ` VALUE` arm took the colon as a
  required value — `-d` acquired the fabricated value `":"`, and `-hr` was
  split into a fabricated `-h` carrying the mangled value `"r:"`,
  destroying the genuine two-character switch under any spelling a user
  could type.

  A new fallback in `help_text::sections::find_description_gap`,
  `find_colon_separator_gap` — modelled directly on its `=`-separator
  sibling, only ever consulted when neither a 2+-space column gap nor a
  lone `=` token was found anywhere in the line, and tighter than the `=`
  rule has to be (a colon is far more common in ordinary prose): a lone
  `:` token splits the spaced form, and a token whose *own* trailing
  character is `:` splits the glued form only when the token minus that
  colon is itself spec-shaped (`-hr`, `-V` — never `Options:`, a
  heading-shaped word, which ends the scan instead). `strip_colon_separator`
  removes the leftover leading `: ` from the recovered description, the
  same two-step `strip_equals_separator` already uses for `=`. Measured
  fleet-wide via a full-`PATH` sweep: every affected tool corrected a
  description or a fabricated value with no flag or subcommand lost
  anywhere.

- **A usage-block continuation line more indented than its synopsis's own
  base indent was always read as more of the synopsis, with no check that
  it was actually usage grammar rather than an ordinary English
  sentence.** `sg_emc_trespass --help` follows its one-line synopsis with
  two indented prose sentences ("Change ownership of a LUN from another SP
  to this one." / "EMC CLARiiON CX-/AX-family + FC5300/FC4500/FC4700.")
  before its flag rows; both were joined into the synopsis verbatim, and
  `extract_positionals` then mined their bare uppercase words `LUN`, `SP`
  and `EMC` out of it as three fabricated required positional operands
  beside the tool's one real one, `DEVICE`. The same shape was silently
  fabricating (or, for `pngfix`, an outright invented `PNG` positional) on
  `aarch64-linux-gnu-gcc-nm-13`, `ntfsfallocate`, `pngfix`, `ntfsclone`,
  `ntfscmp`, `ntfsfix`, `ntfsresize` and `pkgdata`.

  `help_text::sections::parse_with_profile`'s usage-block loop now checks
  `is_prose_sentence` on a more-indented continuation line and skips just
  that one line (rather than ending the whole block) when it fires —
  skipping, not breaking, because `mdadm --help` interleaves a one-line
  description under *each* of its seven `mdadm --mode ...` alternative
  forms, and ending the block at the first one would have dropped the
  other six. `is_prose_sentence` itself gained one more clause: a trailing
  `...` (docopt repetition notation, `numactl`'s `command args ...`,
  `mkfontscale`'s `[ directory ]...`) is no longer misread as a sentence-
  ending period, which a fleet sweep showed the new call site would
  otherwise have mistaken for prose and used to end usage blocks early —
  19 lost flags on `numactl`, 3 on `mkfontscale`.

  `help_text::sections::extract_positionals` also gained a narrow
  companion fix: a token right after a **self-closed** bracket group
  (`[-V]`, a flag written as its own complete, already-closed optional
  group) is no longer read as that flag's argument the same way a token
  after a still-open group (`[-C`, expecting `<path>]`) or a bare flag
  with no brackets (`-C`, expecting `<path>`) is — recovering
  `sg_emc_trespass`'s own `DEVICE`, `scsi_ready`'s whole `sg3-utils`
  family's `<device>+` operand, `lzgrep`/`xzgrep`'s `PATTERN`, and
  `renice`'s `priority`/`pid`. Deliberately scoped (via the new
  `primary_synopsis_lines`) to a tool's one primary, labelled invocation
  line rather than applied to every usage line fleet-wide: a broader
  sweep found the unscoped version recovered a few more real operands that
  `xtask`'s existence oracle cannot yet attest (a same-name-repeated
  alternate form under `jps`/`jstat`, and the unlabelled-synopsis
  convention `dbus-cleanup-sockets`/`dbus-run-session`/`lvreduce` use),
  and this fix does not claim more than that oracle can currently check.

  `xtask/src/existence.rs`'s own `clean_usage_token` gained the matching
  narrow fix on the detector side: `+` is now trimmed from a usage token's
  ends on the same footing as the `.` in `...` already was, since the
  `sg3-utils` family glues its one-or-more marker directly onto the
  closing `>` (`<device>+`) with nothing that already-trimmed set
  recognized — without it, `mandible-extract`'s own correctly-recovered
  `device` operand didn't match the oracle's uncleaned `device>+` and
  reported as invented on all five affected tools.

  Fleet existence-fabrication count (full-`PATH` sweep, this machine):
  130 → 124 tool(s); zero flags or subcommands lost anywhere.

- **A usage synopsis with no `Usage:` label, or one whose `usage:` is
  preceded by the tool's own name, was never entered — the whole synopsis
  (and any flags documented only in it) fell through to the root
  description or `unparsed`.** `wpa_cli --help` opens `wpa_cli
  [-p<path to ctrl sockets>] [-i<ifname>] [-hvBr] ...` with no `Usage:`
  line at all; `nfsidmap --help` writes the ordinary C `fprintf(stderr,
  "%s: Usage: ...", argv[0])` idiom, `nfsidmap: Usage: nfsidmap [-vh]
  ...`. `starts_with_usage_prefix` tests only a line's own start, so
  neither shape was ever recognized as a usage block:
  `extract_usage_flags` never ran on either line, `wpa_cli`'s root
  description read as the tool's own invalid-option banner run straight
  into its synopsis and entire `commands:` block, and `nfsidmap` reported
  `verbatim` with zero flags.

  Two new, narrowly-scoped recognizers in `help_text::sections`:
  `starts_with_name_prefixed_usage` (the tool's own name, `": "`, then
  `usage:`) and `looks_like_unlabeled_synopsis_line` (the tool's own name
  at a word boundary, plus positive usage-grammar notation — `[`/`<`/`{`
  — in the remainder, and not read as an English sentence), the latter
  tried only when no labelled `usage:` line exists anywhere in the
  document and only in the lines before the document's real body starts.
  Neither loosens `starts_with_usage_prefix` itself or any existing entry
  condition — a tool with a real `Usage:` line is unaffected. `wpa_cli`
  gains six new, undescribed value-spec flags (`-p`, `-i`, `-P`, `-g`,
  `-G`, `-s`); `-a`, `-h`, `-v`, `-B`, `-r` — already described by its
  flag table — merge rather than duplicate, per
  `flag_spelling_already_present`'s existing policy. `nfsidmap` goes from
  `verbatim`/0 flags to `ok`/4 flags.

  Measured on a full-`PATH` sweep: 16 tools move `verbatim` → `ok`, 20
  gain 124 flags fleet-wide, 0 flags are lost anywhere. Four systemd
  tools (`systemd-creds`, `systemd-sysext`, `systemd-confext`,
  `varlinkctl`) initially lost their `Commands:`-derived subcommand
  stubs as a side effect: their own `Commands:` heading is ANSI-corrupted
  (`\x1b[0mCommands:` reads as one alphanumeric run, `0mCommands`, to
  `mentions_commands_word`), and that heading's recognition was
  previously rescued only by accident, via the very description-leak
  this fix removes. Not patched by widening the `command_mode` sticky
  chain to also seed from a usage synopsis's own `COMMAND` word —
  measured and reverted: that seed fabricated a subcommand literally
  named `v2.3.3` out of `containerd`'s and `ctr`'s unrelated `VERSION:`
  block, the exact class of defect [M-10] exists to prevent.

  **Root-caused and fixed instead**: `mandible_core::strip_escapes`
  (previously private to `Text::sanitize`, now exported) now runs once
  over the whole raw `--help` document, in `help_text::sections`, before
  any heading, indentation or column gap is computed from it — not only
  per-field at final display time, which was too late for the layout
  analysis that decides what's a heading at all. `systemd-creds` and
  `varlinkctl` now recover their `Commands:` block through the ordinary
  recognized-heading path (`heading_attested: true`, strictly better
  evidence than the accidental stubs they had before). Measured on a
  second full-`PATH` sweep isolating just this change: it moves exactly
  these two tools and nothing else in the entire fleet — 0 other status
  transitions, 0 other flag or subcommand changes, confirming escape
  bytes were not silently distorting `leading_whitespace`/
  `find_multi_space_gap` byte offsets for any other tool on this
  machine's `PATH`.

  `systemd-sysext` and `systemd-confext` remain a **known, named
  residual regression**: their command rows sit directly under the
  description prose with no `Commands:` heading at all (`systemd-sysext
  [OPTIONS...] COMMAND` synopsis, then five unheaded rows), which is
  categorically the shape spec §7 Tier B rule 1 forbids recognizing
  ("a command block must be introduced by a recognized heading — layout
  alone is never sufficient evidence"). Recovering it safely would need
  a new, separately-measured recognizer restricted to a headingless,
  column-aligned block immediately following the description with no
  intervening heading and no `command_mode` stickiness; not attempted
  here.

- **Two headed command tables — one separated by ` = `, one carrying no
  separator at all — parsed to zero subcommands.** `wpa_cli`'s `commands:`
  block writes each of its ~180 rows as `name [operands] = description`
  (`status [verbose] = get current WPA/EAPOL/EAP status`); the generic
  engine's column-gap-or-dash entry splitter found neither shape, so a
  row's whole line became its candidate "name," failed the name-shape
  test, and was dropped as unattributable. `apt-ftparchive`'s `Commands:`
  heading is worse off still: its first row (`packages binarypath
  [overridefile [pathprefix]]`) shares the heading's own physical line, so
  it was never scanned as content at all — the whole line, plus every
  aligned continuation row beneath it, was absorbed into the root
  description/group text instead.

  Two new recognizers in `help_text::sections` — `scan_bare_command_table`
  (the ` = `/no-separator row shape) and `split_heading_inline_row` (a
  heading sharing its physical line with the table's first row) — recover
  a row's name as only its **leading name-shaped token**, never a run:
  `apt-ftparchive`'s `sources srcpath` must name one command, `sources`,
  with `srcpath` as its own positional operand, not a second command or a
  grandchild. Every recovered node is `invocation_attested: true,
  heading_attested: false` (never `--help`-probe argv) — these tables
  belong to daemon-control clients (`wpa_cli terminate`, `wpa_cli quit`)
  whose "commands" are runtime verbs a probe could act on, so the rows are
  strong existence evidence and deliberately weak safety evidence, the
  same split spec §6 already draws for the headingless-invocation-table
  recognizer.

  Both recognizers are gated on the *current* heading being directly
  recognized (never a `command_mode` chain) and refuse any block already
  carrying a real column gap or ` - ` separator, plus a floor of at least
  two *distinct* recovered names — three guards added after two real
  fabrications turned up mid-development on the live fleet: a heading's
  `command_mode` staying stuck on let `fail2ban-client`'s wrapped
  continuation prose ("...restarting of the server, the / option
  '--restart' activates...") read as commands named `of`, `the`, `option`;
  and `trash-put`'s closing sentence "use one of these commands:" (a real,
  if misleading, hit of the existing generic `mentions_commands_word`
  test) followed by a two-line usage example turned one repeated example
  invocation, `trash`, into a fabricated subcommand of `trash-put`.

  Measured, full PATH (2,254 tools, aarch64 Ubuntu 24.04): exactly three
  tools move, all by gaining subcommands only — `wpa_cli` (+211,
  deduplicated from ~180 rows, some of which alias one name across several
  operand shapes), `apt-ftparchive` (+6, its full real command list), and
  `dpkg-maintscript-helper` (+5, the same generic recognizer catching a
  second, previously-unmeasured real fixture). Zero flags lost or gained
  anywhere, zero status transitions, zero unexplained movement.

## [0.4.1] - 2026-08-23

### Fixed

- **A single-dash long option whose name carries an underscore was split
  into a one-character short flag plus the rest of its own name.**
  `dbiprof` writes `-case_sensitive` and the tree held `-c` with
  `value_name: "ase_sensitive"` — a short flag `dbiprof` does not document
  anywhere. It sat in the same table as `-number=N`, `-reverse` and
  `-version`, all of which the previous release already recovered; the one
  thing that separated it from them was the `_`, which
  `is_option_name_tail` rejected.

  `_` separates words inside a name exactly as `-` does, and none of the
  seven conditions that make this repair safe is measured on which
  separator a name uses. Admitting it therefore moves nothing else:
  `-DFOO_BAR` and `-oOUT_FILE` are still rejected by the uniform-lowercase
  test, `-o out_file` is still rejected by the raw-text scan (it never
  occurs glued), and `-d item_a[,...]` is still rejected by the name-shape
  test's own punctuation rule. Each is asserted by name in `sections.rs`'s
  tests and in `xtask::single_dash_long`'s self-checks.

  Measured on a full-`PATH` sweep (2,254 tools, aarch64 Ubuntu 24.04) the
  change moves **17 tools and 604 flag spellings** — `clang -fchar8_t` and
  `-fno-char8_t`, `llvm-install-name-tool -add_rpath` and
  `-delete_all_rpaths`, `llvm-lipo -verify_arch`, `llvm-otool
  -chained_fixups`, `ffmpeg -pix_fmts` and `-filter_script`, `lto-dump
  -fchar8_t`, and the whole of `ffplay`'s and `ffprobe`'s `AVOption`
  tables. No tool appeared, disappeared, or changed status or tier, and
  the field-level sweep diff reports **no flag lost anywhere**. Every one
  of the 604 recovered names was checked against its own tool's raw
  capture and every one occurs as the leading token of a row that tool
  writes; there were no counter-examples.

  `ffplay` and `ffprobe` are 97% of it, and their rows put the value spec
  in a *space-separated* column (`-is_avc  <boolean>  .D.V..X....  is
  avc`). Both that column and the capability column beside it are
  untouched: they were never in `value_name` — the swallowed name half
  was — and they live in the description, which this repair does not
  write. `ffplay`'s tree keeps the same 1,136 flags and the same 1,135
  descriptions byte for byte; 679 of them stop being fabricated shorts,
  and the fabricated `-b`, `-c`, `-d`, `-g`, `-h`, `-k`, `-l`, `-m`, `-n`,
  `-o`, `-p`, `-r`, `-u` and `-w` disappear while its real `-h`/`--help`,
  `-i input_file`, `-x width`, `-v loglevel` and `-f fmt` stay.

- **A single-dash long option carrying a glued `=value` was split into a
  one-character short flag plus a mangled value.** `dbiprof` writes
  `-number=N` and the tree held `-n` with `value_name: "umber=N"`;
  `gcc` writes `-foffload=<targets>` and the tree held `-f` with
  `value_name: "offload=<targets>"` — a real parser bug the human audit had
  already confirmed on `corpus/gcc/13.3.0`. The value-less rows of the *same
  tables* (`-reverse`, `-pipe`, `-help`) came out right, because
  `help_text::sections::repair_single_dash_long_options` already existed for
  them.

  What that repair could not see is that `=` is a *boundary*: it asked
  whether the whole swallowed run was an option name, and `umber=N` is a
  name plus the value spec the tool glued onto it. The tail is now split at
  the first `=` (`split_glued_value`) and the existing name-shape and case
  predicates are applied to the name half; the glued value spec survives on
  the flag it belongs to, so `-foffload` stays a value-taking flag named
  `<targets>` rather than becoming a boolean. Splitting at the *first* `=`
  is what makes `dbiprof`'s `-match=K=V` come out right.

  **No predicate was loosened, and the discriminator is unchanged in
  substance.** The rule that keeps the GCC/Clang glued-value convention safe
  — no ASCII uppercase anywhere in the reconstructed token — now reads the
  name half, which is exactly where that convention puts its shout:
  Ghostscript's real `-sDEVICE=png16m` has the name token `-sDEVICE`,
  `java`'s real `-D<name>=<value>` has `-D<name>`, `gcc`'s own
  `-Wl,<options>` fails the name-shape test on the comma before case is
  consulted. All three are still rejected, asserted by name in
  `sections.rs`'s tests and in `xtask::single_dash_long`'s self-checks.
  A **spaced** `key=value` argument (`-e var=value`) stores byte-for-byte
  what `-number=N` stores, and is still separated from it by the raw-text
  glued-occurrence scan alone.

  Measured over the 2,301 frozen captures in `audit/queue-captures/`: the
  shape reaches **40 tools and 2,196 flags**, overwhelmingly the GCC
  toolchain and its cross-compiled aliases (`gcc`, `cpp`, `g++`, `as`,
  `c89`/`c99`/`cc`, the `aarch64-linux-gnu-*` and `-13` variants) plus
  `dbiprof` and two LLVM tools. A document-level "this tool uses single-dash
  long options elsewhere" signal was evaluated as an alternative
  discriminator and **rejected on the data**: it fires on 37 of the 40
  true positives but also on 11 of the 13 tools in the false-positive-risk
  population, so it corroborates and does not discriminate.

  Knowingly still split, each declared and asserted rather than described:
  uppercase-led names (`-Xassembler`, `-Wuninit-variable-checking`), names
  carrying an underscore (`dbiprof`'s own `-case_sensitive`, in the very
  table this repairs), bracketed optional values (`-fcompare-debug[=<opts>]`),
  and a tail that ends at the `=` with nothing after it.

- **Three shapes that are not headings were being read as section headings,
  polluting `group` on 197 tools and losing two real option rows.** The
  section scanner promotes a line to a heading on *indentation alone* — any
  line whose next non-blank neighbour is indented further introduces that
  neighbour's block — with no test that the line is heading-shaped. Measured
  over the 2,301 frozen captures in `audit/queue-captures/`, three distinct
  shapes exploit that, and only the first two share a root:

  - **A prose sentence above an indented option table** (**205 tools**, 211
    distinct lines). Overwhelmingly the GNU convention: 56 tools inherit
    `group: "Mandatory arguments to long options are mandatory for short
    options too."`, 13 inherit `"With no FILE, or when FILE is -, read
    standard input."`, and `nano` inherits `"When a filename is '-', nano
    reads data from standard input."` on all 54 of its flags. The flags pane
    renders `group` uppercased as a section header, so every one of these
    printed a sentence where a heading belongs.
  - **A backslash-continued synopsis line** (**7 tools**, 16 distinct
    lines): `update-xmlcatalog`, `wpa_cli`, `zic` and the four `bpfcc`
    tracers. `UPDATE-XMLCATALOG <OPTIONS> --DEL --ROOT --TYPE <TYPE> \`
    rendered as a section header.

  Both are now refused by one predicate, `heading_can_name_a_group`
  (`is_prose_sentence` — a single field with no column gap, five or more
  words, terminated by a full stop — plus `is_line_continuation_fragment`).
  Anchoring prose on the *full stop* rather than on wording or length is
  what keeps the inverse case working: `gcc`/`lto-dump` writes real headings
  that are complete English sentences (`"The following options are specific
  to just the language C:"`) and `objdump` `"At least one of the following
  switches must be given:"`, all colon-terminated, all untouched. The column
  gap is what keeps period-terminated table *rows* out (`arptables`'
  `[!] --version	-V		print package version.`).

  - **A heading sharing its physical line with the first row of its own
    table** (**2 tools**) is a different mechanism and gets a different fix.
    `uconv --help` writes `Options:  -h, --help                    print
    this message`, and the whole line was consumed as the heading, so
    `-h, --help` was in the tree under no spelling a user could type.
    `split_shared_heading_rows` rewrites such a line into the two lines it
    means before the engine sees it, so the recovered row is subject to the
    same block-level alignment decisions as the rows beneath it. `uconv` and
    `zipinfo` each recover exactly the one row they were losing; two more
    tools (`scsi_mandat`, `scsi_satl`) gain a described `-h, --help` in place
    of a synopsis-derived one.

  Separately, in the usage block, **a line the previous one ended with a
  backslash is now a continuation by the tool's own explicit statement**, no
  content test allowed to overrule it. `update-xmlcatalog` wraps its
  synopsis mid-invocation onto a tail beginning `--id`, which tripped the
  `curl` guard ("a continuation that reads as a flag row ends the block") and
  took `--del`, `--root` and `--type` with it — none documented anywhere else
  in that tool's output. `--del` is now a real flag and
  `corpus/update-xmlcatalog/audit-seed2` is promoted out of `[xfail]`;
  `wpa_supplicant` loses a bogus `-i <ifname>` whose description was a
  fragment of its own synopsis, and both tools' usage entries are now whole.

  **Safety (spec §6).** None of this touches
  `is_recognized_command_heading`, `command_mode`, or anything that sets
  `CommandNode::heading_attested`, which is the bit §6's attestation gate
  reads before a word may become `<word> --help` probe argv. Verified rather
  than argued: re-parsing all 2,299 frozen captures before and after, the
  multiset of recovered node names is **identical for every one of them** —
  no node gained, lost, or renamed — so the probe-eligible set is
  bit-for-bit unchanged. 200 fixtures change at all; 194 of them change only
  `group`, and every one of the 115 distinct removed group strings was
  reviewed by eye and is prose or a wrapped synopsis fragment. No tool loses
  a flag, a description, or a subcommand.

- **`xtask sweep-diff`'s `#fp` fingerprint footer silently dropped tools whose
  `value_name` contained one of the format's own separator characters** —
  found on `awk`/`gawk`/`nawk`, whose `-L` flag documents `value_name`
  `"fatal|invalid|no-ext"`. The old `fp_escape` scrubbed only tab and
  newline, so that flag's rendered `#fp` line carried three unescaped `|`s
  where the flag-list separator expected exactly one;
  `parse_fingerprint_line` split on all three, the resulting bogus entries
  had no `=`, and the entire `#fp awk` line — every flag on it — was
  discarded. `fp_escape`/`fp_unescape` now backslash-escape and reverse
  every separator the wire format uses (`\`, tab, newline, `|`, `,`, `=`,
  `:`), so the escaped text is guaranteed free of raw separators and the
  round trip is lossless. Backward compatible, measured: 0 backslashes
  across 2,308 `#fp` lines in a full-`PATH` sweep capture and 0 in
  `coverage-scoreboard.ci.txt`, so unescaping is the identity function on
  every scoreboard on disk today.

- **A flag row that separates its spec from its description with a bare `=`
  token, instead of a column gap, either lost the description entirely or
  kept a leaked `= ` prefix.** `update-xmlcatalog`'s `With:` block
  (`--file <file>       = a local filename`) is column-aligned, so the split
  itself worked, but the description kept its leading `= `; `wpa_supplicant`'s
  `options:` block (`-b = optional bridge interface name`) has only single
  spacing, so no 2+-space gap existed anywhere on the line and the whole row
  fell into `grammar::parse_flag_spec` — the description was lost outright,
  measured at "low confidence: 9% parsed" across its ~28 flags. Two new
  functions in `mandible-extract/src/help_text/sections.rs`,
  `find_equals_separator_gap` (a new fallback in `find_description_gap`,
  ordered after the existing 2+-space/tab rule so every already-working
  aligned split is untouched) and `strip_equals_separator` (applied at both
  places a flag row's description is produced), fix both shapes with one
  mechanism. Real `=` usage inside specs and descriptions
  (`--opt=VALUE`, `ffprobe`'s "0 = disable, 1 = enable", `--enable-gvn-hoist`'s
  "(default = off)", `systemd`'s `--dump-core[=BOOL]`) is left untouched —
  covered by new regression tests for each measured instance.

### Performance

- **The two flag repairs stopped re-reading the whole help text once per
  candidate flag.** Both `repair_repeated_character_flags` and
  `repair_single_dash_long_options` end on the same question — does this
  token occur glued and delimited in the tool's own raw text — and
  `token_occurs_glued` answered it by scanning the entire document, every
  time. The cost was `O(candidates x document)`, and widening the
  single-dash long-option conditions in 0.4.0 put roughly 679 candidates in
  front of it for one tool: `mandible --doctor ffplay` (752 KB of help
  text) went from ~1.4 s to **3.22 s**, `ffprobe` from 628 ms to 1.30 s.

  A new `GluedTokenIndex` answers the same question from **one** pass over
  the document — every maximal run of word characters, keyed by its own
  text — and both repairs share one index per parse. The common candidate
  (`-help`, `-vv`: all word characters) is a single hash lookup, because a
  run being maximal is exactly what the predicate's two boundary conditions
  ask; a candidate carrying a glued value spec (`-foffload=<targets>`)
  additionally checks its remainder against the text after that run's own
  occurrences. Measured on this machine, five runs each: `--doctor ffplay`
  **3.225 s → 0.426 s**, `--doctor ffprobe` **1.302 s → 0.245 s**, with
  `--doctor ffmpeg` unchanged at 0.21 s as a control.

  Pure optimization, and checked as one: a full-`PATH` sweep before and
  after (2,308 tools) produces **byte-identical** scoreboards, and the
  corpus is unchanged with no snapshot rewritten.
  `indexed_form_agrees_with_scanning_form` pins the index against the
  scanning form — which stays, as the readable statement of the predicate —
  over the cases where the two could differ.

## [0.4.0] - 2026-08-23

**Breaking:** `CommandNode` gained a public `invocation_attested` field.
The struct is not `#[non_exhaustive]`, so any downstream struct-literal
construction must be updated. Everything else in this release is additive
parser accuracy: no flag, description, subcommand or usage line that
0.3.2 parsed correctly is parsed differently now, verified field-by-field
over a 2,254-tool sweep.

### Added

- **The README states the measured accuracy, not just the coverage badge.** A green
  `framework support` tick says CI passed; it says nothing about how often a parsed
  tree is right. The "Is it actually universal?" section now carries seed 5's
  human-reviewed figure — 58.1% [43.3%, 71.6%] overall, 80.0% [60.9%, 91.1%] on the
  `ok` stratum — with the test-retest agreement (11/16) beside it, so a reader can see
  both the number and how much to trust it.

- **The aligned multi-column option table: a row's long spelling is no longer
  eaten as the start of its own description.** A tool that lays its options
  out in columns — short spelling, long spelling, and (sometimes) a
  description — put the long spelling right where the description-boundary
  splitter cuts, at the row's first 2+-space gap. Measured on `main`:
  `nano --help`'s 52 flags each kept their short spelling only, with the long
  one glued to the front of their description (`-A` described as
  `"--smarthome Enable smart home key"`, `-C <dir>` as
  `"--backupdir=<dir> Directory for saving unique backup files"`), and
  `jdeprscan`'s `--list` and `--verbose` were lost outright, because their
  rows have no description column for the spelling to hide in. `awk`/`gawk`/
  `nawk` are the same shape aligned with tabs.

  The fix is one new shape test and one recurrence test, both in
  `help_text::sections`: a cell that is *nothing but* an option spelling —
  flag-shaped, and then either stopping or carrying a bare value placeholder
  (`--backupdir=<dir>`, `-C <dir>`) — is read as another spelling of the
  option the row already named, but only in a block where that second column
  actually recurs at the same character offset. Both halves matter, and the
  narrow one carries the safety: a description that merely *begins* with
  something flag-shaped (`-x   --foo is a synonym for --bar`) has real words
  in the cell, so it is never claimed. Recurrence then excludes the one false
  positive found over all 2,301 frozen captures in `audit/queue-captures/` —
  `lto-dump --help`'s default-value column, whose `-1` would otherwise be
  read as a short spelling, and whose three rows land at three different
  offsets.

  35–42 tools on this box's `PATH` show the shape. Deliberately **not**
  claimed: rows naming several shorts at once (`jdeprscan`'s `-? -h --help`),
  which `mandible_core::Flag` has one `short: Option<char>` and no field to
  hold; and (**since fixed** — see the next entry) rows whose spelling cell
  carries a *lower-case* value word (`awk`'s
  `-f progfile\t--file=progfile`). Two
  of nano's own 54 rows are also still missed for a third reason:
  `-%  --stateflags` and `-_  --minibar` are not `is_flag_shaped`, because
  `is_flag_char` allows alphanumerics plus `? # @` and neither `%` nor `_`
  is in that set. Widening it reaches every other user of `is_flag_shaped`,
  `xtask`'s misattribution oracle included, so it is left for its own
  change.

- **Valued cells in an aligned spelling column now pair too: `-f progfile`
  keeps its `--file`.** The residual the entry above named. A cell that is a
  spelling *plus a lower-case value word* is not an
  `is_value_placeholder_only` cell, and that check must stay narrow — a
  lower-case word alone is not distinguishable from prose, which is what
  keeps `arptables --help`'s `--append  -A chain` from being merged into one
  valued flag. So the pairing evidence here is a different one: **two
  adjacent cells that name the same value token.** `-f progfile` beside
  `--file=progfile`, `-d[file]` beside `--dump-variables[=file]`,
  `-v var=val` beside `--assign=var=val` — one option, spelled twice, with
  its value restated. `--append` names no value at all, so `-A chain` has
  nothing to match and that row is untouched.

  Measured over all 2,301 frozen captures in `audit/queue-captures/`
  (2026-08-22): 24 adjacent cell pairs across 5 tools satisfy the test and
  **every one is a genuine short/long alias pair** — `awk`/`gawk`/`nawk`
  (7 rows each: `--file`, `--field-separator`, `--assign`, `--source`,
  `--exec`, `--include`, `--load`) and `ntfsmove`/`ntfswipe`, which write
  the value detached on both sides with a real description after it
  (`-c num  --count num  Number of times to write`). No capture pairs two
  independent flags this way; the near misses correctly refused are
  `arptables`'s `-A chain`, `lsof`'s genuine three-column table, `objcopy`'s
  `--strip-symbols <file>   -N for all symbols listed in <file>`
  cross-reference, and `prove`'s `-a,  --archive out.tgz Store ...`.

  The recovered flag carries the shared value **once**: such a row is
  rejoined as *spellings, then the value* — every cell reduced to its bare
  spelling, and the value appended in the form the first cell that named it
  wrote it. Both halves are load-bearing. Rejoining both cells verbatim
  handed the flag grammar a detached value it terminated the alias list on,
  which lost `--source` from `-e 'program-text'\t--source='program-text'`
  entirely; and taking the value's *form* from the first cell is what keeps
  the rewrite from changing a flag that already parsed. `less --help` writes
  `-P [prompt]   --prompt=[prompt]`, where the short cell's brackets say
  optional and the long cell's `=` says required — keeping the long cell
  verbatim promoted `-P` from `Optional` to `Required` and left the brackets
  stranded inside the value's name. As shipped, `less`'s `-p`, `-P` and `-x`
  gain `--pattern`, `--prompt` and `--tabs` with `value_name` and
  `value_kind` byte-identical to before, and `awk`'s
  `-d[file]`/`--dump-variables[=file]` stays `Optional`/`file` while
  `-f progfile`/`--file=progfile` is `Required`/`progfile`.

  `corpus/awk/5.2.1` gains all seven long spellings in its `expected.snap`
  and names them in its `must_contain_flags` contract. Still **not** claimed,
  unchanged by this: `awk`'s multi-short rows, and its
  `POSIX options:\tGNU long options: (standard)` header line, which is read
  as a section heading and becomes every flag's `group` (the `uconv`/`nano`
  family, already queued).

  A full-`PATH` sweep before and after (2,254 tools matched) reports **zero
  flags lost and zero descriptions changed**; five tools change at all, all
  by gaining a long spelling on a flag that had none: `less`, `pager` and
  `zstdless` (the same `less` help text) plus `ntfsmove` and `ntfswipe`,
  whose `-c num  --count num  Number of times to write` rows *also* stop
  reading the long spelling as the first words of their own description.
  `awk`/`gawk`/`nawk` change too but are invisible to `xtask sweep-diff`,
  which splits a `#fp` line's flag list on `|` and so cannot represent
  `--lint`'s `fatal|invalid|no-ext` value; their fingerprints were compared
  by hand instead.

- **Two new corpus fixtures for the shape, `nano/7.2` and `awk/5.2.1`,** each
  with a `must_contain_flags` contract naming long spellings that did not
  reach the tree before this change (`--smarthome`, `--backupdir`,
  `--zero`; `--characters-as-bytes`, `--dump-variables`, `--copyright`).
  Both are `provenance = "agent"` with no `verdict_scope`. The corpus is 98
  fixtures: 82 ok (0 human, 39 agent-then-human, 43 agent), 16 xfail, 0
  failed.
  `corpus/jdeprscan/audit-seed2` stays `[xfail]` — `--list` and `--verbose`
  now pass, `-h` still does not — with its `meta.toml` note rewritten to say
  which half moved. `nano/7.2`'s `meta.toml` also records a *separate*,
  unfixed defect its snapshot freezes: nano's prose line `When a filename is
  '-', nano reads data from standard input.` is read as a section heading and
  becomes every flag's `group`.

- **12 new corpus fixtures from the seed-4 human audit, and a named list of
  the tools that audit skipped.** `xtask audit fixtures` stages a fixture
  directory per reviewed tool with the reviewer's own verdict note as the
  `[xfail] reason`; three of the fixtures promoted here (`bashbug`,
  `lessecho`, `vim.basic`) were `incomplete` verdicts, so each also needed a
  `[contract]` field that *currently fails* — an `[xfail]` asserting nothing
  is reported by `xtask corpus` as "the bug appears fixed", which is how the
  runner refuses to let a documented-broken fixture be documented-broken
  about nothing in particular. All three defects turned out to be the same
  shape and are pinned with `must_contain_positionals`: bashbug's
  `[bug-report-email-address]` operand, lessecho's `file ...`, and vim's
  `[file ..]` are each documented in the tool's own usage synopsis and each
  absent from the tree. The other nine are `correct` verdicts with a blessed
  snapshot (`provenance = "agent"`, no `verdict_scope` — an agent may claim
  neither). The corpus is 96 fixtures: 80 ok (0 human, 39 agent-then-human,
  41 agent), 16 xfail, 0 failed.

- **`xtask audit report` now names the tools a reviewer verdicted `skip`,
  with the reason where one was recorded.** The stratum table printed a
  per-stratum `skipped` count and nothing more, while `accuracy_over`
  excludes every one of those entries from every accuracy figure in the
  report — so a reader could see that nine tools left seed 4's denominator
  but not which nine, and the exclusion was unauditable. `skip` is the one
  verdict that does not require a note, so the section prints an explicit
  `(no reason recorded)` rather than inventing a justification; 6 of seed
  4's 9 skips carry no reason. `audit/4-report.txt` is committed alongside
  `audit/2-report.txt` as the rendered record.

- **Every corpus fixture now carries a required `[bless] provenance` field —
  `human`, `agent-then-human`, or `agent` — recording who blessed its
  `expected.snap`, the complement `verdict_scope` never had.** `verdict_scope`
  says what a human reviewed; it says nothing about the fixtures nobody
  reviewed at all, and that was most of them — AGENTS.md's own v0.3.1
  measurement found only 3 of 23 newly-passing fixtures carried a human
  `verdict_scope`, the other 20 were agent-blessed trees the suite guarded
  against *changing*, never against being *wrong*. Without this field, "N
  fixtures ok" and "N fixtures human-verified" were indistinguishable in any
  summary, which is exactly the overclaim `verdict_scope` was built to
  prevent for review *scope* but never for the bless act itself.

  The field is required, not optional with a silent default: `xtask corpus`
  fails to load a fixture missing it, naming the file and pointing at
  `corpus/README.md`, rather than let an absent value read as "unknown" (or
  worse, get inferred as reviewed). `xtask corpus`'s summary line now splits
  its `ok` count by provenance (`71 ok (0 human, 39 agent-then-human, 32
  agent)`), `--show <fixture>` and the `--format markdown` report both
  surface it alongside `scope`, and `xtask audit fixtures` always emits
  `provenance = "agent"` for a fixture it generates — an agent may only ever
  write that value; flipping it to `human`/`agent-then-human` is a human-only
  act, mirroring the rule `verdict_scope` already enforces.

  All 84 existing fixtures were backfilled by git-history review, and the
  first thing the field recorded is a result worth stating plainly: **not
  one fixture in this corpus has a human-blessed `expected.snap`.** 39 are
  `agent-then-human` (a human's seed-2 audit `verdict_scope` covers them,
  but an agent wrote the bytes), 45 are `agent`, and 0 are `human` — the
  hand-authored `git`/`tar` seed fixtures included, because their current
  snapshots were re-blessed by later grammar-fix commits. Every bless commit
  in this repository's history carries a `Co-Authored-By: Claude` trailer;
  attributing any of them to a human would have been exactly the overclaim
  the field exists to make impossible.

### Fixed

- **A flag is now associated with the description prose it owns in two
  shapes the grammar used to drop it in.** Both are cases where the text was
  sitting right there in the tool's own `--help` output and never reached the
  flag it belongs to.

  The first is **a description written one space after the spec**, which is
  what a long flag name does to a fixed-width option table: the name overruns
  the description column and the formatter emits a single space instead of
  the padding it can no longer supply. `find_description_gap` found no column,
  the whole line went to the flag grammar as the spec, and its ` VALUE` arm
  took the first word of the prose as a `value_name` and discarded the rest —
  `--md5 Control MD5 generation` became `--md5` valued `Control`, with "MD5
  generation" gone from the tree entirely. `find_sentence_start_gap` is a
  sibling of the existing `find_placeholder_boundary_gap` under the same
  precondition (consulted only when no 2+-space column exists anywhere in the
  line, so no already-working split can move), and it is deliberately narrow
  because the inverse case is the whole risk: the candidate word must be an
  initial capital followed by nothing but lowercase, must have at least one
  more word after it, and the scan stops at the first token that is neither
  that nor value-shaped. `PATH`, `7|8|9|10|11`, `<manifest-path>` and a
  lowercase metavar followed by a capitalized word deeper in the line all
  keep parsing as values, each with its own test.

  The second is **a description written as a flush-left prose paragraph
  naming the option**, the way `jdeprscan` documents all eight of its
  options ("The --for-removal option limits scanning or listing to APIs
  that are deprecated for removal."): its `options:` block has no
  description column at all, so it measured 8 flags, 0.0% with text.
  `backfill_prose_paragraph_descriptions` is a third pass over the assembled
  flag list, alongside the two flag repairs already there. It never creates
  a flag (a paragraph naming an option the table does not list — like
  `apt-ftparchive`'s `--source-override` — cannot fabricate one) and never
  overwrites a description the table already supplied (`apropos` describes
  `--regex` in both places; the table wins). A sentence *indented under*
  another flag's row is that row's continuation text and is never lifted out.

  A third case came out of reviewing the sweep field by field, and is the
  reason the boundary is not simply "the first sentence-shaped word":
  `mariadb`'s `--init-command=name SQL Command to execute ...` split after
  `SQL`, and since the spec already had its `=name` value (first value wins)
  nothing ever read `SQL` back out and the word was dropped. A spec carrying
  its own inline value cannot take another one, so the boundary is now fixed
  at that token and every word after it is description, value-shaped or not.

  Measured on a full `PATH` sweep of this box: 94.64% → 95.54% of flags carry
  text across 2,308 tools, 101 tools changed, and every other counter in the
  scoreboard is byte-identical (no-tier, suspicious, verbatim, man-shaped,
  ok-with-zero-flags, misattribution, existence-fabrication, bundle-collapse,
  framework detection). Five tools moved `low-confidence` → `ok`; none moved
  down. **Zero flags lost fleet-wide**; 7 tools gained one each, every one of
  them a false alias merge coming apart (below). Every changed field on all
  101 tools was checked against the tool's own frozen `--help` capture. In
  the corpus, `curl/8.5.0-all` alone gains 59 descriptions, each replacing a
  fabricated `value_name`.

- **`pair_aliases` no longer unifies two rows that disagree about taking a
  value.** Recovering `ld`/`gold`'s real descriptions made two genuinely
  separate options collide on one shared sentence —
  `--allow-multiple-definition Allow multiple definitions of symbols` and
  `-z muldefs  Allow multiple definitions of symbols` — and pairing them
  destroyed one of the two while giving the survivor
  `--allow-multiple-definition muldefs`, a value neither row documents.
  Measured on the same sweep: 7 tools, 1 flag each, and the only losses in
  it. Narrowing it also un-did three false merges that predate this branch,
  each two unrelated options sharing one sentence: `as`'s `-w` absorbed into
  `--hash-size=<N>` (both "ignored"), `lto-dump`'s `--help` absorbed into
  `-Waggressive-loop-optimizations` (both "[enabled]"), and `gold`'s `-z defs`
  absorbed into `--no-undefined`. Pairing now also requires the two rows to agree about taking a value at
  all; coarse on purpose (`ValueKind` only, never the placeholder's
  spelling), since a source may legitimately name the metavar on one row and
  not the other. Same failure shape as the `lto-dump` incident the
  single-dash exclusion already documents: a description common enough to
  collide.

- **`xtask sweep-diff` now diffs each tool's flags, choices, and subcommands
  by content, not just by count.** During PR #14 the same run that deleted
  `pngfix`'s and `pod2man`'s flag descriptions and fabricated a choices list
  on them (see the `nested_entry_table_starts_at` entry above) reported as
  "identical" — every existing scoreboard column (`flags`, `%flags_text`,
  `status`) is a count, and neither flag was added or removed, so nothing
  about that run moved any of them.

  `xtask coverage`'s `ScoreFormat::Text` scoreboard now carries a `#fp`
  footer, one line per tool, fingerprinting every flag (a stable identity
  independent of `value_name`, description presence, a hash of the
  description text, a hash of the choices list, and `value_name` itself) and
  every subcommand path — full text is never duplicated into the scoreboard,
  only enough to detect a change (`coverage::build_fingerprint`,
  `coverage::fingerprint_lines`). `sweep-diff` reads it back
  (`transition::parse_scoreboard`) and reports, per tool, exactly which flags
  were added/removed, which flags' description/choices/value_name changed,
  which subcommands were added/removed, and any tier/framework change — a
  new "Field-level changes" section in both `--format text` and `--format
  markdown`, alongside the existing status/flag-count/appeared/disappeared
  sections, which are unchanged. A scoreboard from before this footer existed
  still loads (`ParsedScoreboard::fingerprints` stays empty for it) and is
  reported as field-diff-unmeasured rather than silently read as "no
  changes." The report's own `Overall: IDENTICAL`/`CHANGED` line now accounts
  for field-level content too, so a run that only edits a description's text
  no longer reads as identical — still non-blocking (maintainer decision D4:
  `sweep-diff` exits `0` regardless, same as before).

- **`sweep-diff`'s field-level section no longer goes silent on a tool that
  loses every flag.** The `#fp` footer above shipped skipping the line
  entirely for a row with no flags and no subcommands, on the assumption
  that "nothing to fingerprint" and "not fingerprinted" were the same case.
  They aren't, and a real two-sweep diff (2,254 tools) found both costs at
  once: roughly a quarter of the fleet (verbatim tools, zero-flag `ok`
  tools) reported as field-diff-unmeasured instead of measured-with-nothing,
  and a tool that had flags before and loses every one of them produced a
  `#fp` line on the "before" side and none on the "after" side — read as
  unmeasured instead of every flag removed, going quiet on exactly the
  regression direction this fingerprint exists to catch (the flag-count-loss
  section still caught it independently, so this was never a detection
  hole, only a misleading message on the section built specifically because
  counts can lie). `coverage::fingerprint_lines` now emits a `#fp` line for
  every row unconditionally; `transition::diff` now tells apart "both sides
  measured" (diff normally, including an empty side reporting every flag on
  the other side as added/removed), "neither side has an entry" (the
  genuine legacy case — scoreboard predates the footer entirely, reported
  field-diff-unmeasured as before), and "one side only" (read as an empty
  fingerprint on the missing side rather than as unmeasured).

- **A nested command table no longer folds into the flag description above
  it.** `scan_flags_block`'s continuation rule was pure indentation: any line
  deeper than the block's own entries continued the previous flag's
  description, no matter what it actually was. `btrfs --help` puts
  `--help`/`--version` at indent 2 and then, after a blank line, a large
  command table at indent 4 whose rows each carry their own description one
  indent deeper (indent 8) — the whole table, dozens of lines, folded into
  `--version`'s description as one long run-on sentence.

  A new shape-based, repetition-gated detector
  (`nested_entry_table_starts_at`) now looks ahead from a candidate
  continuation line for at least two name/description row pairs at the same
  indent before treating it as a table rather than prose — a single ragged
  continuation line still reads as an ordinary wrapped description, only
  genuine repetition ends the block early. All seven of `btrfs --help`'s
  real flags now parse with only their own description
  (`corpus/btrfs/audit-seed2`); the command table itself is recovered as
  subcommands by the headingless-invocation-table entry below, which is
  what clears that fixture's `[xfail]`.

  That detector's first version broke a different, equally real shape: a
  flag with **no inline description of its own** (`pngfix --strip=[none|
  crc|unsafe|...]:`, `pod2man --guesswork=rule[,rule...]`) whose entire
  description is the deeper-indented block below it — a value-choice list
  or keyword list that can itself look table-shaped once a long choice's
  own wrapped continuation line, or a genuine bare-word keyword list,
  supplies the "row followed by something deeper" pattern the detector
  looks for. Breaking there doesn't mis-split, it deletes: the flag has
  nowhere else for that text to go, so `--strip` and `--guesswork` were
  each left with an empty description (`--guesswork` also fabricating a
  bogus choice list from whatever came after the wrongly-ended block). The
  break is now gated on the entry row actually being continued: it only
  fires when that row already carries its own non-empty description on its
  own line, which is exactly the shape `--version`'s `print version
  string` has and `--strip`/`--guesswork` do not. New fixtures
  `corpus/pngfix/1.6.43` and `corpus/pod2man/5.01` cover it; a full-`PATH`
  sweep confirms every one of the six tools the first version moved
  (`jpackage`, `less`, `pager`, `pngfix`, `pod2man`, `zstdless`) now parses
  byte-identically to before that detector existed.

- **`btrfs --help`'s headingless command table is now recovered as
  subcommands, two levels deep.** `scan_flags_block`'s nested-entry-table
  detector (previous entry) correctly ends the flags block where the table
  starts, but the table itself was then silently dropped — no heading
  introduces it, and every other command-recovery path in the generic
  layout parser requires one (spec §7 Tier B rule 1). A new recognizer
  (`help_text::sections::scan_headingless_invocation_table`) admits a run
  of rows instead when every row starts with the tool's own name at a word
  boundary — the evidence a heading would otherwise supply — and at least
  two rows repeat the name-row/deeper-description-row shape. `btrfs device
  add ...` reads as child `device`, grandchild `add`; consecutive sibling
  rows sharing one following description (`device delete`/`device remove`)
  share it; every emitted name is checked to occur literally in the raw
  text (spec [M-10]).

  Recovered nodes carry a new, second attestation bit,
  `CommandNode::invocation_attested` — existence-attested (unlike a
  fabricated phantom subcommand) but deliberately **not** probe-eligible:
  spec §6's `--help` probe gate keeps reading `heading_attested` only, so
  none of this subtree is ever sent as argv, only `heading_attested` is
  (spec §6 rule 0's closing paragraphs, and a new §7 Tier B subsection,
  record the decision). The coverage harness's structure-sanity and
  attestation-gated-stub detectors (`xtask::status`, `xtask::audit`) accept
  either bit as evidence of a real command, so these nodes are never
  mis-flagged as fabrication; the [M-10] existence detector
  (`xtask::existence`) gained a matching `tool_name_prefixed_row_words`
  rule so it agrees rather than reporting every recovered node as invented.

  `corpus/btrfs/audit-seed2` flips `[xfail]` → `ok` (17 top-level groups,
  most with their own grandchildren). The pngfix/pod2man near-miss set from
  the previous entry stays byte-identical — neither of those flags' choice
  lists starts with the tool's own name, so this recognizer never reaches
  them.

- **A full-`PATH` sweep's scoreboard write no longer fails `EACCES` inside
  namespace containment.** `xtask coverage`/`audit freeze` re-exec under
  `unshare --user --map-root-user` before probing (spec §6/§8); GitHub
  Actions run 32063212492 showed all 16 `path-sweep.yml` shards finish their
  full sweep and then die writing `shard-N.md`, because the checkout
  directory is owned by a UID the namespace doesn't map, so the contained
  "root" has no `CAP_DAC_OVERRIDE` over it. The pre-exec process — which
  still has ordinary filesystem access — now opens `--out` and clears
  `FD_CLOEXEC` on it before entering containment, and the contained process
  writes the scoreboard through that inherited fd instead of reopening the
  path. Uncontained runs, `--tools`-pinned runs, and non-Linux platforms are
  unaffected (`mandible_extract::exec::containment::secure_out_file`,
  `enter_or_refuse_with_scoreboard`, `write_scoreboard`).
- **`path-sweep-summary` now fails when zero shards reported.** Previously the
  step always exited 0, so a completely dead sweep (all 16 shards killed)
  still painted a green tick — the exact "tick asserting something false"
  failure this project already burned itself on once. 1..15 shards missing
  is unchanged (still reported as partial coverage, still green); 0 shards
  is now a distinct case that fails the job.

### Changed

- **`.deb`/`.rpm` packages are now built for aarch64 as well as x86_64**,
  natively on `ubuntu-24.04-arm`, alongside the existing x86_64 build. Each
  architecture's package is verified after building: its declared
  `Architecture:`/`%{ARCH}` field is asserted against the runner it was
  built on, and the `.deb` is installed with `dpkg -i` and smoke-tested with
  `mandible --version` / `mandible --doctor tar`. Package artifacts are now
  uploaded per-architecture (`mandible-packages-x86_64`,
  `mandible-packages-aarch64`) rather than under one shared name.

## [0.3.2] - 2026-08-17

Two user reports drove this release, and each was measured to its mechanism
before anything was changed: a background warm that pegged CPU through a
quadratic search-index rebuild, and a `cargo install` that could push a
small machine into memory exhaustion — the latter fixed mostly by
resurrecting install paths that already existed but were invisible.

### Fixed

- **Your container names are no longer shown as docker commands.** Reported
  from real use: `mandible docker` rendered the reporter's own running
  containers as subcommands of `docker stop`, `docker rm` and friends.

  The cobra tier probes `<tool> __complete <path> ""` and trusted every
  candidate it got back as a subcommand name, on the documented premise that
  an empty word returns subcommands only. That premise is wrong at a leaf:
  cobra emits the node's real subcommands and then *appends whatever the
  command's own completion function returns*, which is application code
  reading live state. So `docker __complete stop ""` answers with running
  container names, `docker __complete run ""` with image names, and
  `docker __complete network rm ""` with network names — private data, drawn
  as commands. Each fabricated node was then warmed like any other, so the
  probe count grew with the size of *your* data rather than with the tool.

  A candidate list now becomes subcommands only when **every** candidate in
  it carries a description. cobra writes real subcommands as
  `name<TAB>description` from its own formatter, while a completion function
  returning a plain list of strings produces bare rows; one bare row
  condemns the whole list, because cobra marks no boundary between the two
  halves. Measured across 631 real command paths on docker 29.7.2 and gh
  2.45.0: 85 fully-described lists, all genuine subcommand lists, and 50
  bare-or-mixed lists, all argument data — every real subcommand kept, every
  argument value dropped (spec Appendix A [M-2a]).

  The trade is deliberate and one-directional: a rare real subcommand whose
  author left its short description empty, sitting in a list that also
  carries argument values, is dropped here. The `--help` tier still finds
  it, and a missing rare subcommand is a far smaller harm than rendering
  your containers as commands.
- **The fuzzy search index is rebuilt once per batch of warmed nodes instead
  of once per node**, removing a quadratic term from background warming.
  Every arrival from the warmer used to restart the index and re-inject the
  whole, growing tree; arrivals are now coalesced into at most one rebuild
  per event-loop iteration, throttled to one per 250ms while nobody is
  searching. A deferred rebuild is never dropped, and the throttle is
  bypassed whenever a query is active or the search box is focused, so search
  never lags the tree it is searching (spec §5.2). Rendering is unchanged.

  Measured cost of the removed term, replaying arrivals against a synthetic
  tree: **99ms at docker's ~255 nodes, 395ms at 510, 1.62s at 1,020, and
  17.7s at spec §5.2's 4,096-node cap — against 6.6ms for a single batched
  rebuild at that cap.** Clean n², so the bigger the tool the worse it got.

  Read the end-to-end number carefully, because it is much smaller than that
  suggests. Holding `mandible docker` open in a real pty on a 4-core box,
  process CPU over the 22-second warm went **6.07s → 5.72s**, all of it on
  the UI thread (**1.13s → 0.51s**). The remaining ~5.1s is the warmer
  threads doing the extraction the warm exists to do — ~10 threads flat out
  on 4 cores — which is where any further work on warming CPU belongs. The
  index storm was a real bug and is gone; on docker it was 10% of the CPU,
  not the bulk.
- `nucleo`'s matcher thread pool is capped at 4 threads instead of defaulting
  to one per core. Secondary and precautionary: the item set is bounded by
  the node cap, so past a handful of threads a rebuild's coordination costs
  more than its parallel scoring saves. Not measurable on a 4-core box, and
  not the fix above.
- **The README's pre-built binary links were four literal `(...)`
  placeholders** while the install section led with `cargo install` — so the
  one user path that avoids compiling entirely was invisible. The table now
  points at the stable `releases/latest/download/` asset URLs (verified
  live), documents `cargo binstall mandible` (verified resolving against a
  real release), and the from-source path carries a note for RAM-backed
  `$TMPDIR` systems (`CARGO_TARGET_DIR=… cargo install mandible -j 2`),
  where a `cargo install` was measured pushing a small machine toward
  memory exhaustion (peak ~1.2 GB of concurrent rustc RSS at `-j 4` plus a
  ~470 MB transient build tree, which lands in `$TMPDIR`).

### Changed

- **The background warmer runs one probe per core (clamped `[2, 8]`) instead
  of four per core (clamped `[4, 32]`).** The oversubscription assumed a
  warming job blocks on its child costing no CPU — true for a typical small
  C tool, measured false for `docker`, whose CLI burns 70–100ms of real CPU
  per spawn. Sixteen concurrent probes on a four-core machine was the warm
  pegging every core for minutes. The warm now takes longer on cheap-probe
  trees, in background time nobody waits on; user-expanded nodes still jump
  the queue, so the visible tree fills as fast as before.
- **Release binaries are stripped** (`[profile.release] strip = "symbols"`):
  5.6 MB → 4.0 MB measured on aarch64.

## [0.3.1] - 2026-08-15

On the 94-tool development set, re-reviewed blind in the TUI with no prior
verdicts or notes shown: **tools judged outright `wrong` fell from 27 to 7.**
`incomplete` did not move (23 → 25).

That categorical collapse is the claim this release makes. It is deliberately
stated as counts on a named set rather than as an accuracy percentage: these
94 tools are the ones every fix here was developed against — several against
their exact captured bytes — so any ratio computed from them is a dev-set
figure carrying an unquantified upward bias, and dressing it in a confidence
interval would only lend sampling precision to a number whose error is not
sampling error. For the record and not as a headline, the correct/judged
counts are 53/85 here against 30/83 at 0.3.0, on different denominators.

**No fleet accuracy figure is claimed, and `spec.md`'s `[M-20]` remains
deliberately unfilled.** That measurement requires reviewing a fresh draw of
*unseen* tools from the frozen queue this release ships (`audit/queue.toml`,
2,299 tools, cursor 0); until then the project states no accuracy number, and
this section is not one.

Read the flat `incomplete` count both ways. Tools stopping being mangled
before they stop being partial is the shape a grammar fix should have — and
the residual is now dominated by defects grammar cannot reach: positional
arguments, `ar`-style modifiers, and environment variables are all cases
where the extracted tree has no slot for the kind of thing being documented.

Every fleet number below was measured on a single aarch64 Ubuntu 24.04
machine's `PATH`, and is a property of that installed tool set.

### Fixed

- **A bare-word block no longer swallows the flag table that follows it.**
  A block of enum values or operands ended only when a line dedented below
  it, so a tool that nests such a list *inside* its options table and then
  resumes the table at an equal-or-deeper indent had every flag from that
  point on recorded as a *choice* instead. `tar --help` lost `--old-archive`,
  `--pax-option` and `--posix` to the `FORMAT is one of the following:` enum
  under `--format` — in a corpus fixture that was green, blessed and
  contract-gated for its entire life while missing them, because the flags
  were not absent from the tree, they were in the wrong field. `sg_dd` lost
  `--progress` and `--verify` outright and reached the tree with its four
  surviving flags stripped of every description.

  A bare block now ends where a flag row resumes. This is the removal of an
  inconsistency rather than a new heuristic: the section engine already
  reads a flag-shaped line as a headingless flags block, and the usage-block
  scan already ends on that same signal. The break is non-destructive — the
  parser resumes at that exact line, so a wrong break re-routes a tail and
  never drops it. Scanned across all 81 corpus fixtures before landing:
  exactly two trees change, and both are the two defects above.

- **Single-dash long options keep their real names.** A tool that spells a
  long option with one dash — `qemu-arm64-static`'s `-help`, `gcc`'s
  `-pass-exit-codes`, and essentially all of `ffmpeg`'s CLI — had every one
  of those options read as its own first character carrying the rest of the
  name as a required value: `-help` became `-h` taking a value literally
  named `elp`, `-cpu` became `-c` + `pu`, `-print-search-dirs` became `-p` +
  `rint-search-dirs`. The real option was in the tree under no spelling a
  user could type. A `PATH` sweep measured **132 tools and 8,784
  flags**, the largest remaining defect signal by a wide margin. Both
  weightings, since they differ a lot and only quoting the larger one would
  overstate it: 17.6% of every flag extracted, but 5.7% of tools (132 of
  2,299) — the flag share is inflated by a few enormous option tables,
  `ffmpeg` alone contributing 45 of the recovered options.

  The repair (`help_text::sections::repair_single_dash_long_options`) is a
  post-pass over each node's assembled flag list, admitting a flag on the
  same seven conditions the `single-dash-long` detector counts the defect
  with, character for character. Two of those conditions carry the whole
  safety argument. The flag must be **option-table-sourced**, which keeps the
  bundled-short population (`rpcbind`'s `[-adhilswfr]`) out; and the
  reconstructed token must be **uniformly lowercase**, which is the only
  thing separating this family from the GCC/Clang glued-value convention —
  `cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8`, `perl
  -Idirectory`, `cc -oOUTFILE` — thousands of **correct** parses fleet-wide
  that a looser rule would have converted into fabricated long options. The
  case test reads the whole token rather than the tail, because `-oOUTFILE`
  has a lowercase flag letter and only its argument shouts.

  Replaying all 81 corpus fixtures before and after changed exactly three —
  `qemu-arm64-static` (11 options recovered), `gcc` (18) and `ffmpeg` (45) —
  and left the other 78 byte-identical, including all 30 tools a human
  reviewer judged correct and both of the family's declared out-of-scope
  misses (`ip`'s bracketed `-h[uman-readable]`, `sg_emc_trespass`'s
  `-hr:`). `corpus/qemu-arm64-static/audit-seed2` is promoted out of
  `[xfail]`, and the family is now ratchet-gated at zero alongside the other
  two that share its structural fingerprint.

- **mandible no longer starts daemons on the machine it is documenting.**
  Running `mandible blkmapd` — or any of a large class of system binaries —
  started an NFS daemon that outlived the process. **622 leaked processes**
  were found on one developer box, the oldest five days old: `blkmapd` ×148,
  `rpc.idmapd` ×144, `rpc.gssd` ×144, plus `sudo_logsrvd` holding
  `0.0.0.0:30343` and `[::]:30343`, `guacd` holding `127.0.0.1:4822` for five
  days, and `pam-auth-update` burning a full core for three days. This was a
  defect in the shipped tool, not in the test harness: the TUI builds its
  runner from the same tier stack.

  The cause was a speculative probe. `completion` and `zsh` are a *framework
  protocol's* words — a subcommand invocation to a tool that speaks the
  protocol, two ordinary positionals to one that does not. Sending them
  unasked to every binary on `PATH` is the bare invocation the execution
  policy has always prohibited, arriving through the list of "inert shapes"
  because every shape on that list had been validated against tools that
  *parse* their arguments. A daemon that ignores its arguments and starts
  anyway falsifies that premise. 437 of the 622 survivors came in through
  this one argv (219 `completion zsh`, 218 `completion bash`).

  The completion-script tier now asks for that argv only when the tool itself
  provides evidence the subcommand exists — either the `spf13/cobra` marker
  in the compiled binary (cobra registers a `completion` command itself, and
  may hide it, so the bytes are the only evidence available) or the tool's own
  `--help` naming `completion`/`completions` as a command. Evidence is read
  from the artifact or from the tool's own output, never from a list of tool
  names.

  Two other symptoms of the same probe are fixed with it. `docker-proxy
  completion zsh` left Go's `flag` package stopped at the first non-flag
  argument, so it attempted to bind `0.0.0.0:-1` and wrote its startup error
  to a terminal it did not own, aborting full sweeps. And the probe was two
  thirds of extraction time: over 94 audited tools, three of them accounted
  for 85.3s of 129.5s. Measured before → after, with every other column of
  the scoreboard byte-identical: `vim.basic` 20 304 ms → 287 ms, `bashbug`
  20 657 ms → 49 ms, `jconsole` 40 081 ms → 22 065 ms (its remainder belongs
  to a different, unrelated probe).

- **A probe that daemonises no longer outlives the probe.** Behind the fix
  above, as the second layer: killing the probe's process group cannot reach
  a child that has called `setsid`, and the leak was never a hang to begin
  with — every one of 2,302 traced probes returned normally, with its escapee
  already gone. mandible now adopts orphaned descendants instead of leaving
  them to init, identifies the ones a given probe started by a token in the
  environment it inherited, and kills and reaps them before that probe is
  reported complete. Bounded by rounds and a wall-clock budget, so a process
  that cannot be killed costs milliseconds rather than becoming a new way to
  hang. Linux only; elsewhere the previously documented residual risk stands.

- **Bundled short flags in a usage synopsis are read as the set of switches
  they are.** A line opening `[-2CDlNuVv]` names eight boolean flags in the
  ordinary getopt convention; the grammar read it as one flag, `-2`, carrying
  a required value named `CDlNuVv`, so one switch survived and the rest were
  destroyed. Measured across this machine's 2,302 `PATH` tools by the
  bundled-short-flag oracle added last release: **58 tools, 465 destroyed
  flags**, an average of 8 lost per affected tool and 22 at the worst
  (`groff`'s `[-abcCeEgGijklNpRsStUVXzZ]`). `tcpdump` alone recovers 25.

  The split is keyed on the shape of the swallowed text — a run of distinct
  single-character flag names, either alphabetized or spanning both cases —
  never on a tool name, and it is asked only of a synopsis token. Two other
  defect families produce the identical stored shape and are deliberately
  untouched, because in both of them the parse is already correct: the
  GCC/Clang single-dash convention (`-Zscript`, `-Idirectory`,
  `-fdump-scos`), where the glued text really is a value, and flags repeated
  to mean "more of it" (`-vv`, `-DDD`). A two-character cluster is left alone
  as well — the fleet's two-character population is about half real collapses
  (`ssh-keygen`'s `[-hU]`) and half genuine multi-character flags (`xxd -ps`,
  `rpcgen -Ss`), with nothing separating them on shape, so that recall is
  given up rather than risk splitting a working tool.



Recovery work, mostly. Several tools were reporting confident results over
documents mandible had never actually read, and the fixes for that are the
bulk of this release. Across the PATH sweep, flags carrying text moves
94.18% to 94.82% on ~2,290 tools, but the aggregate is the least interesting
number here: 23 tools stopped claiming to be complete when they were not,
13 stopped being unparseable, and `curl` went from 12 flags to 258.

Accuracy remains unmeasured. Every figure in this project counts things; none
of them yet checks whether what was extracted is correct. The audit tool added
below exists to answer that, and it has not finished running.

### Added

- **A corpus fixture can now state that a flag does *not* exist:
  `must_not_contain_flags`.** Every `[contract]` field until now was a
  positive claim — it named something a real tool really has and failed
  when the parser dropped it. That covered the omission half of what can go
  wrong and none of the invention half, so a phantom flag was a defect no
  fixture could pin, and a defect that cannot be pinned cannot announce its
  own repair through strict xfail. The live instance:
  `mariadb-check --help` prints a `Variables (--variable-name=value)`
  defaults table whose header ruler is read as a flag row, and the tree
  gains an option whose long name is thirty-one `-` characters. The
  existence oracle is correctly silent on it — its question is "does this
  spelling occur in the raw text", and the ruler does, literally.

  The field is matched by exactly the matcher `must_contain_flags` uses,
  negated, root flags only; it asserts nothing about the raw capture,
  nothing about the spelling an entry did not name, and nothing below the
  root. Dropping an entry is a weakening and is reported by
  `xtask corpus --baseline-dir` as `CONTRACT WEAKENED`, the same as
  dropping a positive one. New fixture `corpus/mariadb-check/2.7.4` is its
  first user, `[xfail]` on that ruler.

- **`mandible --report <tool>`.** Assembles a paste-ready bug report: the
  mandible version, the tool's version when it can be recovered, the
  `--doctor` diagnostic, and the raw `--help` capture, followed by the issues
  URL. It goes through the same sanctioned probe path as everything else and
  adds no new argv shape. Most tools never print their version in `--help`,
  so that line usually asks the reporter to supply it rather than guessing.

- **`mandible --review <seed>`.** The audit review loop, run inside the real
  interface. Each sampled tool opens exactly as `mandible <tool>` would, a
  verdict is a keypress, and the manifest is written after every one, so an
  interrupted session resumes instead of restarting.

- **The audit instrument (`xtask audit`).** A bounded, random, human-reviewed
  sample comparing what mandible extracted against what the tool actually
  prints. This is the first thing in the project that measures agreement with
  truth rather than with the parser's own prior output. It draws a
  deterministic stratified sample, presents raw text beside the parse, records
  verdicts with notes, reports accuracy per stratum with confidence intervals
  and never as a bare percentage, and turns each reviewed tool into a corpus
  fixture. Known defect classes are pre-tagged so a reviewer confirms them
  once instead of re-deriving them per flag.

- **Truncation-confession detection (spec §6 rule 2b).** Some tools admit in
  their own output that what you just read is not everything. `curl --help`
  ends with `For all options use the manual or "--help all".` mandible now
  recognizes that convention, closed and content-keyed, never keyed on a
  tool's name, and re-probes with exactly the word the tool printed. `curl`
  recovers 258 flags instead of 12.

  This added an `InertArgv` shape, so it went through spec §6's amendment
  process: `--help` always precedes the word, so a getopt that stops at the
  first non-option still reaches it; the word is copied from the tool's own
  already-trusted output and never fabricated; expansion happens at most once
  and is never chained into a confession printed inside the expanded document;
  and it is refused outright for any tool on the never-probe list, with no
  special case (`pkill` confessing does not unlock `pkill --help all`).

  Where a confession is detected but cannot be followed, the tool's status
  caps at a new **`incomplete`** rather than reporting a confident `ok` on a
  document the tool itself called partial. Two further shapes are detected
  but deliberately not followed yet: ffmpeg's unquoted `-h full` table row and
  gcc's `--help=<class>`. Following either means new argv shapes, each needing
  its own deliberation. Detecting them moved 23 tools to `incomplete`: the
  whole gcc and clang family, plus ffmpeg.

### Fixed

- **The parser was reading the wrong stream.** Which of stdout and stderr the
  parser read was decided by "stdout if non-empty, else stderr". `openssl cmp
  --help` prints two diagnostic lines to stdout and its entire help to stderr,
  so the parser received the banner and discarded the document. Roughly 150
  openssl subcommands share that shape. Each stream is now judged on its own
  and the help-shaped one is parsed. Measured across the full sweep: no tool
  lost a single flag, 11 tools gained 169 flags between them, all from zero,
  and 13 moved from `verbatim` to `ok`. Recovered outright include `mkfs.fat`
  and its aliases, `tune2fs`, `btrfs-convert`, and `xfs_scrub`.

- **The verbatim pane now shows what the probe received.** It previously ran
  the text through the same sanitizer that prepares strings for the data
  model, which collapses runs of whitespace, so column alignment was destroyed
  and the pane could never match what a terminal shows. Indentation and
  alignment are preserved, both streams are displayed and labelled, and only
  terminal control sequences are neutralized. This is what exposed the stream
  bug above, one commit after it landed.

- **`--doctor` was computing a superseded metric.** It divided described flags
  by all flags, a definition replaced some time ago by one that excludes flags
  which could never carry text. So `mandible --doctor git` printed
  `0.0% described`, reading as total failure, where the truth is that nothing
  in git's help is describable at all. It now reuses the same accessors the
  scoreboard does, prints a dash rather than a zero when nothing is
  describable, and states `accuracy: unmeasured` in its own output.

- **The background warmer could hang the interface.** `systemctl <anything>
  --help` returns the root help byte-for-byte at any depth, so every
  subcommand appeared to have the same 18 children and the warmer expanded 18
  to 18² to 18³ until it hit its cap, starving the interface thread. Root text
  is now cached and a subcommand whose output is byte-identical to it degrades
  to verbatim.

- **Multi-column option tables and wrapped usage lines** parse correctly, and
  flag spellings are recovered from usage synopses where the flag table omits
  them.

### Changed

- **`pct_described` is now `pct_flags_with_text`,** and the scoreboard prints
  `accuracy: unmeasured` beside it. The old name claimed something the number
  never measured: it counts whether a flag has text attached, not whether the
  text is right. `lsof` scored 79% under it while roughly a quarter of its
  flags were actually correct. The denominator also changed, to exclude flags
  whose only source is a usage synopsis and which therefore could never carry
  a description.

- **Tests run under `cargo nextest`,** with a separate `cargo test --doc` step
  because nextest cannot run doctests. The rule behind it is that test results
  are read by machine and never by grepping the human-readable output. A
  `grep -c FAILED` had matched test data containing the word FAIL and reported
  a confidently wrong count.

- **Contributor documentation** leads with what a reader can do rather than
  what the project requires them to know, and both issue templates are forms
  that guide rather than briefs to absorb.

### Known defects

Stated plainly, because they are visible in normal use:

- **Single-dash long options are mis-parsed.** GCC-family options like
  `-fdump-scos` are stored as a short `-f` carrying the value `dump-scos`, and
  openssl's `-help` becomes `-h` with the value `elp`. This affects gcc, clang,
  lto-dump, openssl and their relatives, which is a large share of any
  developer machine. It predates this release. It is the first grammar item
  scheduled once the audit finishes.

- **Node descriptions are unreliable.** Of the ten curated corpus fixtures,
  four carry a correct description, three carry something wrong (a section
  heading, the tool's own error output, a version banner) and two carry none.

- **Subcommands discovered from a compiled artifact are not probed.** A name
  read from a cobra binary's own command table is refused by the gate that
  exists to stop invented names becoming arguments, so tools like `git-lfs`
  show their subcommands as bare stubs.

## [0.2.2]

Two general parser fixes. Between them, described coverage across the PATH
sweep moves **89.23% → 94.18%** on 2266 tools, with no tool losing anything.

### Fixed

- **Tab-aligned entry tables had no description column.** `find_description_gap`
  looked only for runs of two or more *spaces*, so a tool separating its
  columns with tabs looked like it documented nothing. `mokutil --help` was
  reported as **38 flags, 0 described** while every description sat plainly in
  the output; it now reads 38 at 100%. The same fix recovers 11 real commands
  for `mysqladmin`/`mariadb-admin`, whose command list is tab-separated.

  A tab now counts as a gap on its own, because it is never decoration — it
  advances to the next 8-column stop, so one tab already separates columns by
  at least as much as the two spaces a space-run must have.

- **A second column of option *spellings* is no longer read as a description.**
  The necessary companion to the above: `awk --help` prints POSIX short options
  beside their GNU long equivalents, so treating that tab as a description gap
  gave `-f progfile` the "description" `--file=progfile`. That would have been
  reported as **28 flags, 100% described** with every description a lie. A
  description that is a single token beginning with `-` is now recognised as a
  synonym and dropped, leaving the honest "no description" awk actually offers.

- **A positional documented as the first row of an options table no longer
  discards the table.** The flags-vs-bare-words decision read only the section's
  first content line. `kill --help` opens `Options:` with `<pid> [...]`, so
  every flag beneath it was thrown away: **0 flags**, confirmed by deleting just
  that row, after which the same build read 6 at 100%. The decision now looks
  past at most three leading non-flag rows at the block's own indent, and still
  requires a real `-`-leading row — bounded deliberately, since "look harder for
  flags" is how fabrication starts. A bare-word command table contains no such
  row and is unaffected.

## [0.2.1]

A safety rule was covering for a bug instead of solving it. Fixing the bug
turned the rule into a narrower, better one — and turned thirteen tools that
showed nothing into eleven that show real flag lists.

### Added

- **`pkill`, `killall`, `fuser`, `reboot`, `shutdown` and the rest are now
  browsable.** They were refused outright; they are now invoked as
  `<tool> --help` and nothing else. That single shape is measured harmless on
  all of them and is where their flag lists live: `pkill` yields 27 flags,
  `killall` and `fuser` 16 each, all fully described. Twelve of the thirteen
  went from `no-tier` to `ok` on the PATH sweep, and overall described
  coverage moved 89.20% → 89.23%. (`killall5` still shows nothing — it has no
  `--help` at all. `kill` parses to zero flags because its options block opens
  with a `<pid> [...]` row rather than a flag; that is a pre-existing parser
  gap, now merely visible.)

### Fixed

- **`-h` is an action flag on machine-state tools, and mandible sends `-h` as a
  fallback.** Measured, with the machine saved only by polkit because the probe
  ran unprivileged: `halt -h`, `poweroff -h`, `reboot -h` and `shutdown -h` each
  returned "Call to … failed: Interactive authentication required" — that is,
  each *attempted the real operation* (`-h` is the halt in `shutdown -h now`).
  mandible falls back to `-h` whenever `--help` fails, so as root, or wherever
  polkit is permissive, the fallback alone would reboot the machine. The
  previous blanket ban was preventing this without anyone having written it
  down; the replacement rule refuses `-h` on these tools explicitly.

- **A probe could hand a tool an empty first positional, and `pkill -- ""`
  terminates every process it can reach.** The clap `CompleteEnv` probe ran
  `<tool> -- <partial>`, and at the root the partial was empty. `--` is the
  option terminator essentially every getopt program discards, so the empty
  string arrived as the tool's first positional — and a program whose first
  positional is a pattern reads an empty pattern as *match everything*.
  Measured in a private PID namespace: `pkill -- ""` killed every process
  there, itself included.

  This is the mechanism behind the machine reset that produced the never-probe
  list in 0.1.x. That list refuses thirteen named tools; this argv was going to
  the other 2253 on `PATH`. It is now refused at the `run_inert` chokepoint for
  every tool, so no tier can reintroduce it (spec §6 rule 2a). Cobra's
  completion word stays permitted — it is protocol-required and, unlike the
  above, never the first positional, because the `__complete` sentinel precedes
  it.

  The rationale recorded for the never-probe list turned out to be **wrong**,
  and is corrected in `spec.md`. It claimed `killall foo --help` kills
  everything named `foo`; on glibc, GNU getopt permutes arguments, so `--help`
  is processed first. `pkill --help`, `pkill victim --help` and
  `killall victim --help` were all measured killing nothing. Positional shapes
  stay refused for those tools anyway — permutation is a glibc behaviour, not a
  guarantee — but the list is no longer what stands between a user and the
  measured hazard.

### Changed

- **Canonical repository is now `https://github.com/AS-FOSS/mandible`.** The
  `repository` field drives both the crates.io metadata and the `mandible
  mandible` screen, so the easter egg follows automatically.

### Removed

- **The clap `CompleteEnv` probe.** Besides being the source of the argv above,
  it never once identified a real clap tool. clap's protocol has no
  self-identifying trailer like cobra's `:N` directive, so detection was only a
  shape heuristic, and on the PATH sweep it matched ten unrelated tools —
  `echo`, `bzless`, `bzmore`, `validlocale`, `xdg-user-dir`,
  `update-alternatives` among them. (`echo -- ""` prints `--`, which starts
  with a dash and so "looked like" a flag.) Removing it deletes eight bogus
  `native` tiers, three fabricated flags and one fabricated node; described
  coverage moves 89.19% → 89.20% across 2266 tools with no tool losing
  anything. Re-adding it would need a way to confirm the protocol before
  trusting the response, and a spelling that never passes an empty positional.

## [0.2.0]

Six fixes, three of them found by rendering a deliberately awkward CLI through a
real pseudo-terminal rather than by any test.

### Fixed

- **The USAGE line no longer repeats the command name.** `docker import`
  rendered as `import docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]`.
  The check asked whether the usage's *first word* was the node's name, but
  cobra and argparse both print the full command path, so the name was stapled
  on the front of nearly every subcommand of every such tool. It now scans the
  whole leading run of command words. Tools that print no name at all
  (`Usage: [OPTIONS] FILE`) still get one added, which is what the prepending
  was for.

- **A token wider than the pane is broken across lines instead of discarded.**
  It was ellipsis-truncated, so a 150-character URL rendered as
  `https://registry.example.com/v2/org…` with everything after it unrecoverable
  from the parsed view. Splits are placed by display width, so a double-width
  character cannot straddle the boundary and overflow the pane.

- **A relative tool path works.** `mandible ./scripts/tool.py` failed with
  "No such file or directory" for a file plainly present: the path was checked
  against the caller's working directory, then the probe ran with its own
  directory redirected into a scratch dir (§6 rule 8). Resolution now yields an
  absolute path — via `std::path::absolute`, deliberately not
  `fs::canonicalize`; see below.

- **argparse subcommands survive a styled section heading.**
  `add_subparsers(title="commands")` is the ordinary way to name that block,
  and the dedicated scan was gated on the heading reading `positional
  arguments`, so a styled heading collapsed the entire command tree to a single
  node. The scan's structural evidence — a `{a,b,c}` pseudo-entry with deeper
  lines beneath it — is stronger than the heading text ever was, and still
  refuses a plain positional carrying `choices=[...]`.

- **A command list at the same indent as its heading is recognized.** `dnf` 4
  prints its whole command list flush at column 0 under a flush-left heading;
  the engine required content indented *more* than its heading, so `mandible
  dnf` showed one node and no subcommands. Now 30.

- **A pending row's spinner no longer touches the name.** `dnf`'s longest
  command rendered `check-update⋯ loading`, one mangled word rather than a name
  and its status — the same defect fixed for summaries in an earlier release
  (`apt-get`'s `dselect-upgradeFollow`) and missed in the sibling branch,
  because no tool in the suite had a pending row at the column until `dnf`
  gained subcommands.

### Notes on two near-misses

Both were caught by the PATH-wide coverage sweep and by nothing else; the unit
suite was green through both.

- Making resolved paths absolute with `fs::canonicalize` **defeated §6 rule 0**.
  `is_never_probe` matches on the file name, and `reboot`, `poweroff`,
  `shutdown` and `telinit` are symlinks to `systemctl` — resolving renamed them
  before the refusal ran. It also broke ten `iptables*` tools, which dispatch on
  `argv[0]`. Fixed by using `std::path::absolute`, which does not follow links.
- The same-indent command rule initially **fabricated 28 subcommands** out of
  `mysqlslap`'s config-variable table (`port 3306`, `no-drop FALSE`), because at
  a shared indent every row is a candidate heading for the rows beneath it and
  `init-command` contains the word "command". A heading must now not itself look
  like a row.

Final sweep is identical to baseline on every aggregate — 89.19% described
across 2266 tools, 1 suspicious, 320 verbatim — with zero status changes, zero
nodes lost, and `dnf` the only gain.

### Internal

- `scripts/smoke_cli.py`: a deliberately awkward argparse CLI for exercising
  layout by hand — a twelve-level command chain, four flag-table shapes, tokens
  with no whitespace to wrap at, and sixty flags. It found three of the bugs
  above within minutes of existing.

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

  Each path is masked under both its logical and its canonicalized spelling, so
  a probe that resolves its own working directory is covered too — on macOS
  `$TMPDIR` sits under `/var`, a symlink to `/private/var`.

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
[#6](https://github.com/AS-FOSS/mandible/issues/6). The last two shared a
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
[#1](https://github.com/AS-FOSS/mandible/issues/1).

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
  now `https://github.com/AS-FOSS/mandible`. All 7 crates, the binary
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
