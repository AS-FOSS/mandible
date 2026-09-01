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
| **CHANGELOG holds only what a user of the release would notice — behavior, features, breaking changes, visual changes.** Docs/README edits, CI and workflow changes, scripts, tooling, refactors with no visible effect: commit history only, never an entry. | `CHANGELOG.md`, and the release body generated from it | The file is what readers get as release notes; padding it with process changes buries the entries that matter. On 2026-09-01 a release-notes-layout tweak was written into `[Unreleased]` alongside real fixes. Contributors are credited automatically at the bottom of the release body (`scripts/release_notes.sh`), so they never need an entry either. (Maintainer rule, 2026-09-01.) |
| **A CHANGELOG entry is one line: never insert manual line breaks to make it "compact" or "readable" in the source.** | `CHANGELOG.md` | The release body and any renderer reflow markdown themselves; hand-wrapped lines only make entries inconsistent with each other and turn every later edit into a re-wrapping exercise. Write the whole entry as a single logical line and trust whatever displays it. (Maintainer rule, 2026-08-26.) |
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
$ python3 -m venv .venv && .venv/bin/pip install pyte
$ .venv/bin/python scripts/pty_screenshot.py --keys '/run,<enter>,<tab>' \
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

### 3.4 A guard is not done until you have watched it fail

When you add or change a detector, a lint, a meta-check, or a CI guard,
break the thing it protects — plant the defect, remove the row, disable the
lane — and confirm the run goes red **naming what you removed**. If it stays
green, or fails without naming it, the guard is decorative. Running it
against the healthy repo and seeing green proves nothing at all: a guard is
uniquely easy to write in a form that can never go red, and its green runs
then read as evidence forever. This project has already paid for instrument
blindness once — the fabrication count read 154 when the true number was 52,
because the existence oracle could not see shapes it claimed to measure, and
nothing in its output said so. Commit before you attack your own work (§5)
so the restore afterwards has something to restore to.

---

## 4. Environment facts

### Facts about this repository's own tooling

These cost real time when rediscovered.

- **Ubuntu 24.04 sets `kernel.apparmor_restrict_unprivileged_userns=1`,** which
  blocks the unprivileged user namespaces `exec::containment` builds a
  full-`PATH` sweep's containment out of. It is the default on GitHub's
  `ubuntu-latest` and on stock Ubuntu 24.04 developer machines, and it can flip
  mid-session; it surfaces as two failing `exec::containment` tests. CI grants the capability
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
  warnings`. Tests cannot be *linked* for macOS from a Linux host, but clippy
  type-checks everything and catches the whole class.
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
- **CI and the framework matrix carry `paths-ignore` for `**/*.md`, `docs/**`,
  `LICENSE-*`, `NOTICE`, `.gitignore`, `packaging/**`, the release-only
  scripts and the release/install-matrix/nix/sweep workflow files; the PATH
  sweep runs on push only for `mandible-core/**`, `mandible-extract/**`,
  `xtask/**` and the Cargo manifests (`paths`, an allowlist).** A docs,
  packaging or workflow-only push skips everything; a scripts push skips the
  hour-long sweep. Before that allowlist a one-line release-script edit on
  main triggered a full PATH sweep (2026-09-01).
- **`xtask corpus --bless` invents an `expected.snap` for xfail fixtures that
  intentionally have none.** After any bless, check `git status` for new
  untracked snapshots and delete them — committing one silently converts an
  xfail into a guarded wrong tree.

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

- **Never attach a session URL anywhere on GitHub — commit messages, PR
  bodies, issue or PR comments — even when tooling is configured to add one
  automatically.** A session link is private workflow detail; publishing it
  leaks information about the maintainer's setup and process. There is no
  exception.
- **Never address other people on GitHub without the maintainer's explicit
  consent.** No replies to outside contributors, no comments on issues or PRs
  beyond what the maintainer asked for in that specific instance. The account
  speaks with the maintainer's voice, and an agent answering a stranger
  commits the maintainer to words they never chose.
- **One branch and one PR per bundle of tasks in a session.** A second is
  allowed only when the grouping genuinely needs separating — and then ask
  explicitly before opening it. Anything less disciplined produces a PR list
  that reads as ceremony rather than work (eight PRs were once opened for
  one-file edits in a single day, one of them to delete a 2-byte file).
- **If an artifact leaks into commit history, notify the maintainer first —
  never force-overwrite.** Rewriting published history is destructive and
  irreversible for everyone downstream; whether and how to do it is the
  maintainer's decision alone. Report what leaked and wait.
- **Docs and trivial no-risk changes go direct to main; a PR is for work
  where pre-merge verification earns its cost.** Docs-only pushes skip CI
  entirely (`paths-ignore`, §4), so a self-opened, self-merged PR for a
  paragraph adds a merge commit and a dead branch while gating nothing.
  Direct to main: prose (README/AGENTS/spec wording), CHANGELOG entries
  (run the guard locally), file deletions, `.gitignore`, typo-class fixes.
  PR: parser/extraction logic, features, releases — the units where review
  has actually caught bugs here.
- **Never loosen a detector or parser rule to catch more cases if it can
  degrade tools that already work.** A permissive instrument hides the
  defects it exists to find, and a detector that fires on correct parses
  cannot be used to gate. When a rule misses a case, first measure what
  loosening would cost across the fleet; if it admits any currently-correct
  parse, keep the strict rule, record the miss as a documented lower bound,
  and fix the *scoring* rather than the check. Out-of-scope misses stay
  counted and named in every report — hiding them is goalpost-moving.
- **Public prose describes the change, not the conversation.** Commit
  messages, PR bodies, and issue/PR comments state what changed and why, in
  the fewest words that stay clear. Never paste the maintainer's private
  instructions verbatim, transcribe the discussion that led to a decision, or
  dump spec/documentation text into them — link the section instead. No
  personal details of any kind (names beyond the git author, machines,
  schedules, private context). If a comment is longer than the diff is
  interesting, it's too long.
- **The spec states the design; it never narrates its status.** No
  "approved but not yet implemented", no "to be done", no "this changes
  when X ships", and no conversation residue ("maintainer decision
  \<date\>") in `spec.md` or any other doc — §16's decisions log is the
  one dedicated home for rulings, and version-stamped section titles
  ("Revision 4 (0.5.0)") are the one sanctioned marker. Write every design
  as the final specification and edit it directly when the design changes.
  Status narration litters the spec and is exactly the residue that gets
  left behind after the work ships. (Maintainer rule, 2026-08-29.)
- **Commands in public prose are written for a stranger's machine, not
  transcribed from yours.** A "see it yourself" block is instructions, not a
  session log: no absolute paths from the writer's setup (`/tmp/ptyvenv`,
  scratch dirs, venv locations), no local scaffolding steps a reader doesn't
  need. Give the shortest portable commands that reproduce the result from a
  fresh clone; where a helper needs one-time setup, describe it in a phrase
  ("in a venv with `pyte` installed") rather than pasting your own setup
  lines. The `no_machine_local_paths` lint guards code; this rule is the
  same idea for PR/issue/commit text, where no lint will catch it.
- **Commit per unit of work, not per session.** A session limit once killed 220
  uncommitted lines and left the tree not building. An interim commit that
  compiles beats an uncommitted one that does not.
- **Commit before you attack your own work.** Disabling a check to prove its
  test fails is required here (§3.1, §3.4), and the restore afterwards is a
  destructive command: an agent ran `git checkout --` on the file it had just
  written but not yet committed, and lost it. Commit first, then attack, then
  restore — the restore has something to restore *to*.
- **Fix the defect you found; do not write it up.** When you discover a real
  defect while doing something else and the fix is contained, fix it in the
  same change. The test is blunt: if documenting the limitation costs more to
  carry than the fix costs to write, write the fix. A fix you have *verified*
  is never a "known issue" — verifying was the expensive part. A defect that
  produces a silently wrong answer is never deferrable: a loud failure can
  wait behind a caveat; a wrong value nobody is told about cannot. When a fix
  is genuinely out of scope, file an issue naming what it would take — never
  leave a caveat in prose that reads as a decision nobody made.
- **No fix may reduce the information rendered.** When a change removes text
  from the wrong place — a row smeared into a neighbour's description, a
  spelling folded into an alias row, a value name dropped by a merge — the
  same change must render that text in its right place, and every alias fold
  or merge touched is checked against the raw help text for dropped rows and
  dropped values. A filed issue is not a home; a diff that only deletes the
  misplaced text is an unfinished fix. The failure this prevents cleared
  every gate twice in one round: ar's `@<file>` row was "contained" into
  rendering nowhere, and ffplay's `--help topic` row vanished into an alias
  fold while tests, corpus, and sweeps all stayed green — a user of the
  release simply lost documented spellings. (Maintainer rule, 2026-08-31.)
- **A result that exists only on one machine is not a result.** `audit/queue.toml`
  is called *tracked* by `xtask::queue`'s module doc and again by spec §16's
  storage note — and was never committed by any commit on any branch, because
  the command that writes it had never been run. Two documents asserting a file
  exists is not evidence that it does. If something is meant to be tracked,
  `git add` it and confirm with `git ls-files`; if a doc claims a file is
  tracked, that claim is checkable and belongs in a test.
- **A location that only resolves on the machine you are sitting at never goes
  into a committed file.** Working against local, untracked data is normal here
  — the frozen `--help` captures under `audit/queue-captures/` are exactly that
  — and a task brief will often hand you where they live. That is permission to
  *read* them. It is not licence to write where they live into a source file, a
  test, a fixture, or a doc comment: such a line passes every gate on the
  machine that wrote it and is a lie on every other one. Anything a committed
  file must be able to open is reached repo-relative
  (`include_str!("../../corpus/…")`) or committed beside it as a corpus fixture.
  `mandible-extract/tests/no_machine_local_paths.rs` enforces this over the
  workspace's Rust sources and records the incident that produced it; it cannot
  see `.toml`, markdown, commit messages or PR bodies, so those are on you.
  Three agents in a single round had to be stopped mid-task for this one line,
  which is why it is written down as well as linted.
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

### 5.1 Change-trigger matrix

When you touch the left column, the right column moves **in the same
commit/PR** — this is a lookup, not a judgment call at the end of a round.
Each row points at the section that says why.

| If you change… | You must also… |
|---|---|
| Anything a user of a release would notice | add one single-line entry under CHANGELOG `## [Unreleased]` (§2) |
| A measurement that contradicts `spec.md` Appendix A | update Appendix A with the new number and its method (§4) |
| A design contract — schema, probe/argv rules, display semantics | amend the governing `spec.md` section; spec.md is the design authority, and a PR body is not a record |
| Rendering code | capture before/after pty screens (§3.2) and attach them to the PR |
| An extraction tier's argv construction | keep/add a test exercising the **real** argv (§3.1) |
| The `unsafe` count in `mandible-extract` | update the §5 exception list AND the crate doc comment in `mandible-extract/src/lib.rs` |
| Fixtures via `xtask corpus --bless` | check `git status` for invented xfail snapshots and delete them (§4) |
| A detector, lint, or CI guard | watch it fail first (§3.4) |

### 5.2 Recurring task playbooks

The two workflows that repeat every round, in the shape *what you see → what
you run → what you decide*, so they stop being re-derived from history.

**Adding or fixing a parser family.**
- *See:* a tool renders wrong in the TUI, or a sweep/verdict names a shape.
- *Run:* reproduce against the captured bytes or the live tool; write the
  corpus fixture first; implement in the framework/shape tier — never
  per-tool (§1); `xtask coverage --tools <affected…>` as the cheap pre-check
  (a pinned list reproduces full-`PATH` numbers exactly), then the full
  sweep when the change warrants it; the §5 gates.
- *Decide:* whether the recognizer can admit a currently-correct parse — if
  it can, keep it strict and fix the scoring instead (§5). Ship with a "see
  it yourself" block: the exact `mandible <tool>` commands and what changed,
  written portably (§5) — the maintainer verifies parser fixes visually, not
  by fixture green — plus pty screenshots in the PR.

**Cutting a release.**
- *See:* the maintainer has asked for a release, CHANGELOG `[Unreleased]`
  holds the round, and the maintainer's visual pass is done.
- *Run:* the release PR, then the tag; watch the workflow run whose
  tag/headBranch matches the tag just pushed — never "the latest run" —
  and confirm the assets and crates it actually published.
- *Decide:* nothing. Release timing and content are the maintainer's call;
  never tag or publish unprompted.

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

**Growth is policed by earned entries, not a line count.** There is no line
budget (the old ~200-line cap was arbitrary and is retired — maintainer,
2026-08-29). What keeps this file honest is the rule above: every entry names
the failure it prevents, and an entry whose cause is fixed gets deleted.
Design still belongs in `spec.md`, human process in `CONTRIBUTING.md`.

**Date-stamp anything environment-dependent**, and re-verify rather than trust
it. Facts about other people's tools go stale.
