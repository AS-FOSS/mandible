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
# Who blessed this fixture's expected.snap — required on every fixture.
# See "Human vs. agent: `[bless] provenance`" below.
[bless]
provenance = "agent"

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
min_status = "ok"                    # floor: ok > incomplete > low-confidence
                                     # > verbatim > no-tier ("suspicious"
                                     # meets no floor)
min_subcommands = 20                 # coarse floor, not an exact count
must_contain_flags = ["--paginate"]  # optional spot-checks, root flags only
must_contain_positionals = ["pid"]   # same, for root positional operands —
                                     # matched on the operand's name, which
                                     # is what a user actually types. A
                                     # trailing `...` ("file...") also
                                     # requires the operand to be
                                     # repeatable: `[file...]` with the
                                     # dots glued loses the marker today
                                     # while `[file ...]` keeps it, and no
                                     # other field states the difference
must_contain_modifiers = ["a", "U"]  # same, for the single-letter modifiers
                                     # a tool documents in a modifier table
                                     # (`ar`'s `[a]`, `[U]`). Bare letters,
                                     # no brackets; case is significant,
                                     # since `[u]` and `[U]` are different
                                     # modifiers on the tools that have them

# The one *negative* claim: root flag spellings the tree must NOT carry.
# Everything above says "the parser dropped something real"; this says
# "the parser invented something". See "Stating that a flag does not
# exist" below for exactly what it does and does not assert.
must_not_contain_flags = ["--------------------------------"]

# The other negative shape: spellings that really exist and must NOT
# resolve to the same entity. Guards the alias-run fold specifically. See
# "Stating that two flags did not fuse" below.
must_keep_separate = [["-w", "-X"], ["-C", "-CC"]]

# A flag must own the named choice values, so a choices block that
# attached to the wrong flag is checkable instead of only visible in a
# snapshot diff. Matched the way `must_contain_flags` matches; the flag
# must exist and its choices must include every value listed.
[contract.must_attach_choices]
"--warnings" = ["gnu", "obsolete", "portability"]

# A flag's rendered description must contain this text — substring match
# after collapsing runs of whitespace on both sides to a single space
# (descriptions wrap), case-sensitive. Makes description recovery
# checkable instead of guarded only by the snapshot.
[contract.must_describe]
"--target" = "the triple to build for"

# Same idea, for a subcommand's own flags — keyed by its path (space-
# separated, tool's own name excluded), since `must_contain_flags` alone
# can only ever assert what a tool publishes at its *root*. Requires a
# `[[capture]]` for that subcommand's own `--help`/`-h`.
[contract.must_contain_flags_by_path]
restore = ["--source", "--staged"]

# A root flag's value placeholder must contain this text, keyed by the
# flag's own spelling. Substring match after collapsing whitespace, the
# same rule `must_describe` uses, and satisfied when *any* entity carrying
# that spelling matches — a tool can document one spelling on two rows
# (`vim`'s `-r` and `-r (with file name)`), and a first-match rule would
# report the wrong one. The gap it closes: a value spec that lost a token
# (`-V[N][fname]` read as `-V` plus `N`) leaves every other field intact,
# so nothing but the snapshot could see it.
[contract.must_value_name]
"-V" = "fname"

# Which dimensions of the tree a human actually verified before blessing
# this fixture. Optional; see "What `--bless` does and does not assert"
# below for the values and why an absent field means *no* scope, never
# every scope.
verdict_scope = ["flags", "subcommands"]

[xfail]                              # present only while the bug is unfixed
broken = true
reason = "command groups under flush-left headings are dropped; renders verbatim"
issue = "https://github.com/<org>/<repo>/issues/NNN"  # optional
```

### Stating that a flag does *not* exist: `must_not_contain_flags`

Every other `[contract]` field is a **positive** claim — it names something
the real tool really has, and fails when the parser drops it. That covers
the omission half of what can go wrong and none of the invention half: a
parser that reads a table ruler, a decorator, or a stray line of punctuation
as an option produces a flag nobody can point at, because there is no field
whose job is to say "this must not be here."

`must_not_contain_flags` is that field, and `corpus/mariadb-check/2.7.4` is
the instance it was built for. That tool's `Variables (--variable-name=value)`
defaults table opens with a header ruler, and the parser emits a flag whose
long name is that ruler. The tool has no such option.

It is matched **exactly the way `must_contain_flags` is**, negated:
`--foo` asserts no root flag has the long name `foo`, `-x` asserts none has
the short name `x`, a bare word is matched against the long name verbatim.
Write the spelling as it would be typed. What it deliberately does *not*
claim, so a fixture author never asserts more than they looked at:

- **Nothing about the raw capture.** This is a statement about the parsed
  tree only. The mariadb ruler occurs literally in `help.txt` and must go
  on occurring there — the capture is byte-exact. (This is also why the
  existence oracle cannot catch this defect: its question is "does this
  spelling occur in the raw text", and here it correctly answers yes.)
- **Nothing about the other spelling.** `--foo` says nothing about a short
  `-f`, and `-x` says nothing about any long name.
- **Nothing below the root.** Root flags only, the same scope
  `must_contain_flags` has. A subcommand inventing a flag would need a
  by-path analogue; this field does not quietly cover it.

A fixture that produces no root at all satisfies this vacuously and is not
reported — the one asymmetry with the positive fields, which a missing tree
trivially breaks. Dropping an entry is a weakening exactly as dropping a
`must_contain_flags` entry is, and `--baseline-dir` reports it as one.

### Stating that two flags did not fuse: `must_keep_separate`

`must_not_contain_flags` catches invention. It says nothing about a
different failure mode: two real, distinct flags read correctly on their
own, then wrongly merged onto one multi-spelling entity. An earlier
alias-run fold did exactly this — it merged rows on description equality
and fused unrelated flags together — and had to be reverted, because
nothing said the fold was not allowed to do that.

`must_keep_separate` is that field: a list of spelling groups, each
naming spellings that must resolve to distinct entities.

```toml
must_keep_separate = [["-w", "-X"], ["-C", "-CC"]]
```

Each inner list is checked independently. `cargo xtask corpus` fails,
naming the group and which of its spellings collapsed together, when two
or more spellings in one group resolve to the same entity. Matched
**exactly the way `must_contain_flags` is**, root flags only — the same
scope every other flag-shaped field has.

This is a negative claim, the same shape as `must_not_contain_flags`, and
follows it for the missing-root case: "these spellings never fused" holds
vacuously of a tree with no flags at all, so a fixture that produces no
root satisfies it and is not reported.

### A flag must own its choices: `must_attach_choices`

`--format`'s choice values belong to `--format`, not to whichever flag
happens to sit next to it in the source. A choices block that attaches to
the wrong flag produces a tree that looks complete — the values are
somewhere — and only a snapshot diff, read carefully, would ever say so.

```toml
[contract.must_attach_choices]
"--warnings" = ["gnu", "obsolete", "portability"]
```

The named flag, matched the way `must_contain_flags` matches, must exist
and its `Entity::choices` must include every value listed. This is a
**positive** claim: `cargo xtask corpus` fails when the flag is absent, or
when any listed value is not among the choices attached to it, naming
either the missing flag or the missing values. A fixture that produces no
root fails this exactly as it fails `must_contain_flags`.

### A flag's description says what it should: `must_describe`

A flag can carry a description and still carry the wrong one — text
recovered from the wrong line, or from a neighboring row. Nothing but a
byte-exact snapshot diff caught that until now (issue #102 item 5).

```toml
[contract.must_describe]
"--target" = "the triple to build for"
```

The named flag's rendered description must contain this text as a
substring. Both sides are compared after collapsing runs of whitespace to
a single space, because a real description wraps and a fixture author's
TOML value may not wrap the same way; the match itself is case-sensitive.
`cargo xtask corpus` fails when the flag is absent, or when the
description does not contain the text, naming what was expected and what
the description actually is (truncated to a readable length). A fixture
that produces no root fails this exactly as it fails `must_contain_flags`.

### A positional's description says what it should: `must_describe_positional`

`must_describe` walks only `root.flags()`, so nothing could assert a
positional's own description the way `invoke-rc.d` documents `action`
right under its usage line — until now (docs/shapes.md S-127).

```toml
[contract.must_describe_positional]
"action" = "Initscript action. Known actions are"
```

The named positional, matched on `Entity::primary_name`, root only, must
exist and its rendered description must contain this text as a substring
(same whitespace-collapsing comparison `must_describe` uses). `cargo xtask
corpus` fails when the positional is absent, or when its description does
not contain the text. A fixture that produces no root fails this exactly
as it fails `must_contain_positionals`.

### Stating that a description carries text it must not: `must_not_describe`

`must_describe`'s substring check cannot say a description is
*contaminated*: an unheaded example block folding onto a flag's real
description (`corpus/nfsslower-bpfcc/0.29.1`) still contains the real
text, so the positive check still passes. `must_not_describe` is the
negative half, the same shape `must_not_contain_flags` is for invented
flags.

```toml
[contract.must_not_describe]
"-p" = "trace pid 121 only"
```

The named flag's rendered description must NOT contain this text as a
substring, matched the way `must_describe` matches. Satisfied vacuously
when the flag is absent or the fixture produces no root, the same
reasoning `must_not_contain_flags` uses.

### What `--bless` does and does not assert: `verdict_scope`

`--bless` freezes the *entire* tree into `expected.snap` — node summaries,
flag descriptions, usage lines, all of it — regardless of which parts of
that tree a human actually read before running it. That gap is exactly
what the lsof cautionary tale above cost: a snapshot that matched itself
perfectly while three quarters of its flag descriptions were wrong,
because the person who blessed it never looked.

`[contract]`'s `verdict_scope` makes the *claim* a bless makes machine-
readable instead of leaving it to a prose comment (or worse, to nothing).
It is a list of which dimensions of the tree were actually looked at by a
human before this fixture was blessed:

- `"flags"` — the flag list (names, short/long spellings) was checked
  against the raw capture.
- `"subcommands"` — the subcommand tree was checked against the raw
  capture.
- `"descriptions"` — flag and node *prose* was checked against the raw
  capture (`corpus/README.md`'s full bless workflow, "every flag's
  description against the line it came from").
- `"usage"` — usage/synopsis lines were checked.

**An absent `verdict_scope` means no scope is claimed — never "every
dimension."** A blessed `expected.snap` freezes every field whether or
not a human read it, so treating silence as "everything verified" would
let the exact overclaim this field exists to prevent survive by omission.
This is deliberately the conservative reading: it is always safe to add a
truthful claim later, never safe to have quietly claimed one that wasn't
made. Most fixtures in this corpus (everything captured before this field
existed, and the hand-authored `git`/`tar` seed fixtures, which went
through the full bless workflow but never had the claim recorded) are
unscoped for exactly this reason — that does not mean they weren't
reviewed, only that the review's scope isn't machine-readable for them.

`xtask corpus --show <fixture>` prints a fixture's scope alongside its
raw capture and parsed tree; a checking run's per-fixture line adds a
`verdict_scope: ...` note when one is set; the `--format markdown`
transition report carries a `scope` column so a reviewer scanning a green
run can see, without opening `meta.toml`, which rows have unreviewed
prose. `check_contract` never reads this field — it is a record of what a
human checked by eye, not itself a check.

The 36 `audit-seed2` fixtures promoted from the seed-2 human audit
(`git show c9bfe76`) all carry `verdict_scope = ["flags", "subcommands"]`,
matching that audit's own declared scope: the reviewer judged flag and
subcommand accuracy only, and never looked at prose.

### Human vs. agent: `[bless] provenance`

`verdict_scope` records what a human reviewed. It has no complement: nothing
records which fixtures were blessed by an agent with **no** human eyes on
them at all, which is most of this corpus — the AGENTS.md measurement at
v0.3.1 found only 3 of 23 newly-passing fixtures carried a human
`verdict_scope`; the other 20 were agent-blessed trees the suite then
guarded against *changing*, not against being *wrong*. `[bless] provenance`
makes that complement machine-readable instead of leaving it to be inferred
from whether `verdict_scope` happens to be set.

It is a **required** top-level table, present on every fixture:

```toml
[bless]
provenance = "agent"
```

Three values:

- `"human"` — a human ran the bless that produced the *current*
  `expected.snap` bytes. **No fixture in this corpus carries this value
  today**, and that is the field's first finding rather than an oversight:
  every `expected.snap` here was written by a `--bless` run inside a commit
  carrying a `Co-Authored-By: Claude` trailer, including the `git`/`tar`
  seed fixtures, whose current snapshots were re-blessed by later
  grammar-fix commits. The value exists so a human bless has somewhere to
  be recorded when one happens.
- `"agent-then-human"` — the snapshot bytes are agent-authored, and a human
  has reviewed this fixture's tree and left a `verdict_scope` recording
  what they checked. It deliberately does **not** assert an ordering: a
  grammar fix landing after the human's review may have re-blessed bytes
  that human never saw, so this is strictly weaker than `"human"` and must
  never be read as "the current tree was human-verified". What it does
  assert is that a human looked at this fixture at some point and said so.
- `"agent"` — an agent blessed it and no human has left a review record.
  The conservative default, and the only value `xtask audit fixtures` ever
  writes. It is also the value for a fixture with a `verdict_scope` but no
  `expected.snap` at all (`mariadb-check/2.7.4`, still `[xfail]`): nothing
  has been blessed there, so there is no bless to attribute.

**An agent always writes `"agent"` here, and only a human may flip it to
`"human"` or `"agent-then-human"`.** This is the mirror of the rule
`verdict_scope` already carries ("an agent must never claim `verdict_scope`")
extended to the blessing act itself: an agent judging its own bless as
human-verified would defeat the field before it recorded anything. The
conservative default exists for the same reason `verdict_scope`'s absence
means "no scope claimed" rather than "every scope" — it is always safe to
upgrade a truthful claim later, never safe to have quietly overclaimed one.

`cargo xtask corpus`'s summary line splits its `ok` count by this field —
`71 ok (0 human, 39 agent-then-human, 32 agent)` — specifically so "N ok"
can never be read as "N human-verified"; `xtask corpus --show <fixture>`
prints the value alongside `scope`, and the `--format markdown` report
carries it as its own `provenance` column next to `scope`.

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
   files you captured), `[xfail]` describing what's wrong, and a required
   `[bless]` table. You do not need to produce `expected.snap` — a fixture
   marked broken has no expected tree yet — but `[bless] provenance` is
   still required: an agent writes `provenance = "agent"` here always (see
   "Human vs. agent" above); a human contributor may write `"human"` only
   once they've actually blessed the tree themselves.
5. **Open a PR containing only the fixture.** CI confirms the parser really
   does fail on it (an xfail that passes is flagged — the bug may already be
   fixed). A maintainer merges it: the bug is now an executable, reproducible
   backlog entry instead of a prose report.
6. **Optionally, fix it** — in the same PR or a later one, by you or anyone:
   improve the grammar (never add per-tool logic), run
   `cargo xtask corpus --bless` to accept the new snapshot, fill in
   `[contract]`, set `[bless] provenance` (`"agent"` unless a human is doing
   this bless and reviewing the tree themselves, in which case `"human"`),
   remove `[xfail]`. `cargo xtask corpus` (without `--bless`)
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
- **Deduplicate hash-identical captures across versions.** Many tools change
  their version string without changing a byte of their help text, and a
  second copy of the same bytes buys nothing while adding a file every future
  reader has to check against the first. Two fixtures may reference one
  capture; two copies of one capture is waste.
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
a dropped `must_contain_flags`/`must_contain_flags_by_path`/
`must_contain_positionals`/`must_not_contain_flags`/`must_keep_separate`/
`must_attach_choices` entry, a removed `must_describe` entry, a fixture
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
