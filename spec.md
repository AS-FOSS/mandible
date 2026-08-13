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
| `t` | Verbatim view: re-probe this node and show the tool's own `--help` output instead of the parse |
| `y` | Copy: the selected flag's spelling, or the node's full command path |
| `?` | Keybinding overlay |
| `r` | Re-extract this tool. Preserves expansion, selection, filter and view mode; abandons the in-flight warming cascade and restarts it |
| `q`, `Ctrl-C` | Quit |
| Mouse | Click row selects; click chevron toggles; wheel scrolls the pane under the cursor |

`y` is not a nice-to-have. Looking up a flag in order to type it is the terminal
step of the core journey, and a reference tool that can't hand you the string
makes you retype it.

`t` is the escape hatch for the one failure mode the rest of this document's
honesty machinery cannot signal. §7 Tier B's staged degradation labels a node it
could not parse, and the confidence cap marks a thin parse as a guess, but a
grammar that misreads a layout and produces a *plausible* tree is
indistinguishable from one that read it correctly — that is exactly what the
fabricated-subcommand regressions ([M-8], apt-get, git bisect) looked like from
the inside, and each was caught by a human reading the tool's real output beside
ours. Rather than reserve that check for whoever runs the coverage harness, `t`
puts it one key away for every user, on every node.

It re-probes rather than retaining raw text on every node: retention costs
megabytes across a warmed tree to serve one node at a time, and a retained copy
would show what the tool said at startup rather than what it says now — the same
staleness argument that removed the cache (§11). Rule 0 of §6 applies unchanged:
`pkill --help` is shown, because that shape is measured harmless, but an
interactive request does not widen what may be run — `pkill something --help`
stays refused here exactly as it is in the extraction pipeline.

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
| **clap `CompleteEnv`** (`COMPLETE=zsh <tool>`) | **Near-absent in the wild**, and since removed as a source. `ripgrep` errors; `cargo` prints ordinary help [M-4]. Detection had no protocol-guaranteed signal, so on a PATH sweep it matched ten tools of which none were clap — see §7 Tier E. |
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

**The raw pane (key `t`, §2) deliberately does not go through
`Text::sanitize`.** Its whole job is showing the tool's own bytes, and
`sanitize`'s whitespace-collapsing and paragraph-unwrapping are exactly what
would destroy the column alignment a reviewer needs to judge the parser
against. A second constructor, `Text::sanitize_preserving_layout`, exists for
this one path: it strips ANSI/OSC/DCS escapes, stray carriage returns, and
other C0 controls (a raw terminal escape or a lying `\r` could scramble the
reader's terminal, so this much neutralization is not optional even for a
"raw" view), and expands tabs to spaces at 8-column stops, because `ratatui`
gives a bare `\t` zero display width and leaving it unexpanded would misalign
columns rather than preserve them. It does not collapse whitespace, trim, or
unwrap paragraphs, and it is truncated to the same `MAX_TEXT_CHARS` bound as
`sanitize`. `Text::sanitize` itself, and every use of it on the path that
feeds the IR, is untouched; only `mandible-extract`'s `help_text::raw_help*`
functions call the preserving variant. The two constructors are verified
apart: diffing the raw pane against independently captured `--help` output
for `du` (column alignment) and `curl --help all` (large output) came back
byte-identical.

One consequence worth knowing when comparing the pane to your own terminal:
mandible probes tools by absolute resolved path, so a tool that echoes its
own `argv[0]` prints `Usage: /usr/bin/du` in the pane and `Usage: du` in a
shell where `du` was found via `PATH`. That difference is correct: it is
what the tool actually received as `argv[0]` in each case, not a defect in
either the probe or the pane.

The raw pane also displays stdout and stderr **both**, labelled, even though
§7 Tier B's parser reads only one of the two per its own rule. See that
section for why the two paths differ.

### 4.2 Provenance is per field, not per node

```rust
pub struct Provenance {
    /// Which sources contributed to this item, ordered by contribution.
    pub sources: SmallVec<[Source; 2]>,
    /// Set only when a heuristic tier produced this item.
    pub confidence: Option<f32>,
}

pub enum Source {
    NativeDynamic { protocol: &'static str },  // "cobra-dunder-complete"
    KnownSpec { provider: &'static str },      // "carapace", "withfig"
    CompletionScript { shell: &'static str },
    ManPage { format: ManFormat },             // Mdoc | Man
    HelpText,                                  // a structured block: table, .TP, ...
    HelpTextSynopsis,                          // a usage line — spellings, never prose ([M-15], §13.1b)
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

**`mandible --report <TOOL>`** assembles a paste-ready bug report: mandible's
own version, the target tool's version when recoverable, the `--doctor`
diagnostic (reusing its report-building code rather than re-deriving it), and
a raw `--help` capture, followed by the repository's issues URL. It goes
through the same sanctioned probe chokepoint every other tier uses and adds
no new argv shape. The honest limitation: a tool's version is scraped
best-effort from the same `--help` banner already captured (clap's own
template opens with `"<name> <version>"`, which this recognizes), but most
tools never print a version in `--help` at all, so the report usually asks
the person filing it to paste `<tool> --version` themselves. Recovering it
automatically would mean issuing a `--version` probe, which is a new argv
shape against §6 rule 2's closed list and has deliberately not been taken.

**`mandible --review <SEED>`** (with `--audit-dir`, default `audit`) opens
the audit review loop (§13.1c) inside the real TUI: it walks
`audit/<SEED>.toml`'s pending entries in file order, opening each tool
exactly as `mandible <tool>` would (same tree, same lazy fill, same raw
pane), and saves a verdict to the manifest immediately after every
confirmation, never batched, so a killed session resumes at the next pending
entry with everything answered so far intact.

---

## 6. Execution safety policy

mandible runs other people's binaries. This is the part of the design that can
damage a user's machine, and it gets its own section and its own tests.

**Rules, binding on every tier:**

1. **Never invoke a bare binary.** Revision 1's clap probe ran `<tool>` with no
   arguments to see whether it honored `COMPLETE=`. Running an arbitrary binary
   bare is how you launch a REPL, block on stdin, start a daemon, or trigger a
   tool whose no-arg default is an action. (That probe has since been removed
   entirely — §7 Tier E — but the rule stands for every tier: an argv is never
   empty.) Note that rule 2a is the necessary companion: counting arguments is
   not enough, because an *empty* argument satisfies this rule while being the
   opposite of inert.
2. **Only inert argv shapes.** A tier may invoke a tool only as:
   `__complete <words...>`, `completion <shell>`, `--help`, `-h`, `help
   [<words...>]`, `<words...> --help`/`-h` (a subcommand path's own probe,
   `HelpLongForPath`/`HelpShortForPath`), `<words...> --help <word>` (rule
   2b, below), or `-- <partial>` under `COMPLETE=`. Any other shape requires a
   spec amendment. The `COMPLETE=` shape is currently **unused** — no tier
   constructs it since Tier E's clap probe was removed — and it is retained on
   the type only so removing a public enum variant is not forced into a patch
   release.

   2a. **No empty argument the tool could read as its first positional.** Rule 1
   only counts arguments, and an empty string satisfies it while being the
   opposite of inert. `--` is the option terminator essentially every getopt
   program discards, so `<tool> -- ""` delivers an empty string as the tool's
   first positional — and a program whose first positional is a pattern reads
   that as *match everything*. Measured: `pkill -- ""` terminated every process
   in a private PID namespace, pkill included. This was the actual mechanism
   behind the machine reset that motivated rule 0, which masked it for thirteen
   named tools while the same argv still went to the rest of PATH.

   Enforced at the `run_inert` chokepoint, not at call sites, so no tier can
   reintroduce it. Exactly one empty argument is permitted: cobra's completion
   word, which is protocol-required (`docker __complete` without it fails with
   "requires at least 1 arg(s)") and is never the first positional — the
   `__complete` sentinel precedes it, and a non-cobra tool rejects that word
   rather than acting on it. So the rule is checkable: an empty element is
   allowed only behind a guard word, never straight after `--`.

   2b. **`InertArgv::HelpExpand` — the truncation-confession follow-up
   (WS5, approved amendment).** `curl --help` ends its own output:

   ```text
   This is not the full help, this menu is stripped into categories.
   Use "--help category" to get an overview of all categories.
   For all options use the manual or "--help all".
   ```

   That is a **truncation confession** — the tool telling the reader, in its
   own words, that what was just printed is not the complete document.
   Measured: `curl --help` recovers 12 flags; `curl --help all` recovers
   258, and mandible was reporting the 12-flag document as `ok` at full
   confidence. It is a convention, not a curl quirk (`ffmpeg -h long`/`-h
   full`, `git help -a`, `gcc --help=<class>` are the same genus), so this is
   a new, general argv shape rather than a curl special case — which is
   exactly why rule 2's closed list needs a new member, not a `mandible-
   extract` patch that quietly bypasses it.

   A new shape needs an amendment because it is new *argv the crate can
   construct*, not because it is dangerous by any measure this section
   already tracks: it is refused by rule 0 exactly like every other non-
   `["--help"]` shape (`run_inert`'s own `argv.args() != ["--help"]` check
   needs no change to cover it), it cannot produce an empty argument (rule
   2a — `word` is checked non-empty before this shape is even constructed,
   see below), and it is bounded by the same timeout, output cap, and
   scratch-directory redirect as every other probe. What rule 2 exists to
   police is the *shape*, and this is a shape nothing on the closed list
   already covers: `--help` followed by a second, tool-supplied word.

   **`InertArgv::HelpExpand { words, word }`** renders as `[..words,
   "--help", word]` (`mandible-extract/src/exec/policy.rs`). Three
   constraints, all enforced in `help_text::confession` and
   `HelpTextTier`, not left to caller discipline:

   - **`word` comes from the tool's own printed directive, never a prose
     heuristic and never a fabricated word.** `help_text::confession::
     detect_directives` recognizes a closed, content-keyed grammar — a
     quoted `"--help <word>"`/`"-h <word>"` shape, the word bare and the
     quote immediately closing right after it (curl's own `"--help all"`)
     — never keyed on the tool's name, so it fires identically for any
     tool that happens to print the same convention. `word` is copied
     verbatim from what matched; nothing about the word is invented,
     guessed, or derived from the tool's identity.
   - **`--help` precedes `word`.** So a getopt that stops at the first
     non-option (BSD/busybox-style, unlike glibc's permuting one) still
     reaches `--help` before ever considering `word` as a positional —
     this can never degrade into a bare positional some other getopt
     routes elsewhere, the exact hazard rule 2a exists to close for a
     *different* shape (`-- ""`). Putting `word` first was never on the
     table for this reason alone.
   - **Expansion is followed at most once, never chained.** A confession
     detected inside an *expanded* document is not looked at: the probe
     that fetches the expanded text returns it as-is, with no second call
     back into `detect_directives` on that result. This is structural, not
     a depth counter that could be miscalibrated — the function that
     issues the one follow-up probe simply never recurses into itself.

   **Interaction with rule 0 (the never-probe list) and the attestation
   gate (`heading_attested`, this section's closing paragraph on
   `HelpTextTier`).** Neither conflicts with this amendment, and both are
   worth saying explicitly rather than leaving a reader to wonder:

   - **Rule 0 wins unconditionally.** `HelpExpand`'s rendered argv is never
     exactly `["--help"]` (it is always `[..words, "--help", word]`, `word`
     non-empty), so `run_inert`'s existing check refuses it for every
     tool on the never-probe list with no code change and no special
     case — a `pkill`-named tool that confesses is refused the expansion
     exactly as it is refused every other non-`--help` shape.
   - **A directive-sourced word is structurally attested by construction.**
     The attestation gate exists to stop a *fabricated* word — one a
     grammar guessed at from layout — from becoming argv (this section's
     closing paragraph). A confession's `word` is not fabricated and is
     not a subcommand name any grammar inferred: it is copied verbatim
     from text the tool itself already printed, in response to a probe
     that was itself already attested (the root by construction, or a
     subcommand path that already passed the gate to be probed at all).
     So `word` needs no *separate* attestation check — it inherits the
     attestation of the probe that produced it, the same way the root's
     own `--help` probe is exempt from the gate because it is the name
     the user typed, not a word any parser invented.

   **Scope, deliberately narrow.** Only the single-word "expand to one
   complete document" shape ships (`help_text::confession`'s closed
   `FOLLOWABLE_WORDS` vocabulary, `all` only for now). curl's *other*
   directive, `--help category`, is detected (so `incomplete` still fires
   honestly) but not followed: following it returns a menu of category
   *names*, not flags, and turning that into real recovery needs
   enumerating each category as its own probe — a materially bigger
   feature (`--help category` → N probes) this amendment does not cover
   and does not partially build.

   A confession that is detected but not followed — an unrecognised
   word/shape, a failed follow-up probe, or a rule 0 refusal — caps the
   node's status at `incomplete` (§13.1's status ladder: `ok > incomplete >
   low-confidence > verbatim > no-tier`) rather than reporting a confident
   `ok` on a document the tool's own text already said was truncated.

   **Detection-only extension: two more shapes (WS5's own genus list,
   finally recognized).** curl was the specimen this rule was built from,
   but it was never the only one named — this rule's own genus list, above,
   already cited `ffmpeg -h long`/`-h full` and `gcc --help=<class>`.
   Neither is curl's *quoted* `"--help <word>"` shape, so
   `help_text::confession::detect_directives` did not see either one, and
   both tools were measured reporting a confident `ok` over a document
   their own text already said was incomplete — ffmpeg 91 flags at 97%
   described, gcc 43 flags at 95% described. Two grammar additions close
   that, **detection only**:

   - **ffmpeg's shape** is unquoted, inside a flag-table row rather than
     prose: `-h long -- print more options`. `help_text::confession::
     match_unquoted_table_row` recognizes `<flag> <word> -- <description>`,
     anchored to the trimmed line's start exactly as the quoted form is
     anchored to the character right after an opening quote — a bare `--`
     token must sit directly between the captured word and a non-empty
     description, which is what keeps it off an ordinary flag row (`-h,
     --help  show this help message and exit`: a comma sits where this
     grammar requires a space) and off a distinct, longer flag name
     (`--help-all`: no space between `--help` and `-all`).
   - **gcc's shape** is a flag *definition*, not an invocation example:
     `--help={common|optimizers|...}[,...].` lists `--help` itself as
     taking a value. `help_text::confession::match_flag_value_row`
     recognizes `<flag>=<opener>...`, requiring the character right after
     `=` to be one of `{`, `[`, `<`, `(` — the punctuation a
     class/placeholder enumeration opens with, never a bare word — so a
     hypothetical literal-valued row (`--help=yes`) or an optional-value
     row (`--help[=FMT]`, where `[` sits before `=`, not after it) is never
     mistaken for it. The word recorded is the first class name
     (`"common"`), taken verbatim.

   Both are safe to add **without** a rule 2 amendment, for the same reason
   detecting curl's shape was: detection only changes what gets *recorded*
   on the node (a `Confession`), never what argv gets *constructed*. Rule
   0's `argv.args() != ["--help"]` check, the attestation gate, and every
   other execution-safety mechanism in this section are untouched, because
   nothing new is ever run.

   **Following either is explicitly deferred, not shipped.** Neither
   shape's word is added to `FOLLOWABLE_WORDS`, and no new `InertArgv`
   variant exists to construct the argv either would need: ffmpeg's own
   invocation is `-h long` (bare `-h`, no `--help` prefix at all — not
   `HelpExpand`'s `[..words, "--help", word]` shape), and gcc's is
   `--help=common` (one joined token, not `--help` and `all` as two
   separate ones — also not `HelpExpand`'s shape). Each is *new argv this
   crate does not yet construct*, so each needs its own rule 2 amendment
   and its own §6 deliberation before it can ship, exactly as this
   amendment itself was required for curl's `--help all` — that is
   deferred work (WS5b), not a gap in this change. Until then, both
   directives are recorded with `followed: false` and cap status at
   `incomplete` via the ladder above — the same honest-but-incomplete
   outcome curl's own `--help category` already gets. An undetected
   confession is a false `ok`; a detected-but-unfollowed one is honest,
   which is what this extension buys on its own.
3. **stdin is always `/dev/null`.** No tier may ever inherit or pipe stdin.
4. **Hard wall-clock cap**, 2 s for `detect`, 10 s for `extract_node`. On expiry
   kill the **process group**, not just the child — completion scripts spawn
   helpers, and killing only the direct child leaks them.
5. **Bounded output.** Read at most 8 MiB of stdout+stderr per invocation; a tool
   that streams forever must not exhaust memory. Reader threads (or a poll loop)
   are mandatory to avoid pipe deadlock on large output.
6. **Sanitized environment, and a new session.** Clear `LESS`; **set**
   `PAGER`, `MANPAGER`, `GIT_PAGER`, and `SYSTEMD_PAGER` to `cat` (not merely
   clear them — absence is the weaker property, since several ecosystems read
   an unset pager variable as "go find one yourself" rather than "don't
   page"); set `TERM=dumb`, `NO_COLOR=1`, `COLUMNS=100`, `LC_ALL=C.UTF-8`.
   Spawn the probe as the leader of a **brand-new session**, not merely a new
   process group: `process_group(0)` alone leaves the child in the same
   session as mandible, so the session's controlling terminal is still
   reachable, and a descendant can `open("/dev/tty")` to read and write it
   directly regardless of what stdin/stdout/stderr were redirected to.

   A user reported `mandible systemctl` freezing their entire TUI, with a
   pager observed. `env_clear()` used to leave `PAGER` merely *absent*
   rather than set, and `process_group(0)` alone does not sever the
   controlling terminal — together, the working theory was that an absent
   `PAGER` let a tool go find `less` itself, which then opened `/dev/tty`
   directly for keyboard input (bypassing piped stdout entirely) and left
   termios changes on the tty device that a process-group kill cannot
   undo (termios state lives on the device, not the process).
   [M-17] measured that this specific theory does not hold for
   `systemctl`: systemd's own pager gate checks `isatty` on its *own*
   stdout/stderr, which `run_inert` always makes pipes, so no argv this
   crate constructs against `systemctl` ever reaches the pager at all — a
   74-verb sweep plus direct `strace` confirmation that `less` itself never
   attempts `/dev/tty` once its own stdout is non-tty, even with a real
   controlling terminal available via the session. But [M-17] also
   confirmed, with a shim that does nothing but attempt
   `open("/dev/tty")`, that the underlying mechanism the report pointed at
   is real and general, independent of `systemctl` or pagers specifically:
   under `process_group(0)` alone, a descendant *can* reach a real
   controlling terminal; spawning the probe in its own session (via
   `pre_exec` + `setsid()`, this crate's one audited `unsafe` — see
   `mandible-extract/src/exec/spawn.rs`) makes that same `open` fail with
   `ENXIO`. `tests/exec_policy.rs`'s `dev_tty_hazard` shim test is the
   regression net: it fails without the session fix and passes with it.
   The pager variables are kept set to `cat` anyway as defense-in-depth
   against a tool whose own pager gate is weaker than systemd's.
0. **Programs that signal processes or change machine state are invoked only
   as `<tool> --help`.** `kill`, `pkill`, `killall`, `killall5`, `skill`,
   `xkill`, `fuser`, and the system-state commands `halt`, `poweroff`,
   `reboot`, `shutdown`, `telinit`, `init` may be run with exactly that one
   argument vector. Every other shape — `-h`, `help <word>`, `<word> --help`,
   `completion <shell>`, `__complete` — is refused before anything is spawned.

   This began as a total ban, after a user reported `mandible pkill` freezing
   their machine badly enough to require a reset. Two later measurements
   reshaped it.

   **The reason originally given was false.** It held that rule 2's
   `<tool> <word> --help` shape makes `killall foo --help` kill everything
   named `foo`. On glibc, GNU getopt permutes arguments, so `--help` is
   processed wherever it sits: `pkill --help`, `pkill victim --help` and
   `killall victim --help` were all measured killing nothing. The reset's real
   mechanism was rule 2a's empty argument.

   **What the ban was silently protecting against is real, and was never
   written down: `-h` is not a help flag on these tools.** Measured against
   systemd's multi-call binary, saved only by polkit because the probe ran
   unprivileged — `halt -h`, `poweroff -h`, `reboot -h` and `shutdown -h` each
   *attempted the real operation* (`-h` is the halt in `shutdown -h now`).
   mandible falls back to `-h` whenever `--help` fails, so that fallback alone
   would have rebooted a machine running as root.

   So the rule keeps what is measured harmless and refuses what is measured
   dangerous, instead of trading one for the other. `--help` yields real flag
   lists — `pkill` 27 flags, `killall` and `fuser` 16 each, all fully
   described; twelve of the thirteen went from `no-tier` to `ok` on the PATH
   sweep. Positional shapes stay refused because argument permutation is a
   glibc behaviour rather than a guarantee (BSD and busybox getopt stop at the
   first non-option), and because the background tree warmer would reach any
   subcommand a future parser change starts emitting, unasked.

   The general form of that last hazard — a *fabricated* word becoming argv
   for any tool, not just these — is now closed for the one place in this
   crate that constructs a positional `--help` probe: Tier B's
   `<word> --help`/`-h` (`HelpTextTier`, `mandible-extract/src/help_text/mod.rs`)
   fires only when `NodeHints::heading_attested` is true — the word came
   from a recognized command heading (or the chain a heading started), never
   from layout alone. A non-attested node is not probed in any shape; the
   tier declines and records a per-node, per-tier failure (§5.3) rather than
   fabricating a probe or letting the tree silently gain an
   empty-but-successful node in its place. The root is exempt by construction
   (`Runner::extract_full_for` passes `heading_attested: true` for it, since
   it is the name the user typed, not a word any parser invented), so the
   ordinary `<tool> --help` root probe is unaffected. `mandible-extract/tests/exec_policy.rs`'s
   shim suite proves both halves: an attested word is still probed and its
   real flags recovered; a non-attested one reaches the tool's binary not at
   all — verified by running the same assertion against the pre-gate code
   path and watching it fail.

   This list stays anyway, and is not made redundant by that gate. The two
   close different gaps: the gate above governs *when a word is trusted
   enough to become argv at all*, while this list governs *what these
   thirteen specific programs may be asked to do even with a trusted word*
   — `--help` remains their only permitted shape regardless of provenance,
   because for them even a genuine, correctly-attested subcommand name is
   still a target (`killall foo --help` looks safe by attestation and is
   refused anyway, since `foo` naming a real process is exactly the risk).
   The gate also inherits whatever a grammar's own heading-recognition gets
   wrong — `heading_attested` is only as trustworthy as the rules in
   `help_text/sections.rs` that set it, and those have needed several fixes
   (AGENTS.md §2's invariant table records more than one) — while this list
   is closed on a fact about the program itself, independent of any parser.
   Belt and suspenders, on two different axes.

   **This is a safety rule, and is deliberately not the per-tool knowledge §1
   forbids.** §1 governs *extraction* — "if a tool renders badly, fix the
   general parser" — because such lists grow without bound and rot. This list is
   about what may be *executed at all*, is closed, and every entry shares one
   property that is a fact about the program rather than about its output
   format. The check lives in `exec::run_inert`, which every tier goes through,
   so no tier can reach one of these by another route; a test asserts a shim
   named `pkill` is never executed under any argv but `--help`, and *is*
   executed for that one — both halves matter, since silently refusing the
   permitted shape would quietly undo the coverage this rule now allows.

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

   **One subdirectory per variable, never one shared directory.** They pointed
   at a single path once, which is a filesystem shape no real machine has — a
   tool writing `$XDG_CACHE_HOME/x` and reading `$HOME/x` saw one file — so
   every probe ran against an environment that cannot occur.

   **The residual risk is now measured, not hypothetical.** The timeout kills the
   probe's *process group*, which a child that calls `setsid` leaves — so
   anything that daemonises survives it. A full-`PATH` sweep in CI loses roughly
   three shards in sixteen to the runner being reclaimed, and instrumenting each
   probe on both sides named the tools that started and never finished:
   `chromedriver` (starts a browser-driver server), `vimtutor` (launches vim),
   `ghci` (opens a REPL), `syscount.bt` (attaches kernel probes). The common
   property is not the tool but the behaviour: **`--help` is not what these
   programs do when they don't recognise it**, and what they do instead outlives
   the process group we can reach.
   
   Exposure differs sharply by use, which is why this is a documented limit
   rather than a blocker: interactive use probes one tool and its subcommands,
   while the coverage harness runs ~1,500 arbitrary binaries in one process and
   is the only place the orphans accumulate.

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

9. **Mask the redirect back out of the output.** Rule 8 has a cost the other
   rules don't: it changes what tools *say*. A tool printing a `$HOME`-derived
   default prints *ours* — `docker --help` reported its config location as
   `/tmp/mandible-exec-L3saJ8/.docker`, a directory deleted moments later that
   never existed for the reader, with nothing on screen marking it as anything
   but docker's own documentation. The safety mechanism had become a source of
   confidently false documentation, which is the failure §7's whole degradation
   ladder exists to prevent when a *parser* causes it.

   Each scratch path is replaced with **the variable that stood in for it**
   (`$HOME/.docker`), at the same boundary that applied the redirect, so every
   tier, `--doctor` and the verbatim view get it without knowing. Deliberately
   *not* the reader's real home directory: the tool never told us that, and
   filling in a blank is the same move as inventing structure, only smaller. It
   is how man pages write such defaults anyway, and it is identical on every
   machine, so a fixture captured from a real tool cannot bake in the capturing
   machine's paths.

   This is what rule 8's one-subdirectory-per-variable requirement buys. With a
   single shared directory there is no correct answer to write back, because
   `/tmp/…/.docker` could have come from any of seven variables.

   Matching is on this invocation's exact path, never a `/tmp/mnd-*` pattern, so
   a temp path a tool legitimately prints is untouched. Every path is registered
   under **both its logical and its canonicalized spelling**, because a probe
   that resolves its own working directory prints the physical one: on macOS
   `$TMPDIR` sits under `/var`, a symlink to `/private/var`, so registering one
   form left the output reading `cwd=/private$PWD` — a mangled hybrid harder to
   spot than an unmasked path.

   **Residual:** a tool wraps its own help text at the `COLUMNS` we set, so a
   path split across two lines cannot be matched. The scratch prefix is kept
   short to make that rarer; it does not eliminate it.

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
parse, and aggregate `%flags_text` sits around 96%.

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
  this tier exists for. The rule for which of the two the *parser* reads is
  not "stdout if non-empty, else stderr": that rule shipped first and was
  wrong. `openssl cmp --help` prints two `CMP info: ...` diagnostic lines to
  stdout and its entire ~60-line help to stderr, so it handed the parser the
  banner and threw the document away, reachable by roughly 150 openssl
  subcommands in the same shape. The rule now judges each stream on its own
  with a help-shaped-output check (`looks_like_help_output`, D1.3.1) and
  parses whichever stream looks like help:

  | stdout empty? | stderr empty? | picks |
  |---|---|---|
  | yes | yes | stdout (empty; nothing to pick) |
  | yes | no | stderr, the only stream available |
  | no | yes | stdout, the only stream available |
  | no | no, stdout help-shaped | stdout, regardless of stderr |
  | no | no, stdout not help-shaped, stderr help-shaped | stderr |
  | no | no, neither help-shaped | stdout, the default when there is nothing to prefer |

  Ties (both streams help-shaped) break toward stdout, the conventional
  stream for a well-behaved tool's `--help`. The streams are never
  concatenated for the parser: merging a diagnostic preamble into the
  document is how banner text becomes fabricated flags. `pick_stream` in
  `mandible-extract/src/help_text/mod.rs` carries the full truth table and
  reasoning above its definition.

  This is the *parsing* path only. The verbatim/raw pane (key `t`, §2) shows **both**
  streams, labelled, independent of what the parser chose to read. A
  reviewer checking the parser's work needs to see the diagnostic lines the
  parser correctly discarded, not just the document it kept. See §4.1 for
  the raw pane's own sanitization rule, which is deliberately different from
  the IR's.

  Measured, full PATH, 2,240 tools joined, before and after the fix: 0
  flag-count losses across any tool, 169 flags gained across 11 tools (every
  one from zero), 13 tools moving `verbatim` → `ok`, `verbatim_count` 321 →
  308.
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

**Measured cost on a full-screen program with no `completion` subcommand**
(task #14, 2026-08-13): `vim.basic` extracts in **~21.25s**, a ~200x outlier
against the fleet, even though its 45 flags all parse correctly and Tier B's
own probe/parse together cost single-digit milliseconds. Isolated with
`CompletionScriptTier::detect()` called directly against the real binary:
**20.03s**, which is the whole story. `vim.basic` has no `completion`
subcommand, so `probe_and_extract_flags`'s two sequential probes — argv
`["completion", "zsh"]`, then `["completion", "bash"]` (spec's own request-
zsh-first-bash-fallback order above) — each land on vim's ordinary "open
these two file names" behavior: it enters its normal full-screen editor
session (confirmed by the alternate-screen-buffer escape sequence,
`\x1b[?1049h`, appearing in the captured stdout) instead of erroring out on
an unrecognized subcommand. Given no controlling terminal (`run_inert`'s
sandboxing gives every probe its own session, spec §6 rule 6), it neither
produces the completion output Tier C wants nor exits — it just sits until
each probe's own `PROBE_TIMEOUT` (10s) kills it. Two probes × 10s ≈ the
entire measured 21.25s; parse time is not involved at all, and there is no
superlinear behaviour in `help_text::sections`'s parser to fix here — this
is pure probe time, correctly bounded by the existing timeout rather than
hanging forever.

This is a shape, not a `vim` special case: any interactive full-screen
program that (a) has no `completion` subcommand and (b) does not validate
an unrecognized positional before entering its main loop pays the same
2×`PROBE_TIMEOUT` tax from this tier's detection probe alone. No fix is
applied here. The two changes that would plausibly help — shortening
`PROBE_TIMEOUT`, or an early-exit heuristic that watches for alt-screen
escape sequences in the probe's streamed output and kills the child before
the timeout — both touch the exec sandboxing path spec §6/§8 gates behind a
fleet measurement (a shortened timeout risks turning a genuinely slow but
real completion-script generator into a failure; an escape-sequence
early-exit is a new detection mechanism, not yet measured against the
fleet for false positives). Recorded here per the same discipline as [M-16]
and D3: a diagnosis is durable, a speculative change to `exec/` is not.

**Not built.** Measured before building ([M-14]), re-scoped after a second
measurement contradicted its headline case ([M-16]), and deliberately
**off by default** — see "Trigger and default" below. This section records
what a correct implementation would be, so the next attempt does not
re-derive it.

Two sub-cases of very different quality:

- **`mdoc(7)` pages** use *semantic* macros — `.Fl` for a flag, `.Ar` for an
  argument, `.Nm`/`.Cm` for command names — so the AST genuinely distinguishes
  "this is a flag" from "this is prose." Real structure, not inference.
- **`man(7)` pages** are typeset prose with weak semantic tagging. Section
  boundaries (`NAME`, `SYNOPSIS`, `OPTIONS`, `EXAMPLES`) extract reliably;
  individual flag/description pairs need the same heuristics as Tier B.

**Do not** regex the rendered output of `man <tool>`, and do not parse `mandoc -T
tree` — the OpenBSD manual documents that format as unstable and explicitly says
not to write parsers against it. There is no `-T json`.

**Implementation: a pure-Rust subset parser, not `libmandoc` FFI.** Revision 2
specified `libmandoc` via `bindgen`, and that is superseded. `libmandoc` is not
a shipped library on Linux [M-6], so it would mean vendoring mandoc's source and
building it with `cc` — and `#![forbid(unsafe_code)]` rules out the FFI
regardless. [M-14] measured what a subset parser would actually need: target
man(7) `.TP`/`.IP` + `.B`, with `.It Fl` for mdoc (only ~20 of the relevant
pages are mdoc, so an mdoc-first plan aims at the wrong majority). **Do not gate
on an `OPTIONS` section** — `bash`, `ps` and `tmux` document options under
`DESCRIPTION`, and that gate alone cost 28 tools; gate on the *tag line*
beginning with a flag, which is also what excludes examples (`ps` tags its
examples with `.TP`).

**Man pages are generated too, and that decides the design.** help2man,
asciidoc/docbook→man, mdoc and hand-written roff partition this space the same
way clap/cobra/argparse partition help text — the Tier A′ insight, one tier
down. So the first step is a generator survey with a go/no-go per generator,
not a parser. [M-16] is why this is not optional: **git's 184 `git-*.1` pages
contain zero `.TP` macros** — asciidoc emits bold-run paragraphs instead — so a
`.TP`-targeting parser recovers nothing from the tool revision 2 named as this
tier's motivating case. git is therefore **not** the headline case; its flags
are reachable far more cheaply through `-h` ([M-16]) and, at the root, through
a usage-synopsis grammar. The honest remaining value is [M-14]'s measured
`.TP` set: `ssh` (52 entries against 0 today), `bash` (162 against 18), `ps`,
`tcpdump`, `mdadm`.

Multi-page discovery is still required for the tools that do benefit: a tool's
structure is spread across `<tool>-<sub>.N` siblings, found via `MANPATH`/`man -k`.

**Trigger and default — this reconciles a contradiction this spec used to
carry.** Revision 2 positioned this tier as a **prose backfill** merged by
authority (structural 60 / prose 180, §4.4), i.e. enriching parses that
already succeeded. [M-14]'s own conclusion says the opposite: *fire only as a
zero-confidence fallback*, which "keeps staleness away from the ~1,500 tools
that already parse and avoids authority-merge questions entirely." Those are
different tiers, and the disagreement sat unresolved in this document.

**Resolved in [M-14]'s favour** (maintainer decision, 2026-08-11):

1. **Zero-confidence fallback only.** This tier fires only where the help-text
   tiers produced nothing usable. It never enriches a parse that already
   succeeded — so a tool like `git restore`, which yields 16 flags from `-h`,
   is never touched by it.
2. **Off by default, opt-in if built.** Enrichment-by-merge is the more
   invasive reading and is not the default behaviour.
3. Rationale, and it is a UX judgement as much as a technical one: a man page
   is a *different document* from the tool's own help, written at a different
   time, and silently blending the two makes a pane that no longer corresponds
   to anything the tool actually prints. That is the cleanliness cost, and it
   is the same objection [M-14] recorded as "avoids authority-merge questions
   entirely."

Where it does fire, per-field provenance labels the prose `man`, so a reader
can always see that a description came from a page rather than from the
binary.

### Tier E — native, self-describing binaries

Highest structural authority, lowest cost-efficiency. Attempted last for *cost*
reasons — it is the only tier that spawns a process per node — but it wins
structural conflicts (§4.4) because it reflects the version actually installed.

**Gated on prior evidence, never speculative (2026-08-12).** This tier only
constructs a `__complete` argv for a tool whose *own compiled bytes* already
identify it as cobra, via Tier A′'s `identify_from_artifact`. It used to ask
every tool on `PATH`, because asking was the only way to find out who answered.

Reported from real use: probing `wall` that way broadcast the literal text
`__complete` to every logged-in terminal on the reporter's machine, because
`wall` treats an unrecognized first positional as the message to send. That is
the same shape as `pkill -- ""` under §6 rule 2a, an argv that is inert for
nearly every tool and an action for one family, and it is the second time that
shape has caused a real-world side effect. A containment list
(`exec::spawn::HELP_ONLY_PROBE`) closes the measured cases. It cannot close the
general one, because it can only ever name tools somebody has already been
bitten by. The gate closes the general one.

Measured, full PATH, before and after, 2,248 tools joined: **no status
transitions, no flag-count gains, no flag-count losses**, and an identical
aggregate (`pct_flags_with_text` 94.83 both sides). Tools eligible for a
`__complete` probe fell from every tool swept, 2,302, to **5**: `docker`,
`dockerd`, `gh`, `git-lfs`, `ollama`. So the speculative form was contributing
nothing to extraction while carrying the whole risk.

The cost of the gate is that a genuine cobra tool whose artifact check fails,
a stripped binary being the realistic case, loses this tier rather than being
probed anyway. Nothing on this machine's PATH was in that position. The
`HELP_ONLY_PROBE` entries stay in place regardless: an artifact fingerprint is
a heuristic, and defence in depth costs nothing here.

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
- **clap `CompleteEnv`** (`COMPLETE=<shell> <tool> -- <partial>`): **probed once,
  now removed.** It was always marginal — measured opt-in and rare, with `ripgrep`
  erroring and `cargo` printing ordinary help [M-4] — but it was removed for two
  concrete reasons rather than for rarity.

  It could not be spelled safely. With an empty partial it rendered as
  `<tool> -- ""`, and `--` is the option terminator essentially every getopt
  program discards, so the empty string arrived as the tool's first positional:
  `pkill -- ""` was measured terminating every process in a PID namespace (see
  §6 rule 2a). Spelled `<tool> --` instead it is harmless but wrong, because `--`
  is a no-op for most tools, which then print ordinary output that the shape
  heuristic reads as candidates — measured at 16 tools spuriously acquiring this
  tier, 8 of them flagged suspicious.

  And it never worked. Unlike cobra's `:N` directive, clap's protocol has no
  self-identifying trailer, so detection was only ever a shape heuristic; on the
  PATH sweep it matched ten tools and **none** were clap (`echo -- ""` prints
  `--`, which starts with a dash and so "looked like" a flag). Re-adding it needs
  a way to confirm the protocol before trusting the response — gating on Tier A′
  framework identification would supply one — and a spelling that never passes an
  empty first positional.
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
│   ├── manpage/                 # Tier D: pure-Rust roff subset [feature = "manpage"]
│   ├── native/                  # Tier E: cobra `__complete` probes
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

Per-tier modules sit behind feature flags so Tier D — which only makes sense
where man pages exist, and which is off by default in any case (§7 Tier D) —
is not a hard requirement for a Windows user who wants Tiers A/B/C/E. Note the
original reason for gating it, a C toolchain for `libmandoc`, no longer
applies: §7 Tier D is a pure-Rust subset parser.

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

### 9.1a Flag rows: a table, or honestly not one

The detail pane's flag list is a three-column table — spelling, value
placeholder, description. Three rather than two because a placeholder answers a
different question from a spelling (`--env` is what you type, `list` is what it
takes); run together as `--env list` they read as one token, while in their own
columns the list can be scanned down either one.

- **The description column is one number for the whole list.** Not a target the
  wide rows are allowed to miss. A column that most rows share and some rows
  don't is not alignment, it is noise that looks like alignment — and it is
  worse than no column at all, because the eye keeps trying to use it.
- **A row too wide for the column hangs**: its description starts on the next
  line, at the column. It never pushes the column right for itself, and the
  spelling is never truncated to force alignment (as in §9.1, names win). The
  row costs one extra line, which is the only cost nothing else has to pay.
- **An outlier spelling is excluded from the measurement, not clamped to it.**
  Clamping sets a column the outlier still misses; excluding lets it hang while
  every other row stays aligned. Threshold: a spelling wider than 45% of the
  pane does not get a vote.
- **Below the width where the table can leave prose a readable amount of room
  (28 columns), stop pretending and stack**: spelling and value on one line,
  description indented beneath. A table whose columns have eaten the pane
  breaks six words of prose across six lines; a stacked list at the same width
  reads normally. This is the same judgement as the tree's width ladder —
  degrade to a different layout deliberately rather than to a worse version of
  the same one.

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
| **5 — Tier D + F** | Pure-Rust roff subset parser (feature-gated, **off by default**), generator survey first, multi-page discovery; user overrides | A generator survey with go/no-go per generator; `ssh` and `bash` gain prose where they have none today ([M-14]). **Not** `git` — its pages carry zero `.TP` and its flags come from `-h` instead ([M-16]) |
| **6 — distribution** | crates.io release, `cargo-deb`/`cargo-generate-rpm`, man page for mandible itself, shell completions | `cargo install mandible` works; `.deb` and `.rpm` install cleanly |

Deliberately **not** on the roadmap: local NL search (§17).

---

## 13. Testing & the coverage harness

### 13.1 The coverage harness

This is the most important testing artifact in the project and revision 1 lacked
it. `cargo xtask coverage` runs extraction across every executable on `PATH` and
emits a scoreboard:

```
tool        tier(s)              nodes  flags  %flags_text  ms     status
docker      carapace+help          162    836        100%   180    ok
curl        help                     1    241         96%    90    ok
openssl     help                     1    112         71%   140    ok  (stderr)
somecli     help                     3     12         33%    60    low-confidence
weirdtool   —                        0      0          —    240    no-tier
```

The scoreboard is **checked into the repo** and diffed on every parser change,
and carries a literal `accuracy: unmeasured` line (see below) until an
instrument actually measures correctness rather than mere presence.

This is what makes "universal, no per-tool adjustment" **measurable** rather than
aspirational. Without it, every grammar tweak is evaluated against the one tool
you happened to be looking at, and there is no way to see that fixing `tar`
regressed `xz`. It is also the signal for when a tier has stopped earning its
complexity.

Regression gate: `%flags_text` aggregate and `no-tier` count may not worsen.
`%flags_text` is `described / describable`, not `described / total` — see
§13.1b's metric design rules for why the denominator excludes flags a source
could never have described in the first place.

**`%flags_text` alone is not a quality signal, and trusting it hid two real
bugs, not one.** The Tier B phantom-subcommand defect [M-10] reported `tar` as
`ok` at `100% described` (this column's name at the time) while 39 of its 40
nodes were fabricated — invented nodes *inflate* the metric. The scoreboard
therefore also carries a **structure-sanity** column: count of nodes whose name
fails `^[a-z][a-z0-9_.-]*$`, and count of nodes with no flags, no children, and
no summary. Any tool with a non-zero count is marked `suspicious`, and
`suspicious` is a gated metric exactly like `no-tier`.

**`lsof` (`corpus/lsof/4.95.0`, `[xfail]`) is the second bug, and the reason
this column was renamed from `%described` to `%flags_text`.** It scored 79%
"described" — every number above suggesting a good parse — while its options
table packs three flag+description pairs onto one physical line and the
generic parser reads only the first, so roughly three quarters of its
"described" flags actually carry a *different* flag's description.
`%flags_text` has only ever measured whether text is *attached*; it has never checked
whether that text is *right*, and `%described` was a name that let a reader
assume it did. `%flags_text` is the honest name for the same ratio, unchanged
in every other respect — see §13.1b. The **misattribution detector**
(`xtask/src/misattribution.rs`) is this project's first step toward an actual
correctness signal: a re-examination of text the pipeline already captures
(no new probes) that flags a flag description containing another flag's
literal spelling, attested at a column-aligned position elsewhere in the
tool's own raw help text — the exact shape of `lsof`'s bug, generalized. It is
a heuristic with a measured, nonzero false-positive rate, reported in the
scoreboard's `misattr` column and `misattribution_suspect_tools` footer field,
and **deliberately not gated**: see that module's own doc comment for the
full rule, the false positives it had to be hardened against, and why a
brand-new detector must not fail a build the first time it runs. Until a
lower-false-positive accuracy instrument exists, every scoreboard also carries
a literal `accuracy: unmeasured` line, so a reader can never mistake
`%flags_text` for it again.

**The "anti-fabrication oracle" this section originally called for turned out
to be two checks, not one — a distinction that cost a cycle to discover and
should not have to be rediscovered.** Misattribution (above) answers "does a
description belong to the flag it's attached to?" — its victim is `lsof`'s
column-bled options table. A second, independent question is "does everything
extracted actually *occur* in the tool's own output, or was it invented?" —
and its victim is [M-10], this project's worst shipped defect: `tar` gained 39
phantom subcommands named things like *"treat them as errors"* (a wrapped
continuation line mistaken for a new table entry), `dd` 40, `less` 65,
`apt-get` seven words lifted from its own description paragraph — every one
reported `100%` "described," because a fabricated node's own fabricated flags
look exactly as described as a real node's. The **existence detector**
(`xtask/src/existence.rs`) is this check: every help-text-sourced subcommand
name and flag spelling the tier emitted must occur literally in the tool's own
raw captured text — a subcommand name additionally at a line-start-ish
position (the real, measured shape of a command-list entry; a bare substring
match alone is too weak against a fabricated name built from an ordinary
English word that also happens to appear once in unrelated prose). It compares
against *pre-normalization* spellings, not the IR's stored form: alias pairing
(`mandible_core::merge::pair_aliases`, e.g. `gh`'s `-R`/`--repo`) means a
flag's two spellings need not sit together in the raw text, only each occur
somewhere in it; value stripping (`--gpg-sign[=KID]` stored as `gpg-sign`)
means a spelling is checked as a word-bounded *prefix*, not an exact token;
and a negatable boolean (`--[no-]source`, `mandible_core::Flag::negatable`)
is checked against its real bracketed raw form, never the bare `--source` that
never actually appears. Same properties as misattribution and for the same
reasons: it reuses `misattribution::RecordingProbe` (no new probes), is scoped
to `Source::HelpText`/`Source::HelpTextSynopsis` only (every other source —
Cobra `__complete`, a completion script, a native probe — is structural and
legitimately silent in help text), and is reported in the scoreboard's `exist`
column and `existence_fabrication_tools` footer field **without being gated**
— a brand-new detector with no fleet-wide baseline must not fail a build the
first time it runs (§13.1b).

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
% of flags with text (`%flags_text`), and pass/fail. The gate fails on
regressions in `no-tier`, `suspicious`, or framework-detection failures.

This is the natural home for the §13.1 scoreboard once it stops depending on
whatever happens to be installed on a developer's laptop.

### 13.1b Metric design rules

`pct_flags_with_text` (`%flags_text` in the scoreboard; named `pct_described`
until this rename — see below) was originally `described / total`. [M-15]'s
usage-synopsis flag grammar recovered 1,618 real flags — a usage line lists
spellings, never prose, by construction — and the ratio *fell*, 94.18% →
91.17%, because every recovered flag counted as "undescribed" against a
source that could never have described it. Recall was punished for
succeeding. That is a defect in the metric, not in the grammar, and it is the
same shape of mistake this project has now made five times:

- **[M-10]** — invented subcommand nodes *inflated* the ratio (`tar`
  reported `ok` at 100% described while 39 of its 40 nodes were fabricated).
  Fixed by the structure-sanity column (above), not by trusting the ratio.
- **[M-16]** — `verbatim` conflated "the tool printed nothing this grammar
  can use" with "the tool rendered a man page," a fifty-times overestimate
  (`verbatim_count=314` against `man_shaped_count=6`) if either reading were
  assumed from the other. Fixed by a dedicated `man_shaped` check reading the
  captured text directly, never inferred from `verbatim` alone.
- **A sweep-timing false transition** — `waagent2.0` was reported
  `ok` → `verbatim`, a *downward* transition that a per-tool status gate
  would have flagged red, on two coverage runs against **identical code**.
  Its elapsed time halved between them (41.9s → 21.4s), i.e. both runs sat
  near the 10s extract cap and machine load decided which side of it the
  tool landed on. A timing-derived status is a statement about the machine
  that happened to run it, not about the parser.
- **The [M-15] denominator defect** — the ratio punished [M-15]'s recovered
  synopsis flags by counting them as undescribed against a source that
  structurally cannot supply a description.
- **The name itself** — `pct_described`/`%described` measured only whether a
  flag had *text attached*, never whether the text was *right*, and a name
  that says "described" reads as an accuracy claim it never earned. `lsof`
  (`corpus/lsof/4.95.0`) is the proof: it scored 79% "described" while
  roughly a quarter of its flags carried another flag's description
  entirely. See below.

Three rules, derived from the first four of those incidents rather than
asserted in the abstract:

1. **A gated metric must be monotone under added true information.** An
   improvement that adds correct flags and loses nothing must never worsen
   any number a gate reads. `pct_flags_with_text = described / total`
   violated this the moment a source existed that could add flags but never
   descriptions.
2. **Denominators are conditioned on what the source could have provided.**
   A flag whose only source cannot supply a description (spec [M-15]:
   `mandible_core::Source::HelpTextSynopsis`) is excluded from
   `pct_flags_with_text`'s denominator entirely — see
   [`mandible_core::Provenance::describable`] and
   [`mandible_extract::ExtractionResult::describable_flag_count`] — rather
   than counted as a description the grammar failed to find. The raw flag
   count is kept as its own, ungated column: a spelling-only flag is real
   information, just not part of a ratio to gate on.
3. **A status derived under resource pressure (timeout-adjacent) is a
   statement about the machine, not the parser.** Wall-clock-derived
   signals (a parse-time ceiling, a sweep that ran under contention) must
   not silently flip a correctness gate; a machine-load explanation should
   be distinguishable from an actual regression before either is reported
   as one.

Rule 3 is not scoped to the corpus gate — it applies to any wall-clock
assertion in the test suite. `mandible-extract/src/help_text/sections.rs`'s
`repeated_identical_banner_does_not_explode_into_duplicate_subcommands`
(20,000 repetitions of a banner, guarding the same O(n²)-on-repetitive-input
class this module's `MAX_RECOVERED_ENTRIES` cap exists for) false-failed
twice under concurrent-compile load — 7.5s observed at load average 20+ on
4 cores, 4.3s alone, clean on a quiet re-run — with the parser unchanged
between runs, matching the `waagent2.0` pattern exactly. Its timing
assertion was demoted to a non-blocking `eprintln!` warning (budget: 10s,
comfortably above every observed run) so a genuine reintroduction of the
blowup — which lands in seconds-to-minutes, not a borderline overage —
still prints loudly, while the correctness assertion (exactly 2 subcommands
recovered, not 40,000) stays a blocking `assert_eq!`. A second timing
assertion (`mandible-extract/src/exec/spawn.rs`'s `timeout_kills_process_group`,
asserting a 200ms-timeout kill completes within 5s) was surveyed against
the same rule and left blocking: it carries a 25x margin already, is not
CPU-bound work competing for cores under contention (so is far less
exposed to the mechanism that broke the two cases above), has no observed
false failure, and is one of the few tests that directly exercises the
process-group-kill safety property in spec §6/§8 — demoting it trades a
safety check for convenience without evidence it flakes.

`pct_flags_with_text` is `described / describable`. Fleet-wide on this
aarch64 box (2,266 `PATH` tools), the redefinition returns it to
**94.19%** — within 0.01 of the 94.18% figure that predates [M-15]'s
synopsis-flag recovery — while the recovered flags remain fully counted in
the raw total (48,278, unchanged) and 204 tools move `low-confidence` →
`ok` with zero tools moving the other direction. See Appendix B.

**A fifth rule, from the fifth incident:** a metric's *name* is part of its
design, not decoration, and a name a reader could reasonably mistake for a
stronger claim than the metric makes is itself a defect — the same category
of bug as a wrong denominator, just harder to grep for. `pct_described` was
renamed to `pct_flags_with_text` (the scoreboard column: `%described` →
`%flags_text`) for exactly this reason: it changes nothing about how the
ratio is computed, only what it is honestly called. Every scoreboard also
now carries a literal `accuracy: unmeasured` line until an instrument
actually measures correctness — see §13.1's own note on the misattribution
detector, the first step toward one. `xtask::coverage::parse_aggregate_footer`
still reads a scoreboard's old `pct_described=` key for backward
compatibility; it never writes one. See Appendix B for the historical note on
the column-name change.

**A sixth rule: a mass status promotion must carry its own spot-audit
stratum, sampled at random, not asserted from the aggregate that produced
it.** Corpus-green (§13.2) plus a clean scoreboard sweep-diff (§13.1) prove
that nothing which worked before **broke** — every existing fixture still
passes and no tool transitioned to a worse status. They can **never** prove
that the tools newly promoted to `ok` are actually right, because neither
instrument looks at a single one of them with a human eye; both are
structurally blind to a defect the promoting change itself introduces. The
narrow lesson behind this rule is measured, not hypothetical: of the seed-2
audit's 13 `forced-inclusion`
entries — tools promoted `low-confidence` → `ok` by an earlier change and
never independently reviewed before being force-included into this audit —
only **1 of 13** (7.7%, §13.1c) held up as `correct` once a human actually
read the output. A same-sized ordinary random draw would not have missed
that badly by chance alone. **Any change that promotes more than a handful
of tools to `ok` must therefore include a spot-audit of 5–10 randomly drawn
promoted tools, recorded in the audit manifest as its own stratum** — the
same discipline §13.1d's frozen queue applies to the whole population and
§13.1e's calibration applies to a detector's fleet-wide count, now applied to
a promotion event specifically, before its count is trusted.

**The mechanism now exists**: `xtask audit spot-audit --event <name>
--promoted <tool,tool,...> --sample <n> --draw-seed <seed>`. The draw is
reproducible, not hand-picked — `--draw-seed` mixed with `--event` (via
`rng::stratum_seed`, the same per-stratum seed mix the frozen queue's own
shuffle-stratification uses) seeds a Fisher-Yates shuffle over `--promoted`,
so the same event name and seed always draw the same tools. Each drawn tool
is tagged with `Entry::spot_audit_event`, reported by `cmd_report` as its
own `spot-audit:<event>` row, distinct from both the ordinary parse-status
strata and `FORCED_INCLUSION_STRATUM` — the gap this section originally
named: `include_reason`/`forced-inclusion` answers *why a tool bypassed the
draw* and tallies every such tool under one label regardless of reason,
which is wrong for a mechanism needing one row *per promotion event*. When
`--promoted` names fewer tools than `--sample` (the exact shape of the first
real case below), every promoted tool is drawn and the shortfall is stated
explicitly — never a silently smaller sample, never a padded count. A tool
already present in the manifest (the common case: a promotion event's
tools were usually already sampled by the ordinary draw, under a now-stale
pre-fix verdict) is tagged into the new stratum without its verdict, note,
or amendment history being touched — only `xtask audit amend` may correct
a verdict, never a draw.

**First promotion event, backfilled**: 942890d's synopsis short-flag-cluster
fix changed 5 of the 94 seed-2-audited tools (tcpdump, xfs_io, filefrag,
tmux, eqn — all gaining flags, none losing any), all already present in
`audit/2.toml` with pre-fix verdicts (`wrong`/`incomplete`, all four
non-tmux entries carrying the `bundled-short-flag` defect family the fix
addressed). `xtask audit spot-audit --event bundled-short-flag-942890d
--promoted tcpdump,xfs_io,filefrag,tmux,eqn --sample 8 --draw-seed 942890`
drew all 5 (below the 5–10 target, reported as such) into
`spot-audit:bundled-short-flag-942890d`; the maintainer's 2026-08-13 TUI
re-inspection confirming all 5 now parse correctly is recorded as five
`xtask audit amend` corrections, each preserving the original verdict in
its amendment history. Headline: 35/86 (40.7%, 95% CI [30.9%, 51.3%]) ->
40/86 (46.5%, 95% CI [36.3%, 57.0%]) — denominator unchanged (the same 86
already-judged entries), only the numerator moves, since five tools already
counted as judged were legitimately re-verified correct after a real fix.

### 13.1c The audit instrument: comparing against truth

Misattribution and existence (§13.1) each re-examine text the pipeline
already captured, so both are still, structurally, comparisons against the
parser's own prior output. `xtask audit` and `mandible --review` are the
project's fourth testing instrument and the first to compare output against
independently established truth: a human reads the tool's own raw `--help`
text side by side with the parsed tree and judges whether the tree is right.

**Subcommands** (`xtask audit <subcommand>`): `sample` draws and persists a
sample; `review` is the interactive terminal loop; `emit`/`ingest` are its
non-interactive twin, since this project's CI has no tty (AGENTS.md §3.2):
`emit` writes every pending pair to a file for offline reading, `ingest`
reads a plain-text verdicts file back in; `report` renders accuracy;
`fixtures` turns a reviewed tool into a staged `corpus/`-shaped fixture, a
`correct` verdict becoming a real `expected.snap` and a `wrong`/`incomplete`
verdict becoming `[xfail]` with the reviewer's note as `reason` (`--bless`'s
own reasoning, applied to a human read instead of an automated one).
`mandible --review <SEED>` (§5.3) is a fifth entry point onto the same
manifest, reviewing inside the real TUI instead of a terminal loop.

**The draw is stratified, deterministic, and force-includable — via a
frozen queue, not a live re-sweep (§13.1d).** `xtask audit freeze` sweeps
`PATH` once, classifies every tool by parse status (`ok`/`low-confidence`/
`verbatim`/`no-tier`/`suspicious`, whatever `status::compute` actually
produces for the population, not a fixed bucket set), and shuffle-stratifies
the result into an ordered queue (`audit/queue.toml`) so the sample's status
mix reflects the real population's. `xtask audit sample` then just advances
that queue's cursor by `--sample` tools — no re-probing, no
reclassification, at draw time. A tool can additionally be force-included
outside the queue draw, but only with a recorded reason
(`audit/force-include.txt`, `<tool> <reason...>` per line). An unconditional
inclusion with no stated reason is exactly the kind of unauditable claim
this instrument exists to rule out, so force-included entries are tallied
under their own `forced-inclusion` stratum in `audit report` rather than
blended into the random draw's numbers.

**Verdicts are `correct`, `incomplete`, `wrong`, or `skip`.** A `wrong` or
`incomplete` verdict must carry a note, enforced identically in both entry
paths (the TUI refuses to save a blank-note draft; `xtask audit ingest`
fails the line): for those two verdicts the note *is* the finding, and a bare
`wrong` with nothing recorded about what was wrong is useless to whatever
fix the audit is meant to feed. `correct` and `skip` do not require one,
since forcing prose out of a reviewer with nothing to add is how a review
loop starts collecting "n/a". `skip` is recorded, not omitted: a skipped
entry still occupies its slot and appears in `audit report`, just excluded
from the accuracy ratio.

**Three pre-tagged known-defect classes** are computed once at sample time
and shown to the reviewer before they record a verdict, so confirming is
"leave it alone" and overriding is one `k1=`/`k2=`/`k3=` token in the
verdict line or note:

- **K1**: the GCC-family single-dash-long-option mis-parse. A flag like
  `-fdump-scos` is stored as short flag `-f` with `value_name` `dump-scos`
  instead of as the long-form spelling it actually is. This is a real,
  measured parser defect, not a detector artifact, and is scheduled as
  grammar item 1 once the parser freeze the audit is running under lifts.

  **K1's signature is a shape three defect families share**, which is why
  the tag alone cannot stand in for a family label (§13.1e). `short.is_some()
  && long.is_none() && value_name.is_some()` is produced by the GCC case
  (`-pass-exit-codes` → `-p` + `ass-exit-codes`), by a collapsed bundle
  (`tmux`'s `[-2CDlNuVv]` → `-2` + `CDlNuVv`), *and* by a repeated-character
  flag (`bpftrace`'s `-vv` → `-v` + value `v`, measured on all five `.bt`
  tools in the seed-2 sample — the reviewer read these as "missing", since
  the TUI shows two `-v` rows and no `-vv`). All three sit under `k1 = true`
  in `audit/2.toml`. The families are distinguished by what the value text
  *is*: a long-option word, a run of distinct single-character flag letters,
  or a repeat of the flag's own letter. A detector for any one of them will
  fire on the other two unless it makes that distinction, which is precisely
  the kind of thing calibration surfaces and a fleet count alone hides.
- **K2**: the existence detector's own tokenizer gap (`xtask::existence`),
  not a parser defect. **Closed.** It was characterized on a full
  2302-tool `PATH` sweep — 656 fabrications, hand-classified by the shape
  of their raw-text occurrence — and 613 of the 656 (93%) turned out to be
  detector artifacts of three kinds, all now fixed:

  | n | shape |
  |---|---|
  | 359 | a subcommand at an item position of a multi-column or comma-joined index (`busybox` 247, `openssl` 112). `line_start_words` only considered a line's *first* token; `list_row_words` now reads a whole list row. |
  | 200 | the oracle read the **wrong stream**. `RecordingProbe` carried its own superseded copy of `help_text::pick_stream` ("stdout if non-empty"), so on every tool that banners to stdout and helps to stderr (`mkfs.fat`, `tune2fs`, `btrfs-convert`, `xfs_scrub`, `encguess`, …) it compared a correct tree against a version string. The decision is now exported from `mandible-extract` and imported, not restated. |
  | 54 | a long flag whose value spec is glued on with a word-shaped first character (`--perf-no_read_workqueue` → `long: "perf-no"`, `value_name: "_read_workqueue"`). `long_candidates` now reconstructs it, as `short_candidates` already did for `-fdump-scos`. |

  The residual 43 are **genuine**: 42 are the short half of a flag alias
  `merge::pair_aliases` mis-merged (GCC's `-f…` rows paired with
  `--param=…` rows), and one is an invented short alias (`dockerd`'s `-h`,
  whose help text documents only `--help`). Both are parser defects the
  oracle is correctly reporting, so K2 no longer explains anything and the
  pre-tag exists only to catch a regression.

  **What the oracle still cannot see, and never could:** it checks whether
  a reported spelling *occurs*, not whether the parse that produced it is
  right. A *collapsed* bundled short flag — `tmux`'s `[-2CDlNuVv]` read as
  a single flag `-2` taking a required value `CDlNuVv` — occurs literally
  in the help text, so it attests cleanly while being badly wrong. Zero
  fabrications is not a claim of a correct parse. That belongs to the
  family-detector work, not to K2.
- **K3**: a subcommand stub whose help was never fetched, from either of
  two causes. Either the attestation gate permanently refused to probe a
  name that came from a native/cobra artifact rather than a recognized
  `--help` heading (never a "not yet fetched", a "cannot ever be fetched"),
  or the tool's subcommands simply carry no flags because their own help
  was never probed by the single-pass extraction the sample is drawn from.

**`audit report` states accuracy per stratum with a Wilson 95% confidence
interval, never a bare percentage.** This is the same discipline `%flags_text`'s
own history (§13.1b) exists to enforce elsewhere. It also reports accuracy
under views with each known class excluded (K1-excluded, K2-excluded,
K3-excluded, and all three excluded together), so a reader can see how much
of the raw number is attributable to a known, already-scheduled cause versus
genuinely unexplained.

**Scope, decided for the seed-2 run:** the audit measures flag accuracy and
command/subcommand accuracy only. A node's own prose description and
usage-section formatting are explicitly out of scope and deferred, so
neither drives a verdict here. The boundary that keeps this consistent: a
*flag's* description attached to the *wrong* flag is in scope, because that
is flag data mis-attribution (the same shape `lsof`'s bug was, §13.1); the
*node's* own prose description is not, because that is a different kind of
claim this audit is not yet reviewing.

**Display-only findings are excluded from the accuracy denominator, never
from the record (task #28).** A reviewer's `wrong`/`incomplete` verdict
sometimes lands on a defect that turns out to be `mandible --review`'s TUI
mis-rendering a correct extraction — a wrapped usage synopsis, a width
that goes out of bounds — rather than the parser getting the tree wrong.
The maintainer's ruling: *"those are not accuracy, those are probably UI
rendering issue. parsing was fine."* `skip` cannot record this, because
`skip` means "the reviewer did not judge this tool," which is false here —
the defect *was* judged, and real. Instead, the `display-only`
[`mandible_core::audit::DEFECT_FAMILIES`] label (already part of the
closed family set) marks it, and [`Entry::is_display_only`] is what
`xtask::audit::accuracy_over` (and every view built on it — the K1/K2/K3
sensitivity table, every per-stratum row in `audit report`) excludes on:
the verdict, note, and fixture all stay exactly as recorded, and
`audit report` prints the excluded findings in their own dedicated
section plus an `out-of-scope` column per stratum, so the number can never
go quietly missing.

Like `xtask::detector::Ground::BelowMemberThreshold` a commit earlier this
week made structural for detector-scope exclusions, this exclusion is not
claimable by free-text assertion: `display-only` must be an entry's
*only* family (`validate_families` already requires it come from the
closed set, carry `families_derived` provenance, and sit only on a judged
defect) — a genuine parse-shape family riding alongside `display-only` on
the same entry blocks the exclusion rather than granting it, so two true
labels can never add up to laundering a real defect out of the
denominator.

**`audit/<seed>.toml` is tracked.** It is both the sample manifest and the
verdict record, a verdict written directly onto its own sample entry, so an
accuracy claim carries its evidence rather than depending on a file that
lived on one contributor's machine. **`audit/<seed>/fixtures/` is not
tracked**: it is `xtask audit fixtures`' staging output, and
`corpus/README.md`'s own workflow is to review a staged fixture by hand and
deliberately promote what's ready into `corpus/`, so the staging tree is
scratch by design.

The audit has not finished running. This section documents the instrument,
not a result: no accuracy number is stated here, and none should be read
into anything above. The result, and the final statement of scope, belong in
Appendix A as **[M-20]**, once the audit actually completes.

### 13.1d The frozen sampling queue

**Why this exists.** Before this design, `xtask audit sample` reclassified
the whole `PATH` population — probing every one of ~2,300 tools — on
**every single draw**, costing roughly twenty minutes each time. Worse,
because the strata were recomputed from whatever the parser happened to be
on the day of the draw, two draws taken weeks apart were stratifying
against two different definitions of "ok", so a grammar fix silently
redefined what an already-drawn `audit sample` run had even measured, and
successive draws were not directly comparable.

**The fix: freeze the tool list once, walk a cursor through it.**
`xtask audit freeze` sweeps `PATH` (or a pinned `--tools` list) exactly
once, classifies every tool, shuffle-stratifies the result with a recorded
seed, and writes the ordered queue to `audit/queue.toml`. `xtask audit
sample` then only ever advances that queue's `cursor` by `--sample` tools
and merges the slice into a verdict file — no re-probing, no
reclassification, at draw time. **This is deliberately not implemented by
cross-comparing already-reviewed tools against the current tool list at
draw time** — no set-difference against "what's been done" computed
against a population that drifts as software is installed or removed. The
queue is ordered once, and a cursor advances through it; nothing about a
draw depends on which tools any verdict file has already recorded, which is
what makes repeated draws directly comparable and makes "same queue, same
cursor position" a deterministic, testable guarantee (`xtask/src/queue.rs`
tests this directly: the same `(queue, cursor)` pair always yields the same
tools, and successive draws never overlap).

**Three additions from external review, all implemented:**

1. **Freeze date and a population hash in the manifest.** `queue.toml`
   records `freeze_date` (`YYYY-MM-DD`) and `population_hash` — a stable
   FNV-1a fingerprint over the sorted, deduplicated tool list — so a queue
   can be identified and staleness detected. `xtask audit freeze --check`
   re-hashes the *current* `PATH` population (a directory listing, no
   probing) and reports drift against the frozen hash without touching
   anything, the same "report, don't rewrite" shape `coverage --check`
   already uses.
2. **Shuffle-stratify at freeze time**, not just concatenate strata. Each
   stratum is independently seed-shuffled, then every item is given a
   fractional rank (its position within its own stratum, normalized to
   `(0, 1)`) and the whole population is merged by sorting on that
   fraction. Because every stratum's ranks are spread evenly across
   `(0, 1)`, cutting the merged order at any point yields, from every
   stratum, very close to that same fraction of its own items — so **any
   prefix of the frozen queue is itself a valid, proportionally stratified
   sample**, not just the queue as a whole. (Concatenating shuffled strata
   instead — all of "ok", then all of "low-confidence" — would make the
   *total* order proportional but make an early prefix 100% one stratum,
   exactly the property this rules out.)
3. **Freeze the captured raw help text alongside the tool list.** This is
   the change that actually matters: much of the twenty-minute cost was the
   cost of *probing* (subprocess spawns and their timeouts), not of
   classifying. `xtask audit freeze` persists every `(argv, output)` pair
   each tool's extraction pass recorded — not just the root `--help`
   capture, but every probe a framework's protocol needed (cobra's
   two-probe shape included) — under `audit/queue-captures/`. `xtask audit
   reclassify` replays those bytes through the real extraction pipeline via
   `mandible_extract::exec::Transcript` (the same replay seam the corpus
   regression runner uses) and recomputes every tool's stratum against the
   *current* parser, with **no `PATH` sweep and zero subprocess spawns**,
   running every tool's reclassification in parallel via `rayon` — measured
   during this batch's own build (a real, naive serial first attempt over a
   real 500-tool `PATH` slice on a 4-core evaluation machine took *longer*
   than the parallel live-probing freeze it was meant to replace, 135s
   versus freeze's own ~123s on the same population, which would have made
   an unqualified "fast" claim false; parallelizing recovered roughly half
   that, ~65s). **The honest claim is therefore narrower than "seconds
   regardless of scale":** what removing every subprocess spawn
   unconditionally buys is no `PATH` sweep and no probe-timeout cost: what's
   left is real CPU-bound work — parsing plus the native/cobra artifact
   tier's own binary-byte scan of each tool's on-disk executable — that
   scales with population size and available CPU cores, not with a probe
   count times a timeout. That is still a meaningful, measured win (roughly
   half the wall-clock of a live re-probe on this evaluation machine, with
   zero subprocess risk), and it is what removes the "later slices are
   drawn from stale strata" caveat entirely,
   rather than merely disclosing it: a stratum label can be kept current
   against a newer parser without ever re-sweeping `PATH`.

**Storage: `audit/queue.toml` is tracked; `audit/queue-captures/` is not.**
Same convention `audit/*.toml`/`audit/force-include.txt` already set
(tracked) versus `audit/*/fixtures/` (gitignored). `queue.toml` is small —
one line per tool, a name and a short stratum label — and is *evidence* for
a claim about how the queue was built, the same "a measurement's evidence
lives in git, not on one contributor's laptop" reasoning this section's own
Appendix A discipline already applies to `audit/<seed>.toml`. The captures
are real bulk (one small file set per frozen tool, on the order of several
thousand files for a full-`PATH` freeze) and, critically, **machine-generated
content** — exactly the category the fixture-promotion workflow
(`corpus/README.md`) already treats as something that must never land in a
tracked human-verdict file, a rule that has already cost this project a
cleanup once. Captures are regenerable by re-running `xtask audit freeze`
locally; a queue worth reusing across machines is expected to have its
captures rebuilt locally, not shipped in the repo.

**`--tools` moved from `sample` to `freeze`.** Before this design, `xtask
audit sample --tools <list>` pinned a fixed, reproducible population for
tests and CI. Since `sample` no longer touches `PATH` at all — only
`freeze` does — the flag moved with the sweep it pins. `sample`'s own
`--seed` changed meaning to match: it no longer seeds a draw (the draw's
only randomness is spent once, at `freeze` time, via its own `--seed`), it
only names which verdict file (`audit/<seed>.toml`) the slice is merged
into. Force-include is unaffected either way: it was already independent of
the population, and stays independent of the queue's cursor for the same
reason.

**Honest caveats, stated rather than merely implied:**

- **A frozen population drifts from the machine's real installed tools over
  time.** `xtask audit freeze --check` detects this cheaply, but detecting
  drift is not fixing it — a stale queue still reflects the tool set at
  freeze time until re-frozen. A frozen queue is a snapshot, not a live
  view of "everything on this machine right now," and any sample drawn
  from it should be read that way.
- **Reclassification updates a tool's reported *stratum*, never its
  *position* in the queue.** The shuffle-stratified order was computed
  once, from the strata as they stood at freeze time; recomputing strata
  later (`xtask audit reclassify --update`) can change what stratum a tool
  is *reported* under without re-shuffling where it sits in the cursor
  order. A queue reclassified long after freezing may therefore no longer
  interleave in exact proportion to its *current* stratum composition, only
  to its composition at freeze time — a real drift, but a much smaller one
  than the staleness this design replaces, since the frozen order still
  visits the same tools in the same sequence regardless, keeping successive
  draws comparable.
- **Reclassification still depends on the tool binary resolving on `PATH`
  at the same path.** The native/cobra framework-detection tier
  (`mandible_extract::framework::artifact`) reads a binary's own bytes
  directly off disk to fingerprint it, not from the frozen capture — a tool
  uninstalled since freeze time will report a degraded stratum for a reason
  unrelated to any parser change. This is a file read, not a process spawn,
  so it never reintroduces the *subprocess* cost this design removes — but
  it is measurably not free either, and is plausibly a real share of the
  CPU-bound cost measured above (scanning a large on-disk binary for byte
  markers, once per tool, is real work). Either way, a `reclassify` report
  is only purely "what changed in the parser" when the machine's installed
  tools are also unchanged since freeze.

**No new execution-safety surface.** `xtask audit freeze` issues exactly
the same probes `xtask audit sample`'s old live sweep already issued, all
through the existing `run_inert` chokepoint (spec §6) — nothing about this
design broadens argv, adds a probe shape, or touches the never-probe list.
`xtask audit reclassify` spawns nothing at all: it is a pure replay of
already-captured bytes. The stratum a tool is classified into is still
computed by the same general, framework-keyed parser every other instrument
in this project uses (spec §1) — freezing the queue changes *when*
classification happens, never *how*.

### 13.1e Family detectors and the calibration precondition

A **family detector** generalizes one human finding across the fleet. The
audit (§13.1c) is slow and bounded: a human reads one tool's real output,
one tool at a time, and 94 of them took a full review session. A detector
takes the *shape* that human found — `[-abcXYZ]` collapsing into one flag
with the rest as a value — and asks whether it occurs on each of ~2,300
`PATH` tools, in seconds. `xtask detector` is the harness they register in
(`xtask/src/detector.rs`).

**A family detector is not a correctness instrument and does not need to
be.** The audit remains the only instrument in this project that touches
truth, because only there did a human compare output against the tool's own
reality. A detector's claim is narrower: *this same shape occurs here too*.

That narrowness is exactly where the danger is. A detector produces a
confident fleet-wide number — *"814 tools exhibit this defect"* — and
nothing inside that number knows whether the detector fires on the defect it
names. This project has already shipped that mistake twice with metrics
(§13.1b): [M-10]'s fabricated `tar` nodes *inflated* `%described`, and
`%flags_text` carried a name that read as an accuracy claim it never earned.
A detector is the third instance of the same shape, and the rule that
follows is stated as a precondition rather than a recommendation:

> **A detector's fleet-wide number is not quotable until it has passed
> calibration against the human labels: it must fire on the known-bad tools
> and stay silent on the known-good ones.** A detector that has not passed
> this check is measuring itself. One that has is an amplifier of a verified
> human judgment.

**The labelled set, and why it is a weaker claim than the verdicts.** A
verdict is `correct`/`wrong`/`incomplete` overall; it does not say which
defect family a wrong parse belongs to, and a detector for one family cannot
be calibrated against "wrong in general". `mandible_core::audit::Entry`
therefore carries `families` — a list from the closed `DEFECT_FAMILIES` set
— alongside `families_derived`, which records that those labels are a
**machine reading of the reviewer's note plus the fixture evidence**, not
the reviewer's own classification. The distinction is the same one
`verdict_scope` exists for and the same one §13.1b's fifth rule demands: a
claim must be labelled with its real strength. `families_derived` is an
`Option<bool>` and not a plain `bool` precisely so its *absence* cannot read
as "a human said so" — labels with no recorded provenance are refused, as
are labels on a `correct` or `skip` verdict, which would put a tool into a
detector's expected-fires set on a verdict saying nothing is wrong with it.

**Families are shapes; tool names are data** (§1). The set was derived from
the seed-2 notes rather than drawn up in advance, and no family is in it
without a reviewer's note behind it — a family with no labelled member
calibrates nothing and only makes the set look more complete than the
evidence supports.

**A family name can turn out to cover more than one shape, and then it must
be split rather than detected.** Two names were taken up together because
both looked like they were about how a bracketed or braced value spec is
read; only one of them survived the reading.

`brace-alternation-flag` is **one shape** with three renderings, and the
detector and the fix are both single rules: a `{...}` or `[...]` group whose
`|`-separated members are bare flag spellings. `cache_restore`'s
`{-i|--input} <input xml file>` reaches the grammar through an options
table, `eqn`'s `{-v | --version}` through a spaced synopsis group, `xfs_io`'s
`[[-c|-C] cmd]...` through a nested one; one predicate
(`grammar::parse_flag_alternation`) closed all three, and all three fixtures
flipped on the run that landed it.

`value-name-mangled` is **not one shape**. Its five labelled tools are at
least four unrelated defects, sharing only the *symptom* that `value_name`
came out wrong:

| tool | as written | what is actually wrong |
|---|---|---|
| `apt-ftparchive` | `-s=?` | the `=?` placeholder convention |
| `expand` | `-t, --tabs=N` | a second accepted value form documented only in the description prose |
| `pastebinit` | `-b <pastebin> (default is 'dpaste.com')` | a trailing parenthetical default beside the value |
| `sg_sanitize` | `--count=OC\|-c OC` | an alias alternation whose members each restate the value |
| `update-xmlcatalog` | `--root  = the root XML catalog` | `=` used as a *description* delimiter, read as a value assignment |

A single detector over that list would fire on whatever the author happened
to encode and miss the rest, and its fleet number would name a population no
one could check — the precise failure §13.1e's precondition exists to
prevent, arriving through the *label* rather than through the detector.
The name should be split before any detector is built for it. Note that the
one entry adjacent to the alternation family, `sg_sanitize`'s
`--count=OC|-c OC`, is deliberately refused by `parse_flag_alternation`'s
member rule and asserted as a must-stay-silent self-check: nothing on its
shape says whether one value or two are meant, so claiming it would trade a
known miss for a possible fabrication.

**Unclassified is a recorded state, not a gap to fill.** A judged defect
whose note nobody could confidently sort — a hedged by-reference note with
no fixture to check it against — carries no label, and both `xtask detector
list` and every calibration report print that count. A visible hole bounds
how complete any family's calibration can be; a hole papered over with a
guess silently corrupts a cell of the matrix.

**The confusion matrix has five cells, not four.** Beyond fires-on-bad,
misses, silence-on-good and false alarms, there is *fires on a tool judged
defective of a **different** family*. That is neither a hit nor a false
alarm: the human already said this parse is wrong, so a fire there may be a
mislabel or a genuine second family. Counting it as a false alarm understates
the detector; counting it as a true positive overstates it. Every cell names
its tools, because a disagreement is only useful if a human can go look at
it.

**Not-evaluable is counted, never dropped.** Calibration replays each
audited tool's `corpus/<tool>/audit-seed2/` fixture — the same frozen-bytes,
zero-subprocess replay §13.2 uses, so calibration spawns nothing. A labelled
tool with no fixture is listed by name rather than omitted: a "perfect"
matrix computed over half the labelled set is a worse claim than an
imperfect one computed over all of it.

**A detector may legitimately be uncalibratable, and says so.**
`Detector::family` returns an `Option`, and `None` means "generalizes no
family this labelled set contains". Both existing fleet oracles return it:
across all 94 verdicts, **not one reviewer reported a fabricated subcommand
or flag spelling**, so the existence oracle — the instrument built for
[M-10], this project's worst shipped defect — can be neither confirmed nor
refuted here. That is a property of the sample, not of the oracle, and
reporting it is the honest result. Forcing such a detector onto the nearest
family would manufacture a matrix out of a correspondence nobody verified,
which is the original defect one level up.

**Every report states its own limits, in full, every time.** Not a footnote
and not abbreviated on repeat runs: calibration is against *derived labels*
over the audit's judged tools — a bounded sample of roughly 4% of `PATH`,
not the fleet, and not ground truth about the fleet. Passing means a
detector works on those tools; it says nothing about whether its fleet-wide
count is right. This is the same discipline the coverage scoreboard's
literal `accuracy: unmeasured` line enforces (§13.1b), for the same reason:
a number travels without its context unless the context is printed beside
it.

**`wrong` versus `incomplete` must never become load-bearing.** The boundary
between the two words is thin, and the maintainer has flagged that it may not
have been drawn consistently across the 94 seed-2 verdicts — plausible for
any single reviewer working alone across a full session. Nothing in this
project currently depends on which of the two a tool got, and nothing
should: no consumer here read the boundary carefully enough for it to bear
that weight. Checked directly rather than assumed: `accuracy_over`
(`xtask::audit`) counts `correct` against everything else, collapsing
`wrong` and `incomplete` into one "judged defect" bucket; `verdict_requires_note`
obligates a note under the identical rule for both; `cmd_fixtures` emits the
identical `[xfail]` shape from one shared `"incomplete" | "wrong"` match arm;
and a family label (this section) is derived from the reviewer's note text
plus the fixture evidence, never from which of the two words the reviewer
chose — `unmodeled-help-shape` labels both a `wrong` entry (`ssh-keygen`) and
an `incomplete` one (`mariadb-repair`) with no distinction drawn anywhere
downstream. If a later change ever needs to prioritise which judged defect
to fix first, the ranking is by **family** (does this shape recur across the
fleet) and by **detector count** (how many tools a calibrated detector
actually names) — never by which of the two verdict words a reviewer
happened to type. Prioritising by the word itself would retroactively make
an inconsistently applied distinction load-bearing, which is exactly the
kind of claim this project's verdicts were never built to support.

**A fixed family inverts its own calibration, and the precondition must be
read accordingly.** The bundled-short-flag detector is the first one whose
family was actually repaired, and the moment the grammar landed its
calibration went from 4 hits to **0% recall**, naming `tcpdump`, `tmux`,
`filefrag`, `xfs_io`, `ssh-keygen` and `eqn` as misses. Nothing about the
detector changed; those six fixtures simply parse correctly now, so the
labelled set has nothing left to confirm against. The precondition above is
a claim about *labels recorded against a particular parser*, and it expires
for a family on the commit that fixes it. Two things carry the weight
afterwards, and both are cheaper than re-auditing: the detector's own
hand-built tests, which construct the defective shape directly and assert
the rule still fires on it, and `sweep-diff`, which is the instrument that
actually answers "did fixing this break anything else". A detector reading
zero because the bug is gone and a detector reading zero because it stopped
working are indistinguishable from the fleet number alone — so the
distinguishing evidence has to live in tests, and the fix's own commit
should say which.

**A repaired family is reported as repaired, and the report carries its own
evidence.** Calibration has three verdicts, not two. `REPAIRED` is reached
only when calibration has *inverted* (nothing labelled fires any more, and
there was something to fire on) **and** the detector's own hand-built cases
still hold. Those cases are `Detector::self_checks` — promoted out of
`#[cfg(test)]` and onto the trait precisely because neither consumer runs
under a test harness — and each names the exact number of findings the
detector must report on a hand-built input. The list is only evidence if it
covers **both directions**: at least one case the rule must fire on, because
a deleted detector satisfies every must-stay-silent case, and at least one it
must stay silent on, because a detector firing indiscriminately satisfies
every must-fire case. An empty list is refused rather than passing
vacuously. A detector with no self-checks can never be called repaired.

**REPAIRED is a stated claim, never a suppression**, and the distinction is
the whole point: "the family was repaired" is otherwise the perfect excuse
for a broken detector. So nothing moves between cells to reach it. Recall
still reads 0%, every missed tool stays counted in the FALSE-NEGATIVE cell
and stays named, the declared out-of-scope miss still prints in red, and the
self-check block prints on *every* run — including the ones that do not
reach REPAIRED — so the first time a reader sees the evidence is never the
run where it is being used to excuse a zero. A false alarm blocks REPAIRED
exactly as it blocks PASSES. And an inverted matrix whose self-checks did
*not* hold gets its own loud verdict naming it as the dangerous case, rather
than being rendered as an ordinary failure.

**A ratchet gate asserts the detector alongside the count.** Once a family
is repaired its fleet count is gated at zero (`coverage --check`, via
`detector::ratchet_at_zero`) — against a literal `0` rather than against the
checked-in scoreboard, which is editable and would otherwise let a
reintroducing commit raise its own baseline. The gate's second half is not
optional: **a gate asserting `count == 0` is satisfied by deleting the
detector**, which is [M-10]'s "a metric improved by breaking the thing that
measures it" one level up. So the gate requires the same self-check evidence
the REPAIRED verdict does, and refuses a zero without it. Verified by
attacking it: with `bundling::detect` returning an empty report and the fleet
count at a perfect 0 tools / 0 destroyed flags, `coverage --check` exits 1
and names the six cases that stopped firing.

**A declared scope exclusion must carry a structural predicate, not prose.**
`Scope::known_exclusions` was `(tool, &'static str)`, and adding an entry
silently converted a blocking false negative into a non-blocking named miss
with nothing checking that the sentence named a property of the *shape*.
That was the last goalpost-moving lever, so the reason is now a closed
`Ground` enum carrying a **witness** — the literal token from the tool's own
help text — plus the constant it falls below, referenced rather than
retyped. The arithmetic is computed from the witness and has to agree:
`ssh-keygen`'s `-hU` swallows one member, below `MIN_BUNDLED_MEMBERS = 2`,
so it holds; an author trying to exclude `tcpdump` would have to supply
`-AbdDefhHIJKlLnNOpqStuUvxX#`, whose 25 members are below no threshold, and
the entry is refused as a false negative rather than an exclusion. A
threshold of zero and a witness that is not a cluster token are refused too.
Prose survives as a `note` printed *beside* the generated structural
sentence, never instead of it. A new kind of exclusion means a new `Ground`
variant with its own predicate — a reviewable change to the vocabulary,
which typing a new sentence into a `&str` was not.

### 13.2 Fixed corpus

A fixture (`corpus/<tool>/<version>/`) freezes **both halves** of one
extraction pass: the raw bytes a real probe produced, byte-exact
(`.gitattributes` marks everything under `corpus/` `-text` specifically so
Git's own CRLF/whitespace normalization can never be the thing that quietly
"fixes" a capture), and the `CommandNode` tree mandible's actual tiered
pipeline produces from those bytes today — replayed with **zero
subprocesses**, through the same `Transcript` seam §13.1c's and §13.1d's
own replay uses. **Snapshotting only the IR is not enough**: an IR-only
snapshot can only assert "the tree once looked like this," and a
tool-version bump or a grammar rewrite leaves nothing to re-derive from.
Keeping the raw capture beside it turns every fixture into a live
regression check against whatever the parser does *today*, forever — not a
frozen fact about a parser that no longer exists. There is no more
per-tier bucketing here: §7's Tier A (a vendored catalog) is removed, and
a fixture is no longer filed by which tier resolved it, only by tool and
version — `corpus/README.md` has the full layout and the `meta.toml`
contract (a *descriptive* half, `expected.snap`, that `--bless` rewrites
wholesale, and a *normative* half, `[contract]`, that only an explicit,
reviewed edit may weaken) and the `lsof` cautionary tale
(`corpus/lsof/4.95.0`, `[xfail]` again after being blessed once without
the raw-text-side-by-side review `--bless` does not itself perform).

**`verdict_scope` records which dimensions of the tree a human actually
looked at before blessing it** — some subset of `"flags"`,
`"subcommands"`, `"descriptions"`, `"usage"`. **Absent means no scope was
claimed, never every scope**: a bless freezes every field in the tree
whether or not a human read it, so treating silence as "everything
verified" would let exactly the overclaim `lsof` cost this project
survive by omission — the conservative reading is deliberate, since it is
always safe to add a truthful claim later and never safe to have quietly
claimed one that was not made. Fixtures promoted from the seed-2 human
audit (§13.1c) carry `verdict_scope = ["flags", "subcommands"]`, matching
that audit's own declared scope: the reviewer judged structure, never
prose.

**Strict xfail, in the direction that matters here: an `[xfail]` fixture
whose snapshot and every `[contract]` field now pass fails the run.**
`cargo xtask corpus` does not read that as success — a fixture marked
broken that quietly stops being broken means the bug appears fixed and the
fixture is stale, and the run demands it be promoted (`[xfail]` removed,
the now-passing `expected.snap` kept) rather than staying silently green
under a label that no longer applies. This is how a fix announces itself.
The bundled-short-flag grammar fix (§13.1e) is the worked case: three of
the six fixtures its own family originally judged `wrong` in the seed-2
audit — `tcpdump`, `tmux`, `filefrag` — flipped from `[xfail]` to passing
the run that landed the fix, exactly because leaving them labelled broken
would itself have failed; `xfs_io`, `ssh-keygen`, and `eqn` stayed
`[xfail]`, their own `must_contain_flags` gaps unrelated to the collapse
this particular fix closed. Two of those three have since been promoted by
the *next* family's fix (`brace-alternation-flag`, §13.1e), which is the
mechanism working exactly as intended: `xfs_io`'s gap was
`[[-c|-C] cmd]...` and `eqn`'s was `{-v | --version}`, both of them the
alternation family rather than the bundle one, and both fixture comments
said so in words while they were still red. Both directions are checked on every run, not
only the "did it get fixed" one: a fixture claiming to be broken while
every check quietly passes is exactly as much a bug as an unmarked
regression.

**Current scale: 81 fixtures — 51 passing, 30 `[xfail]`, 0 unexpectedly
failing.** Ten are hand-captured against a real installed version (`git`,
`tar`, `curl` — two versions, `du`, `gcc`, `ffmpeg`, `lsof`, `unzip`,
`zoxide`); the other 71 are `audit-seed2` fixtures, `xtask audit fixtures`
turning a seed-2 human verdict directly into a fixture (`correct` → a real
`expected.snap`, `wrong`/`incomplete` → `[xfail]` with the reviewer's note
as `reason`, §13.1c). The corpus is now substantially the audit's own
output, staged and promoted, rather than a hand-curated tier list — see
`corpus/README.md` for the fixture layout, the full `meta.toml` contract,
and the contribution workflow (a fixture-only PR needs no Rust).

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

**The workspace runs under `cargo nextest run --workspace` in CI**, never
`cargo test --workspace` piped into a text-processing tool. The rule behind
this is not that nextest is faster: it is that human-format test output must
never be parsed, by anyone, for any reason. The concrete failure, self-reported
(AGENTS.md §3.3): `grep -c FAILED` against `cargo test`'s output false-positived on
test *data* that happened to contain the literal word "FAIL" (fixture text,
a variant name, a snapshot value all share one output stream with no
structural separation from the test runner's own report), producing a
confident, wrong pass or fail count. `cargo nextest run` reports a real
nonzero exit code on any failure and can emit `--message-format
libtest-json` when a structured result is actually needed. Read that, or the
exit code, never the prose. Nextest cannot run doctests (an upstream
limitation), so CI runs a separate `cargo test --doc --workspace` step to
cover them.

### 13.4 The detect-to-fix loop, end to end

§13.1–§13.2 introduce five instruments at five different points, each for its
own immediate reason, and nowhere states how they compose. They do, in this
order:

1. **Corpus fixtures** (§13.2) — per-document. Frozen bytes plus the tree
   they should produce; `cargo xtask corpus` catches a regression on one
   tool someone already looked at, replayed through the real pipeline with
   zero subprocesses.
2. **Sweep-diff** (`xtask sweep-diff`) — fleet-wide, not per-document: a
   semantic diff between two full-`PATH` scoreboards, gains and losses
   always reported as two separate totals, never netted, because summing
   them hides exactly the losses that motivated building it — two grammar
   fixes shipped regressions (228 flags across 72 tools; 6 on `lsof` plus 34
   across four more) that both the aggregate `%flags_text` gate and the
   whole corpus stayed green through, caught only by a human diffing a
   before/after sweep by hand. It is the instrument that answers *did fixing
   this break anything else*, and non-blocking by construction (maintainer
   decision D4): `cargo xtask sweep-diff` always exits `0`, and there is no
   `--check`/`--gate` flag to wire to a nonzero exit by accident.
3. **Oracles** — existence and misattribution (§13.1) — fleet-wide
   self-consistency checks: does every extracted name occur in the tool's
   own captured text (existence, built for [M-10]'s fabricated `tar`
   subcommands), and is a description attached to the flag it actually
   describes (misattribution, built for `lsof`'s column-bled options table).
   Neither compares against the tool's real behavior; both re-examine text
   the pipeline already captured.
4. **Audit** (§13.1c) — sampled, and the *only* instrument in this list that
   touches truth. A human reads a tool's own raw `--help` text beside the
   parsed tree and judges it. Everything above this line is
   self-consistency — internally coherent output can still be uniformly
   wrong — which is why the audit exists at all, on 94 tools so far
   (the seed-2 sample).
5. **Family detectors + calibration** (§13.1e) — generalizes one human
   finding across the fleet. A detector encodes the *shape* a human found
   wrong and checks every `PATH` tool for it in seconds; its fleet-wide
   count is not quotable until calibrated against the audit's own labelled
   verdicts — it must fire on the tools the audit called defective for that
   shape and stay silent on the ones it called correct.

**The loop these five compose into:** a human audit finding gets a **family
label** (one of `DEFECT_FAMILIES`, derived from the reviewer's note plus
fixture evidence) → a **detector** generalizes that label's shape across the
fleet → the detector is **calibrated** against the labelled verdicts (fires
on known-bad, silent on known-good) → only once calibrated does its
fleet-wide count become **quotable** → the count motivates a **grammar fix**
in `mandible-extract` → the fix makes the family's **xfail fixtures flip** to
passing, which `cargo xtask corpus`'s strict xfail (§13.2) reads as a demand
to **promote** them rather than a quiet pass → **sweep-diff** runs a
before/after full-`PATH` sweep to prove the fix broke nothing else → the
detector's fleet count is **ratchet-gated at zero** going forward, so any
future regression in that family is visible the moment the count leaves
zero.

**The worked example is the bundled-short-flag family, run start to finish
this week.** The seed-2 audit judged five tools `wrong`/`incomplete` for a
synopsis cluster like `[-AbdDefhHIJKlLnNOpqStuUvxX#]` collapsing into one
flag (`-A`) with every other letter glued on as its value — `tcpdump` losing
25 real flags this way, `xfs_io` 10, `tmux` 7, `filefrag` 7, `ssh-keygen` 1
— plus a sixth, `eqn`, labelled `bundled-short-flag` among several
overlapping families in its own audit note. The detector
(`xtask/src/bundling.rs`) generalized that shape and, on the same full
`PATH` sweep the audit's queue was frozen from (2,302 tools), reported **58
tools with a collapse, destroying 465 real flags** — a number that became
quotable only once every one of the 58 was checked by hand against its own
captured text and no false positive turned up. `help_text::grammar::
parse_bundled_shorts` then read the same synopsis cluster as the *set* of
switches it actually is, and the identical sweep that had measured 58/465
came back at **0 tools, 0 destroyed flags**; `sweep-diff` across the
before/after scoreboards showed 0 flag-count losses against 489 flags
*gained* across 67 tools (nine more than the 58 — the text-versus-tree gap
§13.1e's own doc comment explains: a cluster whose first member also
appeared in an ordinary options table never survived into the tree for the
detector to see, but the fix, reading the raw synopsis directly, recovers it
anyway). Three of the six originally-labelled fixtures — `tcpdump`, `tmux`,
`filefrag` — flipped from `[xfail]` to passing in the run that landed the
fix and were promoted (§13.2); `xfs_io`, `ssh-keygen`, and `eqn` remain
`[xfail]` for gaps this particular fix did not close.

**And the fix inverted its own calibration** — §13.1e states this as the
general rule; this is where it was first observed. Before the fix,
calibrating this detector against the labelled set reported 4 hits; the
moment the grammar landed, the identical calibration run reported **0%
recall**, naming all six labelled tools — `tcpdump`, `tmux`, `filefrag`,
`xfs_io`, `ssh-keygen`, `eqn` — as misses, because every one of those
fixtures now parses correctly and the labelled set has nothing left to
confirm against. A detector reading zero because the bug is fixed and one
reading zero because it silently broke are indistinguishable from the fleet
number alone once that happens; what carries the weight afterward is the
detector's own hand-built tests (which construct the defective shape
directly, independent of any tool ever having exhibited it) and `sweep-diff`
against a fresh full sweep — not the calibration number, which has nothing
left to say once its family is fixed.

---

## 14. Dependency table

| Purpose | Crate | License | Notes |
|---|---|---|---|
| TUI framework | `ratatui` | MIT | `crossterm` backend; mouse support |
| Display width | `unicode-width` | MIT/Apache-2.0 | Required for correct truncation (§9) |
| Fuzzy matching | `nucleo` | MIT/Apache-2.0 | Powers Helix |
| mandible's own CLI | `clap` + `clap_complete` | MIT/Apache-2.0 | `--completions <shell>` emits a real completion script, which Tier C then parses — mandible parsing itself |
| Help-text grammar | `winnow` | MIT | Preferred over `pest` for error recovery |
| Completion script AST | `brush-parser` | MIT | **Replaces `conch-parser`**, which is unmaintained and emits a future-incompat rejection warning today [M-9]. Avoid `yash-syntax` (GPLv3). |
| Man page AST | *(none — hand-written)* | — | **No `bindgen`/vendored mandoc.** Revision 2 specified `libmandoc` via FFI; superseded, because it is not a system library on Linux [M-6] and `#![forbid(unsafe_code)]` rules out the FFI regardless. §7 Tier D is a pure-Rust subset parser over `.TP`/`.IP` + `.B` and `.It Fl` [M-14]. |
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

### Maintainer decisions, recorded so they are not re-litigated

**A tool that returns its root help for every subcommand is shown as-is
(2026-08-12).** After [M-19], every `systemctl` subcommand's verbatim pane
shows the same root help, because `systemctl <verb> --help` genuinely returns
it. A special-case message ("this tool returns its root help for every
subcommand") was proposed and **declined**: if that is how the tool behaves,
showing it is honest, and a reader seeing identical text across 18 subcommands
can draw the conclusion without being told. **Do not re-propose.** The one
residual is that each degraded node keeps its own copy of that text instead of
sharing one, which is a memory cost rather than a correctness one, and does not
justify a special case either.

**Enrichment by authority merge is off by default (2026-08-11).** Shown a
mockup of `git restore` with man prose merged into its 16 already-parsed
flags, the maintainer judged it "nice to have, but kind of defeats the
cleanliness, maybe as an opt-in later." This resolved a contradiction inside
this document rather than demoting an agreed plan: §7 Tier D described prose
backfill as enrichment via authority merge, while [M-14] specified it fires
only as a zero-confidence fallback, which never touches a tool that already
parsed. [M-14]'s reading wins. A tool whose only good documentation is a man
page stays shallow in the tree, and that is a stated limit rather than a bug
to chase.

### Deferred, with the reason each is not simply undone

**Sub-case (b) of the `-h` fallback is unmeasured and must stay that way until
it is measured on disposable infrastructure.** Sending `-h` to a *root* whose
own `--help` is man-shaped is unknown territory, distinct from the shipped
sub-case (a), where a well-behaved root's subcommands detour to man pages.
Six such roots are known. The measurement belongs on an ephemeral CI runner
inside a PID or user namespace under full §6 containment, instrumented on both
sides for files written, children spawned, exit code and wall time, and
recorded as a new `[M-n]` with method. **Never on a development machine.** The
standing posture is that an unmeasured argv broadening is refused: the burden
is on the measurement, not on the objection. Two hardenings ship with the
feature whatever its final scope, namely that `-h` output must itself pass
help-shape validation before being consumed, and that the exec-policy shim
suite covers both halves, the fallback being attempted for a permitted shim
and refused for a `pkill`-shaped one even when that shim's `--help` is
man-shaped.

**Resolved: `xtask audit sample` no longer reclassifies the whole `PATH` on
every draw.** `xtask audit freeze` now snapshots the tool list and its
classification once, into a shuffle-stratified queue, and `xtask audit
sample` just advances that queue's cursor — see §13.1d for the full design,
the storage decision, and the honest caveats a frozen population still
carries (population drift, and reclassification updating a stratum without
re-shuffling the queue's order).

**The invariant table in `AGENTS.md` is due a prune.** Its own maintenance
policy prefers making a mistake impossible over documenting it, and every
parser-lesson row that now exists as a corpus fixture should be deleted in the
same change that lands the fixture. The exec-policy rows stay, since shim
tests enforce them and the rows record why, though each is worth checking:
where the shim test's own comment carries the reasoning, the row can go too.

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
- **[M-14] What a man-page tier would actually recover, measured before
  building one.** Of the 724 tools the PATH sweep still rates weak (verbatim,
  no-tier, low-confidence, or zero/poorly-described flags), **164 have a man
  page whose flag-tagged entries outnumber what mandible finds today**, worth
  about **2,515 option entries**. 472 have a man page with no flag-tagged
  entries at all (prose only — roff would not help them), and 78 have no page.
  The gainers are not obscure: `ssh` 52 entries against 0 today, `bash` 162
  against 18, `ps` 58, `tcpdump` 82, `mdadm` 125, `dash`/`sh` 65.

  Three design constraints fell out of the measurement. **Do not gate on an
  `OPTIONS` section** — `bash`, `ps` and `tmux` document options under
  `DESCRIPTION`, and that gate alone cost 28 tools; gate on the *tag line*
  beginning with a flag, which is also what excludes examples (`ps` tags its
  examples with `.TP`, and a looser rule counted them, inflating the estimate
  to 276 tools / 4,923 entries before it was corrected). **Target man(7)
  `.TP`/`.IP` + `.B`, with `.It Fl` for mdoc** — only ~20 of these pages are
  mdoc, so an mdoc-first plan aims at the wrong majority; combined with [M-6]
  and `#![forbid(unsafe_code)]` ruling out C FFI, a pure-Rust subset parser is
  both smaller and unblocked. And **fire only as a zero-confidence fallback**,
  which keeps staleness away from the ~1,500 tools that already parse and
  avoids authority-merge questions entirely.

  Note also that [M-5]'s "31 man1 pages" is a bare-container artifact and
  understates real systems badly: the machine this was measured on has 9,515.

  2,515 is an upper bound — entries *present in the pages*, not entries a
  parser would extract cleanly.

- **[M-13] Artifact fingerprinting beats prose fingerprinting.** `strings` over
  the binary: `docker` contains `spf13/cobra` 583×, `gh` 283×; `git` and
  `ripgrep` contain zero (correctly — hand-rolled C, and ripgrep dropped clap
  for a custom parser). Meanwhile a help-text signature keyed on cobra's usual
  `Available Commands:` **missed `docker` entirely**, because docker prints
  `Common Commands:`.

- **[M-16] git's subcommands are recoverable today, via `-h` — and the man
  tier as designed would not recover them** (2026-08-10, git 2.43.0,
  aarch64). Found by running the TUI by hand, not by any gate.

  `git <sub> --help` does not print help: it execs `man` and renders
  `GIT-COMMIT(1)`. The man-page banner check catches that and correctly
  degrades to verbatim (§7 Tier B step 3), so **all 22 of git's listed
  subcommands render as raw roff output** while the root parses cleanly.

  **[M-14]'s design does not reach them.** It targets man(7) `.TP`/`.IP`
  + `.B`. git's 184 `git-*.1` pages contain **zero `.TP` macros** — they are
  asciidoc-generated and mark options as bold-run paragraphs
  (`\fB\-\-amend\fR`, 2,426 occurrences, an inflated upper bound since
  inline cross-references repeat). So the tier spec §7 Tier D names git as
  its highest-value case would recover approximately nothing from git.
  Reconciling Tier D with [M-14] must also account for the asciidoc-
  generated dialect, which is a *generator*, not a tool — §1-clean, and the
  same unit of knowledge Tier A′ already parses by.

  **The cheap path is a probe-ordering rule, not a new tier.** `git commit -h`
  prints an ordinary two-column option table the generic grammar already
  handles. Measured across the 22 listed subcommands: **501 option lines on
  21 of them** (only `bisect` yields none). mandible extracts **zero** today,
  purely because the `-h` fallback fires only when `--help` produced *no
  output on either stream* — and a man page is plenty of output.

  The fix is to treat *detected as a rendered man page* as "no usable help"
  and fall back to `-h`, reusing the banner detection that already exists.
  Keyed on an observable property of the output, never on the tool name. The
  one hazard is already contained: `-h` is an action flag on machine-state
  tools, and `HELP_ONLY_PROBE` restricts those to exactly `--help`
  regardless (§6 rule 0).

  **The exposure set, now measured** (2026-08-10, same machine — aarch64,
  2,266 tools on `PATH`; CI's x86-64 image carries ~2,832 and its list will
  differ, so the authoritative run is CI's). The coverage harness gained a
  `man` column that re-runs the existing banner check over text the pipeline
  **already captured** (`CommandNode::unparsed`), so the enumeration costs no
  new probe and no new argv shape — which matters, because measuring an argv
  broadening by performing one would be circular.

  **Six tools have a man-shaped root `--help`:** `byobu`, `byobu-screen`,
  `byobu-tmux`, `git-receive-pack`, `git-upload-archive`, `git-upload-pack`.
  All six currently render `verbatim` with zero flags. None belongs to the
  process-signalling or machine-state classes §6 rule 0 restricts, so the
  measurement campaign for the risky sub-case is six named binaries rather
  than a survey.

  **`verbatim` would have been a 50× overestimate as a proxy.** The same
  sweep reports `verbatim_count=314` against `man_shaped_count=6`: a root
  degrades to verbatim overwhelmingly because nothing parsed, only rarely
  because it printed a man page. Anything reasoning about man-page exposure
  from the verbatim column is wrong by a factor of fifty — the distinction
  has to come from the banner check itself.

  Adding the column changed no parse: `pct_described` 94.18%, `no_tier` 2,
  `suspicious` 1, identical to the run before it. `git`'s own root correctly
  reports **not** man-shaped, which is the true negative that matters — it is
  git's *subcommands* that print man pages, not its root.

- **[M-15] The declination tax: 378 tools report `ok` with zero flags**
  (2026-08-10, mandible 0.2.2 at 7384b6f, aarch64 — note this differs from
  the x86-64 baseline above, and that difference is itself an argument for
  pinning the sweep's environment). Of the 1,895 tools the full-`PATH` sweep
  rates `ok`, **378 (20%) carry no flags at all.**

  This is the exact counterpart to [M-10] and it is invisible to every gate
  §13.1 currently defines. [M-10] was *fabrication* — invented nodes inflate
  `%described`, which the structure-sanity column now catches. This is
  *declination*: a tool the grammar refused to read rather than risk
  fabricating from. It depresses no metric, because a node with zero flags
  has no described-ratio to report at all — the scoreboard prints `—` and
  the tool is excluded from the aggregate rather than counted against it. So
  every precision tightening in `sections.rs` (the apt-get prose rule, the
  mysqlslap same-indent rule, the curl usage-continuation rule) could pay
  for itself in recall elsewhere on `PATH` and nothing would say so.
  §13.1's own lesson applies to its own gate set: a metric a failure mode
  can slip past is worse than no metric.

  **Method:** the checked-in scoreboard, filtered to rows with status `ok`
  and a flag count of 0. Deliberately *not* re-derived by shelling out to
  378 binaries — probing outside `exec::run_inert` bypasses every §6 rule,
  so the finer breakdown below has to be computed inside the harness.

  **Not measured, and worth measuring in the harness:** how many of the 378
  publish flags only in a **usage synopsis**. Spot-checked by hand, that
  case is real and includes marquee tools — `git --help` documents
  `-v/--version`, `-C <path>`, `-p/--paginate`, `--git-dir=<path>` and eight
  more entirely inside its `usage:` block, and mandible extracts none of
  them (`help_text::sections::extract_positionals` mines that block for
  positionals only). `zipinfo` is the same shape. The class is not uniform,
  though, and the honest counterexample matters: `apt-get`'s zero flags is
  **correct** — apt 2.8.3's help has no options section at all — so the 378
  is an upper bound on what any single grammar could recover, not a target.

- **[M-17] The `/dev/tty` hazard behind the `mandible systemctl` freeze
  report, and what actually triggers it** (2026-08-11, systemd 255, less
  643, dash (Ubuntu `/bin/sh`), aarch64, Ubuntu 24.04). A user reported
  `mandible systemctl` freezing their entire TUI, with a pager observed.
  The working theory — `env_clear()` leaves `PAGER` merely *absent*,
  which lets a pager-searching tool go find `less` itself, combined with
  `process_group(0)` not severing the controlling terminal — turned out to
  be right about the mechanism and wrong about which half of it fires for
  `systemctl` specifically.

  **The `PAGER`-absence half does not hold, measured.** systemd's own
  pager gate checks `isatty` on its *own* stdout and stderr before ever
  consulting `PAGER`, and `run_inert` always makes both pipes. A sweep of
  all 74 `systemctl` verbs plus the root, each probed with both `--help`
  and `-h` under the exact sanitized environment `run_inert` uses (run
  inside `unshare -rpf --mount-proc` — read-only queries only, no
  privileged verb was actually dispatched, confirmed by identical output
  across every verb, see the second half below), found **zero** pager
  invocations: every one of the 148 probes produced byte-identical global
  help text to plain `systemctl --help`, with empty stderr and no
  descendant process of any kind. Confirmed at the `less`-binary level
  too, directly via `strace -f -e trace=openat,open`: `less` never
  attempts `open("/dev/tty")` once its own stdout is non-tty — true both
  with no controlling terminal available at all, and (built for this
  measurement, using `openpty()` + `login_tty()` to give a throwaway
  process a real pty as its controlling terminal, standing in for a real
  interactive session) with one genuinely available. `less`'s decision is
  keyed entirely on its own fds, not on whether a reachable controlling
  terminal exists.

  **The session half is real, demonstrated with a positive control.**
  Confirming `process_group(0)`'s residual risk needed a program that
  wants a controlling terminal *unconditionally*, since no argv this
  crate actually constructs against any known tool was found to do so in
  this environment. Built directly: a shim run under the same rig (real
  pty as controlling terminal via `login_tty`, probe spawned exactly as
  `run_inert` spawns it) that does nothing but `open("/dev/tty")`.
  **Under `process_group(0)` alone, it succeeds** — the descendant reaches
  the real controlling terminal, exactly the mechanism the bug report
  pointed at, regardless of which tool or argv triggers it. **Spawning the
  probe as the leader of a new session** (`pre_exec` + `setsid()`, this
  crate's one audited `unsafe`) **makes the same call fail with `ENXIO`.**
  This is `tests/exec_policy.rs`'s `dev_tty_hazard::
  probe_cannot_reopen_the_controlling_terminal` test, verified to fail
  against the pre-fix code and pass against the fix — the AGENTS.md §2
  discipline of a fixture that has actually been made to fail once, not
  prose alone.

  Building the positive control itself needed two non-obvious fixes,
  recorded here since they'd otherwise cost the next person the same
  half-day: (a) `openpty()` must happen *before* the worker becomes a
  ctty-less session leader, or a plain `open()` risks auto-acquiring the
  terminal ambiguously; and more load-bearing, (b) **the pty's master
  side must stay open** — closing it (even in a different process that
  merely held a copy of the fd) hangs up the slave, and `TIOCSCTTY`/
  `login_tty` on a hung-up slave fails with `EIO`, which is easy to
  misread as "the fix already worked" when it is actually a broken rig.

  **Net effect:** both fixes shipped regardless of which half caused the
  original freeze — the pager variables (rule 6) as defense-in-depth
  against a tool whose own pager gate is weaker than systemd's `isatty`
  check, and the session change (also rule 6) because it is the only one
  of the two demonstrated, by measurement, to actually close a real
  `open("/dev/tty")` path. The exact trigger on the reporting user's own
  machine remains unreproduced — this sandbox has no controlling terminal
  at all ([`AGENTS.md` §3.2](./AGENTS.md)), so the user-visible freeze
  itself could not be attempted directly, only the underlying mechanism.

- **[M-18] `systemctl <verb> --help` is safe — measured, not assumed**
  (2026-08-11, same environment as [M-17]). Prompted by rule 0's own
  precedent: `shutdown -h` looked harmless until measured and turned out
  to attempt the real halt, stopped only by polkit. `systemctl`'s verb
  list includes `reboot`, `poweroff`, `kexec`, `halt` and other
  machine-state actions, and the background tree warmer will probe
  `HelpLongForPath` (`systemctl <verb> --help`) for every one of them —
  `HELP_ONLY_PROBE` (rule 0) matches on the *binary's file name*, so it
  restricts the standalone `reboot`/`poweroff`/`halt`/`shutdown`
  multi-call symlinks but not `systemctl` invoked with one of those words
  as a verb.

  All 74 verbs from `systemctl --help`'s own listing, each probed as
  `systemctl <verb> --help` and `systemctl <verb> -h` under the sanitized
  environment (inside `unshare -rpf --mount-proc`), produced output
  **byte-identical** to plain `systemctl --help` — including `reboot`,
  `poweroff`, `kexec`, and `halt` — with empty stderr and exit 0 for
  every case. This is the opposite finding from `shutdown -h`: on
  `systemctl` (unlike its standalone multi-call-binary symlinks), `-h` is
  a genuine alias for `--help`, and GNU getopt's permutation intercepts
  it globally before verb dispatch, for every verb tested, not just the
  ones already covered by rule 0. No change to `HELP_ONLY_PROBE` or any
  other safety list is needed; this closes the concern rather than
  extending the list.

- **[M-19] The actual mechanism behind the `mandible systemctl` freeze:
  a self-similar background-warmer fan-out, not a pager and not `/dev/tty`**
  (2026-08-12, systemd 255, mandible at `fd5212f`, aarch64, 4-core sandbox
  with a working `systemctl`/PID 1 — not the bare container [M-5]/[M-17]
  measured in). [M-17] and [M-18] closed the tty-reachability and per-verb
  safety questions but left the freeze itself unreproduced. It reproduces.

  **Reproduced under `scripts/pty_screenshot.py`, which forks a real pty.**
  `mandible systemctl` under the harness: pressing `j` (move selection
  down) up to 75 times in a row produced **zero change on screen** — same
  frame, byte-for-byte, every capture — while `mandible git` under the
  identical harness updates on every single keystroke. Eventually (around
  the 75th key, ~45s of wall time on this machine) exactly one keystroke
  got through, then it froze again. Ruled out as a screenshot-tooling
  artifact by first confirming `git` visibly changes pane content (not
  just highlight color, which the plain-text capture can't see) on one
  `j`, and by checking with `ps`/`/proc` mid-run.

  **Failure shape: the event loop starved, not a blocked process.** While
  frozen, the `mandible` process was in state `S` (interruptible sleep,
  not `D`), and `ps` showed **132% CPU** across its threads — actively
  running, contending for the CPU, not blocked on a syscall. This is the
  fourth shape the investigation brief distinguished ("the event loop
  starved"), not a hung child, not a `/dev/tty` reopen, not corrupted
  terminal state — the terminal state and the process were both fine, the
  main thread just never got scheduled back around to `event::poll` and
  `term.draw` (`mandible/src/app_runner.rs`'s `run_loop`) for tens of
  seconds.

  **Root mechanism, confirmed at the shell and then via the extraction
  API directly.** `systemctl`'s GNU getopt permutes `--help` to the front
  of argv regardless of what precedes it, and `systemctl` never validates
  a verb before help dispatch — so `systemctl <anything...> --help`
  prints the tool's own root help, byte-identical, no matter how many
  words come first or what they are:

  ```text
  $ diff <(systemctl --help) <(systemctl preset-all get-default daemon-reload halt reboot --help)
  (no difference; exit 0)
  ```

  Calling `Runner::fill_node` directly (the same call the background
  warmer makes) against `systemctl`, `systemctl preset-all`, and
  `systemctl preset-all get-default` in turn showed all three reporting
  **the same 18 subcommands** — because `HelpTextTier::extract_node`
  (`mandible-extract/src/help_text/mod.rs`) has no way to know a probe's
  output describes the root rather than the node it was asked about, so
  it reads the root's "Commands:" section a second (third, fourth…) time
  and reports it as that subcommand's own children.

  **Why that becomes a freeze, not just wrong data.** The background
  `Warmer` (`mandible/src/background.rs`) cascades every discovered
  child's fill unconditionally — "every fill queues the children it just
  discovered" — with no cycle or self-similarity detection (unlike the
  cobra tier's own visited-set protocol, [M-2]). 18 children each
  (wrongly) reporting 18 children of their own is 18², then 18³ = 5,832,
  bounded only by `MAX_WARMED_NODES` (4,096) — a global submission cap,
  not a depth cap. Reaching it means thousands of concurrent
  `systemctl <phantom path...> --help` spawns queued on a
  16-thread pool (`available_parallelism() * 4` clamped to `[4, 32]`; 4
  cores here) plus their stdout/stderr reader threads, all contending
  with the single UI thread for the same 4 cores — enough scheduler
  pressure, measured, to starve a 100ms poll loop for 45+ seconds, which
  is exactly what a user experiences as "the TUI froze."

  **The fix: recognize the hazard from an observable property of the
  output, the same discipline [M-16] used for man-page detection — never
  the tool's name.** `HelpTextTier` now remembers each tool's root
  `--help` text (keyed by resolved binary path) the first time it probes
  the root, and compares every later subcommand probe's raw text against
  it. A byte-identical match degrades that node to verbatim (spec §7
  Tier B step 3 — "never fabricate, degrade to verbatim", the same
  existing machinery a structurally-implausible parse already uses,
  factored out as `verbatim_node`) instead of reporting the root's
  subcommands as its own: `children_filled: true`, `subcommands: []`, raw
  text still available to the `t` view. An empty `subcommands` list is
  what stops `Warmer::warm_children`'s loop from queuing anything further
  for that node, so the cascade halts at depth 1 instead of growing
  exponentially. This generalizes to any multi-call binary or getopt
  permutation with the same behavior, not just `systemctl` — the check
  never inspects the tool's name.

  **Verified no longer reproducing, same pty harness, same commands.**
  Post-fix, `mandible systemctl` under `pty_screenshot.py`: every `j`
  press updates the pane immediately (`preset-all` →`get-default` →
  `show-environment` → …, all 18 of the root's children, one per
  keystroke), matching `git`'s responsiveness exactly. A 40-keystroke
  stress run completed in the harness's own nominal wall-clock time
  (~26s for 40 keys at the harness's fixed pacing) with no stall at any
  point. `HelpTextTier::extract_node`, called the same way against the
  same three-level `systemctl` path directly, now reports 18 subcommands
  at the root and **0** at the first child (previously 18/18/18).
  Regression tests
  (`mandible-extract/src/help_text/mod.rs::tests::
  a_subcommand_probe_identical_to_the_root_does_not_fan_out` and
  `::a_subcommand_probe_merely_similar_to_the_root_is_parsed_normally`)
  pin both the positive case and the negative one (a subcommand that
  shares a preamble with the root but is not byte-identical still parses
  its own real flags) — the first, verified against this section's own
  discipline, fails against the pre-fix tier with the exact symptom
  above (`["preset-all", "get-default"]` reported as the child's own
  children) and passes against the fix.

  The full workspace test suite (584 tests, two more than the 582 this
  investigation started from) and all 6 corpus fixtures stayed green
  throughout — the guard is keyed on exact byte equality to the root, so
  it never fires for a genuinely distinct subcommand's genuinely distinct
  help text, corpus fixtures included.

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

**Post-revision-2 note (2026-08-11): `pct_described`'s denominator changed.**
Not a revision bump on its own, but worth recording here because it changes
what every historical scoreboard number *means* — a reader comparing an old
`coverage-scoreboard.txt`/`coverage-scoreboard.ci.txt` figure against a new
one needs to know the ratio itself moved, not just the tools underneath it.
`pct_described` was `described / total`; it is now `described / describable`
(§13.1b's metric design rules), excluding flags whose only source is a usage
synopsis (`mandible_core::Source::HelpTextSynopsis`, spec [M-15]) from the
denominator, since a synopsis carries spellings, never prose, by
construction. A scoreboard produced before this change has no
`describable_flags` field in its `# aggregate:` footer at all (it defaults
to `0.0` on parse, per `parse_aggregate_footer`'s doc comment) and its
`pct_described` was computed over raw `total_flags`; a scoreboard produced
after it has both fields, and `pct_described` is over `describable_flags`.
The raw flag count (`total_flags`, and the per-row `flags` column) is
unaffected either way and remains directly comparable across the change.

**Post-revision-2 note (2026-08-11, later same day): the column was renamed,
its ratio unchanged.** A second, independent change from the denominator one
above — worth its own entry because it changes what a historical
scoreboard's *column header* means, not what any number in it is. The
scoreboard's `%described` column and its `pct_described` aggregate/footer
field are renamed `%flags_text`/`pct_flags_with_text`: same computation
(`described / describable`, per the note above), same value on any given
scoreboard, only the name changed. The rename exists because `%described`
reads as an accuracy claim — "this flag's text is correct" — when all it has
ever measured is presence — "this flag has text attached" — and `lsof`
(`corpus/lsof/4.95.0`, `[xfail]`) is the proof the gap is real: it scored 79%
"described" while roughly a quarter of its flags carried a different flag's
description, misread from a three-column options table the generic parser
reads as one. See §13.1's note on the misattribution detector
(`xtask/src/misattribution.rs`), the instrument this incident motivated, and
§13.1b's added fifth metric-design rule on names as part of a metric's
design. A scoreboard written before this rename has `pct_described=` in its
`# aggregate:` footer instead of `pct_flags_with_text=`;
`parse_aggregate_footer` reads both, mapped to the same field, so `--check`
against an old baseline still works. Every scoreboard, old or new, also now
carries a literal `# accuracy: unmeasured` line — not parsed by `--check`,
just a standing, honest reminder that nothing here measures correctness yet.

**Post-revision-2 note (2026-08-12): the "anti-fabrication oracle" is two
checks, not one.** WS4 originally described a single instrument; building it
found that misattribution (`xtask/src/misattribution.rs`, added first) and
existence (`xtask/src/existence.rs`, added by this note) check different
things with different victims — see §13.1's own account of both. The
scoreboard gains an `exist` column (right after `misattr`, same tightly-packed
right-aligned style) and the `# aggregate:` footer gains
`existence_fabrication_tools=`, appended after
`misattribution_column_aligned_tools=` and before `total=`. A scoreboard
written before this change has neither key; `parse_aggregate_footer` defaults
`existence_fabrication_tools` to `0` on such a scoreboard, so `--check`
against an old baseline still works. `xtask/src/transition.rs`'s fixed-offset
row parser (`row_offsets`) was updated in the same change to recognize the new
column's width (`has_existence_column`, mirroring the existing
`has_misattr_column`) — without it, `sweep-diff` would have silently misread
every `status` field on any scoreboard carrying the new column, since that
field is sliced by trailing character offset, not by name. Caught by
strengthening `parses_a_freshly_rendered_scoreboard_back_out` to assert
`status`'s actual value (not just that a row with the right key exists)
rather than by any gate — worth recording here because the failure mode is
exactly the kind a presence-only test stays green through.
