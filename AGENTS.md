# AGENTS.md — working agreements for AI agents in this repo

Read this before touching code. Every rule a script can check is a pointer to
that script. What is left is prose, because no lint can catch it.

**Precedence:** `spec.md` is the design authority, so it says what to build and
why. This file is the operational authority, so it says how to work here.
`CONTRIBUTING.md` is for humans. If this file and `spec.md` disagree about
design, `spec.md` wins and you should fix this file.

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

## 2. Hard limits

Every number here is enforced, and none of them is a law of nature. Changing
one is a decision, and a decision goes in spec.md §16.

| Limit | Number | Enforced by |
|---|---|---|
| Code lines in a `.rs` file before `mod tests` | 800 | `scripts/shape_guard.sh` |
| Lines in a function | 150 | `clippy.toml`, `clippy::too_many_lines` |
| Cognitive complexity of a function | 30 | `clippy.toml`, `clippy::cognitive_complexity` |
| Lines in one comment block | 12 | `scripts/shape_guard.sh` |
| Comment lines over code lines in a file | 0.5 | `scripts/shape_guard.sh` |
| Tools a new parser heuristic must move | 5 | `xtask sweep-diff`, read by a human |

Functions that predate the size ceilings carry a scoped `#[allow]` with a
one-line reason. Every such allow is listed in `scripts/ratchet.txt`, and
`scripts/ratchet_check.sh` fails when the tree carries more allows than the
file lists. Files that predate the shape ceilings are counted the same way in
`scripts/shape_baseline.txt`. Both baselines are only ever supposed to shrink.
Adding a line to a baseline is not a fix.

### The scripts

Each was a paragraph in this file once. The script is the rule now. Run
`preflight.sh` and it runs the rest.

| Script | The rule it holds |
|---|---|
| `preflight.sh` | Everything here, in one command, plus the five gates |
| `pr_class.sh` | A prose-only diff goes direct to main, never through a pull request |
| `pr_class_hook.sh` | The same check, wired to `gh pr create` |
| `shape_guard.sh` | File size, comment blocks, comment ratio, and narrative prose |
| `ratchet_check.sh` | The tree may not gain a size-lint exemption |
| `changelog_guard.sh` | A released CHANGELOG section is history and may not be edited |
| `check_submissions.sh` | An audit submission matches its author and its path shape |
| `pty_screenshot.py` | What the TUI actually renders, on a real pseudo-terminal |

Two more live as tests: `std::process` appears only in
`mandible-extract/src/exec/`, and no machine-local path reaches a committed
Rust source (`mandible-extract/tests/no_machine_local_paths.rs`).

---

## 3. Rules no lint can catch

### 3.1 Admitting a new parser heuristic

A new heuristic earns its place by moving at least **five tools** fleet-wide,
or by making a tool that used to render as invented structure render verbatim
instead. Measure it, do not argue it. Sweep with
`cargo xtask coverage --out before.txt`, apply the change, sweep again into
`after.txt`, then run
`cargo xtask sweep-diff --before before.txt --after after.txt`.

Read the gains and the losses as two separate numbers. A heuristic that gains
six tools and loses two has moved four, and the two it lost are the interesting
half. Five is a starting threshold; revising it is a §16 decision.

Never loosen a recognizer to catch more cases when it can degrade a tool that
already works, because a permissive instrument hides the defects it exists to
find. Measure what loosening would cost first. If it admits any
currently-correct parse, keep the strict rule, record the miss as a documented
lower bound, and fix the scoring instead. Out-of-scope misses stay counted and
named in every report.

### 3.2 The comment contract

A comment at a call site says three things at most: the rule in at most three
lines, the fixture path that demonstrates it, and the atlas id or spec section.
Nothing else. No history, no reasoning chain, no tool inventory. The ceiling is
twelve lines per block and half a line of comment per line of code, and
`shape_guard.sh` holds both.

### 3.3 The shape atlas

Recurring shapes of help text live in `docs/shapes.md`, one entry per shape,
five fields each: `id`, `looks like`, `tools`, `handling`, `fleet`. That file
is the only home for shape narrative. A tool that exhibits a known shape adds
its name to that entry's `tools` field and nothing else. It does not earn a new
entry, a new comment, or a paragraph in the spec.

### 3.4 A known defect is a fixture

A parser defect you have reproduced becomes an `[xfail]` corpus fixture with
its note in `meta.toml`. It never becomes a bullet in an issue. A fixture is
executable and it survives; a bullet rots and nobody rereads it. The note says
what looks broken and what a fix would need. See `corpus/README.md`.

An xfail fixture has no `expected.snap`. `xtask corpus --bless` invents one
anyway, so check `git status` after any bless and delete it. Committing one
silently converts an xfail into a guarded wrong tree.

### 3.5 Green gates do not mean it works

Two real bugs here passed a full green suite. A cobra tier built argv without
the literal `__complete` and was completely dead in the real pipeline, because
its unit tests injected a mock probe that bypassed argv construction. So every
extraction tier needs a test that exercises real argv construction, and you run
the real binary against real data before claiming a feature works. "I could not
verify X" is a useful result, and a false "works" costs a session.

Two instruments say less than their names suggest. A green corpus fixture
proves the tree stopped changing, not that it is right, and a tree that is
wrong in a way no detector models reports `ok` forever. `xtask corpus --show
<tool>/<version>` prints `scope: unscoped` when nobody has judged it.
Re-measuring accuracy on the tools you just fixed is train-on-test, so it
measures the fix rather than the parser; `audit/queue.toml` exists to make an
unbiased draw possible. State the denominator, since audited tools that never
became fixtures are measured by nothing. Fleet-wide flag counts and tool-level
audit outcomes are different units, and the larger one never stands in for the
smaller.

### 3.6 No tty in the agent sandbox

`enable raw mode` fails with "No such device or address", so the TUI cannot be
run directly. Verify rendering through `TestBackend`, as in
`mandible-tui/tests/border_integrity.rs`. `TestBackend` alone is not enough. `scripts/pty_screenshot.py` forks a real
pseudo-terminal, sets an explicit window size, and replays the output through a
terminal emulator. Run it in a venv with `pyte` installed:

```console
$ .venv/bin/python scripts/pty_screenshot.py --keys '/run,<enter>,<tab>' \
      90 30 ./target/release/mandible docker
```

It found every rendering bug this project has had, and all of them were
invisible to `TestBackend`. Synthetic fixtures are chosen to be
representative, and real help output is not. Capture a screen before and after
any rendering change, and suspect the fixture when a rendering test passes
first try.

### 3.7 A guard is not done until you have watched it fail

Break the thing your guard protects. Plant the defect, remove the row, disable
the lane, and confirm the run goes red **naming what you removed**. A guard is
uniquely easy to write in a form that can never go red, and its green runs then
read as evidence forever. This project has paid for instrument blindness once:
the fabrication count read 154 when the true number was 52, because the
existence oracle could not see shapes it claimed to measure. Commit before you
attack your own work, so the restore afterwards has something to restore to.

### 3.8 Never parse human-format test output

`cargo nextest run --workspace`, never `cargo test` piped into `grep`. A
`grep -c FAILED` once matched test data containing the word FAIL and reported a
confidently wrong count, twice. Read the exit code, or
`--message-format libtest-json`. Never the prose.

### 3.9 No fix may reduce the information rendered

When a change removes text from the wrong place, the same change must render
that text in its right place. Check every alias fold and every merge you touch
against the raw help text for dropped rows and dropped values. A diff that only
deletes the misplaced text is an unfinished fix. This cleared every gate twice
in one round: `ar`'s `@<file>` row rendered nowhere and `ffplay`'s
`--help topic` row vanished into an alias fold, while tests, corpus and sweeps
all stayed green.

Fix the defect you find on the way. If documenting the limitation costs more to
carry than the fix costs to write, write the fix. A loud failure can wait
behind a caveat; a wrong value nobody is told about cannot.

### 3.10 A result that exists only on one machine is not a result

`audit/queue.toml` was called tracked by two documents and had never been
committed by any commit on any branch. If something is meant to be tracked,
`git add` it and confirm with `git ls-files`. Reading local untracked data is
normal here; writing where it lives into a source file, a test, a fixture or a
doc comment is not. Such a line passes every gate on the machine that wrote it
and is a lie on every other one.

### 3.11 War stories

Each is a bug a reasonable agent would repeat. One line, then where to look.

| Do not | Why, and where |
|---|---|
| Slice a `&str` from tool output at a raw byte offset | Panics off a char boundary. Shipped as a real crash in `help_text::sections`. Use `s.get(..n)` or `s.as_bytes().get(..n)`. |
| Poll a background job with `pgrep -f <string>` | The poller matches its own command line and reports the job alive forever. Use `pgrep -x`, or record the PID. |
| Call an O(n) function inside a `while` loop's condition | It reruns every iteration. One tool took 153s instead of milliseconds. Compute it once. |
| Reason a third time about a bug you have not reproduced | The `systemctl` freeze disproved two theories by measurement. See spec [M-19]. |
| Put a process change in the CHANGELOG | The file is what readers get as release notes. Behavior, features, breaking and visual changes only. |
| Hand-wrap a CHANGELOG entry | Every renderer reflows it. One entry is one logical line. |
| Send a framework protocol word without evidence the tool speaks it | `wall __complete` broadcast to every terminal on a machine, and `completion zsh` left 437 daemons running. See spec §6 rule 1a. |
| Treat a reaped process group as a finished probe | A daemonising child leaves the group and the session. 622 processes were found left behind. See spec §6 rule 4 and `exec/reap.rs`. |
| Add a third `#[allow(unsafe_code)]` to `mandible-extract` | The count is the whole point of `deny` over `forbid`. Update §4 and the crate doc in the same commit. |

---

## 4. Working agreements

- **Never attach a session URL anywhere on GitHub**, in a commit message, a
  pull request body, or a comment. There is no exception.
- **Never address other people on GitHub without the maintainer's explicit
  consent.** The account speaks with the maintainer's voice.
- **One branch and one pull request per bundle of tasks.** A second needs
  asking first. **If an artifact leaks into commit history, notify the
  maintainer and wait**; never force-overwrite published history.
- **Public prose describes the change, not the conversation.** A commit subject
  plus at most three lines. Never paste private instructions, transcribe a
  discussion, or dump spec text; link the section. No personal details beyond
  the git author, and no absolute path from your own machine.
- **The spec states the design and never narrates its status.** No "approved
  but not yet implemented", no "to be done", no conversation residue. §16 is
  the one home for rulings.
- **Commit per unit of work, not per session.** A session limit once killed 220
  uncommitted lines. **Commit before you attack your own work**; an agent ran
  `git checkout --` on a file it had written but not committed, and lost it.
- **`NOTICE` is not optional.** Vendored third-party data carries attribution
  obligations, and it is the most likely real legal exposure here.
- **Never invoke a tool binary outside the argv allowlist in spec §6.** A bare
  invocation is how you launch a REPL, block on stdin, or start a daemon.
- `#![forbid(unsafe_code)]` in every crate except `mandible-extract`, which
  carries `#![deny(unsafe_code)]` and exactly two scoped `#[allow(unsafe_code)]`
  sites, both in `exec/`: `pre_exec` plus `setsid` on the probe-spawning
  function, and `containment::secured_scoreboard_file`'s `File::from_raw_fd`.
  No `unwrap()` on any path reachable from tool input.

### 4.1 Writing

Load the humanizer skill before editing any document. It is vendored at
`docs/vendor/humanizer-SKILL.md` and recorded in `NOTICE`. Then:

- One fact per sentence. About twenty words. Plain verbs.
- No em-dashes. No "not X but Y". No bold mini-headings inside bullets.
- No dates, no branch names, and none of the status phrases
  `scripts/shape_guard.sh` lists. Dates belong in spec §16, Appendix A, and the
  atlas `fleet` field. Nowhere else.
- No inflated claims, no sales words, no vague sources, no stock AI words.

### 4.2 Pull request class

Run `scripts/pr_class.sh`. It prints `code` or `direct`.

`direct` means the diff touches nothing under `mandible*/`, `xtask/src` or the
Cargo manifests, so it goes straight to main. A corpus fixture is captured
bytes plus a contract, so it is data and it goes direct too. A docs-only push
skips CI entirely, so a self-opened self-merged pull request for a paragraph
adds a merge commit and a dead branch while gating nothing.
`scripts/pr_class_hook.sh` is the same check wired to `gh pr create`, and its
header says how to install it. `code` means a reviewer can catch a bug in it,
so it goes through one pull request per phase of work, not per file.

### 4.3 After a context compaction

Run `scripts/preflight.sh`, then reread the matrix below. State that matters
lives in tracked files, and anything you remember that is not in a tracked file
is not evidence.

### 4.4 Change-trigger matrix

Touch the left column and the right column moves in the same commit or pull
request. This is a lookup, not a judgment call at the end of a round.

<!-- directory-triggers:start -->
| Path | You must also… |
|---|---|
| `mandible-core/` | check merge and provenance rules against spec §4.2 and §5.2 |
| `mandible-extract/` | keep a test exercising real argv (§3.5); run `xtask corpus` |
| `mandible-search/` | check the entity kinds the index covers against spec §10 |
| `mandible-tui/` | capture before and after pty screens (§3.6) and attach them |
| `mandible/` | check the CLI surface against spec §2 and regenerate completions |
| `xtask/` | update `docs/instruments.md` if an instrument's question changed |
| `corpus/` | check `git status` for invented xfail snapshots (§3.4) |
| `audit/` | keep the submission path shape that `check_submissions.sh` enforces |
| `scripts/` | watch the guard fail before committing it (§3.7) |
| `.github/` | remember a workflow edit pushed to main runs nothing; verify on a pull request |
| `docs/` | keep `docs/shapes.md` to one entry per shape and five fields (§3.3) |
| `spec.md` | keep every `[M-n]` id resolvable; measurements go to Appendix A |
| `CHANGELOG.md` | run `scripts/changelog_guard.sh` locally |
<!-- directory-triggers:end -->

Three rows are about content rather than a path:

| If you change… | You must also… |
|---|---|
| Anything a user of a release would notice | add one single-line entry under CHANGELOG `## [Unreleased]` |
| A measurement that contradicts spec Appendix A | update Appendix A with the new number and its method |
| A design contract: schema, probe rules, display semantics | amend the governing `spec.md` section |

### 4.5 Recurring playbooks

**Adding or fixing a parser family.** Reproduce against the captured bytes.
Write the corpus fixture first. Implement in the framework or shape tier, never
per tool. Run `xtask coverage --tools <affected>` as the cheap pre-check, then
the full sweep when the change warrants it. Ship with the exact
`mandible <tool>` commands and pty screenshots, because the maintainer verifies
parser fixes visually rather than by fixture green.

**Cutting a release.** Open the release pull request, then the tag, then watch
the workflow run whose tag matches the tag you just pushed. Decide nothing.
Release timing and content are the maintainer's call.

---

## 5. Maintaining this file

This file's failure mode is not being wrong. It is **growing into a junk
drawer** that nobody reads, at which point it stops protecting anything.

**Add an entry when**, and only when, something went wrong that a reasonable
agent would repeat, and the lesson is not already discoverable from `spec.md`,
the type system, or a test that fails loudly.

Prefer making a mistake *impossible* over documenting it. A private field, a
newtype, a `#[deny]`, or a failing test is worth more than a paragraph here. If
you can encode the rule in a script, do that and delete the paragraph. §2 is
the record of every rule that has already made that trip.

**Every entry must state the failure it prevents.** An instruction without a
"why" cannot be evaluated later, so it never gets deleted, so the file rots.

**Delete aggressively.** An entry is dead when its cause is fixed. Deleting it
is a completed task, not a loss. **Do not duplicate `spec.md`**; link to it,
because two sources will disagree at the worst time.

There is no line budget. What keeps this file honest is the rule above. Design
belongs in `spec.md`, human process in `CONTRIBUTING.md`, environment
measurements in spec Appendix A.
