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

(Six rows formerly here — the `Text::sanitize` boundary, widgets assuming
`Text` is clean, per-field provenance, node-scoped extraction, one-node-one-row,
and display-width truncation — plus the two `--help`-only execution-safety rows,
are now written down as `spec.md` rules: §4.1, §9's intro (which cites §4.1),
§4.2, §5.2, §9.1, and §6 rules 0 and 2a respectively. Removed per §6's own
"do not duplicate spec.md" — see the git history of this file for the previous
wording if you need the AGENTS.md-local phrasing.)

| Invariant | Where | Failure it prevents |
|---|---|---|
| Never slice a `&str` derived from tool output at a raw byte offset (`&s[..n]`) | any tier that parses `--help`/similar text | Panics if the offset isn't a UTF-8 char boundary. Shipped as a real crash (`help_text::sections`, found by the coverage harness's first real run, not a synthetic test): a box-drawing glyph early in one real tool's output landed byte 6 mid-character. Use `s.as_bytes().get(..n)` (bounds-checked, no boundary concept for raw bytes) for ASCII-prefix checks, or `s.get(..n)` (returns `None` instead of panicking) generally. |
| Never check whether a process is alive with `pgrep -f <string>` when your own command line contains that string | any agent driving a long background job | `pgrep -f` matches the full command line, so an `until ! pgrep -f "xtask coverage"` poller **matches itself** and reports the job alive forever. Cost a long stretch of this project reporting a sweep as running when it had died. Use `pgrep -x <binary>` (matches the process name), or record the PID. |
| Never call an O(n)-or-worse function from inside a `while` loop's own *condition* | general Rust pitfall, not specific to one file | It reruns every iteration, turning a linear function quadratic. Found via the coverage harness on a genuinely degenerate input (a REPL that ignores `--help` and free-runs printing its own banner): one tool took 153s instead of milliseconds. Compute it once, before the loop. |
| **A reproduction beats three rounds of reasoning.** When a bug resists explanation, reproduce it under the real harness (§3.2) before trusting the next theory. | general debugging method, not one file | [M-19], the `mandible systemctl` freeze: two successive theories — a pager, then a `/dev/tty`/session hazard ([M-17]) — were each individually *disproved by measurement*, not settled by more reasoning about the code. The real mechanism: `systemctl <anything...> --help` returns the tool's own root help byte-identically no matter what precedes it or how deep, so the background warmer's per-node probing treated every one of those fabricated "deeper" nodes as real, cascading 18 → 18² → 18³ subcommands toward the 4,096-node cap and starving the UI thread's own scheduling. Nobody was going to reason their way to that from the code; a `scripts/pty_screenshot.py` reproduction found it directly. |
| **Never write into a CHANGELOG section whose version is already tagged.** New notes go under `## [Unreleased]`. | `CHANGELOG.md`, enforced by `scripts/changelog_guard.sh` in CI | A released section is history: it describes what that tag published, and the release body is generated from it (`scripts/changelog_section.sh`). On 2026-08-12 a change appended roughly 56 lines describing unreleased work under `## [0.2.2]`, a version tagged and published weeks earlier. Nothing complained, and the misattribution would have gone out with the next release. The guard compares every `## [X.Y.Z]` that has a matching `vX.Y.Z` tag against the section that tag actually published, so this fails in CI rather than in a reviewer's memory. |
| **A framework protocol word is never sent to a tool without prior evidence the tool speaks that protocol** — evidence read from the artifact or from the tool's own output, never from its name | `native/` (cobra marker), `completion_script/` (cobra marker or a `completion` command row in the tool's own `--help`), spec §6 rule 1a | `__complete`, `completion <shell>` and `-- <partial>` are subcommand invocations only inside the framework that defines them; to any other program they are ordinary positionals, i.e. rule 1's bare invocation with a non-empty argv. Rule 2's closed list is not wrong, its premise is: **every shape on it was validated against tools that parse argv**, and a program that ignores argv and starts anyway is outside that premise. Both incidents are the same failure — `wall __complete` broadcast a word to every terminal on a machine, and `<tool> completion zsh`/`bash` left **437 daemons running**. Neither was a bad shape; both were a right shape sent to the wrong program. Do not fix this by growing `HELP_ONLY_PROBE`: a name list is §1's forbidden knowledge wearing a safety costume, and it can only ever name the tools someone has already been bitten by. |
| **A probe is not complete while its descendants are alive.** Reaping the process group is necessary and not sufficient | `mandible-extract/src/exec/reap.rs`, spec §6 rule 4 | A program that daemonises leaves the process group *and* the session on its own (`fork`, parent exits, child `setsid`s), after which nothing about the survivor points back at the probe. **622 processes** were found left behind on a developer box, the oldest five days old, `guacd` holding `127.0.0.1:4822` and `sudo_logsrvd` holding `0.0.0.0:30343`. Crucially **not a hang** — all 2,302 `probe-start` lines in a traced sweep had a matching `probe-done` — so no timeout change could have helped, and reading the symptom as "the probe is slow" sends you at the wrong mechanism. The fix is adoption (child subreaper) plus a per-invocation environment token for attribution; the token is what stops it from becoming an indiscriminate kill of everything adopted. It is the *second* layer — the first is not sending the probe at all (row above). |

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

**Two instruments say less than their names suggest. Do not overstate either.**

- **A green corpus fixture proves the tree stopped changing, not that it is
  right.** `ok` means `expected.snap` matches and the `[contract]` holds — and
  a tree that is wrong in a way no detector models reports `ok` forever.
  Measured at v0.3.1: of 23 tools that went broken → passing on one branch,
  only **3** carried a human `verdict_scope`; the other 20 were trees an agent
  blessed and the suite then guarded against *changing*. `xtask corpus --show
  <tool>/<version>` prints `scope: unscoped` when nobody has judged it. "N
  fixtures green" and "N tools parse correctly" are different claims, and only
  the second needs a human.
- **Re-measuring accuracy on the tools you just fixed is train-on-test.** It
  measures the fix, not the parser, and must never be quoted as a fleet
  estimate. The frozen queue (`audit/queue.toml`) exists to make an unbiased
  draw of *unseen* tools possible; until such a draw is reviewed, attach the
  caveat everywhere the number appears rather than in a footnote. State the
  denominator too — audited tools that never became fixtures are measured by
  nothing, so any "N of the M" must say what M excludes.

A related failure of framing, worth naming because it caused real confusion:
fleet-wide **flag** counts and tool-level **audit** outcomes are different
units. "8,784 flags repaired" sounds like it swept the audit set; it fixed 23
of 50 audited tools, because the rest fail for unrelated reasons. Never let
the larger number stand in for the smaller one.

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

### 3.3 Never parse human-format test output

Run tests with `cargo nextest run --workspace` (CI and this playbook both use
it), never `cargo test --workspace` piped into `grep`/`awk`/similar. The rule
is not "use nextest because it's faster" — it is that **human-format test
output must never be parsed, by anyone, for any reason**, and nextest exists
here specifically so nobody has to.

The concrete failure, self-reported twice in consecutive reports before it
became this rule: `grep -c FAILED` against `cargo test`'s output false-
positived on test *data* that happened to contain the literal word "FAIL"
(fixture text, a variant name, a snapshot value — the output stream mixes
program output and test-runner output with no structural separation),
producing a confident, wrong pass/fail count. Nothing about that failure was
exotic; it is the generic risk of treating a human-readable report as a data
format, and it will keep recurring for as long as a human-format stream is
the thing being read. `cargo nextest run` reports a real nonzero exit code on
any failure and can emit `--message-format libtest-json` when a structured
result is actually needed — read *that*, or read the exit code, never the
prose.

---

## 4. Environment facts

### Facts about this repository's own tooling

These cost real time when rediscovered.

- **`rm` on the maintainer's dev box is aliased to a trash tool that does not
  free disk space.** It moves files to `~/.local/share/Trash`, so a cleanup can
  report success while the disk stays full. Thirty agent worktrees once held
  ~90G of `target/` between them, the disk hit 100%, and three running agents
  broke mid-task — each of them independently fighting the same wall. Use
  `/bin/rm`, and check `~/.local/share/Trash` before believing a cleanup
  worked.
- **Ubuntu 24.04 sets `kernel.apparmor_restrict_unprivileged_userns=1`,** which
  blocks the unprivileged user namespaces `exec::containment` builds a
  full-`PATH` sweep's containment out of. It is the default on GitHub's
  `ubuntu-latest` *and* on the dev box, and it can flip mid-session; it
  surfaces as two failing `exec::containment` tests. CI grants the capability
  in the test job (`sudo sysctl -w
  kernel.apparmor_restrict_unprivileged_userns=0`). **That grants what the test
  demands; it does not relax the assertion** — the test exists so a host which
  cannot contain a sweep says so loudly, and weakening it would delete exactly
  that signal. `--tools`-pinned runs are never gated by containment, which is
  why the corpus and coverage jobs stay green regardless.
- **macOS breaks in ways Linux CI cannot see** — mostly dead code under `cfg`,
  which `-D warnings` rejects. Two rounds of red CI were once spent guessing at
  it. Check locally instead: `rustup target add aarch64-apple-darwin`, then
  `cargo clippy --workspace --target aarch64-apple-darwin --all-targets -- -D
  warnings`. Tests cannot be *linked* for macOS on the aarch64 Linux box, but
  clippy type-checks everything and catches the whole class.
- **A fresh agent worktree is created from the repository's default branch, not
  from the branch you are working on.** Five of six agents in one session began
  on a months-old release tag, and one of them wrote an entire task against it
  before anyone noticed. Start every delegated task by comparing `git log -1`
  against the intended base — and if it is wrong, **branch from the intended
  base; never reset to it**. An earlier version of this entry said to reset,
  which cost a commit: two agents turned out to be in the *shared* main
  worktree rather than isolated ones, and a `git reset --hard` in one destroyed
  the other's finished work. It was recovered only because the victim happened
  to have tagged it. A destructive git command is never the right way to
  correct a base, and an agent cannot assume the working tree is its own —
  check `git rev-parse --show-toplevel` before any command that writes.
- **CI never runs on a feature branch push.** `ci.yml`, `frameworks.yml` and
  `path-sweep.yml` all trigger on a push to `main` or a pull request targeting
  it. A long-lived branch can accumulate dozens of commits with nothing gating
  them, and the `CONTRACT WEAKENED` detector in particular is pull-request-only,
  so it does not run at all until a PR exists.
- **All three workflows carry `paths-ignore` for `**/*.md`, `docs/**`,
  `LICENSE-*`, `NOTICE` and `.gitignore`.** A documentation-only push skips CI
  entirely, which is correct but surprising the first time.

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
- **Commit before you attack your own work.** Disabling a check to prove its
  test fails is required here (§3.1), and the restore afterwards is a
  destructive command: an agent ran `git checkout --` on the file it had just
  written but not yet committed, and lost it. Commit first, then attack, then
  restore — the restore has something to restore *to*.
- **A result that exists only on one machine is not a result.** `audit/queue.toml`
  is called *tracked* by `xtask::queue`'s module doc and again by spec §16's
  storage note — and was never committed by any commit on any branch, because
  the command that writes it had never been run. Two documents asserting a file
  exists is not evidence that it does. If something is meant to be tracked,
  `git add` it and confirm with `git ls-files`; if a doc claims a file is
  tracked, that claim is checkable and belongs in a test.
- **`NOTICE` is not optional.** Vendored third-party *data* carries attribution
  obligations, and it is the most likely genuine legal exposure in this project.
- Gates before reporting done: `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace` (§3.3 — never `cargo test --workspace`
  piped into a text-parsing tool), `cargo build --release`.
- `#![forbid(unsafe_code)]` in every crate except `mandible-extract`, which
  carries `#![deny(unsafe_code)]` plus exactly two scoped
  `#[allow(unsafe_code)]` sites, both in `exec/`: the `pre_exec` + `setsid`
  call on the probe-spawning function, which gives every probe its own
  session so a descendant can't reopen the controlling terminal via
  `/dev/tty` ([M-17]); and `containment::secured_scoreboard_file`'s
  `File::from_raw_fd`, which reconstructs the `--out` file a contained sweep
  inherited across `unshare` + re-exec. **This count is the whole point of
  `deny` over `forbid` — if you add a third, it belongs here and in
  `mandible-extract/src/lib.rs`'s crate doc comment in the same commit, or
  the exception list stops being an exception list.**
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
