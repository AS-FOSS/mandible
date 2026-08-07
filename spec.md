# mandible — Design Specification

**A universal, interactive TUI reference for CLI tools, in Rust.**

> `mandible git` opens an explorable tree of every command, subcommand, and flag `git` has — with descriptions, not just names — plus a search bar that finds the flag you half-remember.

This document is the design reference and the build guide. Every claim about the
outside world in this document has been measured on a real machine; measurements
are collected in [Appendix A](#appendix-a--measured-baseline) and cited inline as
**[M-n]**. When a measurement contradicts an assumption, the measurement wins.

**Revision 3.** Revision 3 deletes the vendored spec catalog and the on-disk cache, and reorganizes `--help` parsing around the *framework* that generated the text (§7 Tier A′, §7 Tier B, §11). Revision 2's changes from revision 1 are in [Appendix B](#appendix-b--what-changed-in-revision-2).

---

## Table of contents

1. [Product definition & non-goals](#1-product-definition--non-goals)
2. [Vision & UX flow](#2-vision--ux-flow)
3. [The core challenge: universal CLI introspection](#3-the-core-challenge-universal-cli-introspection)
4. [The intermediate representation](#4-the-intermediate-representation)
5. [The extraction model: authority, laziness, cost](#5-the-extraction-model-authority-laziness-cost)
6. [Execution safety policy](#6-execution-safety-policy)
7. [Extraction tiers, in detail](#7-extraction-tiers-in-detail)
8. [Crate & workspace architecture](#8-crate--workspace-architecture)
9. [TUI design](#9-tui-design)
10. [Search](#10-search)
11. [No cache](#11-no-cache)
12. [Implementation roadmap](#12-implementation-roadmap)
13. [Testing & the coverage harness](#13-testing--the-coverage-harness)
14. [Dependency table](#14-dependency-table)
15. [Packaging & distribution](#15-packaging--distribution)
16. [Open risks & honest caveats](#16-open-risks--honest-caveats)
17. [Investigated and deferred: local NL search](#17-investigated-and-deferred-local-nl-search)
- [Appendix A — Measured baseline](#appendix-a--measured-baseline)
- [Appendix B — What changed in revision 2](#appendix-b--what-changed-in-revision-2)

---

## 1. Product definition & non-goals

**mandible is a reference browser, not a command builder.** The user's journey is:
*"I know roughly what I want; show me the flag and its exact spelling, then let me
copy it."* Everything in this spec is subordinate to that.

**The invariant that defines the project:**

> The mandible repository will never contain per-tool logic. No `if tool == "docker"`,
> no vendored per-tool patch file, no tool-name-keyed special case in any tier.
> Tool-specific knowledge lives in exactly two places: (a) third-party structured
> catalogs we consume wholesale as *data*, and (b) user-local override files that
> are never checked into this repo.

If a change would violate that invariant, the correct fix is to improve a general
parser, add a general extraction tier, or accept the gap and show it honestly in
the UI. This is the whole reason the tiered design exists; a single exception
starts the erosion.

**Non-goals** (stated so they don't quietly creep in):

- Executing the tool being documented. mandible never runs a user's command for them.
- Being a shell completion engine. Carapace and friends already do that.
- Natural-language command synthesis. See §17 for why this is deferred, not planned.
- Perfect fidelity. The pipeline is best-effort by construction; the UI's job is to
  make the confidence level legible, not to hide it.

---

## 2. Vision & UX flow

```
$ mandible git
```

opens a full-screen TUI:

```
╭─ search ─────────────────────────────────────────────────────────────────────╮
│ › squash                                                                     │
╰──────────────────────────────────────────────────────────────────────────────╯
╭─ git ──────────────────────╮╭─ git › rebase ─────────────────────────────────╮
│ ▾ git                      ││                                                │
│   ▸ add                    ││  Reapply commits on top of another base tip     │
│   ▸ commit                 ││                                                │
│   ▾ rebase              ●  ││  DESCRIPTION                                    │
│     ▸ --onto               ││  If <branch> is specified, git rebase will      │
│   ▸ stash                  ││  perform an automatic git switch <branch>       │
│   ▸ merge                  ││  before doing anything else. …                  │
│                            ││                                                │
│                            ││  FLAGS                                          │
│                            ││  -i, --interactive     Make a list of commits   │
│                            ││                        about to be rebased      │
│                            ││      --autosquash      Auto-move fixup! commits │
│                            ││  -S, --gpg-sign[=KID]  GPG-sign commits         │
│                            ││                                                 │
│                            ││  carapace · structure ✓ · prose ✓               │
╰────────────────────────────╯╰────────────────────────────────────────────────╯
 ↑↓ move   → expand   / search   y copy   ? help   q quit
```

**Layout.** Vertical: search bar (3 rows) / body (fill) / status bar (1 row).
Body horizontal: tree pane / detail pane. The tree pane is
`Constraint::Min(24)` and the detail pane fills the remainder — **not a
percentage split**. At 80 columns a 35% tree pane leaves ~20 usable cells after
borders and depth indentation, which is not enough for a name plus a summary
[M-7]. Below 60 columns total, drop summaries from tree rows entirely; below 50,
the panes stack vertically and the detail pane is toggled with `Tab`.

**Design principles.**

- **One accent color, used sparingly** — the selected row, and flag names in the
  detail pane. Everything else neutral: dim gray for hints and summaries, default
  foreground for names. No color as decoration.
- **Consistent indentation** — each tree depth is exactly 2 cells. Expanding
  changes only the chevron glyph (`▸`/`▾`), never the row's horizontal layout.
  This matters for mouse hit-testing (§9) and for not making the eye re-track.
- **Breadcrumbs in the detail pane header**, always showing the full path
  (`git › rebase`), so context survives scrolling.
- **Provenance is legible, not decorative.** The footer of the detail pane names
  the contributing sources and whether structure and prose each came from a
  trusted source. This is a trust calibration device, and it must be *accurate*
  (see §4's per-field provenance) or it is worse than nothing.
- **Rounded borders** (`BorderType::Rounded`), consistent 1-cell padding, no
  nested boxes.

**Flags are not tree rows.** `git` alone carries 2,999 flags [M-1]; putting them
in the tree makes the tree useless. Flags live in the detail pane. They are still
independently *searchable* and *addressable* (§4 `NodeRef`, §10), which is what
users actually need — the tree is for structure, search is for flags.

**Interaction model.**

| Key | Action |
|---|---|
| `↑`/`↓`, `k`/`j` | Move tree selection |
| `→`/`Enter`/`l` | Expand (triggers lazy extraction if the subtree is unfilled) |
| `←`/`h` | Collapse, or jump to parent if already collapsed |
| `/` | Focus search |
| `Esc` | Leave search, **keeping** the filter pinned; `Esc` again clears it |
| `Tab` | Move focus between tree and detail pane (detail pane scrolls with `↑↓`) |
| `y` | Copy: the selected flag's spelling, or the node's full command path |
| `?` | Keybinding overlay |
| `r` | Re-extract this tool, bypassing cache |
| `q`, `Ctrl-C` | Quit |
| Mouse | Click row selects; click chevron toggles; wheel scrolls the pane under the cursor |

`y` is not a nice-to-have. Looking up a flag in order to type it is the terminal
step of the core journey, and a reference tool that can't hand you the string
makes you retype it.

---

## 3. The core challenge: universal CLI introspection

**There is no universal, machine-readable standard through which CLI tools expose
their own structure.** Unlike OpenAPI for HTTP APIs, nothing equivalent ships with
most binaries. This is a fact about the ecosystem, not a gap to architect around;
any design that pretends otherwise degrades into per-tool patches.

What exists is a fragmented set of partial mechanisms. Their **measured** coverage:

| Mechanism | Reality on a real machine |
|---|---|
| **carapace-spec catalog** (declarative YAML, hundreds of tools) | 740 tools, 48,224 flag descriptions in the vendored snapshot. `git` 279 nodes / 2,999 flags / 2,979 with prose; `docker` 162/836/836; `gh` 249/1,061/1,061 [M-1]. Zero subprocesses, works on Windows. |
| **cobra `__complete`** (Go: kubectl, docker, gh, helm) | Works, and is version-accurate. But returns **only subcommands** for an empty word; flags require a *second* probe with `"-"` [M-2]. Descriptions are terse. Cost: one subprocess per node per probe [M-3]. |
| **clap `CompleteEnv`** (`COMPLETE=zsh <tool>`) | **Near-absent in the wild.** `ripgrep` errors; `cargo` prints ordinary help [M-4]. It is opt-in and few published binaries enable it. |
| **`<tool> completion bash\|zsh`** | Common. But bash scripts typically *compute* candidates at runtime (`$(git ls-files)`), so static parsing recovers less than it appears. zsh `_arguments` blocks are the description-bearing form and the higher-value target. |
| **man pages** (`mdoc(7)` semantic, `man(7)` prose) | Prose-rich, structure-poor, and **absent on many systems**: this test container has 31 `man1` pages and none for `git` or `curl` [M-5]. `libmandoc` is not a shipped library on Linux [M-6]. |
| **`--help`** | Universal, and the only thing every tool has everywhere. Also the messiest: output may go to **stderr** and the exit code may be **non-zero** [M-8]. |

The honest design goal is therefore **a tiered pipeline that merges the best
available source per field, and degrades visibly rather than silently.**

The key architectural consequence, and the thing that makes "no monkeypatching"
real: *the universal parser is not any one technique — it is the fact that every
technique normalizes into one schema.* The TUI, the search index, and the cache
never know which tier produced a field. That is what lets a Tier 6 be added next
year without touching the UI.

---

## 4. The intermediate representation

```rust
/// mandible-core: the shared schema every extraction tier must produce.
pub struct CommandNode {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: Option<Text>,          // one-line hint
    pub description: Option<Text>,      // long-form prose
    pub usage: Vec<Text>,               // raw usage patterns, kept verbatim
    pub flags: Vec<Flag>,
    pub positionals: Vec<Positional>,
    pub subcommands: Vec<CommandNode>,
    pub examples: Vec<Example>,
    pub hidden: bool,
    pub deprecated: Option<Text>,       // Some(reason) when deprecated
    /// True when this node's children are known-complete. False means the
    /// subtree has not been extracted yet (see §5, lazy extraction).
    pub children_filled: bool,
    pub provenance: Provenance,
}

pub struct Flag {
    pub short: Option<char>,
    pub long: Option<String>,
    pub value_name: Option<String>,     // "FILE" in `--output FILE`
    pub value_kind: ValueKind,
    pub choices: Vec<Text>,             // `--format {json|yaml|table}`
    pub repeatable: bool,
    pub required: bool,
    pub hidden: bool,
    pub deprecated: Option<Text>,
    /// True when inherited from an ancestor (cobra persistent / carapace
    /// `persistentflags`). Rendered in a separate, dimmed group.
    pub inherited: bool,
    /// Display grouping from the source, e.g. tar's "Main operation mode".
    pub group: Option<String>,
    pub description: Option<Text>,
    pub default: Option<Text>,
    pub env_var: Option<String>,
    pub provenance: Provenance,
}

pub enum ValueKind { None, Required, Optional }

pub struct Positional {
    pub name: String,
    pub required: bool,
    pub variadic: bool,
    pub description: Option<Text>,
    pub provenance: Provenance,
}

pub struct Example { pub command: Text, pub explanation: Option<Text> }
```

### 4.1 `Text`: the sanitization invariant

**Every string that originates outside this process is a `Text`, never a
`String`.** `Text` is a newtype whose only constructor sanitizes:

```rust
impl Text {
    /// The ONLY way to build a Text from tool output. Strips C0 control
    /// characters and ANSI/OSC escape sequences, resolves backspace-overstrike
    /// (`_\bX` and `X\bX`, as emitted by rendered man pages), expands tabs to
    /// spaces, collapses runs of whitespace, normalizes newlines, and truncates
    /// to a hard cap.
    pub fn sanitize(raw: &str) -> Text;
}
```

This is an **IR invariant, not a widget concern.** A single `\n` inside a
`ratatui` `Span` shifts cells and eats a pane border; the previous
implementation attempted to fix this twice at the widget layer and reverted both
times. The fix has to be at the boundary where untrusted text enters the IR,
because there are three consumers (tree, detail pane, clipboard) and each would
otherwise need its own defense. Widgets are permitted to assume `Text` is clean.

`Text` retains paragraph breaks (`\n\n`) for the detail pane's description
rendering; the tree pane collapses to a single line at render time.

### 4.2 Provenance is per field, not per node

```rust
pub struct Provenance {
    /// Which sources contributed to this item, ordered by contribution.
    pub sources: SmallVec<[Source; 2]>,
    /// Set only when a heuristic tier produced this item.
    pub confidence: Option<f32>,
}

pub enum Source {
    NativeDynamic { protocol: &'static str },  // "clap-complete-env", "cobra-dunder-complete"
    KnownSpec { provider: &'static str },      // "carapace", "withfig"
    CompletionScript { shell: &'static str },
    ManPage { format: ManFormat },             // Mdoc | Man
    HelpText,
    UserOverride,
}
```

Revision 1 attached one `Provenance` to a node while merging fields
independently. After a three-tier merge the node's badge names whichever tier
landed first, while the flag descriptions underneath may come from a different
tier entirely — **the badge lies.** Since the badge exists specifically as a
trust signal, an inaccurate one is worse than none. Provenance therefore lives on
`CommandNode`, `Flag`, and `Positional` individually, and the detail pane's footer
summarizes: `carapace + help-text · structure ✓ · prose ✓`.

### 4.3 Addressing: `NodeRef`

```rust
pub enum NodeRef {
    Command(Vec<String>),                 // ["git", "rebase"]
    Flag { path: Vec<String>, key: FlagKey },   // ["git","rebase"] + --interactive
}
```

Paths are name-based, which is fine for commands but insufficient for search
results, which must be able to point at a *flag* (§10). `NodeRef` is the single
addressing type used by search, the clipboard, and the cache.

Resolution walks `subcommands` by exact name match at each level. It must not
contain a "skip any segment equal to the current node's name" shortcut — that
silently mis-resolves a subcommand sharing its parent's name.

### 4.4 Merge: two axes of authority

Revision 1 merged with "first tier in priority order wins," which is only correct
if priority equals fidelity. It does not: the tier with the best *structure* is
frequently not the one with the best *prose* [M-1, M-2]. Each source therefore
declares two authority levels, and merge resolves per field against the relevant one:

```rust
pub struct Authority {
    /// Trust for names, nesting, arity, which flags exist.
    pub structural: u8,
    /// Trust for descriptions, summaries, examples.
    pub prose: u8,
}
```

| Source | structural | prose | Why |
|---|---|---|---|
| `UserOverride` | 255 | 255 | Explicit user intent always wins |
| `NativeDynamic` | 200 | 40 | Version-accurate structure; terse or absent prose |
| `CompletionScript` | 150 | 30 | Accurate names; prose rarely present |
| `KnownSpec` (carapace) | 120 | 200 | Curated and prose-rich, but a snapshot that can lag the installed version |
| `ManPage` | 60 | 180 | Excellent prose, weak/partial structure |
| `HelpText` | 80 | 120 | Always available; both axes heuristic |

Merge rules:

- A field is taken from the contributing source with the highest authority on
  that field's axis. Ties break toward the earlier contributor.
- `None`/empty never displaces a value, regardless of authority.
- Flags unify by **alias pairing**, not by long-name equality alone. Sources
  legitimately emit a flag's short and long forms as separate items — `gh
  __complete pr -` returns `--repo` and `-R` as distinct rows with identical
  descriptions [M-2]. Pairing runs *before* merge: within a node, items whose
  descriptions match exactly and whose short/long slots are complementary unify
  into one `Flag`. Revision 1's `same_flag` could never unify these and would
  render one flag twice.
- Subcommands merge recursively by name.
- `children_filled` is the logical OR of contributors.

---

## 5. The extraction model: authority, laziness, cost

### 5.1 The cost problem, measured

Building a cobra tool's tree by recursive probing is not cheap:

```
docker   255 nodes,  232 subprocess spawns,  10.5 s   (depth-capped at 3; docker is deeper)
gh       196 nodes,  182 spawns,             11.6 s
         ~40–65 ms per spawn                                                    [M-3]
```

That is with **one** probe per node. A correct cobra implementation needs two
(subcommands, then flags [M-2]), so ~20–25 s, and uncapped depth is worse. This
is the single largest UX risk in the project and revision 1 did not mention it.

### 5.2 The trait: one node at a time

A whole-tree `extract()` forecloses the only real fix. The trait is therefore
node-scoped:

```rust
pub trait ExtractionTier: Send + Sync {
    fn name(&self) -> &'static str;
    fn authority(&self) -> Authority;

    /// Cheap, side-effect-free check: can this tier plausibly handle `tool`?
    /// Must obey §6 (execution safety). Result is cached per run.
    fn detect(&self, tool: &ResolvedTool) -> bool;

    /// Extract exactly one level: the node at `path`, its flags, its
    /// positionals, and the *names* of its direct subcommands. Do not recurse.
    fn extract_node(&self, tool: &ResolvedTool, path: &[String]) -> Result<CommandNode>;

    /// False when the source is already fully in memory (e.g. carapace), in
    /// which case the runner requests the whole tree in one call and there is
    /// nothing to defer.
    fn is_incremental(&self) -> bool { true }
}
```

The runner:

1. Renders immediately, from a **stub root carrying only the tool's name**. The
   TUI does no extraction before its first frame — resolving the name on `PATH`
   is a filesystem lookup with no spawn. (Revision 3 extracted the root
   synchronously here; that cost ~1.1s for `gh` and ~0.7s for `docker` before
   anything was drawn.)
2. Queues the root for a background fill, then **cascades**: every completed
   fill queues the children it just discovered, walking the whole reachable tree
   on a bounded pool, cancelled on quit and capped at 4096 nodes.
3. On expand, a node not yet filled is queued at the front of that same
   mechanism; nodes still in flight render as `⋯ loading` rows.

**Warming covers the whole tree, not one level ahead.** Revision 3 warmed only
one depth past whatever the user had expanded. That kept the spawn count minimal
but had two costs that outweighed it: an unexpanded node is **invisible to
search** (the index can only hold what has been extracted), and a node that
renders empty with nothing explaining that it needs a keypress reads as a bug
rather than as laziness. Filling everything in the background is the same total
work spread over idle time, and it is what makes a search over the whole tree
honest.

This is **not** a return to §5.1's eager extraction: nothing blocks startup or a
keystroke, and the pool is bounded. The distinction that matters is not *how
much* gets extracted but *what the user waits for* — and the answer is nothing.

**Background fills never expand the node they fill.** Expansion is user intent.
When every node is warmed, auto-expanding on arrival unfolds the entire tree and
buries the user in rows they never asked for.

**Pool sizing is deliberately oversubscribed** — `available_parallelism * 4`,
clamped to `[4, 32]`. A warming job spawns a child process and then spends
nearly all its wall time blocked on it, so one thread per core leaves the machine
idle waiting on I/O.

Non-incremental sources (carapace) return their full subtree at step 1; they cost
nothing, so there is no reason to defer them.

### 5.3 Partial failure is normal

A tier that fails on one node must not invalidate the tier. The runner records
per-node, per-tier status and keeps whatever merged. `TierStatus` is surfaced in
the `?` overlay and in `mandible --doctor <tool>`, so "why is this flag missing"
is answerable without a debugger.

The runner errors only when *no* tier produced a root node.

---

## 6. Execution safety policy

mandible runs other people's binaries. This is the part of the design that can
damage a user's machine, and it gets its own section and its own tests.

**Rules, binding on every tier:**

1. **Never invoke a bare binary.** Revision 1's clap probe ran `<tool>` with no
   arguments to see whether it honored `COMPLETE=`. Running an arbitrary binary
   bare is how you launch a REPL, block on stdin, start a daemon, or trigger a
   tool whose no-arg default is an action. The clap probe must use the protocol's
   argument form (`<tool> --`), never the bare form.
2. **Only inert argv shapes.** A tier may invoke a tool only as:
   `__complete <words...>`, `completion <shell>`, `--help`, `-h`, `help
   [<words...>]`, or `-- <partial>` under `COMPLETE=`. Any other shape requires a
   spec amendment.
3. **stdin is always `/dev/null`.** No tier may ever inherit or pipe stdin.
4. **Hard wall-clock cap**, 2 s for `detect`, 10 s for `extract_node`. On expiry
   kill the **process group**, not just the child — completion scripts spawn
   helpers, and killing only the direct child leaks them.
5. **Bounded output.** Read at most 8 MiB of stdout+stderr per invocation; a tool
   that streams forever must not exhaust memory. Reader threads (or a poll loop)
   are mandatory to avoid pipe deadlock on large output.
6. **Sanitized environment.** Clear `LESS`, `PAGER`, `MANPAGER`, `GIT_PAGER`;
   set `TERM=dumb`, `NO_COLOR=1`, `COLUMNS=100`, `LC_ALL=C.UTF-8`. Without this,
   a tool may page its own help and hang forever, or emit ANSI into the IR.
7. **Never write.** No tier may pass an argument that could name a file the tool
   would create or modify.
8. **Redirect every writable location a probe might reach.** Rule 7 is not
   sufficient, because some tools write *unprompted* on `--help`. Measured: a
   coverage run over `PATH` caused font-cache builders to write into mandible's
   working directory, and `mysql_secure_installation` to write a `.my.cnf`
   containing an empty root password [M-11]. Every probe therefore runs with
   `CWD`, `HOME`, `TMPDIR`, `XDG_*`, and `XDG_RUNTIME_DIR` pointed at a
   per-invocation scratch directory that is deleted afterwards. This is a
   general policy — never a per-tool exclusion list, which would violate §1.
   Full containment needs OS-level sandboxing (namespaces/seccomp); until then,
   document the residual risk rather than claiming the probe is inert.

   **One deliberate exception: toolchain-resolution variables.** Redirecting
   `HOME` breaks every version-manager shim, because they resolve the program
   they stand in for *through* it — `mandible cargo` reported "rustup could not
   choose a version of cargo to run" instead of cargo's help, and the same
   applied to pyenv, nvm, rbenv, asdf, sdkman and volta. A whole class of
   developer tooling was unusable, which is a poor trade for a containment
   boundary that these variables do not weaken much: each names a *toolchain*
   directory, not the user's home, so a misbehaving probe has a far narrower
   blast radius than `$HOME`.

   So `RUSTUP_HOME`, `CARGO_HOME`, `PYENV_ROOT`, `NVM_DIR`, `RBENV_ROOT`,
   `ASDF_DIR`, `SDKMAN_DIR` and `VOLTA_HOME` are passed through, while `HOME`
   itself stays redirected. Passing them through is not enough on its own:
   almost nobody sets them by hand, so the manager falls back to a documented
   path under the real `$HOME` — exactly what the sandbox replaces. The default
   is therefore materialised from the real home before the redirect, and only
   when that directory exists.

   This is a **closed list of ecosystems, not of tools**, which is what keeps it
   on the right side of §1: the knowledge is "how version managers locate
   toolchains", not "how cargo works". Adding an ecosystem is one entry; it
   never grows per tool.

A test asserts rules 1, 2, and 3 by running the full pipeline against a shim
binary that logs its argv and environment, and failing on any invocation outside
the allowlist.

---

## 7. Extraction tiers, in detail

Tiers are listed by the order they are *attempted*, which is now purely a cost
ordering — cheapest first. **Conflict resolution is by `Authority` (§4.4), not by
attempt order.** These are two different things and conflating them was
revision 1's central error.

### Tier A — REMOVED (was: vendored spec catalog)

Revision 2 ranked a vendored 739-tool carapace-spec snapshot first. **Revision 3
deletes it**, along with the vendoring script, the 11 MB payload, and the
third-party data attribution it carried.

The reasoning, recorded so it is not re-proposed:

- **It violated §1.** A per-tool catalog is per-tool knowledge — the thing this
  project forbids — merely relocated from code into data. That it was somebody
  else's data did not make it not-per-tool.
- **It could not be current.** A snapshot is a point-in-time copy; the tool on
  the user's machine is not.
- **It cost more than it bought.** 11 MB of a 16 MB binary — the data outweighed
  all the code 4× — to raise flag-description coverage from a measured 87% (live
  parsing, 904 tools) to 99.5% on the 251 tools it happened to contain [M-12].

The replacement is not "lose those descriptions." It is **parse by the framework
that generated the help text**, below.

### Tier A′ — framework identification

The load-bearing insight of revision 3: **help text is not written by hand, it is
*generated*, and only a small closed set of generators exists.** Per-tool
knowledge is unbounded and forbidden; per-*framework* knowledge is bounded at
~15 entries and is the correct unit of parsing. A grammar fix for argparse
improves every Python CLI ever written; a catalog entry improved exactly one tool
until it went stale.

Measured on 1,563 executables with usable `--help`: three fingerprints cover 71%,
about a dozen cover ~80%, even with deliberately crude patterns [M-12].

**What was implemented instead, and why the numbers disagree.** [M-12] measured
*recall* — how much of a real machine crude patterns could plausibly reach. The
implementation went the other way and uses narrow, high-precision markers
(`clap_builder` in the binary, `spf13/cobra`, the literal GNU argp footer), which
identify **~17%** of a real machine's tools rather than 71%.

That gap is deliberate, not a shortfall. A *wrong* framework silently applies the
wrong grammar, and the tool has no way to tell you it did; an unidentified one
falls back to the general engine and is honestly marked low-confidence. Given
those two failure modes, precision is worth far more than recall.

It also cannot be closed by fingerprinting alone. The unidentified bulk is C
tools — LLVM, binutils, util-linux, iptables, gpg — and most do expose
`getopt_long` in their dynamic symbol table, so they *are* detectable. But a
`getopt_long` profile would be the general engine under another name: it would
parse nothing better, while lifting those tools out of the "unidentified"
confidence cap and raising both the detection rate and their confidence scores
for free. That is precisely the failure §13.1 warns about — a metric improved by
the thing it exists to detect. **Widening a fingerprint is only worth doing
alongside a grammar that earns it**, never to move the number.

Detection rate is therefore not a target. Coverage is: unidentified tools still
parse, and aggregate `%described` sits around 96%.

Identify the framework in this order, most reliable first:

1. **From the artifact.** For compiled binaries, scan embedded strings —
   `spf13/cobra` appears 583× in `docker` and 283× in `gh`, unambiguously [M-13].
   For scripts, read the shebang plus the import line (`import argparse`,
   `require('commander')`, `use clap`). This is ground truth, not inference.
2. **From the help-text signature.** Distinctive marker strings — argparse's
   `show this help message and exit`, click's `Show this message and exit.`,
   cobra's `Available Commands:`, GNU argp's `Mandatory arguments to long options`.
3. **Unidentified** — fall through to the generic layout parser.

Signature matching alone is fragile and must never be the only method: it missed
`docker` entirely, because docker prints `Common Commands:` rather than cobra's
usual `Available Commands:` [M-13]. That failure is exactly why artifact
fingerprinting leads.

`--doctor` reports the detected framework. This converts "mandible is wrong about
tool X" into "the argparse grammar mishandles Y" — a general, fixable bug report
instead of a per-tool complaint.

### Tier B — `--help` parsing, per framework

**The primary tier.** `--help` is the only source every tool has, everywhere, and
it is always current because it comes from the installed binary.

Parsing is dispatched on the framework identified in Tier A′. There is one
grammar per framework, not one grammar for everything:

| | frameworks |
|---|---|
| Python | argparse, click, docopt |
| Rust | clap v2, clap v3/v4 |
| Go | cobra, urfave/cli, stdlib `flag` |
| Node | commander, yargs, oclif |
| JVM / .NET | picocli, System.CommandLine |
| C / POSIX | GNU argp & `getopt_long`, BSD-terse, busybox |
| PHP / Ruby | Symfony Console, OptionParser / Thor |

Each grammar knows its framework's exact section headings, row layout, value
syntax, and continuation rules — so it parses precisely rather than guessing.
This is the "one well-engineered parser, built once" principle applied at the
right granularity: **once per generator, not once per tool, and not once for the
whole world.**

**Degradation is staged, and never fabricates:**

1. Framework identified → its grammar, high confidence.
2. Unidentified → the generic layout parser below, marked low-confidence.
3. Generic parse yields nothing structurally plausible → **render the raw help
   text verbatim**, labelled `unparsed`, with the framework shown as unknown.

Step 3 is a feature, not a failure. A tool that conforms to no convention is
displaying its help the way its author intended; showing that text untouched is
honest and useful. It is also strictly better than the alternative already
shipped and fixed once here: inventing 39 subcommands for `tar` out of wrapped
description lines [M-10]. **Never fabricate structure. Degrade to verbatim.**

The generic fallback parser (step 2) is built with `winnow`:

- **A `Usage:` line grammar.** Usage lines have a learnable grammar — this is
  what `docopt` formalized: `[OPTIONS]`, `<required>`, `[optional]`, `...` for
  repetition, `|` for alternatives, `{a|b|c}` for choices.
- **A layout-driven section parser** for `Options:`/`Flags:`/`Commands:` blocks.
  Group lines by leading-whitespace runs and indentation depth, so
  `-v, --verbose    Enable verbose output` tokenizes structurally as
  (short, long, description) regardless of the exact spacing — which varies
  between tools but is consistent *within* a tool. Detect the description column
  once per block and apply it to the block.
- **Preserve section headings as `Flag::group`.** `tar --help` groups 171 flags
  under headings like "Main operation mode"; `git --help` groups commands under
  "work on the current change". Discarding that grouping is the difference
  between a scannable pane and an undifferentiated wall.
- **Never invent subcommands.** This tier shipped a bug where wrapped
  description continuation lines and enum value lists were parsed as commands:
  `tar` gained 39 phantom subcommands named *"treat them as errors"* and
  *"extracting (default)"*, `dd` 40, `less` 65 [M-10]. Fabricated structure is
  strictly worse than missing structure — a user cannot tell it is wrong. Four
  binding rules:
  1. A command block **must** be introduced by a recognized heading
     (`Commands:`, `Subcommands:`, `Available Commands:`, `SUBCOMMANDS`, or a
     git-style group heading). Layout alone is never sufficient evidence. `tar
     --help` has no such heading, so the correct answer is **zero subcommands**.
  2. A line sitting at the description column with nothing at the name column is
     a **continuation** of the previous row, never a new row.
  3. A candidate command name must look like one: `^[a-z][a-z0-9_.-]*$`, no
     whitespace. *"treat them as errors"* fails; `commit` passes.
  4. An indented list nested under a flag is that flag's **`choices`**, not
     subcommands — `gnu`/`oldgnu`/`pax`/`posix` under `tar --format=` are enum
     values, and the IR already has a field for them.
- **Confidence must fall when the grammar is guessing.** The same bug was
  reported as `ok` at `100% described`, because invented nodes inflate the
  metric rather than depressing it. Any block that yields names failing rule 3,
  or a node with no flags, no children, and a non-identifier name, must reduce
  confidence and mark the tool `suspicious` in the coverage scoreboard (§13.1).
- **Recurse for subcommands.** Revision 1 parsed only the root, which for `git`
  yields subcommand names and zero subcommand flags. Recursion is per-node under
  §5.2 laziness: `<tool> <sub> --help` runs when that node is expanded.
- **Read stdout *and* stderr, and do not require exit 0.** Measured: `openssl
  --help` writes 0 bytes to stdout and 2,908 to stderr; `ip --help` exits 255
  with output only on stderr [M-8]. Both are exactly the "older Unix utility"
  this tier exists for. Prefer stdout when both are non-empty.
- **Attach `confidence: f32`** derived from how much of the output the grammar
  actually consumed, and surface it. Being honest about a best guess is better UX
  than presenting heuristic output with man-page confidence.

### Tier C — completion script structural parsing

For tools not in a catalog that support `<tool> completion bash|zsh|fish`
(clap, cobra, click, oclif, and many hand-rolled CLIs): generate the script, then
**parse it with a real shell grammar, not regex.** Parsing never executes it,
which is the safety property that matters when processing untrusted output.

- **Crate choice: `brush-parser`.** Revision 1 selected `conch-parser`, which is
  unmaintained and today emits *"contains code that will be rejected by a future
  version of Rust"* on build [M-9]. Maintenance, not licensing, is the risk here.
  Avoid `yash-syntax` (GPLv3 — statically linking it would oblige the whole
  binary under GPL).
- **Prioritize zsh `_arguments` over bash.** `_arguments` blocks carry
  `'-v[enable verbose output]'` — spelling *and* description in one structure.
  Bash completion functions carry only spellings, and typically compute candidates
  at runtime (`$(git ls-files)`, `_get_comp_words_by_ref`), so static parsing
  recovers substantially less than revision 1 implied. Request zsh first, bash as
  fallback.
- Walk `complete -F`/`compgen -W` registrations and `case "$prev" in` branches as
  typed AST nodes.

### Tier D — man page structural extraction

Two sub-cases of very different quality:

- **`mdoc(7)` pages** use *semantic* macros — `.Fl` for a flag, `.Ar` for an
  argument, `.Nm`/`.Cm` for command names — so the AST genuinely distinguishes
  "this is a flag" from "this is prose." Real structure, not inference.
- **`man(7)` pages** are typeset prose with weak semantic tagging. Section
  boundaries (`NAME`, `SYNOPSIS`, `OPTIONS`, `EXAMPLES`) extract reliably;
  individual flag/description pairs need the same heuristics as Tier B.

**Do not** regex the rendered output of `man <tool>`, and do not parse `mandoc -T
tree` — the OpenBSD manual documents that format as unstable and explicitly says
not to write parsers against it. There is no `-T json`. Use `libmandoc`
(`mparse_alloc` → `mparse_readfd` → `mparse_result` → walk `mdoc_node()`/
`man_node()`) via `bindgen`.

Two corrections to revision 1:

- **`libmandoc` is not a shipped library on Linux** — no `.so` or headers in a
  default install; `mandoc` exists only as a source package [M-6]. Using this tier
  on the platform where most man pages live means **vendoring mandoc's source and
  building it with `cc`** (ISC-licensed, feasible). That makes this the *most*
  build-complex tier, not merely "a real build-complexity cost." It is off by
  default behind a `manpage` feature.
- **Multi-page tools are unaddressed and matter most.** `git`'s structure is not
  in `git.1`; it is spread across `git-commit.1`, `git-rebase.1`, and so on.
  Extracting a tree requires discovering sibling pages via `MANPATH`/`man -k` and
  the `<tool>-<sub>.N` convention. This is the highest-value un-specced source of
  prose for classic Unix tools and belongs in this tier's design.

Position this tier as a **prose backfill** (structural 60 / prose 180, §4.4), not
as a structure source. It is the exact complement of Tier E.

### Tier E — native, self-describing binaries

Highest structural authority, lowest cost-efficiency. Attempted last for *cost*
reasons — it is the only tier that spawns a process per node — but it wins
structural conflicts (§4.4) because it reflects the version actually installed.

- **cobra `__complete`** (kubectl, docker, gh, helm, and much of modern infra):
  **the protocol requires two probes per node.** `__complete <path> ""` returns
  subcommands only; flags require `__complete <path> "-"` [M-2]. Revision 1
  documented only the first, and an implementation following it produced zero
  flags. Parse the trailing `:N` directive line; candidate lines are
  `value\tdescription`, with a `=` suffix marking value-taking flags.
  - **Depth cap** (default 6) and a **visited set** keyed by the candidate list's
    hash: some tools echo root completions for unrecognized paths, which
    recurses forever.
  - **Alias detection**: `gh co` is "Alias for \"pr checkout\"" — recursing it
    duplicates an entire subtree. Detect the `Alias for` convention and the case
    where a child's candidate set equals a sibling's, and record it in
    `CommandNode::aliases` instead of recursing.
- **clap `CompleteEnv`** (`COMPLETE=<shell> <tool> -- <partial>`): keep the tier,
  but **do not build a roadmap milestone on it.** Measured: `ripgrep` errors and
  `cargo` prints ordinary help — neither supports it [M-4]. It is opt-in and rare.
  Probe with `<tool> --` under `COMPLETE=zsh` (never bare — §6 rule 1).
- **argcomplete** (Python): the `_ARGCOMPLETE` env-var convention. Same shape,
  lowest priority within this tier.

### Tier F — user override

`~/.config/mandible/overrides/<tool>.toml`, merged with `Authority { 255, 255 }`.
This exists so the rare bad case has a clean exit; the pipeline never depends on
one existing.

**Policy, binding:** overrides are user-local and **never vendored into this
repository**. This single rule is what actually enforces the §1 invariant —
without it, the first hard tool gets an override committed to git and the
per-tool patch pile begins.

---

## 8. Crate & workspace architecture

```
mandible/                          (workspace root)
├── mandible-core/                 # IR, Text sanitization, Provenance, Authority, merge, NodeRef
├── mandible-extract/              # the tiered pipeline + runner
│   ├── known_specs/             # Tier A: carapace snapshot + index
│   ├── help_text/               # Tier B: winnow grammar
│   ├── completion_script/       # Tier C: brush-parser AST walking
│   ├── manpage/                 # Tier D: libmandoc FFI  [feature = "manpage"]
│   ├── native/                  # Tier E: cobra, clap, argcomplete probes
│   ├── overrides/               # Tier F
│   └── exec/                    # §6 policy: the ONLY place std::process is used
├── mandible-cache/                # on-disk cache, keying, invalidation
├── mandible-search/               # nucleo index over commands AND flags
├── mandible-tui/                  # ratatui UI: tree, detail, search, overlay
├── mandible/                      # the `mandible` binary
└── xtask/                       # coverage harness, spec vendoring, packaging
```

**`mandible-extract/src/exec/` is the only module permitted to use
`std::process`.** A `#![deny]`-style test greps the workspace for `Command::new`
outside that module and fails the build otherwise. Centralizing this is what makes
§6 auditable rather than aspirational.

Per-tier modules sit behind feature flags so Tier D's `libmandoc` dependency —
which needs a C toolchain and only makes sense on Unix — is not a hard
requirement for a Windows user who wants Tiers A/B/C/E.

**Default features:** `known-specs`, `help-text`, `completion-script`, `native`.
**Optional:** `manpage` (C toolchain), `withfig`.

---

## 9. TUI design

`ratatui` with the `crossterm` backend. Mouse support comes free, so
click-to-expand is a real affordance rather than a keyboard-only one.

**Widgets are permitted to assume text is clean** — see §4.1. All sanitization
happens at the IR boundary. This is a hard layering rule, and the reason is
empirical: the previous implementation hit border corruption while scrolling,
where description text overwrote the pane's `│` border, and two attempts to fix
it inside the tree widget failed and were reverted. The cause is untrusted text
containing newlines, tabs, ANSI, or backspace-overstrike reaching a `Span`; a
widget-level fix can only ever patch one of the three consumers.

Belt-and-braces: the tree row builder still truncates to the pane's inner width
using **display width** (`unicode-width`), not byte or `char` count, because CJK
and emoji in descriptions are double-width and a `char`-count truncation
overflows the border by one cell per wide character.

**Rendering rules.**

- Tree rows are built at fixed column offsets: `[indent 2·depth][chevron 1][space
  1][name][space][summary dim]`. Fixed offsets make mouse hit-testing arithmetic
  rather than guesswork: chevron is hit when `col == 2·depth`.
- The flattened row list is **cached** and invalidated on expand/collapse, search
  change, or lazy fill. Rebuilding it per keypress (three times per event, as in
  the prior implementation) is wasted allocation that grows with tree size.
- The detail pane renders flags grouped by `Flag::group`, with inherited flags in
  a final dimmed "Inherited" group, and hidden/deprecated flags suppressed unless
  toggled with `.`.
- Scroll state is per-pane; the wheel scrolls the pane under the cursor.

**Empty and degraded states are designed, not incidental:** a node whose children
are still being extracted shows a subtle spinner row; a tool where only Tier B
fired shows the confidence in the footer; a tool no tier resolved shows the
per-tier status list with a suggestion to try `--doctor`.

### 9.1 Tree rows: one node, one row

**No wrapping in the tree pane, ever.** Row index ↔ node stays a bijection, which
keeps selection, scrolling, mouse hit-testing, and filtering arithmetic instead
of bookkeeping. Truncation costs nothing here because the detail pane shows the
full text on selection; a tree summary only has to disambiguate `push` from
`http-push`, which ~30 characters does.

```
╭ git ───────────────────────────────────────────╮
│▾ git             the stupid content tracker    │
│    add           Add file contents to the ind… │
│  ▸ bisect        Use binary search to find th… │
│  ▾ stash         Stash the changes in a dirty… │
│      push        save your local modification… │
╰────────────────────────────────────────────────╯
```

- **Summaries align to a computed column**, not `name + space`. The column is
  `min(longest indent+name over the whole flattened row set, 40% of pane width)`.
  Compute over *all* rows, never the viewport — a viewport-derived column jumps
  as you scroll, which is worse than no alignment. It is stable until expand or
  collapse.
- **Truncate at a word boundary with `…`.** The ellipsis is a real signal that
  the detail pane has more; a mid-word cut just looks broken.
- **The name column never yields to the summary.** A long name truncates the
  summary to nothing before truncating itself — you can navigate without
  summaries, never without names.
- Width ladder: full layout above 60 columns; **names only** below it (drop
  summaries rather than showing eight useless characters); stacked panes below 50.

### 9.2 The styling contract

One accent, spent only on information. Everything else is neutral.

| Element | Style |
|---|---|
| Node name | Default foreground |
| Selected row | Accent + reversed |
| Tree summary | Muted |
| Focused pane border | Accent; unfocused muted |
| Breadcrumb | Ancestors muted, leaf bold |
| Section heading (`DESCRIPTION`, `FLAGS`) | Bold muted |
| **Flag spelling** | **Accent** — the payload the user came for |
| Value placeholder (`<FILE>`) | Muted italic |
| Flag description | Default foreground |
| Inherited flag group | Entire group muted |
| Deprecated | Muted + a `(deprecated)` tag |
| Search match characters | Underline, within the name only |
| Provenance footer | Muted |
| Low confidence | Warning color — the **one** sanctioned exception to single-accent |

Four implementation rules that matter more than the palette:

- **ANSI indexed colors, not RGB.** Indexed colors resolve through the user's own
  terminal theme, so mandible looks native in Solarized, Gruvbox, or a light
  terminal with no detection logic. Hardcoded RGB looks wrong in half of them.
  The accent stays configurable.
- **Prefer `DarkGray` over `Modifier::DIM` for muted text.** Several terminals
  ignore `DIM` outright and others render it nearly invisible — a portability
  trap that only manifests on someone else's machine.
- **Respect `NO_COLOR` and `TERM=dumb`**, degrading to bold/reverse/underline
  only. A colour-depth ladder (truecolor → 256 → 16) is deliberately **not**
  implemented: it would require choosing specific RGB values, which the first
  rule above rules out. Named ANSI colours already work at every depth that has
  colour at all.
- **Highlight search matches.** `nucleo` returns match indices for free;
  underlining matched characters is the difference between "the list changed"
  and "here is why this matched."

#### What may be drawn, and what may not

The rule: **a glyph may only be used if there is something legible to fall back
to.** This is what separates the techniques mandible uses from the ones it
refuses, and the distinction is not aesthetic — it is about how each *fails*.

| Technique | Fails on | Failure mode |
|---|---|---|
| Box-drawing, block elements | non-UTF-8 locale, bare Linux console | falls back to `+-\|` |
| Colour (named ANSI) | `NO_COLOR`, `TERM=dumb`, no TERM | falls back to bold/reverse |
| Bold, reverse, underline | almost nothing | — |
| **Italic, `DIM`** | **many terminals silently ignore them** | **must never be the *sole* distinction between two kinds of text** |
| Sixel / Kitty graphics | most terminals, most tmux, many SSH sessions | raw bytes on screen |
| Nerd Font icons | any machine without the patched font | `□`, meaning nothing |

Two properties decide it:

1. **Detectability.** `NO_COLOR`, `TERM` and the locale can be inspected. A
   terminal can be asked about its colour depth; it can never be asked what font
   it is using. That alone rules Nerd Fonts out permanently — Sixel is at least
   probe-able.
2. **How it degrades.** Losing colour loses emphasis; the text remains. Losing
   the font loses the meaning and leaves a box.

This matters more for mandible than for most TUIs because of *where* it gets
used: SSH'd into an unfamiliar machine, or inside a minimal container with
`LANG` unset, trying to work out a CLI you do not know. Polish that evaporates
exactly where the tool is most needed is not polish.

Implemented in `mandible-tui/src/glyphs.rs`: two glyph sets chosen at startup
from `LC_ALL`/`LC_CTYPE`/`LANG`, with `MANDIBLE_ASCII=1` as an override for a
terminal that claims UTF-8 and renders it badly anyway. Enforced by a test that
renders a full frame over ASCII-only content and asserts no cell contains a
non-ASCII symbol — content from the tool itself is exempt, since reproducing a
tool's own text exactly matters more than any of this.

Markup handling is staged: Tier A prose is flattened to plain text today
(`Text::sanitize_markdown`). The better end state keeps parsed spans in the IR so
inline code and link labels can be *styled* rather than stripped — git's prose is
dense with both. Do this only once the plain-text path is stable.

---

## 10. Search

**`nucleo`** — the matcher behind Helix. Faster than `fuzzy-matcher`/`skim`,
correct on Unicode graphemes, and designed to match on a background thread pool
so typing never blocks.

**Index entries are `NodeRef`s, and flags get their own entries.** Revision 1
folded flag names into the parent command's haystack, so searching `--squash`
selected `git rebase` rather than the flag. Since finding a flag is the product's
core job (§1), each `Flag` is its own index entry, with a haystack of
`short + long + value_name + description` and a `NodeRef::Flag` payload.
Selecting one selects the parent command and scrolls the detail pane to that flag.

**Two match modes, name-only by default.** Matching one combined haystack
(name + summary + description + flag value) is correct and *looks* arbitrary:
searching `branch` in `git` returns `switch` via "Switch branches", and since
only name matches are underlined, nothing on screen explains why that row is
there. `/` opens the box in **name mode** — command names and flag spellings
only — and pressing `/` again toggles **wide mode**, the combined haystack. The
search bar's title shows which is active. Name mode is the default because its
results explain themselves; wide mode finds more and is one keystroke away.
Name mode filters the index's own result set rather than maintaining a second
index, using a subsequence test so it can never reject something the fuzzy
ranking accepted for the same reason (`gco` → `checkout` still works).

**Filtering preserves hierarchy.** A flat result list rendered with
`depth = path.len() - 1` produces indentation pointing at ancestors that aren't
on screen. Instead, matching a node force-expands its ancestor chain and the tree
renders normally with non-matching siblings hidden. This is also what makes the
spec's intent — pin the filter, then navigate the narrowed tree — actually
achievable; with a flat list, expand/collapse keys mutate state nothing reads.

**Threading.** Drive `Nucleo::tick` from the event loop's poll timeout, not from a
blocking spin inside the keystroke handler. A 50 ms synchronous deadline per
keystroke on the UI thread defeats the reason nucleo was chosen.

**Ranking.** Boost exact prefix matches on names above description matches, so
typing `reb` puts `rebase` above every command whose description contains
"rebase".

---

## 11. No cache

**There is no on-disk extraction cache.** Revision 2 specified one, keyed on
binary identity plus a build-time source fingerprint. Revision 3 removes it.

**Why it cannot be made correct.** A cache key can only observe the things it
hashes. Help output routinely changes while every hashed input stays identical:

- `docker` gains subcommands when a plugin is installed — the docker binary is
  untouched.
- `git` gains subcommands from any `git-*` on `PATH`, and from aliases in
  `~/.gitconfig`.
- `kubectl` behaves the same way with its plugins.

No fingerprint over the binary catches any of these. A cache that is *usually*
fresh is a cache that will be confidently wrong at some point, and this project
already shipped one staleness bug whose only symptom was a correct fix appearing
not to work.

**Why removing it is affordable.** Lazy node-at-a-time extraction (§5.2) means a
launch only ever extracts the root: 179 ms for `git`, 221 ms for `docker`,
against the 10.5 s that eager whole-tree extraction cost [M-3]. That is well
inside the budget for a TUI a human then reads for seconds.

**If it is ever reintroduced**, the only acceptable design is
revalidate-rather-than-guess: store a hash of the tool's root help output, and
re-probe that single command on open (one subprocess, ~40 ms) to decide whether
the cached tree is still valid. Guessing from file metadata is not acceptable.

---

## 12. Implementation roadmap

Reordered from revision 1 by measured payoff. Tier A is 740 tools and 48k
descriptions for zero subprocesses [M-1]; it is the fastest path to a product
that is actually useful, and it still exercises the merge against Tier B.

| Phase | Scope | Exit criteria |
|---|---|---|
| **0 — foundation** | Workspace; `mandible-core` (IR, `Text::sanitize`, `Provenance`, `Authority`, merge, `NodeRef`); `mandible-extract/exec` (§6); cache crate; packaging skeleton (LICENSE, NOTICE, README, CI) | `cargo test` green; the exec-policy test passes; the "no `Command::new` outside `exec/`" test passes |
| **1 — Tier A + TUI** | carapace snapshot + indexed loader; full tree/detail/status TUI; caching | `mandible git`, `mandible docker`, `mandible gh` render full trees with real descriptions in **< 200 ms** from cache and **< 1 s** cold |
| **2 — Tier B** | `winnow` help-text grammar, recursive per-node, stdout+stderr, groups, confidence | `mandible curl`, `mandible tar`, `mandible openssl`, `mandible ip` all produce useful trees; coverage harness reports its first scoreboard |
| **3 — lazy + search** | Node-at-a-time runner, background warm, `nucleo` index over commands **and** flags, hierarchy-preserving filter | `mandible kubectl` interactive in < 1 s; typing `--squash` selects the flag, not the command |
| **4 — Tier E + C** | cobra two-probe protocol with depth cap/visited set/alias detection; clap `CompleteEnv`; zsh `_arguments` then bash | A cobra tool absent from the catalog renders correctly; a `completion`-only tool renders correctly |
| **5 — Tier D + F** | libmandoc FFI (vendored, feature-gated), multi-page discovery; user overrides | A BSD-lineage `mdoc` tool and a Linux `man(7)` tool both extract; `git` gains prose from `git-*.1` |
| **6 — distribution** | crates.io release, `cargo-deb`/`cargo-generate-rpm`, man page for mandible itself, shell completions | `cargo install mandible` works; `.deb` and `.rpm` install cleanly |

Deliberately **not** on the roadmap: local NL search (§17).

---

## 13. Testing & the coverage harness

### 13.1 The coverage harness

This is the most important testing artifact in the project and revision 1 lacked
it. `cargo xtask coverage` runs extraction across every executable on `PATH` and
emits a scoreboard:

```
tool        tier(s)              nodes  flags  %described  ms     status
docker      carapace+help          162    836        100%   180    ok
curl        help                     1    241         96%    90    ok
openssl     help                     1    112         71%   140    ok  (stderr)
somecli     help                     3     12         33%    60    low-confidence
weirdtool   —                        0      0          —    240    no-tier
```

The scoreboard is **checked into the repo** and diffed on every parser change.

This is what makes "universal, no per-tool adjustment" **measurable** rather than
aspirational. Without it, every grammar tweak is evaluated against the one tool
you happened to be looking at, and there is no way to see that fixing `tar`
regressed `xz`. It is also the signal for when a tier has stopped earning its
complexity.

Regression gate: `%described` aggregate and `no-tier` count may not worsen.

**`%described` alone is not a quality signal, and trusting it hid a real bug.**
The Tier B phantom-subcommand defect [M-10] reported `tar` as `ok` at `100%
described` while 39 of its 40 nodes were fabricated — invented nodes *inflate*
the metric. The scoreboard therefore also carries a **structure-sanity** column:
count of nodes whose name fails `^[a-z][a-z0-9_.-]*$`, and count of nodes with
no flags, no children, and no summary. Any tool with a non-zero count is marked
`suspicious`, and `suspicious` is a gated metric exactly like `no-tier`.

The general lesson, worth stating because it will recur: **a coverage metric that
can be gamed by the failure mode it is meant to detect is worse than no metric**,
because it converts a silent bug into a confidently-reported success.

### 13.1a The framework-support workflow

A GitHub Actions workflow reports, on every run, which frameworks mandible
supports and how well — rendered into the run's summary page via
`$GITHUB_STEP_SUMMARY`, which accepts markdown.

Two jobs, neither of which needs a long download or a multi-hour run:

1. **Framework matrix.** Install roughly one representative tool per supported
   framework (`ripgrep`→clap, `gh`→cobra, `httpie`→argparse, `black`→click, a
   `commander` package, a picocli jar, coreutils→argp, …) and assert that
   mandible (a) identifies the expected framework and (b) extracts a
   non-trivial tree. ~2–4 minutes with apt/pip/npm.
2. **PATH sweep.** `ubuntu-latest` already ships ~1,500 executables. Run the
   coverage harness over the runner's own `PATH` — zero installation cost.

The summary table carries, per framework: tools detected, flags extracted,
% described, and pass/fail. The gate fails on regressions in `no-tier`,
`suspicious`, or framework-detection failures.

This is the natural home for the §13.1 scoreboard once it stops depending on
whatever happens to be installed on a developer's laptop.

### 13.2 Fixed corpus

Golden-file tests snapshot **both** the raw tool output and the resulting
`CommandNode` tree. Snapshotting only the IR means a tool-version bump forces you
to re-derive from scratch; snapshotting the raw output lets you re-run the parser
against yesterday's bytes.

- **Tier A**: `git` (a good stress test — its own completion is hand-written bash,
  so the catalog is doing real work), `docker`, `kubectl`.
- **Tier B only**: `curl`, `tar` (171 flags in named groups), `openssl` (help on
  stderr), `ip` (exit 255), and a deliberately malformed fixture.
- **Tier C**: a tool shipping `completion zsh` but absent from the catalog.
- **Tier D**: one `mdoc` page and one `man(7)` page, to verify the AST-vs-heuristic
  split inside the tier behaves as designed.
- **Tier E**: a recorded cobra transcript (both probe forms), replayed through a
  mock so the test needs no network or installed tool.

### 13.3 Required test classes

- **Real-argv tests.** Every tier needs at least one test that exercises the
  *actual* argv construction, not just the parser behind it. A prior cobra
  implementation omitted the literal `__complete` from its extract-path argv and
  the tier was silently dead in the real pipeline; its unit tests passed because
  they injected a mock probe that bypassed argv construction entirely.
- **Execution-policy tests** (§6): a shim binary logs argv/env; any invocation
  outside the allowlist fails the suite.
- **Sanitization tests**: ANSI, C0, backspace-overstrike, tabs, embedded newlines,
  CJK/emoji width, and a 10 MB pathological string.
- **Render tests** against `ratatui::backend::TestBackend`, asserting that
  **border cells are intact** for adversarial description text at several widths
  and scroll offsets. This is the regression test for the bug that was reverted
  twice.
- **Fuzzing** the Tier B grammar (`cargo-fuzz`) — it consumes untrusted text.
- **Merge property tests**: merge is associative over authority; a `None` never
  displaces a `Some`; alias pairing is idempotent.

---

## 14. Dependency table

| Purpose | Crate | License | Notes |
|---|---|---|---|
| TUI framework | `ratatui` | MIT | `crossterm` backend; mouse support |
| Display width | `unicode-width` | MIT/Apache-2.0 | Required for correct truncation (§9) |
| Fuzzy matching | `nucleo` | MIT/Apache-2.0 | Powers Helix |
| mandible's own CLI | `clap` + `clap_complete` | MIT/Apache-2.0 | Also dogfoods the Tier E clap protocol |
| Help-text grammar | `winnow` | MIT | Preferred over `pest` for error recovery |
| Completion script AST | `brush-parser` | MIT | **Replaces `conch-parser`**, which is unmaintained and emits a future-incompat rejection warning today [M-9]. Avoid `yash-syntax` (GPLv3). |
| Man page AST | `bindgen` + **vendored** mandoc | MIT / ISC | Not a system library on Linux [M-6] — vendor the source and build with `cc`. Feature-gated. |
| C build | `cc` | MIT/Apache-2.0 | For vendored mandoc only |
| Parallelism | `rayon` | MIT/Apache-2.0 | Bounded pool for background subtree warming |
| Paths | `directories` | MIT/Apache-2.0 | XDG cache/config resolution |
| Serialization | `serde`, `serde_json`, `serde_yaml` | MIT/Apache-2.0 | IR, cache, carapace specs |
| Compression | `flate2` | MIT/Apache-2.0 | Cache entries |
| Errors | `thiserror` (libs), `anyhow` (binary) | MIT/Apache-2.0 | |
| Logging | `tracing` + `tracing-subscriber` | MIT | Behind `MANDIBLE_LOG`; never writes to the TUI's terminal |
| Clipboard | `arboard` | MIT/Apache-2.0 | For `y`; degrade to OSC-52 when unavailable |
| Testing | `insta`, `proptest`, `cargo-fuzz` | MIT/Apache-2.0 | Snapshots, properties, grammar fuzzing |

**Build-time (not shipped):** Python 3 + PyYAML, used by the catalog vendoring
script. Revision 1's table omitted this even though the vendoring step already
depended on it.

**Data dependencies** (revision 1 omitted these entirely while scrutinizing crate
licenses — this is the more likely real exposure):

| Data | Source | Obligation |
|---|---|---|
| carapace specs | `carapace-sh/carapace-bin` | Verify current license text at vendor time; record source commit and date; carry in `NOTICE` |
| withfig specs (optional) | `withfig/autocomplete` | MIT; carry in `NOTICE` |
| mandoc source (optional) | `mandoc.bsd.lv` | ISC; carry in `NOTICE` |

---

## 15. Packaging & distribution

The project should be shippable as an open-source repo, via `cargo install`, and
through `apt`/`dnf`, without rework. That constrains layout from day one.

**Repository layout.**

```
LICENSE-MIT        }
LICENSE-APACHE     } dual-licensed MIT OR Apache-2.0 — the Rust ecosystem standard
NOTICE             Third-party data attribution (§14) — required, not optional
README.md          What it is, install, a screenshot, the honest coverage story
CONTRIBUTING.md    Including the §1 invariant, stated prominently
CHANGELOG.md       Keep-a-changelog format
spec.md            This document
.github/workflows/ ci.yml (fmt, clippy -D warnings, test, coverage-harness diff),
                   release.yml (tagged cross-platform binaries)
xtask/             coverage harness, vendoring, packaging
packaging/         debian/, rpm/, mandible.1 (man page for mandible itself)
```

**Cargo metadata.** Every crate carries `description`, `license`, `repository`,
`readme`, `keywords`, `categories`, and `rust-version` (MSRV, tested in CI).
Internal crates that are not independently useful are published anyway — a
workspace cannot be `cargo install`ed otherwise — so their descriptions must make
the relationship clear.

**Distro packaging constraints, which shape earlier decisions:**

- Vendored data must be reproducible from a script with a recorded source commit;
  distro maintainers will ask where the 11 MB came from.
- Default features must build with no network and no C toolchain. That is why
  Tier D is opt-in.
- Ship completions for mandible itself and `packaging/mandible.1`, installed to the
  standard paths.
- `cargo-deb` and `cargo-generate-rpm` metadata live in `mandible/Cargo.toml`.
- Respect `$XDG_CACHE_HOME`/`$XDG_CONFIG_HOME`; never write outside them.

---

## 16. Open risks & honest caveats

1. **Cold-start cost is the top UX risk.** 10–25 s for cobra-heavy tools if
   extraction is eager [M-3]. Mitigated by lazy per-node extraction (§5.2) and
   caching (§11) — both of which must exist early, not in a polish phase.
2. **Running other people's binaries can damage a machine.** Mitigated by §6, and
   §6 is only real because `exec/` is the sole module allowed to spawn processes
   and a test enforces it.
3. **Description coverage is the actual product value, and only Tiers A and D
   supply it well.** A tool absent from the catalog with no man page — which is
   *every internal company CLI*, precisely the case "universal" is for — renders
   as names with sparse prose. This is the honest limit of the design and the UI
   must show it rather than hide it.
4. **Vendored catalog staleness and weight.** 11 MB, a point-in-time snapshot,
   with no automatic refresh. Mitigated by preferring a live `carapace` binary,
   showing the vendoring date, and an `xtask` refresh path.
5. **Tier B/C fragility across tool updates.** A `--help` layout can change
   between minor versions and silently degrade extraction. The confidence score
   and provenance footer exist so this fails *visibly*; the coverage harness
   (§13.1) exists so it fails *measurably*.
6. **Rendering untrusted text into a terminal.** Mitigated by §4.1 and by the
   border-integrity render tests (§13.3).
7. **Attribution and licensing of vendored data** (§14) — the most likely genuine
   legal issue, and the easiest to fix now.
8. **Windows is weakest for Tiers C and D.** Tier A (pure data) and Tier B (help
   text, which every tool has everywhere) carry that platform.
9. **Multi-page man discovery is unproven.** It is the highest-value un-built
   piece for classic Unix prose, and it is also the one most likely to need
   per-distro path fiddling — which brushes against the §1 invariant. Keep it
   general (`MANPATH` + `man -k`) or don't ship it.

---

## 17. Investigated and deferred: local NL search

A local tool-calling model (e.g. `cactus-compute/needle`, ~26–30M params, MIT
weights with an MIT ONNX export) was investigated for queries that share no
vocabulary with the target — *"squash my last 3 commits"* will never fuzzy-match
`rebase --interactive`. **The conclusion is to defer it, and the reasoning is
recorded here so it isn't re-litigated.**

**The license finding is worth keeping regardless.** The model weights and the
ONNX export are MIT, but the **Cactus inference engine is a custom, non-OSI
license**: free use is granted only to individuals for personal/educational/
non-commercial use, organizations under *both* $2M funding and $2M revenue,
educational institutions, and 501(c)(3) nonprofits — with the grant terminating
automatically if a qualifying org crosses either threshold. If mandible depended on
that engine, any downstream user past those thresholds would need a commercial
license from Cactus Compute merely for using mandible. **Do not take that
dependency.** If the model is ever revisited, load the MIT ONNX export via `ort`
or `tract` (both MIT/Apache-2.0), or reimplement the small architecture in
`candle`. Avoid the prebuilt `needle-cq4.zip` — it is a proprietary quantization
format tied to Cactus's kernels.

**Why deferred:**

- **The "the CommandNode tree is almost exactly Needle's tool registry" claim does
  not survive the data.** `git` alone is 279 nodes and 2,999 flags [M-1]. A 26M
  model with an 8k BPE vocab cannot take that as a registry. You would need
  retrieval to pre-narrow candidates first — at which point retrieval is doing the
  work and the model is re-ranking, a materially different design.
- **It needs a fine-tune to work on CLI phrasing.** Needle was trained on general
  function-calling data (`get_weather(location)`), not on the terse jargon of
  `git`/`ffmpeg`/`tar`. That is a data-generation project, not an integration.
- **The cheap version probably captures most of the value.** The reason
  "squash my last 3 commits" fails today is that matching runs over *names*.
  Descriptions are already indexed for 2,979 git flags [M-1]; a BM25/tf-idf pass
  over description and example text should be tried first, and the residual
  failure set is what would justify a model.
- **It inverts the product's identity** — a fast, local, instant reference becomes
  a 61 MB ML dependency.

Vendor-published figures in this section (parameter count, distillation source,
throughput) are **unverified vendor claims**, not measurements.

---

## Appendix A — Measured baseline

Measured 2026-08-05 on Ubuntu (Linux 6.17, x86-64). Re-measure before treating
any of these as current.

- **[M-1] Vendored carapace snapshot.** 740 tools; 48,224 flag descriptions total;
  2,814 descriptions longer than 120 chars; 10 containing embedded newlines.
  `git` 279 nodes / 2,999 flags / 2,979 described. `docker` 162/836/836.
  `gh` 249/1,061/1,061. `curl` 273 flags. `tar` 171 flags. `ffmpeg` present but 0 flags.
- **[M-2] cobra probe shape.** `gh __complete ""` → 28 subcommands, **no flags**.
  `gh __complete "-"` → `--help`, `--version`. `gh __complete pr "-"` → `--help`,
  `--repo`, **and `-R` as a separate row with an identical description**.
  Output ends with a `:N` directive line on stdout; a human-readable directive
  note goes to stderr.
- **[M-3] Recursive walk cost.** `docker`: 255 nodes, 232 spawns, 10.5 s.
  `gh`: 196 nodes, 182 spawns, 11.6 s. Both depth-capped at 3, one probe per node.
  ~40–65 ms per spawn.
- **[M-4] clap `CompleteEnv` availability.** `COMPLETE=zsh rg` → *"ripgrep requires
  at least one pattern to execute a search"*. `COMPLETE=zsh cargo` → ordinary help
  text. Neither supports the protocol.
- **[M-5] Man page availability.** Test container: 31 pages in
  `/usr/share/man/man1`, none for `git` or `curl`.
- **[M-6] libmandoc.** No `libmandoc*` in `/usr/lib/x86_64-linux-gnu`; `mandoc`
  available only as an apt source package.
- **[M-7] Pane width.** At 80 columns, a 35% tree pane is 28 cells; minus borders
  and a depth-3 indent, ~20 cells remain for name plus summary.
- **[M-8] `--help` stream and exit code.** `openssl --help`: 0 bytes stdout,
  2,908 bytes stderr, exit 0. `ip --help`: 0 bytes stdout, 972 bytes stderr,
  exit 255. `ffmpeg -h`: 5,365 stdout **and** 1,827 stderr.
- **[M-9] `conch-parser`.** Builds with *"the following packages contain code that
  will be rejected by a future version of Rust: conch-parser v0.1.1"*.
- **[M-10] Tier B phantom subcommands** (2026-08-05, first implementation).
  Tools with no subcommands at all reported: `tar` 39 nodes, `dd` 40, `less` 65,
  `zstdless` 65, `sed` 17, `zoxide` 13, `find` 11, `zramctl` 9. Phantom names
  were wrapped description fragments (*"treat them as errors"*, *"extracting
  (default)"*, *"silently skip over them"*) and `--format=` enum values
  (`gnu`, `oldgnu`, `pax`, `posix`). `tar` and `dd` were reported `ok` at
  `100% described`; only `less`/`zstdless` tripped `low-confidence`.
- **[M-11] Probes that write.** A coverage run invoking `--help` across 1,665
  `PATH` executables caused font-cache builders to write into the working
  directory, and `mysql_secure_installation` to write a `.my.cnf` containing an
  empty root password. `--help` is not reliably a read-only operation.
- **[M-12] Framework distribution** (2026-08-06, 1,634 executables in
  `/usr/bin`, `/bin`, `/usr/sbin`; 1,563 with usable `--help`). Classified by
  deliberately crude fingerprints: generic `Usage:` 31.2%, clap-v4 24.6%,
  GNU argp/getopt 15.5%, argparse 4.1%, picocli 2.0%, BSD-terse 0.6%,
  clap-v2 0.4%, then dotnet/urfave/go-flag/symfony/cobra/oclif/click ~1% total;
  unmatched 20.4%. **Three fingerprints cover 71%; about a dozen cover ~80%**,
  and better-engineered patterns would improve on that. Separately: flag
  descriptions from live `--help` parsing alone reached **87.0%** across the 904
  tools absent from the carapace catalog, versus 99.5% for the 251 tools in it.
- **[M-13] Artifact fingerprinting beats prose fingerprinting.** `strings` over
  the binary: `docker` contains `spf13/cobra` 583×, `gh` 283×; `git` and
  `ripgrep` contain zero (correctly — hand-rolled C, and ripgrep dropped clap
  for a custom parser). Meanwhile a help-text signature keyed on cobra's usual
  `Available Commands:` **missed `docker` entirely**, because docker prints
  `Common Commands:`.

---

## Appendix B — What changed in revision 2

| Area | Revision 1 | Revision 2 | Why |
|---|---|---|---|
| Tier priority | One `priority: u8`; first tier wins | Two-axis `Authority` (structural/prose); attempt order is cost, conflict order is authority | The best-structure tier is usually not the best-prose tier [M-1, M-2] |
| Tier ordering | Tier 0 (native) first | Tier A (catalog) first | Catalog is 740 tools / 48k descriptions for zero spawns; clap `CompleteEnv` is nearly absent [M-1, M-4] |
| Extraction trait | `extract() -> whole tree` | `extract_node(path)` + laziness | Eager extraction is 10–25 s for docker/gh [M-3] |
| Cost model | Absent | §5.1, with measurements | Largest UX risk in the project |
| Execution safety | Implicit; probe ran bare binaries | §6, binding rules + enforcement test; `exec/` is the only module allowed to spawn | Running an arbitrary binary bare is not a cheap operation |
| Provenance | One enum per node | Per-field, with `Source` list | Per-node provenance is inaccurate after a merge, and an inaccurate trust badge is worse than none |
| Text handling | Unspecified | `Text::sanitize` as an IR invariant | Root cause of the border corruption reverted twice |
| Flag identity | `same_flag` on long name | Alias pairing before merge | Sources emit `--repo` and `-R` separately [M-2] |
| IR fields | — | `hidden`, `deprecated`, `choices`, `group`, `inherited`, `children_filled` | Needed for correct display and lazy fill |
| Search | Flags folded into parent haystack | Flags are first-class index entries; hierarchy-preserving filter | "Find the flag" is the core job; flat filtering breaks navigation |
| cobra protocol | One probe | Two probes + depth cap + visited set + alias detection | One probe returns zero flags [M-2]; unbounded recursion is reachable |
| `--help` capture | stdout, exit 0 | stdout **and** stderr, any exit code | `openssl`/`ip` fail the revision-1 rule [M-8] |
| Tier B recursion | Root only | Per-node, lazily | Root-only yields no subcommand flags |
| Man tier | System `libmandoc` | Vendored mandoc + `cc`; multi-page discovery | Not a system library on Linux [M-6]; `git`'s tree lives across `git-*.1` |
| Shell parser | `conch-parser` | `brush-parser` | Unmaintained; future-incompat warning [M-9] |
| Catalog storage | 11 MB `include_str!` | Indexed, one tool deserialized per lookup | Whole-catalog deserialize per lookup |
| Cache key | Binary content hash | Path/size/mtime/inode + versions; negative caching | Hashing a 50 MB binary costs more than the parse |
| Layout | 35%/65% | `Min(24)` + fill, responsive breakpoints | 35% is ~20 usable cells at 80 columns [M-7] |
| Testing | Golden files | Golden files + **coverage harness** + real-argv, exec-policy, sanitization, border-integrity, fuzz | Makes universality measurable; a mocked-past argv bug shipped a dead tier |
| Data licenses | Absent | §14 table + `NOTICE` requirement | Most likely genuine legal exposure |
| Packaging | Absent | §15 | Shipping to crates.io/deb/rpm constrains layout from day one |
| NL search | Phase 6 feature | §17, deferred with reasoning preserved | Registry-size claim fails on real data; needs a fine-tune project |
| UX | — | `y` copy, `?` overlay, `--doctor`, designed degraded states | Copying the flag is the end of the core journey |
