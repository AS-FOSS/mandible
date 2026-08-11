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
| Provenance is **per field**, never per tree | `mandible-core/src/provenance.rs` | A trust badge that lies after a multi-tier merge — worse than no badge |
| Extraction is **node-scoped** (`extract_node`), never whole-tree | `mandible-extract/src/tier.rs` | Eager extraction: 232 subprocesses and 10.5s for `docker`. Do not reintroduce a whole-tree `extract()`. |
| **One node = exactly one tree row.** No wrapping in the tree pane | `mandible-tui/src/render/tree_pane.rs` | Row index ↔ node stops being a bijection, breaking selection, scrolling, mouse hit-testing, and filtering all at once |
| Truncate by **display width** (`unicode-width`), never `char` or byte count | `mandible-tui` | CJK/emoji overflow the border by one cell per wide character |
| Never slice a `&str` derived from tool output at a raw byte offset (`&s[..n]`) | any tier that parses `--help`/similar text | Panics if the offset isn't a UTF-8 char boundary. Shipped as a real crash (`help_text::sections`, found by the coverage harness's first real run, not a synthetic test): a box-drawing glyph early in one real tool's output landed byte 6 mid-character. Use `s.as_bytes().get(..n)` (bounds-checked, no boundary concept for raw bytes) for ASCII-prefix checks, or `s.get(..n)` (returns `None` instead of panicking) generally. |
| `-h` is **not** a help flag on machine-state tools and must never be sent to them | `mandible-extract/src/exec/spawn.rs` (`HELP_ONLY_PROBE`) | On systemd's multi-call binary `-h` is an *action* flag — `shutdown -h` is the halt in `shutdown -h now`. Measured: `halt -h`, `poweroff -h`, `reboot -h` and `shutdown -h` each returned "Call to … failed: Interactive authentication required", i.e. each **attempted the real operation** and was stopped only by polkit because the probe ran unprivileged. As root, or with permissive polkit, the probe reboots the machine. mandible falls back to `-h` whenever `--help` fails, so this is reachable by ordinary means. These tools are restricted to exactly `--help`, which is measured harmless and is where their flag lists live. |
| An argv element is **never the empty string**, unless a guard word precedes it | `mandible-extract/src/exec/spawn.rs` (`run_inert`) | Rule 1 ("never a bare invocation") only counts arguments. `--` is the option terminator essentially every getopt program discards, so `<tool> -- ""` delivers the empty string as the tool's *first positional*, and a program whose first positional is a pattern reads that as “match everything”. Measured: `pkill -- ""` terminated every process in a private PID namespace, pkill included (rc=143). This was the real mechanism behind the reported machine reset — the never-probe list masked it for thirteen tools while the same argv went to the other 2253. The one exception, cobra's completion word, is safe because `__complete` shields it: it is never the first positional. Note the near-miss fix: respelling it as `--` alone is harmless but *wrong* — `--` is a no-op for most tools, so they print their ordinary output and the completion heuristic reads it as candidates (`whoami --` → a username). The sweep caught that at 16 newly “native” tools, 8 of them `suspicious`. |
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

**But `TestBackend` alone is not enough**, and the record on that is
unambiguous. `scripts/pty_screenshot.py` forks a real pseudo-terminal, sets an
explicit window size (the part naive attempts miss — without `TIOCSWINSZ` the
pty is 0×0 and ratatui renders nothing), drives it with keystrokes, and replays
the output through a terminal emulator to produce the actual screen as text:

```console
$ python3 -m venv /tmp/ptyvenv && /tmp/ptyvenv/bin/pip install pyte
$ /tmp/ptyvenv/bin/python scripts/pty_screenshot.py --keys '/run,<enter>,<tab>' \
      90 30 ./target/release/mandible docker
```

It found the markdown leak, the ragged re-wrap, apt-get's mangled
`dselect-upgradeFollow`, the unbounded detail-pane scroll, and the ragged flag
columns — every rendering bug this project has had. All were invisible to
`TestBackend`, because synthetic fixtures are chosen to be representative and
real `--help` output is not.

It is a debugging tool, not part of CI, and it is deliberately **not mentioned
in the README** — it once generated the README's terminal art, which is what it
is no longer for.

The failure mode it guards against is specific and keeps recurring: a
`TestBackend` test written from synthetic input passes, ships, and the defect is
plainly visible the moment a real tool is rendered. The ragged flag columns are
the type specimen — `flag_descriptions_share_one_column` asserted alignment over
three short flags at one comfortable width, and every one of them fitted the
column, so the test passed for six releases while `docker --help` rendered its
descriptions at three different columns in the same list. **When you change
rendering, capture a screen before and after**, and when a rendering test
passes first try, suspect the fixture before believing the result.

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
- `#![forbid(unsafe_code)]` in every crate except `mandible-extract`, which
  carries `#![deny(unsafe_code)]` plus exactly one scoped
  `#[allow(unsafe_code)]` on the probe-spawning function in `exec/`, for the
  `pre_exec` + `setsid` call that gives every probe its own session so a
  descendant can't reopen the controlling terminal via `/dev/tty` ([M-17]).
  No `unwrap()` on any path reachable from tool input.
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
