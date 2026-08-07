# AGENTS.md — working agreements for AI agents in this repo

Read this before touching code. It is short on purpose. Every entry exists
because something actually went wrong, and it says which failure it prevents so
a future reader can judge whether it still applies.

**Precedence:** `spec.md` is the design authority — what to build and why. This
file is the operational authority — how to work here without repeating known
mistakes. `CONTRIBUTING.md` is for humans. If this file and `spec.md` disagree
about design, `spec.md` wins and you should fix this file.

---

## 1. The invariant that defines the project

> **No per-tool logic, ever.** No `if tool == "docker"`, no tool-name-keyed
> special case in any extraction tier, no per-tool patch file vendored into this
> repo.

Tool-specific knowledge lives in exactly one place: user-local override files
under `~/.config/mandible/overrides/`, which are never committed here. (Spec
revision 3 deleted the vendored catalog that used to be the second place — a
per-tool catalog is per-tool knowledge relocated into data, and it cannot stay
current with the tool actually installed. Parsing is keyed by *framework* now:
see spec §7 Tier A′ and `mandible-extract/src/help_text/profile.rs`, where
adding a framework is one `match` arm plus one fingerprint.)

If a tool renders badly, the fix is a better general parser, a new general tier,
or an honest low-confidence badge in the UI. It is never a special case. This is
the entire reason the tiered architecture exists; one exception starts the
erosion that the architecture was built to prevent.

---

## 2. Architectural invariants

Breaking any of these produces a bug that tests will not catch.

| Invariant | Where | Failure it prevents |
|---|---|---|
| `Text::sanitize` (or `sanitize_markdown`) is the **only** way untrusted text enters the IR | `mandible-core/src/text.rs` | Control chars and markup reaching a `ratatui::Span`, which corrupts pane borders. Two widget-level fixes for this failed before the boundary fix worked. |
| Widgets may **assume** `Text` is clean | `mandible-tui` | Re-implementing defenses in each of the three consumers (tree, detail, clipboard), inconsistently |
| `std::process` appears **only** in `mandible-extract/src/exec/` | enforced by `tests/no_process_outside_exec.rs` | Unaudited subprocess spawning; §6 of the spec becomes unenforceable |
| Provenance is **per field**, never per tree | `mandible-core/src/provenance.rs` | A trust badge that lies after a multi-tier merge — worse than no badge |
| Extraction is **node-scoped** (`extract_node`), never whole-tree | `mandible-extract/src/tier.rs` | Eager extraction: 232 subprocesses and 10.5s for `docker`. Do not reintroduce a whole-tree `extract()`. |
| **One node = exactly one tree row.** No wrapping in the tree pane | `mandible-tui/src/render/tree_pane.rs` | Row index ↔ node stops being a bijection, breaking selection, scrolling, mouse hit-testing, and filtering all at once |
| Truncate by **display width** (`unicode-width`), never `char` or byte count | `mandible-tui` | CJK/emoji overflow the border by one cell per wide character |
| Never slice a `&str` derived from tool output at a raw byte offset (`&s[..n]`) | any tier that parses `--help`/similar text | Panics if the offset isn't a UTF-8 char boundary. Shipped as a real crash (`help_text::sections`, found by the coverage harness's first real run, not a synthetic test): a box-drawing glyph early in one real tool's output landed byte 6 mid-character. Use `s.as_bytes().get(..n)` (bounds-checked, no boundary concept for raw bytes) for ASCII-prefix checks, or `s.get(..n)` (returns `None` instead of panicking) generally. |
| A bare-word block becomes subcommands only under a *recognized* heading (or a chain started by one) plus a name-shape check — never from layout alone | `mandible-extract/src/help_text/sections.rs` | [M-10]: Tier B fabricated 39-65 phantom subcommands per tool from wrapped description continuation lines and `--format=`-style enum value lists. Fabricated structure is worse than missing structure — a user can't tell it's wrong. The coverage harness's structure-sanity column (spec §13.1) is the regression net: `%described` alone stayed at 100% while this was happening. |
| Every probe's CWD, `HOME`, `TMPDIR`, and the writable `XDG_*` vars point at one scratch dir, created fresh **per invocation** and removed on drop | `mandible-extract/src/exec/spawn.rs` | [M-11]: `--help` is not reliably read-only. A font-cache builder wrote into the invoking CWD and `mysql_secure_installation` wrote a `.my.cnf` with an empty root password, from nothing but `--help`. A CWD-only redirect doesn't reach `$HOME`. |
| A bare-name grid becomes subcommands only when its rows are **column-aligned** (fields separated by 2+ spaces) | `mandible-extract/src/help_text/sections.rs` | [M-10] again, by a different route: `apt-get --help`'s description paragraph became the subcommands *"and"*, *"information"*, *"about"*, *"them"*, *"from"*, *"authenticated"*, *"sources"*. Its opening sentence ("apt-get is a **command** line interface…") passed the recognized-heading test and the wrapped prose lines are all name-shaped words at a matching indent. Prose is single-spaced; a real grid is aligned. Layout, not vocabulary, is the discriminator. |
| A usage-block continuation line **must not itself read as a flag entry** | `mandible-extract/src/help_text/sections.rs` | The inverse failure: `curl --help` runs its flag rows straight under `Usage:` with no blank line and no `Options:` heading, so every flag was consumed as usage text and the tool reported **zero flags** — at status `ok`, because nothing was fabricated for the structure-sanity check to catch. Silently-missing structure is as wrong as invented structure and harder to notice. A usage continuation is an alternative invocation form; it never starts with `-`. |
| Rendered **man pages** are not `--help` output and must not reach the help-text grammar | `mandible-extract/src/help_text/sections.rs` | `git bisect --help` renders GIT-BISECT(1), and mining roff prose for structure produced the subcommands *"follows"*, *"testing."*, *"command"*, *"skipped."*. Detected by the `NAME(1) … NAME(1)` banner both margins carry. Until Tier D exists, the honest outcome is verbatim rendering. |
| A command name never **ends** with `.`/`-`/`_` | `mandible_core::is_command_name_shaped` | Interior ones are legitimate (`mount.nfs`, `apt-get`), so the character class allows them — but allowing them at the end let sentence fragments (*"testing."*, *"skipped."*) pass the name-shape check and become nodes. |
| Never check whether a process is alive with `pgrep -f <string>` when your own command line contains that string | any agent driving a long background job | `pgrep -f` matches the full command line, so an `until ! pgrep -f "xtask coverage"` poller **matches itself** and reports the job alive forever. Cost a long stretch of this project reporting a sweep as running when it had died. Use `pgrep -x <binary>` (matches the process name), or record the PID. |
| Never call an O(n)-or-worse function from inside a `while` loop's own *condition* | general Rust pitfall, not specific to one file | It reruns every iteration, turning a linear function quadratic. Found via the coverage harness on a genuinely degenerate input (a REPL that ignores `--help` and free-runs printing its own banner): one tool took 153s instead of milliseconds. Compute it once, before the loop. |

---

## 3. Verification playbook

### 3.1 Green gates do not mean it works

Two real bugs in this project passed a full green suite:

- A cobra tier whose `extract()` built argv without the literal `__complete`.
  The tier was **completely dead** in the real pipeline. Its unit tests passed
  because they injected a mock probe that bypassed argv construction.
- Cached trees served from before a parser fix, making a correct fix look broken.

**Rules that follow:**

- Every extraction tier needs at least one test exercising **real argv
  construction**, not just the parser behind it.
- Before claiming a feature works, run the real binary against real data.
- Report honestly when something is unverified. "I could not verify X" is a
  useful result; a false "works" costs someone a debugging session.

### 3.2 There is no tty in the agent sandbox

`enable raw mode` fails with *"No such device or address"*. Do not try to run
the TUI directly.

**Rendering must therefore be verified through `TestBackend`**, which needs no
terminal at all — see `mandible-tui/tests/border_integrity.rs`.

A pty harness (`scripts/pty_screenshot.py`) used to live here: it forked a real
pseudo-terminal, set an explicit window size (the part naive attempts miss —
without `TIOCSWINSZ` the pty is 0×0 and ratatui renders nothing), and replayed
the output through a terminal emulator to produce the actual screen. It found
the markdown leak, the ragged re-wrap, apt-get's mangled
`dselect-upgradeFollow`, and the unbounded detail-pane scroll — all invisible
to `TestBackend`, which uses synthetic fixtures rather than real tool output.

It was removed because its output is no longer wanted in the README. **Its
absence is a real gap**: content regressions that only appear against real
`--help` text now have no automated net. If you are changing rendering, the
mitigation is to widen the `TestBackend` suite with a fixture captured from a
real tool, rather than to assume synthetic input is representative — the two
border-corruption bugs that shipped both passed synthetic tests.

---

## 4. Environment facts

Do not re-derive these. They are measured, with method, in **`spec.md`
Appendix A** (`[M-1]`…`[M-9]`). The ones that most often surprise:

- `clap`'s `CompleteEnv` is essentially **absent in the wild** — `ripgrep` and
  `cargo` both lack it. Do not build a milestone on it.
- cobra needs **two probes per node**: `""` returns subcommands only, `"-"`
  returns flags.
- `libmandoc` is **not a system library on Linux**.
- `--help` output may go to **stderr** and exit **non-zero** (`openssl`, `ip`).
- **Two pitfalls when picking a real binary for a framework/real-argv test**
  (batch 6 part 4): (a) `cargo` is commonly a `rustup` proxy that reads
  `$HOME/.rustup` to pick a toolchain, which fails under the exec sandbox's
  mandatory per-probe scratch `HOME` (§2's row above) with `rustup could not
  choose a version of cargo to run` — use the toolchain's real `cargo`
  (`~/.rustup/toolchains/*/bin/cargo`) or a non-rustup clap binary (`zoxide`
  worked well: real flags and subcommands, no external state). (b) Never use
  `mandible`'s own binary as an artifact-fingerprinting test target:
  `framework::artifact::BINARY_MARKERS` embeds its own search patterns
  (e.g. `spf13/cobra`) as literal bytes, and `mandible` statically links
  `mandible-extract`, so a scan of mandible's own binary "detects" itself.
- `ripgrep` depends on the `clap` crate but hand-rolls its own `--help`
  formatter [M-13] — its output is not representative of clap's own
  template. Use a tool whose help text actually came from clap's
  formatter (`cargo`/`zoxide`) when fixture-testing the `ClapV3V4` grammar.

If you measure something that contradicts Appendix A, the measurement wins —
update Appendix A in the same commit, with the method.

---

## 5. Working agreements

- **Commit per unit of work, not per session.** A session limit once killed 220
  uncommitted lines and left the tree not building. An interim commit that
  compiles beats an uncommitted one that does not.
- **`NOTICE` is not optional.** Vendored third-party *data* carries attribution
  obligations, and it is the most likely genuine legal exposure in this project.
- Gates before reporting done: `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `cargo build --release`.
- `#![forbid(unsafe_code)]` in every crate. No `unwrap()` on any path reachable
  from tool input.
- Never invoke a tool binary outside the argv allowlist in spec §6. Running a
  bare binary is how you launch a REPL, block on stdin, or start a daemon.

---

## 6. Maintaining this file

This file's failure mode is not being wrong — it is **growing into a junk
drawer** that nobody reads, at which point it stops protecting anything.

**Add an entry when**, and only when:

- Something went wrong that a reasonable agent would repeat, **and**
- The lesson is not already discoverable from `spec.md`, the type system, or a
  test that fails loudly.

Prefer making a mistake *impossible* over documenting it. A private field, a
newtype, a `#[deny]`, or a failing test is worth more than a paragraph here. If
you can encode the rule in code, do that instead and skip the entry.

**Every entry must state the failure it prevents.** An instruction without a
"why" cannot be evaluated later, so it never gets deleted, so the file rots.

**Delete aggressively.** An entry is dead when its cause is fixed. Deleting it
is a completed task, not a loss — the old "`--refresh` trap" section here was
deleted the same day `SOURCE_FINGERPRINT` landed, which is the intended
lifecycle. Review the whole file whenever you finish a batch of work.

**Do not duplicate `spec.md`.** Link to it. Duplication means two sources that
will disagree, and the disagreement will be discovered at the worst time.

**Keep it under ~200 lines.** If it grows past that, something belongs in
`spec.md` (design), `CONTRIBUTING.md` (human process), or the bin.

**Date-stamp anything environment-dependent**, and re-verify rather than trust
it. Facts about other people's tools go stale.
