# The corpus — how mandible stays fixed once it's fixed

This directory holds **frozen captures of real tools' help output, paired with
the parse mandible is expected to produce from them**. It is the project's
regression ratchet: once a tool is confirmed working, no future grammar change
may silently break it, because the exact bytes that worked are here and CI
re-parses them on every PR.

If you found a tool that mandible renders badly, this is where your
contribution goes — and you can contribute it **without writing any Rust**.

## What the corpus is, and is not

- It is **test data only**. The `mandible` binary never reads this directory;
  nothing here changes what any user sees at runtime. The only thing that
  fixes a tool for users is a grammar change in `mandible-extract`.
- It is therefore **not** the per-tool knowledge the project's core invariant
  forbids (see CONTRIBUTING.md, "The invariant"). A corpus entry is a *fact
  about a failure*; the parser may never consult it, key on it, or special-case
  the tool it names. Runtime per-tool overrides remain user-local
  (`~/.config/mandible/overrides/`) and are never committed here.
- A fixture cannot go stale in the way vendored catalogs did. It asserts
  "given these frozen bytes, the parser produces this tree" — a statement
  about the *parser*, true forever. When a tool's help changes in a new
  version, that's a **new sibling fixture**, not an update to the old one.

## Layout

```
corpus/<tool>/<version>/
  meta.toml         # the contract, and the map from argv to capture files
  help.txt          # a raw capture, byte-exact — named by meta.toml's
                    # [[capture]], not by a fixed convention (see below)
  help.stderr.txt   # that capture's stderr, only when non-empty
  expected.snap     # the CommandNode tree snapshot (plain YAML — see below)
```

`<version>` is the tool's own reported version (`git --version` → `2.43`).
If a version bump produced byte-identical help output, skip the new directory.

## `meta.toml`: the contract vs. the snapshot

Each entry has a **descriptive** half and a **normative** half, and the
distinction is what makes the ratchet work:

- `expected.snap` is *descriptive*: the current exact parse, as plain YAML —
  `mandible_core::to_snapshot` run through `serde_yaml`, nothing more. It may
  change in any PR that changes the grammar; `cargo xtask corpus --bless`
  rewrites it to match a fresh extraction, and reviewing that diff is the
  review artifact. (Not `cargo insta` — a CLI binary can't drive `insta`'s
  review workflow sanely, since it assumes a test binary with dynamic
  per-test snapshot paths. `xtask corpus` does a plain file compare instead,
  with `--bless` as `insta`'s `review` equivalent.)
- `meta.toml` is *normative*: the promises that may never silently weaken.
  `cargo xtask corpus` fails, naming the tool and exactly which promise
  broke, if a change violates them. Weakening one is allowed only by editing
  this file — an explicit, reviewable act.

**`--bless` is a human assertion of correctness, not a mechanical accept.**
It rewrites `expected.snap` to match whatever the parser just produced —
it does not check that production against anything, and a clean `cargo
xtask corpus` run afterward only proves the snapshot now matches itself,
never that either one is right. Running `--bless` and committing the diff
without reading it is indistinguishable, to every later reader and to CI,
from asserting the new tree is correct. Before you run it, **read the raw
capture and the resulting tree side by side** — every flag's description
against the line it came from, every subcommand against the heading that
named it — and only bless once you can say the tree is what the raw text
actually says. `corpus/lsof/4.95.0` is the fixture that proves what
skipping this costs: it was committed green (`--bless`, then `[xfail]`
removed) by blessing a parse where a three-column options table had been
read as one column, so roughly three quarters of its "described" flags
carried another flag's description instead of their own — a snapshot that
matched itself perfectly and was still wrong. It is `[xfail]` again now,
specifically because that review did not happen the first time.

```toml
[tool]
name = "git"
version = "2.43.0"
platform = "ubuntu-24.04"            # where captured
captured_with = "mandible 0.2.2"     # capture tooling version

# One entry per probe a tier actually sends. A generic-parser tool needs one
# (its root `--help`); cobra needs two per node (`""` and `-` each return
# different halves — subcommands vs. flags, spec Appendix A [M-*]); a tool
# with subcommand fixtures needs one entry per captured node. `argv` is the
# full command line, argv[0] included, exactly as a contributor would type
# it — but note the runner strips argv[0] before matching: the replay seam
# (`mandible_extract::exec::Transcript`) keys on `InertArgv::args()`, which
# is the real argument vector a tier sends and *excludes* the tool name.
# `["git", "--help"]` here therefore matches a tier's `--help` probe, and
# `["git", "commit", "--help"]` matches its `commit --help` probe.
[[capture]]
argv = ["git", "--help"]
stdout = "help.txt"
stderr = "help.stderr.txt"           # optional, omit when empty
exit_code = 0                        # optional, defaults to 0

[contract]
expected_framework = "generic"       # from `mandible --doctor git`; the
                                     # detected Framework's name, or
                                     # "generic" when Tier A′ found none
min_status = "ok"                    # floor: ok > low-confidence > verbatim
                                     # > no-tier ("suspicious" meets no floor)
min_subcommands = 20                 # coarse floor, not an exact count
must_contain_flags = ["--paginate"]  # optional spot-checks, root flags only

# Same idea, for a subcommand's own flags — keyed by its path (space-
# separated, tool's own name excluded), since `must_contain_flags` alone
# can only ever assert what a tool publishes at its *root*. Requires a
# `[[capture]]` for that subcommand's own `--help`/`-h`.
[contract.must_contain_flags_by_path]
restore = ["--source", "--staged"]

[xfail]                              # present only while the bug is unfixed
broken = true
reason = "command groups under flush-left headings are dropped; renders verbatim"
issue = "https://github.com/<org>/<repo>/issues/NNN"  # optional
```

Lifecycle rules, enforced by `cargo xtask corpus`:

- `xfail` → passing: allowed in any PR. This is the good direction.
- passing → `xfail`, or weakening any `[contract]` field: only via an explicit
  edit to `meta.toml`, justified in the PR description.
- A fixture, once merged, is **never deleted** because it became inconvenient.
  It may be deleted if the capture itself was wrong (mis-captured, wrong tool).
- **Strict xfail, both directions.** A fixture marked `[xfail]` whose
  snapshot and every `[contract]` field now pass **fails the run** — that
  means the bug got fixed and the fixture is stale; promote it (remove
  `[xfail]`, keep `expected.snap`). A fixture marked `[xfail]` that still
  fails some check is fine and does not fail the run — that's the expected,
  documented-broken state. Both directions are checked on every run, not
  just the "did it get fixed" one: it's equally a bug if a fixture claims to
  be broken but every check quietly passes.

## Contributing a fixture: the workflow

You found a tool that parses badly. The steps:

1. **Diagnose**: run `mandible --doctor <tool>`. Note the detected framework
   and tier status — this goes in `meta.toml` and the issue.
2. **Capture** the raw output *the same way mandible probes it* (sanitized
   env, both streams). Until `mandible capture` ships (see Status below):

   ```console
   $ TERM=dumb NO_COLOR=1 COLUMNS=100 LC_ALL=C.UTF-8 \
       <tool> --help > help.txt 2> help.stderr.txt
   ```

   Delete `help.stderr.txt` if empty (and leave it out of `meta.toml`'s
   `[[capture]]` entry — `stderr` is optional there for exactly this case).
   Do the same for any subcommand that demonstrates the problem, with its
   own `[[capture]]` entry naming its own argv and files.
3. **Review your capture for private data.** Help output can embed your
   hostname, username, or paths. mandible's own probes mask sandbox paths
   back to `$HOME`-style variables; a manual capture has no such protection —
   read the file before you commit it.
4. **Write `meta.toml`** with `[[capture]]` (mapping the argv you ran to the
   files you captured) and `[xfail]` describing what's wrong. You do not
   need to produce `expected.snap` — a fixture marked broken has no expected
   tree yet.
5. **Open a PR containing only the fixture.** CI confirms the parser really
   does fail on it (an xfail that passes is flagged — the bug may already be
   fixed). A maintainer merges it: the bug is now an executable, reproducible
   backlog entry instead of a prose report.
6. **Optionally, fix it** — in the same PR or a later one, by you or anyone:
   improve the grammar (never add per-tool logic), run
   `cargo xtask corpus --bless` to accept the new snapshot, fill in
   `[contract]`, remove `[xfail]`. `cargo xtask corpus` (without `--bless`)
   enforces that **every other fixture stays green** — that is the entire
   point. A fix that breaks another fixture will be named, and the tension
   goes to review instead of shipping. Note the order: blessing happens
   *before* removing `[xfail]`, and that's deliberate — the strict-xfail
   check on the very next plain run is what then tells you to remove it.

Fixture-only PRs (step 5) need no Rust and are near-trivially mergeable.
Grammar PRs (step 6) are held to CONTRIBUTING.md's full bar.

## Which frameworks exist

Parsing is keyed by the *framework that generated the help text*, never by
tool name. The authoritative list is the `match` in
`mandible-extract/src/help_text/profile.rs` plus the fingerprints in
`mandible-extract/src/framework/`; `mandible --doctor <tool>` reports what was
detected for your tool. A tool detected as nothing is parsed by the generic
layout engine (`expected_framework = "generic"`) — that is normal and most
hand-rolled tools live there. If you believe a tool's framework *should* be
detected and isn't, say so in the fixture's xfail reason; fingerprint
regressions are gated on `expected_framework` exactly like parse regressions.

## Size and hygiene

- Captures are text, typically a few KiB. Anything over 256 KiB needs a
  justification in the PR (some tools are genuinely huge; `curl --help all`
  is legitimate).
- **Never commit a binary.** If a fixture concerns artifact fingerprinting
  (detection from the compiled binary rather than its output), record the
  extracted marker strings in `meta.toml`, not the executable.
- Captures are byte-exact: no re-wrapping, no trailing-whitespace cleanup,
  no editor "fixing" the file. The mess is the test — the repo's root
  `.gitattributes` marks everything under `corpus/` as `-text` specifically
  so Git's own CRLF/whitespace normalization can't be the thing that quietly
  "fixes" it.

## Running the suite

```console
$ cargo run -p xtask -- corpus            # check every fixture, exit non-zero on any regression
$ cargo run -p xtask -- corpus --bless    # rewrite expected.snap to match a fresh extraction
$ cargo run -p xtask -- corpus --dir some/other/corpus   # point at a different corpus root
$ cargo run -p xtask -- corpus --baseline-dir /tmp/corpus-at-main   # also flag weakened [contract]s
```

`--baseline-dir` diffs every fixture's `[contract]` against a second, plain
corpus directory and prints a prominent `CONTRACT WEAKENED: <fixture> <field>`
line for each field that got weaker (lowered `min_status`/`min_subcommands`,
a dropped `must_contain_flags`/`must_contain_flags_by_path` entry, a fixture
newly marked `[xfail]`, or a fixture missing entirely) — reported, never
gated, since weakening a contract deliberately is still legal (the lifecycle
rules above). This binary **has no git access and never will** — the
workspace-wide `no_process_outside_exec` test forbids `std::process` outside
`mandible-extract/src/exec/`, `xtask/src` included — so `--baseline-dir` takes
a plain directory, never a git ref: populate it however you like (a CI step
running `git archive <base-ref> corpus | tar -x -C <dir>` before invoking
`xtask corpus` is the intended shape). Omit the flag and nothing changes —
`.github/workflows/ci.yml`'s `corpus` job does not pass it yet, so this check
does not currently run in CI; wiring that step is open follow-up work, not
yet done.

Runs with **zero subprocesses**: every fixture is replayed through the real
tiered extraction pipeline via the `Transcript` probe
(`mandible-extract/src/exec/probe.rs`), never a live spawn. Per-fixture
output names the tool and exactly what broke, e.g.:

```
git/2.43.0   ok                   (2.9ms)  snapshot: match
tar/1.35     ok                   (6.1ms)  snapshot: match
```

(A fixture still marked `[xfail]` reports its unmet promises instead, e.g.
`xfail (as expected)  (1.4ms)  contract: must_contain_flags: missing --paginate;
snapshot: none yet (legal while [xfail])` — see the `[xfail]` example above.)

A fixture also fails outright if it parses slower than ~100ms — deliberately
coarse, a mechanical net for an accidental O(n²)-in-a-loop bug rather than a
gate on ordinary millisecond-scale noise (see AGENTS.md's invariant table for
the incident that motivated it), and applied regardless of `[xfail]` status:
a fixture's *content* may be documented-broken, but a slow parse never is.

## Status

The corpus contract above is adopted, and the runner described in "Running
the suite" implements it in full: snapshot + `[contract]` + strict-xfail
(both directions) + the parse-time ceiling, over the two seed fixtures —
both green (`tar`: the [M-10] phantom-subcommand war story locked in at
zero subcommands; `git`: promoted from `[xfail]` once its
`extract_positionals` defects and a related negatable-flag defect were
fixed, its `restore` captures now also exercising [M-16]'s man-page→`-h`
fallback in replay). Still open:

- [ ] `mandible capture <tool>` — one-command fixture bundle with masking
- [ ] `xtask`-generated `FRAMEWORKS.md` table
- [ ] Wiring `.github/workflows/ci.yml`'s `corpus` job to populate a
      baseline directory from the PR's base ref and pass `--baseline-dir`,
      so `CONTRACT WEAKENED` actually appears on real PRs — the `xtask`
      side (`--baseline-dir`, `contract_weakened_lines`) is implemented and
      tested; the CI step that feeds it a real baseline is not
- [x] This contract document
- [x] The `xtask corpus` runner itself
- [x] Wiring `cargo xtask corpus` into CI as a required check (`.github/
      workflows/ci.yml`'s `corpus` job — a hard gate, unlike the PATH
      sweep, because this runner spawns no subprocesses and reads only
      frozen bytes: nothing about it can flap)
- [x] A markdown transition report for CI to post (`cargo xtask corpus
      --format markdown`, written to `$GITHUB_STEP_SUMMARY`) — status,
      node/flag counts, and named subcommand/flag deltas, never a raw
      `expected.snap` diff
