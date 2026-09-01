# mandible — Design Specification

**A universal, interactive TUI reference for CLI tools, in Rust.**

> `mandible git` opens an explorable tree of every command, subcommand, and flag `git` has — with descriptions, not just names — plus a search bar that finds the flag you half-remember.

This document is the design reference and the build guide. Every claim about the
outside world in this document has been measured on a real machine; measurements
are collected in [Appendix A](#appendix-a--measured-baseline) and cited inline as
**[M-n]**. When a measurement contradicts an assumption, the measurement wins.

**Revision 4.** Revision 4 specifies the 0.5.0 entity schema and the sectioned detail pane (§4.5, §9.3). Revision 3 deleted the vendored spec catalog and the on-disk cache, and reorganized `--help` parsing around the *framework* that generated the text (§7 Tier A′, §7 Tier B, §11). Revision 2's changes from revision 1 are in [Appendix B](#appendix-b--what-changed-in-revision-2).

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
`Constraint::Min(24)` and the detail pane fills the remainder, never a
percentage split: at 80 columns a 35% tree pane leaves too few usable cells
for a name plus a summary after borders and depth indentation [M-7]. Below
60 columns total, drop summaries from tree rows; below 50, stack the panes
and toggle the detail pane with `Tab`.

**Design principles.**

- **One accent color, used sparingly**: the selected row, and flag names in
  the detail pane. Everything else neutral — dim gray for hints and
  summaries, default foreground for names. No color as decoration.
- **Consistent indentation.** Each tree depth is exactly 2 cells; expanding
  changes only the chevron glyph (`▸`/`▾`), never the row's horizontal
  layout, which matters for mouse hit-testing (§9) and for not making the
  eye re-track.
- **Breadcrumbs in the detail pane header**, always showing the full path
  (`git › rebase`), so context survives scrolling.
- **Provenance is legible, not decorative.** The footer names the
  contributing sources and whether structure and prose each came from a
  trusted source, and it must be accurate (§4.2) or it is worse than
  nothing.
- **Rounded borders** (`BorderType::Rounded`), consistent 1-cell padding, no
  nested boxes.

**Flags are not tree rows.** `git` alone carries 2,999 flags [M-1]; putting
them in the tree makes the tree useless. Flags live in the detail pane and
stay independently searchable and addressable (§4.3, §10): the tree is for
structure, search is for flags.

**Interaction model.**

| Key | Action |
|---|---|
| `↑`/`↓`, `k`/`j` | Move tree selection |
| `→`/`Enter`/`l` | Expand (triggers lazy extraction if the subtree is unfilled). `Enter` accepts instead under `--print-selection`, below |
| `←`/`h` | Collapse, or jump to parent if already collapsed |
| `/` | Focus search |
| `Esc` | Leave search, keeping the filter pinned; `Esc` again clears it |
| `Tab` | Move focus between tree and detail pane (detail pane scrolls with `↑↓`) |
| `t` | Verbatim view: re-probe this node and show the tool's own `--help` output instead of the parse |
| `y` | Copy: the selected flag's spelling, or the node's full command path |
| `?` | Keybinding overlay |
| `r` | Re-extract this tool. Preserves expansion, selection, filter and view mode; abandons the in-flight warming cascade and restarts it |
| `q`, `Ctrl-C` | Quit |
| Mouse | Click row selects; click chevron toggles; wheel scrolls the pane under the cursor |

`y` is not a nice-to-have: looking up a flag in order to type it is the
terminal step of the core journey, and a reference tool that cannot hand
you the string makes you retype it.

`t` is the escape hatch for the one failure mode the rest of this
document's honesty machinery cannot signal: a grammar that misreads a
layout and produces a plausible tree is indistinguishable from one that
read it correctly, until a human reads the tool's real output beside ours
[M-10]. Rather than reserve that check for the coverage harness, `t` puts
it one key away for every user, on every node. It re-probes rather than
retaining raw text, since retention costs megabytes across a warmed tree
and would show what the tool said at startup rather than now, the same
staleness argument that removed the cache (§11). §6 rule 0 applies
unchanged here: `pkill --help` is shown, since that shape is measured
harmless, but an interactive request does not widen what may be run —
`pkill something --help` stays refused exactly as in the extraction
pipeline.

**Handing the command over: `--print-selection`.** The journey `y` serves
ends at the prompt, and the clipboard is one paste short of it. `mandible
--print-selection <tool>` browses identically, except that `Enter`
accepts: it prints the selected node's full command path, plus the
selected flag's spelling when search landed on one, to stdout and exits.
`→`/`l` still expand, so nothing becomes unreachable; `q`/`Ctrl-C` still
quit, printing nothing, which leaves a shell binding's line untouched.
Without the flag, `Enter` is one of the three expand keys, unchanged.

**Accepting is bound to the focus, not to the key.** `Enter` accepts only
from browse focus, the tree or the detail pane. While the search box has
focus it does exactly what it does without the flag: commits the query,
moves focus to the tree, keeps the filter — a key that is a search box's
only commit key cannot be reassigned, since that would leave `Esc` as the
sole way out of the box. The flag journey therefore ends one `Enter` later
than the tree selection it made: the first closes the search, the second
accepts what search landed on.

A TUI cannot type into the shell that launched it, so the shell reads the
line back through a command substitution and puts it on its own prompt.
That requires stdout to carry the composed line and nothing else, a
property of the whole program rather than the renderer: the UI draws on
stderr in this mode, and everything else that touches the terminal — the
OSC-52 clipboard fallback, and the color-support check — is asked of that
same stream (`mandible-tui`'s `terminal::Sink`). A stray escape sequence on
stdout here is not cosmetic; it is a corrupted command on someone's
prompt.

The spelling composed is the long one where the tool documents one, else
the short letter, always the affirmative form (`--color`, never the
un-runnable `--[no-]color`). No value placeholder is appended: the line is
handed over to be edited, and a literal `FILE` in it is worse than a flag
left unfinished.

`mandible --shell-init bash|zsh` prints the binding that closes the loop:
for bash a `bind -x` function assigning `READLINE_LINE`/`READLINE_POINT`,
for zsh a zle widget assigning `BUFFER`/`CURSOR`, both on `Ctrl-X m`,
readline's own extension prefix, unbound in either shell by default. It
reads the first word already on the line as the tool to open, and replaces
the line with what comes back.

---
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
| **cobra `__complete`** (Go: kubectl, docker, gh, helm) | Works, and is version-accurate. Flags require a *second* probe with `"-"` [M-2]. The empty-word probe returns subcommands **plus the command's own `ValidArgsFunction` output** — live user data at a leaf — so only fully-described candidate lists may be read as subcommands [M-2a]. Descriptions are terse. Cost: one subprocess per node per probe [M-3]. |
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
/// Every public struct here is `#[non_exhaustive]`: downstream crates
/// build through a constructor and assign the public fields, which is what
/// lets 0.5.x add fields (§4.5's remaining stages each do) without a
/// breaking release.
pub struct CommandNode {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: Option<Text>,          // one-line hint
    pub description: Option<Text>,      // long-form prose
    pub usage: Vec<Text>,               // raw usage patterns, kept verbatim
    /// Every documented item this node carries — flags, positionals,
    /// modifiers, environment variables — as one kind-tagged vector,
    /// in document order within each kind (§4.5). Read one kind through
    /// `flags()`, `positionals()` or `entities_of(kind)`.
    pub entities: Vec<Entity>,
    pub subcommands: Vec<CommandNode>,
    pub examples: Vec<Example>,
    pub hidden: bool,
    pub deprecated: Option<Text>,       // Some(reason) when deprecated
    /// True when this node's children are known-complete. False means the
    /// subtree has not been extracted yet (see §5, lazy extraction).
    pub children_filled: bool,
    pub provenance: Provenance,
}

pub enum ValueKind { None, Required, Optional }

pub struct Example { pub command: Text, pub explanation: Option<Text> }
```

### 4.5 One entity kind, not four parallel vectors

The 0.5.0 schema replaces `Flag`/`Positional` (and the never-built modifier
and env-var vectors they would have implied) with one entity type:

```rust
pub struct Entity {
    pub kind: EntityKind,
    /// Every documented spelling, in document order. Dissolves the
    /// multi-spelling bug: ffplay documents `-h`, `-?`, `-help`, `--help`
    /// as one row and today's `short: Option<char>` + `long:
    /// Option<String>` can hold only two of the four.
    pub spellings: Vec<Spelling>,
    pub value_name: Option<String>,
    pub value_kind: ValueKind,
    pub choices: Vec<Choice>,
    pub description: Option<Text>,
    pub group: Option<String>,
    pub see_also: Vec<Text>,
    /// An environment variable documented on this entity's own row — a
    /// `[env: FOO]` annotation or an override file's `env_var` key. This
    /// is a *cross-reference a flag carries*, distinct from an
    /// `EntityKind::EnvVar` entity, which is a variable documented as an
    /// item in its own right.
    pub env_var: Option<String>,
    pub provenance: Provenance,
    // ... plus the flags carried over from the type this replaced:
    // repeatable, required, hidden, deprecated, inherited, default.
}

pub enum EntityKind { Flag, Positional, Modifier, EnvVar }

/// One enumerated value an entity may take. `name` is a bare identifier —
/// searched and compared, the same convention `Spelling::name` follows —
/// and never carries the tool's scope-flag decoration. `description` is
/// `Text`, sanitized through the same §4.1 boundary as every other string
/// the IR carries from tool output; it is `None` for the common bare-list
/// case (`tar --quoting-style`'s `literal`/`shell`/`c`/...) and `Some` when
/// the tool documents one per value (ffmpeg/ffplay's AVOption constants).
pub struct Choice { pub name: String, pub description: Option<Text> }
```

`short()`, `long()`, `negatable()`, and `single_dash()` are derived from
`spellings` by shape, never stored: two dashes is long, one dash is long
when the name is longer than a single character (`-help`, `-vv`, `-CC`) and
short otherwise. A one-character single-dash spelling is a short flag,
because `-x` is `-x` whichever slot a previous schema filed it under.

A dashless kind carries exactly one `Spelling`, and that bare name is the
whole of its spelling: a positional's `pathspec`, a modifier letter, a
variable name. It has no `short()`/`long()`; `primary_name()` reads it.
`repeatable` covers both notations for "may be given more than once" — a
flag accepted repeatedly, and a positional written with an ellipsis
(`<pathspec>...`) — so the ellipsis needs no field of its own.

`Spelling`'s shape:

```rust
pub struct Spelling {
    pub name: String,
    pub dashes: Dashes,   // None | Single | Double
    pub negatable: bool,
    /// `Some(n)` when the tool documents an abbreviation bracket — the
    /// minimum accepted prefix length: `-r[esolve]` is `name: "resolve"`,
    /// `abbrev: Some(1)`; `-rc[vbuf]` is `name: "rcvbuf"`, `abbrev:
    /// Some(2)`. `None` for every other spelling.
    pub abbrev: Option<usize>,
}
```

One constraint carries over from `Flag::negatable` and `Flag::single_dash`:
the searched/copied name never smuggles punctuation. `-h`, `-?`, `-help`,
`--help`, `--[no-]foo` are all representable as name plus rendering
metadata, never as a name containing dashes or brackets; `abbrev` extends
the same rule, so `name` is always the full word (`"resolve"`, never
`"r[esolve]"` or the bare prefix `"r"`). `Spelling::render` reproduces the
bracket form a tool actually printed; `key()`/`short()`/`long()` address
the full name regardless of how much of it a row abbreviated, so `ip`'s
`-r[esolve]` and `-rc[vbuf]` key as `Long("resolve")` and `Long("rcvbuf")`,
two different flags rather than one flag's two readings — see
`docs/shapes.md` S-006.

Rules that govern the migration:

- **`#[non_exhaustive]` lands in the same pass.** It blocks cross-crate
  struct literals — 61 sites at the time of the decision — and the entity
  migration rewrites those same sites anyway.
- **Sequence: one kind at a time** — flags, then positionals, then
  modifiers, then env vars. The two relocation stages (flags,
  positionals) move existing data into the one vector and leave every
  corpus snapshot byte-identical, so a snapshot diff there means the code
  is wrong, never the fixture; `FlagSnapshot` now writes one `spellings`
  key holding every rendered `Spelling` in document order, while
  `PositionalSnapshot` keeps its original shape, since a positional never
  had a slot contest to dissolve. The two emission stages (modifiers, env
  vars) recover items no tier produced before, so a snapshot gaining one is
  the stage working; a fixture may move only when its own tool documents
  that kind, which the new section's `skip_serializing_if` makes
  structural. Neither emission stage may reshape a frozen snapshot section
  to make room.
- **Env vars are strict-sections-only**: an `EntityKind::EnvVar` may be
  produced only from a row under an explicitly labeled environment heading.
  Never scavenged from ALL_CAPS words in prose — `PATH`, `FILE`, `TERM`
  placeholders are exactly the fabrication class §13.1e's family detectors
  exist to catch. A tool documenting env vars only in its man page gets no
  ENVIRONMENT section; mandible renders the author's documented surface,
  it does not claim completeness (§1). No inferred env-to-flag
  cross-references: `see_also` is populated only from explicit statements.
- **Display contract** for each kind is §9.3's; the two sections change
  together.

**The argfile sigil flag.** The GNU-binutils/LLVM/JDK response-file
convention — `@<file>`, `@<filename>`, `@FILE` — is a `Flag`
(`EntityKind::Flag`), never a positional and never its own kind: an option
parser splices the named file's contents into `argv` in place of this
token, and the row is position-independent and repeatable, which is what a
flag is for (`docs/shapes.md` S-021). It is modeled as one
`Spelling { name: "@", dashes: Dashes::None }` with `value_name` the row's
own placeholder kept verbatim and `value_kind: Required`. Its `Spelling`
carries no dash, the one deliberate exception every other `Flag`'s spelling
avoids, addressed by `FlagKey::Name("@")` — the same key a dashless kind
uses — so search and `--print-selection` reach it exactly as any other
flag.

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

This is an IR invariant, not a widget concern. A single `\n` inside a
`ratatui` `Span` shifts cells and eats a pane border, and a prior
implementation's two widget-level fixes both had to be reverted, since the
IR has three consumers (tree, detail pane, clipboard) and a widget-level fix
can only patch one. Widgets are permitted to assume `Text` is clean.

**Within the prose tier, reflowing is the rule and structure is the
exception.** A description is hard-wrapped to whatever width its author
wrote for, and the pane re-wraps it to its own, so those breaks are noise
that `sanitize` joins — the alternative is a re-wrap coming out ragged
against already-short lines. Some breaks are the author's meaning, though,
and are recognized per line against the paragraph the line sits in:

- **Indented deeper than the paragraph's base indent** (the smallest
  indentation any line in that paragraph carries), so a uniformly indented
  block is ordinary prose and reflows, and only a line indented within its
  block is structure.
- **A list row**: `- `, `* `, `+ `, `• `, `1. `, `1) `.
- **An example invocation**: an `Example:`/`e.g.` label followed by
  command-shaped text, by shape only, never a tool name (§1). The
  command-shape half of that test is what makes it safe: without it every
  prose sentence after an `Example:` label would qualify, so the
  recognizer deliberately misses `Example: cp src dst` rather than admit an
  ordinary sentence.

A structural line keeps its break and its indentation relative to the
paragraph's base, clamped so a source documenting inside a wide table
cannot hand the pane an indent that leaves prose no room; the line after
one starts fresh. Everything else joins. The parser hands descriptions over
with source breaks intact, rather than pre-joining them, because the
decision belongs to the one place that can make it — text that still has
the breaks.

`Text` retains paragraph breaks (`\n\n`) and preserved single breaks for
the detail pane, which wraps each logical line at its own indent; the tree
pane collapses all of it to one line at render time. The `\n`-free
invariant a widget relies on is unchanged, since every newline in a `Text`
is one `sanitize` put there deliberately.

**Sanitization has two tiers, chosen by whose layout the text is.** Prose is
mandible's to set, so its source line breaks are noise and
`Text::sanitize` unwraps them. A synopsis and a raw `--help` dump are the
author's own layout — the spacing of a usage line, the columns an options
table is padded into — and collapsing them destroys information the reader
came for. The second tier, `Text::sanitize_preserving_layout`, strips
ANSI/OSC/DCS escapes, stray carriage returns, and other C0 controls (a raw
escape or a lying `\r` could still scramble the reader's terminal) and
expands tabs to spaces at 8-column stops, since `ratatui` gives a bare `\t`
zero display width. It does not collapse whitespace, trim, or unwrap
paragraphs, and is truncated to the same bound as `sanitize`.

Three paths take the layout tier, and no others: the raw pane (key `t`),
whose whole job is showing the tool's own bytes; `CommandNode::usage`,
whose synopses §9.3 already treats as content whose layout is not
mandible's; and `CommandNode::unparsed`, the verbatim fallback §7 Tier B
step 3 degrades to. The third follows from the first: that fallback exists
to show the author's document because mandible could not read it, so it is
the raw pane under a different label, and text mandible has admitted it
does not understand is the last text it may silently reformat. Each of the
three is handed one already-line-split string at a time. Everything else
that feeds the IR, descriptions above all, goes through `Text::sanitize`,
including a `Choice`'s own `description` — there is no second, laxer path
for text arriving nested one level deeper. The rule that decides between
the two tiers is ownership of the layout, never the field: mandible sets
prose, the author sets everything shown as drawn. Verified apart by
diffing the raw pane against independently captured `--help` output for two
real tools, byte-identical.

**A usage form keeps the indentation its author gave it, and the pane
reproduces the alignment that indentation was drawn for.** A tool lines its
alternative invocation forms up against the `Usage:` label it printed in
front of the first one. Since `USAGE` is already the section heading, the
pane drops that label — which moves the first form left by however many
columns the label occupied, while later forms stay where the author put
them. So every form shifts left by the first form's own content column
(its indentation plus the label), landing the first form at the block
indent with the rest kept at their positions relative to it, the alignment
the tool actually drew. A form indented less than that shift clamps at the
block indent rather than going negative, since once the label it was drawn
against is gone it cannot be aligned as drawn. `CommandNode::usage` stores
the author's own indentation; the compensation is the pane's, computed per
node.

mandible probes tools by absolute resolved path, so a tool that echoes its
own `argv[0]` prints `Usage: /usr/bin/du` in the pane against `Usage: du` in
a shell that found it via `PATH` — correct in both cases, since it reflects
what the tool actually received.

The raw pane displays stdout and stderr both, labelled, even though §7
Tier B's parser reads only one of the two per its own rule (see that
section for why).

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
independently. After a three-tier merge the node's badge named whichever
tier landed first, while the flag descriptions underneath could come from a
different tier entirely — the badge lied, and since a badge exists
specifically as a trust signal, an inaccurate one is worse than none.
Provenance therefore lives on `CommandNode` and each `Entity`
individually, and the detail pane's footer summarizes:
`carapace + help-text · structure ✓ · prose ✓`.

### 4.3 Addressing: `NodeRef`

```rust
pub enum NodeRef {
    Command(Vec<String>),                 // ["git", "rebase"]
    Flag { path: Vec<String>, key: FlagKey },   // ["git","rebase"] + --interactive
}
```

Paths are name-based, which is fine for commands but insufficient for
search results, which must point at any entity a node carries — a flag, or
a dashless positional/modifier/env-var addressed by `FlagKey::Name` (§10).
`NodeRef` is the single addressing type used by search, the clipboard, and
the cache.

Resolution walks `subcommands` by exact name match at each level, and must
not contain a "skip any segment equal to the current node's name"
shortcut, which would silently mis-resolve a subcommand sharing its
parent's name.

### 4.4 Merge: two axes of authority

Revision 1 merged with "first tier in priority order wins," correct only if
priority equals fidelity. It does not: the tier with the best structure is
frequently not the tier with the best prose [M-1, M-2]. Each source
therefore declares two authority levels, and merge resolves per field
against the relevant one:

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

- A field is taken from the contributing source with the highest authority
  on that field's axis. Ties break toward the earlier contributor.
- `None`/empty never displaces a value, regardless of authority.
- Flags unify by alias pairing, not by long-name equality alone, since
  sources legitimately emit a flag's short and long forms as separate
  items — `gh __complete pr -` returns `--repo` and `-R` as distinct rows
  with identical descriptions [M-2]. Pairing runs before merge: within a
  node, items whose descriptions match exactly and whose short/long slots
  are complementary unify into one `Flag`.
- Subcommands merge recursively by name.
- `children_filled` is the logical OR of contributors.

---
---

## 5. The extraction model: authority, laziness, cost

### 5.1 The cost problem, measured

Building a cobra tool's tree by recursive probing is not cheap: `docker`
takes 255 nodes, 232 subprocess spawns, 10.5 s; `gh` takes 196 nodes, 182
spawns, 11.6 s; both depth-capped at 3, roughly 40–65 ms per spawn [M-3].
That is with one probe per node. A correct cobra implementation needs two,
subcommands then flags [M-2], so uncapped depth and full recursion cost far
more. This is the single largest UX risk in the project.

### 5.2 The trait: one node at a time

A whole-tree `extract()` forecloses the only real fix, so the trait is
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

1. Renders immediately, from a stub root carrying only the tool's name.
   Resolving the name on `PATH` is a filesystem lookup with no spawn, so the
   TUI does no extraction before its first frame.
2. Queues the root for a background fill, then cascades: every completed
   fill queues the children it just discovered, walking the whole reachable
   tree on a bounded pool, cancelled on quit and capped at 4096 nodes.
3. On expand, a node not yet filled is queued at the front of that same
   mechanism; nodes still in flight render as `⋯ loading` rows.

**Warming covers the whole tree, not one level ahead.** Warming only one
depth past what the user had expanded kept the spawn count minimal but cost
more than it saved: an unexpanded node is invisible to search, since the
index can only hold what has been extracted, and a node that renders empty
with nothing explaining that it needs a keypress reads as a bug rather than
laziness. Filling everything in the background is the same total work
spread over idle time, and it is what makes a search over the whole tree
honest. This is not a return to §5.1's eager extraction: nothing blocks
startup or a keystroke, the pool is bounded, and what changed is not how
much gets extracted but what the user waits for, which is nothing.

**Background fills never expand the node they fill.** Expansion is user
intent; auto-expanding on arrival, once every node is warmed, would unfold
the entire tree and bury the user in rows they never asked for.

**Pool sizing is one worker per core, clamped to `[2, 8]`.** An earlier
design oversubscribed on the theory that a warming job spawns a child and
blocks on it, costing no CPU of its own. That held for a typical small C
tool and was measured false where warming is heaviest: a `docker`
invocation burns 70–100 ms of real CPU per spawn (Go runtime startup plus a
daemon round trip), so many concurrent probes on a 4-core machine pegged
every core for the duration of the warm, reported by a real user as the
tool maximizing their CPU for minutes. One probe per core keeps the machine
responsive; the cost is a slower background warm, paid in time nobody is
waiting on, since the expand path still jumps the queue.

Non-incremental sources (carapace) return their full subtree at step 1;
they cost nothing, so there is no reason to defer them.

### 5.3 Partial failure is normal

A tier that fails on one node must not invalidate the tier. The runner
records per-node, per-tier status and keeps whatever merged. `TierStatus`
is surfaced in the `?` overlay and in `mandible --doctor <tool>`, so "why is
this flag missing" is answerable without a debugger. The runner errors only
when no tier produced a root node.

**`mandible --report <TOOL>`** assembles a paste-ready bug report:
mandible's own version, the target tool's version when recoverable, the
`--doctor` diagnostic, and a raw `--help` capture, followed by the
repository's issues URL. It goes through the same sanctioned probe
chokepoint every other tier uses and adds no new argv shape. A tool's
version is scraped best-effort from the `--help` banner already captured,
but most tools never print one there, so the report usually asks the
person filing it to paste `<tool> --version` themselves rather than
issuing a new probe shape against §6 rule 2's closed list.

**`mandible --review <SEED>`** (with `--audit-dir`, default `audit`) opens
the audit review loop (§13.1c) inside the real TUI: it walks
`audit/<SEED>.toml`'s pending entries in file order, opening each tool
exactly as `mandible <tool>` would, and saves a verdict to the manifest
immediately after every confirmation, never batched, so a killed session
resumes at the next pending entry with everything answered so far intact.

### 5.4 Subcommand paths, and children the parent never documented

**`mandible <tool> <sub> [<sub>...]`** opens the tool at that node, the same
place browsing there lands: ancestors expanded, the node selected, nothing
else expanded, since expansion is user intent (§5.2). The path is an intent
held across frames, not an action taken at startup, since the tree is a
bare stub until the background root fill arrives. A path the tree turns out
not to have is reported in the status line once the parent that would hold
it is known-complete, never before, rather than refused at the command
line, since the tree beside the message is where the real name is.
`--doctor` and `--report` take a tool name alone; extra words there are
refused rather than dropped.

**A `<parent>-<sub>` executable on `PATH` is a child discovered by
convention.** `cargo --help` never lists `clippy`; `cargo-clippy` sits on
`PATH`, and `cargo clippy` works because cargo dispatches to it. git does
the same for `git-lfs`. The rule is keyed on the naming convention alone,
never on a tool's name (§1):

- **The parent's own documentation wins.** A sibling whose name the tool
  already documents is dropped, since the attested node already reaches
  the same command and a guess must not overwrite what the tool said. The
  rest are appended after the documented children.
- **A parent that documents no command at all gets none of these.**
  Dispatching on a first argument is a thing a tool says it does by listing
  at least one command of its own. `dpkg --help` lists no commands, and the
  27 `dpkg-*` programs beside it are separate tools `dpkg deb` does not
  reach; without this rule `mandible dpkg` opened on 27 rows of guesses and
  nothing else. Keyed on what the parent's own text said, never on its
  name, and it does not suppress the case this section exists for
  (`cargo`/`git` both document plenty) or make the convention reliable
  where a real list exists (`apt --help` lists commands, so `apt-get` and
  `apt-cache` are shown, marked, and are not `apt get`).
- **Root level only.** The convention is a tool dispatching on its first
  argument; nothing dispatches `cargo clippy fix` to `cargo-clippy-fix`, so
  nothing looks for one, and a discovered node's own children come from its
  binary's help in the ordinary way.
- **Name-shape checked, first `PATH` entry wins, alphabetical, capped.**
  The same command-name-shape rule every tier applies (§7 Tier B) keeps a
  versioned or capitalized helper out; a shadowed sibling is reported once,
  under the binary that would actually run; ordering is alphabetical since
  there is no document order to preserve; capped at 64 per parent so a
  `libexec` directory on `PATH` cannot hand the warmer hundreds of extra
  probes.
- **It is tree assembly, not an extraction tier.** Discovery reads the
  running machine's `PATH`. A tier that did it would make every corpus
  fixture (§13.2, frozen bytes, zero subprocesses) depend on what happens
  to be installed beside the tool, and would put a machine-local fact into
  `--doctor`'s account of what the tool said.

**Probing follows the binary, not the guess.** A discovered node's probe
target is its own binary with the path rebased onto it: `["cargo",
"clippy"]` is probed as `cargo-clippy` at `["clippy"]`, a root `--help`
byte for byte what `mandible cargo-clippy` already runs. The expand path
and the raw view (`t`) use the same redirect, so the pane shows the
document the tree was built from and names the argv that produced it.
`CommandNode::discovered_binary` carries this on the node (`None` for
everything a tier produces); merge keeps any contributor that has one,
since a merge can only add evidence, and the snapshot format omits it when
absent, so no fixture moves.

**The node is marked unverified, and stays marked.** A filename is evidence
about the filesystem and a guess about the tool: cargo really does
dispatch `cargo clippy`, and nothing dispatches `dpkg query` to
`dpkg-query`. The tree row carries an `unverified` marker and the detail
pane names the binary the guess came from, ahead of every other caveat, since
how well that binary's own help parsed says nothing about whether the
parent dispatches to it (§9.2). Showing the row is right, since the command
is usually real and otherwise unreachable from the tool the user opened;
showing it unmarked would be the same move as inventing structure.

---
---

## 6. Execution safety policy

mandible runs other people's binaries. This is the part of the design that
can damage a user's machine, and it gets its own section and its own tests.

All ten rules below are enforced at the `exec::run_inert` chokepoint, the
single place every tier spawns a process through, so no tier can bypass one
by another route. A test runs the full pipeline against a shim binary that
logs its own argv and environment, and fails on any invocation outside the
allowlist below.

**Rules, binding on every tier:**

0. **Programs that signal processes or change machine state are invoked
   only as `<tool> --help`.** `kill`, `pkill`, `killall`, `killall5`,
   `skill`, `xkill`, `fuser`, `halt`, `poweroff`, `reboot`, `shutdown`,
   `telinit`, `init` may run with exactly that one argv. Every other shape
   is refused before anything is spawned.

   This began as a total ban after `mandible pkill` froze a machine badly
   enough to need a reset. The mechanism was rule 2a's empty argument, not
   argument permutation as first assumed — measured directly: `pkill
   --help`, `pkill victim --help`, `killall victim --help` all killed
   nothing on this box. What the ban was actually protecting against, never
   written down until it was measured: `-h` is not a help flag on these
   tools. `halt -h`, `poweroff -h`, `reboot -h`, `shutdown -h` each attempt
   the real operation, and mandible falls back to `-h` whenever `--help`
   fails. [M-17] and [M-18] have the full measurement. `--help` itself is
   safe and yields real flag lists for all thirteen, so the rule keeps what
   is measured harmless and refuses what is measured dangerous.

   This is a safety rule about what may be *executed*, closed and short,
   and is deliberately not the per-tool knowledge §1 forbids: §1 governs
   extraction, where a per-tool list would grow without bound. Every entry
   here shares one fact about the program itself — it signals processes or
   changes machine state — independent of its output format.

   A second, narrower gate closes the general form of the same hazard for
   every tool, not just these thirteen: Tier B's `<word> --help`/`-h`
   probe (§7 Tier B) fires only when a node's `heading_attested` bit is
   true, meaning the word came from a recognized command heading, never
   from layout alone. A non-attested node is declined, never probed under a
   fabricated word. The root is exempt by construction, since it is the
   name the user typed. This gate and rule 0's list are deliberately
   independent: the gate governs when a word is trusted enough to become
   argv at all, the list governs what these thirteen programs may be asked
   to do even with a trusted word, since for them even a genuine,
   correctly-attested subcommand name is itself a target. A second,
   separate attestation bit, `invocation_attested`, marks a name found by
   layout evidence strong enough for the coverage harness to trust as real
   (§7 Tier B's headingless-invocation-table recognizer) without being
   heading evidence strong enough to probe — the two bits are never
   conflated, and this gate reads only `heading_attested`.

1. **Never invoke a bare binary.** An argv is never empty. Running an
   arbitrary binary with no arguments is how you launch a REPL, block on
   stdin, start a daemon, or trigger a tool whose no-argument default is an
   action.

   **1a. A framework protocol's own words are a bare invocation to a tool
   that does not speak the protocol.** `__complete`, `completion <shell>`,
   and `-- <partial>` are subcommand invocations only in the framework that
   defines them; fired at an arbitrary binary they are ordinary positionals,
   and rule 1's prohibition applies in substance even though the argv is
   non-empty. `wall __complete` broadcast that word to every logged-in
   terminal on a reporter's machine, because `wall` treats an unrecognized
   first positional as the message to send; `completion zsh`/`bash` sent
   speculatively left hundreds of daemons running (rule 4). Neither was a
   bad shape; both were a right shape sent to the wrong program. So a
   protocol word requires prior evidence that the tool speaks the protocol,
   read from the tool itself, never from its name: Tier E gates
   `__complete` on the `spf13/cobra` marker in the compiled binary, Tier C
   gates `completion <shell>` on that same marker or the tool's own
   `--help` naming the command (§7). A per-tool list of who may be probed
   would be §1's forbidden knowledge wearing a safety label; this evidence
   requirement replaces the need for one.

2. **Only inert argv shapes.** A tier may invoke a tool only as:
   `__complete <words...>`, `completion <shell>`, `--help`, `-h`,
   `help [<words...>]`, `<words...> --help`/`-h` (a subcommand path's own
   probe), `<words...> --help <word>` (rule 2b), or `-- <partial>` under
   `COMPLETE=` (currently unused — no tier constructs it since Tier E's
   clap probe was removed, kept on the type only so removing it is not a
   breaking change). Any other shape needs a spec amendment.

   **2a. No empty argument the tool could read as its first positional.**
   `--` is the option terminator essentially every getopt program discards,
   so `<tool> -- ""` delivers an empty string as the tool's first
   positional, and a program whose first positional is a pattern reads that
   as *match everything*: `pkill -- ""` was measured terminating every
   process in a private PID namespace. Exactly one empty argument is
   permitted, cobra's completion word, which is protocol-required and never
   the first positional — the `__complete` sentinel always precedes it.

   **2b. `InertArgv::HelpExpand` — the truncation-confession follow-up.**
   Some tools state, in their own printed text, that `--help` is not the
   full document and name the word that gets the rest. See `docs/shapes.md`
   S-080. `word` is copied verbatim from a closed, content-keyed grammar
   matched against the tool's own output, never guessed and never keyed on
   the tool's name; `--help` always precedes `word` in the rendered argv,
   so a getopt that stops at the first non-option still reaches `--help`
   first; and expansion is followed at most once, structurally, with no
   recursive call back into detection. Rule 0's list still wins
   unconditionally, since `HelpExpand`'s argv is never exactly `["--help"]`.
   A confession's `word` needs no separate attestation, since it is copied
   from a probe that was itself already attested. Scope is deliberately
   narrow: only the single-word "expand to one complete document" shape is
   followed; a confession detected but not followed (an unrecognized word,
   a failed probe, or a rule 0 refusal) caps the node's status at
   `incomplete` rather than reporting a confident `ok` over a document the
   tool's own text called incomplete. Two further confession shapes are
   detected but deliberately not followed, each needing its own future rule
   2 amendment: an unquoted flag-table row and a flag-value class
   enumeration (S-080 again). Detecting without following changes only what
   gets recorded on the node, never what argv gets constructed, so neither
   needs an amendment on its own.

3. **stdin is always `/dev/null`.** No tier may ever inherit or pipe stdin.

4. **Hard wall-clock cap**, 2 s for `detect`, 10 s for `extract_node`. On
   expiry, kill the process group, not just the child, since a completion
   script can spawn helpers a direct-child kill would leak.

   The process group is not sufficient on its own: a program that
   daemonises (`fork`, parent exits, child `setsid`s) leaves the group and
   the session, after which nothing about the survivor points back at the
   probe. This is not a hang — every probe that leaked one still returned
   normally within its own timeout — so no timeout change could have
   caught it. [M-24] has the measured count and the tools involved.

   `run_inert` reaps before returning. The process marks itself a child
   subreaper (`prctl(PR_SET_CHILD_SUBREAPER)`), so an orphaned descendant is
   reparented to mandible regardless of how many times it forked or
   `setsid`ed. A per-invocation token in the probe's environment,
   cross-checked against `/proc/<pid>/environ`, attributes a survivor to
   *this* probe, since adoption alone cannot distinguish a leaked daemon
   from a concurrent probe's legitimate child. Killing is by pid, never by
   process group, since an escapee's pgid is usually its already-recycled
   direct-child pid. Linux only; elsewhere the leak stands as the residual
   risk rule 8 documents. This is containment for a probe that should never
   have been sent, never a reason to send a riskier one — the fix is rule
   1a and §7 Tier C's evidence gate.

5. **Bounded output.** Read at most 8 MiB of stdout+stderr per invocation,
   since a tool that streams forever must not exhaust memory. Reader
   threads or a poll loop are mandatory to avoid pipe deadlock on large
   output.

6. **Sanitized environment, and a new session.** Clear `LESS`; set (not
   merely clear) `PAGER`, `MANPAGER`, `GIT_PAGER`, `SYSTEMD_PAGER` to `cat`,
   since several ecosystems read an *unset* pager variable as "go find one
   yourself"; set `TERM=dumb`, `NO_COLOR=1`, `COLUMNS=100`,
   `LC_ALL=C.UTF-8`. Spawn the probe as the leader of a brand-new session,
   not merely a new process group: `process_group(0)` alone leaves the
   child in mandible's own session, so its controlling terminal stays
   reachable, and a descendant can `open("/dev/tty")` directly regardless
   of what its own stdio was redirected to. [M-17] measured the mechanism
   directly with a shim that only attempts that open: under
   `process_group(0)` alone it succeeds; spawning the probe in its own
   session (`pre_exec` + `setsid()`, this crate's one audited `unsafe`)
   makes the same call fail with `ENXIO`. The pager variables stay set to
   `cat` regardless, as defense in depth against a pager gate weaker than
   the one [M-17] measured.

7. **Never write.** No tier may pass an argument that could name a file the
   tool would create or modify.

8. **Redirect every writable location a probe might reach.** Rule 7 is not
   sufficient, since some tools write unprompted on `--help` — [M-11] found
   a coverage run causing font-cache writes and a `mysql_secure_installation`
   config with an empty root password. Every probe runs with `CWD`, `HOME`,
   `TMPDIR`, `XDG_*`, and `XDG_RUNTIME_DIR` pointed at a per-invocation
   scratch directory, deleted afterward, one subdirectory per variable
   rather than one shared directory (a shared directory is a filesystem
   shape no real machine has, and let a tool see one file under two
   different variables). The redirect is all-or-nothing: if the scratch
   directory cannot be built, the probe is refused with a named error,
   never run against the inherited environment.

   Full containment needs OS-level sandboxing; until then this is a
   documented limit, not a closed one. The timeout kills the probe's
   process group, which a `setsid`ing child leaves, so anything that
   daemonises can survive it — a CI sweep measured this directly, naming
   the tools that started and never finished (a browser-driver server, an
   editor, a REPL, a kernel-probe attacher). The common property is not the
   tool but the behavior: `--help` is not what these programs do when they
   do not recognize it, and what they do instead outlives the process
   group mandible can reach. Exposure differs sharply by use: interactive
   use probes one tool and its subcommands, while the coverage harness runs
   thousands of arbitrary binaries in one process and is the only place
   orphans accumulate.

   One deliberate exception: toolchain-resolution variables
   (`RUSTUP_HOME`, `CARGO_HOME`, `PYENV_ROOT`, `NVM_DIR`, `RBENV_ROOT`,
   `ASDF_DIR`, `SDKMAN_DIR`, `VOLTA_HOME`) pass through, since redirecting
   `HOME` breaks every version-manager shim that resolves the program it
   stands in for through it. `HOME` itself stays redirected. This is a
   closed list of ecosystems, not of tools, which keeps it on the right
   side of §1: the knowledge is how version managers locate toolchains, not
   how any one tool works. Where a manager falls back to a documented path
   under the real `$HOME` when its own variable is unset, that default is
   materialized from the real home before the redirect, since almost nobody
   sets these variables by hand.

9. **Mask the redirect back out of the output.** A tool printing a
   `$HOME`-derived default prints the sandbox's, not the reader's — measured
   producing `docker --help` output naming a scratch directory deleted
   moments later, with nothing marking it as anything but docker's own
   documentation. Each scratch path is replaced with the variable that
   stood in for it, at the same boundary the redirect applied, never with
   the reader's real home directory, which the tool never actually stated.
   Every path is registered under both its logical and its canonicalized
   spelling, since a probe that resolves its own working directory prints
   the physical one and a symlinked `TMPDIR` would otherwise leave a
   mangled hybrid path on screen. Matching is on this invocation's exact
   path, never a pattern, so a temp path a tool legitimately prints is
   untouched. Residual: a path a tool wraps across two lines at the
   `COLUMNS` this policy sets cannot be matched; the scratch prefix is kept
   short to make that rare.

**A convention-discovered node (§5.4's `<parent>-<sub>` children, named by a
file on `PATH`) adds no argv shape and no exemption.** It is probed as its
own binary's root `--help`, never as a subcommand word, so it needs no
attestation, exactly as the root the user typed does not. Every rule above
still applies to that binary on its own terms — rule 0 matches the file
name it was discovered under, rule 8's redirect and rule 4's reap are the
same probe machinery. Discovery itself spawns nothing; it is a directory
read.

---
---

## 7. Extraction tiers, in detail

Tiers are listed in the order they are attempted, which is a cost ordering,
cheapest first. Conflict resolution is by `Authority` (§4.4), never by
attempt order; conflating the two was revision 1's central error.

### Tier A — REMOVED (was: vendored spec catalog)

Revision 2 ranked a vendored 739-tool carapace-spec snapshot first. Revision 3
deletes it, along with the vendoring script, the 11 MB payload, and the
third-party data attribution it carried.

A per-tool catalog is per-tool knowledge, the thing §1 forbids, merely
relocated from code into data belonging to someone else. It also could not
stay current: a snapshot is a point-in-time copy, and the tool on a user's
machine is not. [M-12] has the coverage numbers it bought against what it
cost. The replacement is parsing by the framework that generated the help
text, below, never a return to a per-tool catalog.

### Tier A′ — framework identification

Help text is not written by hand, it is generated, and only a small closed
set of generators exists. Per-tool knowledge is unbounded and forbidden;
per-framework knowledge is bounded at 18 entries (`mandible-extract/src/
help_text/profile.rs`) and is the correct unit of parsing. A grammar fix for
argparse improves every Python CLI ever written; a catalog entry improved
exactly one tool until it went stale.

Identification proceeds in this order, most reliable first:

1. **From the artifact.** For a compiled binary, scan embedded strings — a Go
   binary linking `spf13/cobra` says so directly in its own bytes,
   independent of which headings that cobra version's `--help` happens to
   render this week [M-13]. For a script, read the shebang plus the import
   line. This is ground truth, not inference.
2. **From the help-text signature.** Distinctive marker strings: argparse's
   `show this help message and exit`, click's `Show this message and exit.`,
   cobra's `Available Commands:`, GNU argp's `Mandatory arguments to long
   options`. Weaker, and must never be the only method: it missed `docker`
   entirely, because docker prints `Common Commands:` rather than cobra's
   own default [M-13]. That gap is why step 1 leads.
3. **Unidentified** — fall through to the generic layout parser, Tier B.

The implementation deliberately trades recall for precision: narrow,
high-confidence markers identify a minority of a real machine's tools rather
than the majority a looser fingerprint could reach [M-12]. A wrong framework
silently applies the wrong grammar with no way to signal it did; an
unidentified tool falls back to the general engine and is honestly marked
low-confidence. Widening a fingerprint is worth doing only alongside a
grammar that earns it, never to move the detection number on its own — a
metric improved by the thing it exists to detect is the same failure §13.1
warns about, one tier up.

Detection rate is therefore not a target; coverage is. `--doctor` reports
the detected framework, which turns "mandible is wrong about tool X" into
"the argparse grammar mishandles Y" — a general, fixable bug report instead
of a per-tool complaint.

### Tier B — `--help` parsing, per framework

The primary tier. `--help` is the only source every tool has, everywhere,
and it is always current because it comes from the installed binary.

**One shared engine, not eighteen grammars.** A single `winnow`-based layout
parser (`mandible-extract/src/help_text/sections/`) reads section headings,
column-aligned tables, usage synopses, and continuation folding by shape
alone. A `FrameworkProfile` (`help_text/profile.rs`) is consulted by that one
engine and is deliberately narrow: which extra heading vocabulary a
framework's own templates use for a command block, whether the framework has
a subcommand concept at all, and which heading introduces a positional-
argument block. A profile carries no grammar of its own — no value-spec
syntax, no continuation-folding rule — because the shared low-level grammar
already handles `--opt=VALUE` / `--opt VALUE` / `--opt <value>` /
`--opt[=VALUE]` and indent-relative continuation folding uniformly across
every framework tested. Adding a framework is one `match` arm in `profile()`
plus one fingerprint in Tier A′, nothing more. If a framework is ever found
whose shape the shared engine genuinely cannot express, the fix is to widen
the engine, which improves every framework at once, never to add a
per-framework knob only one arm sets.

**Degradation is staged, and never fabricates:**

1. Framework identified → the shared engine with that framework's profile,
   high confidence.
2. Unidentified → the same engine with the generic heading vocabulary only,
   marked low-confidence.
3. The parse yields nothing structurally plausible → render the raw help
   text verbatim, labelled `unparsed`, framework shown as unknown.

Step 3 is a feature, not a failure: a tool that conforms to no convention is
displaying its help the way its author intended, and showing that text
untouched is honest. It is also strictly better than the alternative already
shipped and fixed once: inventing 39 phantom subcommands for `tar` out of
wrapped description lines [M-10]. Never fabricate structure; degrade to
verbatim.

**A command block requires a recognized heading.** Layout alone is never
sufficient evidence that a block of text names subcommands.
`Commands:`/`Subcommands:`/`Available Commands:`/`SUBCOMMANDS`, a git-style
group heading, and headings mentioning "operation(s)" all qualify; a bare
word list under no heading does not. A candidate name must match
`^[a-z][a-z0-9_.-]*$` with no whitespace, and every emitted name is checked
to occur literally in the tool's own raw text (the existence oracle, §13.1).
Two evidence classes short of a heading are structurally distinguished and
tracked separately, `invocation_attested` versus `heading_attested`: a row
that repeats the tool's own name, or a table whose row shape is unambiguous
even without a heading, is real but weaker evidence than an explicit
heading, and the difference governs which nodes are eligible for further
probing (§6 rule 0). A block yielding names that fail the shape test, or a
node with no flags, no children, and no summary, drops confidence and marks
the tool `suspicious` in the coverage scoreboard (§13.1) rather than
inflating it. See `docs/shapes.md` S-013 (never invent subcommands from
wrapped description lines), S-016 (headingless invocation table naming the
tool), S-017 (headed command table with a non-standard separator), S-018
(heading sharing a line with its first row), S-019 (pseudo-heading rewind
inside a sticky chain), S-022 (an "operations" heading), S-092 (a settings
table misread as subcommands), and S-094 (a non-command "help topics"
heading that breaks a sticky chain).

**An indented list nested under a flag is that flag's `choices`, never
subcommands.** A per-value description, when the source documents one, is
kept on the value it describes rather than dropped. See S-014 (bare-word
choices block), S-015 (described choice values in a scope-flag sub-table).

**Read stdout and stderr, and do not require exit 0.** `openssl --help`
writes 0 bytes to stdout and 2,908 to stderr; `ip --help` exits 255 with
output only on stderr [M-8]. Each stream is judged independently by a
help-shape check, never by "stdout if non-empty":

| stdout empty? | stderr empty? | picks |
|---|---|---|
| yes | yes | stdout (nothing to pick) |
| yes | no | stderr, the only stream available |
| no | yes | stdout, the only stream available |
| no | no, stdout help-shaped | stdout, regardless of stderr |
| no | no, stdout not help-shaped, stderr help-shaped | stderr |
| no | no, neither help-shaped | stdout, the default |

Ties break toward stdout. The two streams are never concatenated for the
parser: merging a diagnostic preamble into the document is how banner text
becomes a fabricated flag (S-091, S-029). This is the parsing path only; the
raw pane (key `t`) always shows both streams, labelled, independent of which
one the parser chose, so a reviewer can see what was correctly discarded
(§4.1).

**Section headings are preserved as `Flag::group`.** A heading is recognized
by relative indentation, since real headings sit at no fixed column. Running
prose whose hard wrap places an indented line beneath an ordinary sentence
is the one systematic false positive, handled by three binding rules
depending on whether the sentence ends on the promoted line, is marked with
a continuation backslash, or continues onto the indented line: see S-011
(hanging-indent prose misread as a heading). A usage stanza can also be
labelled by its own preceding description sentence rather than by its own
head line: see S-012.

**A confidence score is attached**, derived from how much of the output the
grammar actually consumed, and surfaced in the UI. Being honest about a best
guess is better UX than presenting heuristic output with man-page
confidence.

**Recursion.** Revision 1 parsed only the root; subcommand flags need a
probe per node. Recursion is lazy, per-node, under §5.2: `<tool> <sub>
--help` runs only when that node is expanded.

**Entities beyond flags and subcommands** — modifiers (a letter glued to an
operation letter, `ar rv`), the argfile sigil (`@<file>`), and environment
variables documented under an explicit heading — are recognized by shape and
become their own `EntityKind` (§4.5), rendered in their own panel section
(§9.3). None is inferred from prose that merely mentions the word: an
ALL_CAPS word in a usage placeholder is not an environment variable, and a
heading that only mentions "operation" without one-word invocation verbs
beneath it is not a modifier table. See S-020 (modifier table), S-021
(argfile row), S-023 (environment section).

Every recognizer above is admitted only after being checked against the
full frozen `PATH` capture set, never assumed from one tool alone; a recorded
miss is preferred to a recognizer that invents a section (§13.1e). The
atlas (`docs/shapes.md`) is the record of what was found and what fired on
it; this section states the rule each recognizer enforces, not its
history.

### Tier D — man page enrichment

Two sub-cases of very different quality. `mdoc(7)` pages use semantic macros
(`.Fl` for a flag, `.Ar` for an argument), so the AST genuinely distinguishes
a flag from prose. `man(7)` pages are typeset prose with weak semantic
tagging: section boundaries extract reliably, but individual flag/
description pairs need the same heuristics as Tier B.

Never regex the rendered output of `man <tool>`, and never parse `mandoc -T
tree` — the OpenBSD manual documents that format as unstable and says not to
write parsers against it; there is no `-T json`.

**Implementation is a pure-Rust subset parser, never `libmandoc` FFI.**
`libmandoc` is not a shipped library on Linux [M-6], so using it would mean
vendoring and building mandoc's C source, and `#![forbid(unsafe_code)]`
rules out the FFI regardless. The parser targets man(7) `.TP`/`.IP` + `.B`,
with `.It Fl` for mdoc — most relevant pages are man, not mdoc [M-14]. It
gates on the tag line beginning with a flag, never on an `OPTIONS` section
heading, since several real tools document options under `DESCRIPTION`
instead [M-14].

**Man pages are generated too, the same insight as Tier A′ one tier down.**
help2man, asciidoc/docbook-to-man, mdoc, and hand-written roff partition
this space the way clap/cobra/argparse partition help text. The first step
is a generator survey with a go/no-go per generator, not a parser: git's own
184 `git-*.1` pages are asciidoc-generated and contain zero `.TP` macros, so
a `.TP`-targeting parser recovers nothing from git regardless of how well it
is built [M-16]. git's flags are reachable far more cheaply through `-h`
(§7 Tier B, [M-16]).

Multi-page discovery, for the tools this tier does help, walks
`<tool>-<sub>.N` siblings via `MANPATH` and `man -k`.

**Trigger: zero-confidence fallback only, off by default.** This tier fires
only where the help-text tiers produced nothing usable; it never enriches a
parse that already succeeded, and shipping it at all is opt-in. §16 records
the ruling and why. Where it does fire, per-field provenance labels the
prose `man`, so a reader can see a description came from a page rather than
from the binary.

### Tier C — completion script structural parsing

For a tool that supports `<tool> completion bash|zsh|fish` (clap, cobra,
click, oclif, and many hand-rolled CLIs): generate the script, then parse it
with a real shell grammar, never regex. Parsing never executes the script,
which is the safety property that matters when processing untrusted output.

**Crate: `brush-parser`.** Revision 1 selected `conch-parser`, unmaintained
and rejected by a future Rust version at build time [M-9]. `yash-syntax` is
avoided as GPLv3, which would oblige the whole binary under GPL if statically
linked.

**zsh before bash.** zsh's `_arguments` blocks carry a spelling and a
description in one structure; bash completion functions carry only
spellings and typically compute candidates at runtime. Static parsing
recovers far less from bash. Walk `complete -F`/`compgen -W` registrations
and `case "$prev" in` branches as typed AST nodes.

**Gated on prior evidence that the subcommand exists, never sent
speculatively.** `completion <shell>` is a framework protocol's own words,
and sending them to a program that does not speak the protocol is a bare
invocation under §6 rule 1a. `CompletionScriptTier` constructs that argv
only when Tier A′'s artifact scan already found the `spf13/cobra` marker (a
cobra binary registers `completion` itself, whether or not the author
mentions it, and it may be hidden), or when the tool's own root `--help`
names `completion`/`completions` as the first token of a line — the shape
of a command-table row, which adds no new probe since `--help` is already
sent to every tool. [M-23] has the cost this gate removed and the honest
limit that remains: a tool with a real, hidden `completion` subcommand that
is not cobra loses this tier.

### Tier E — native, self-describing binaries

Highest structural authority, lowest cost-efficiency. Attempted last because
it is the only tier that spawns a process per node, but it wins structural
conflicts (§4.4) because it reflects the version actually installed.

**Gated on prior evidence, never speculative.** This tier only constructs a
`__complete` argv for a tool whose own compiled bytes already identify it as
cobra, via Tier A′. Probing every tool on `PATH` to find out who would
answer was the previous design: probing `wall` that way broadcast the
literal text `__complete` to every logged-in terminal on the reporter's
machine, because `wall` treats an unrecognized first positional as the
message to send — the same shape as `pkill -- ""` under §6 rule 2a, an argv
that is inert for nearly every tool and an action for one family. [M-23] has
the fleet-wide measurement showing the gate costs nothing extraction can
see.

- **cobra `__complete`** requires two probes per node: flags need
  `__complete <path> "-"` [M-2], not only the empty-word probe revision 1
  documented. The trailing `:N` directive line is parsed and discarded;
  candidate lines are `value\tdescription`, a `=` suffix marking a
  value-taking flag. `__complete <path> ""` does not return subcommands
  only: cobra appends the command's own `ValidArgsFunction` output, which is
  application code reading live state, so a leaf's response is entirely
  argument data [M-2]. A candidate list becomes subcommands only when every
  candidate in it carries a non-empty description, since that is the only
  distinction cobra's own wire format offers between a subcommand row and a
  `ValidArgsFunction` row [M-2a]. A depth cap (default 6) and a visited set
  keyed by the candidate list's hash stop a tool that echoes root
  completions for unrecognized paths from recursing forever. The `Alias for
  "..."` convention, and a child whose candidate set equals a sibling's, are
  detected and recorded in `CommandNode::aliases` instead of being
  recursed into, which would otherwise duplicate a whole subtree.
- **clap `CompleteEnv`** (`COMPLETE=<shell> <tool> -- <partial>`) was probed
  once and removed. It could not be spelled safely: an empty partial sent
  `--` as the tool's first positional, which `pkill -- ""` demonstrated
  terminating every process in a PID namespace (§6 rule 2a). Spelled `<tool>
  --` instead it is harmless but wrong, since `--` is a no-op for most tools
  and their ordinary output then gets misread as candidates. And it never
  reliably worked: unlike cobra's self-identifying `:N` trailer, detection
  was only ever a shape heuristic, and on a full sweep it matched ten tools,
  none of them clap [M-4]. Re-adding it needs confirmation of the protocol
  before trusting a response, and a spelling that never passes an empty
  first positional.
- **argcomplete** (Python): the `_ARGCOMPLETE` env-var convention. Same
  shape, lowest priority in this tier.

### Tier F — user override

`~/.config/mandible/overrides/<tool>.toml`, merged with `Authority { 255,
255 }`. This exists so the rare bad case has a clean exit; the pipeline
never depends on one existing.

**Policy, binding:** overrides are user-local and never vendored into this
repository. This rule is what actually enforces the §1 invariant — without
it, the first hard tool gets an override committed to git and the per-tool
patch pile begins.

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
click-to-expand is a real affordance, not a keyboard-only one.

**Widgets are permitted to assume text is clean.** All sanitization happens
at the IR boundary (§4.1); this is a hard layering rule. Untrusted text
containing newlines, tabs, ANSI, or backspace-overstrike reaching a `Span`
caused border corruption while scrolling in a prior implementation, and two
widget-level fixes failed and were reverted, because a widget-level fix can
only ever patch one of several consumers. The tree row builder still
truncates to the pane's inner width using display width (`unicode-width`),
not byte or `char` count, since CJK and emoji are double-width and a
`char`-count truncation overflows the border by one cell per wide
character.

**Rendering rules.**

- Tree rows are built at fixed column offsets:
  `[indent 2·depth][chevron 1][space][name][space][summary dim]`. Fixed
  offsets make mouse hit-testing arithmetic: the chevron is hit when
  `col == 2·depth`.
- The flattened row list is cached and invalidated on expand/collapse,
  search change, or lazy fill, never rebuilt per keypress.
- The detail pane groups flags by `Flag::group`, with inherited flags in a
  final dimmed "Inherited" group, and hidden/deprecated flags suppressed
  unless toggled with `.`.
- Scroll state is per-pane; the wheel scrolls the pane under the cursor.
- **The parsed view and the raw view each keep their own scroll position**,
  vertical and horizontal. `t` restores the exact position the view being
  entered was last left at; movement in one never moves the other, and
  nothing is mapped or scaled between them, since mandible's own layout and
  the tool's own raw text place the same flag at unrelated coordinates. An
  unscrolled node opens at top-left; a remembered position clamps to the
  extent the view has when restored; changing the selected node clears both
  views' memory.
- **Preformatted content scrolls horizontally instead of wrapping; prose
  does not.** The raw `--help` view (`t`) and a node's USAGE synopsis lines
  are the tool author's own layout, and wrapping them reflows spacing that
  was part of their meaning. `h`/`l`/`←`/`→` scroll that content when the
  detail pane has focus, clamped to the widest line, with a marker in the
  border when more content sits off that edge. The summary, description,
  and flag list keep wrapping to pane width as everywhere else. Governed by
  `[ui] horizontal_scroll` in `~/.config/mandible/config.toml`, default
  `true`.
- **`horizontal_scroll = false` wraps every view; it never clips.** A
  preformatted line wider than the pane continues onto the next row instead
  of ending at the border, in the raw view and the `unparsed` fallback
  included, without being reflowed: a fitting line arrives byte for byte, a
  row keeps its internal spacing, the cut prefers a whitespace boundary and
  falls back to a character boundary only when a token has none, and a
  continuation row carries the line's own leading indent.

**Empty and degraded states are designed, not incidental:** a node whose
children are still being extracted shows a subtle spinner row; a tool where
only Tier B fired shows the confidence in the footer; a tool no tier
resolved shows the per-tier status list with a suggestion to try
`--doctor`.

### 9.1 Tree rows: one node, one row

**No wrapping in the tree pane, ever.** Row index ↔ node stays a bijection,
which keeps selection, scrolling, mouse hit-testing, and filtering
arithmetic rather than bookkeeping. Truncation costs nothing here, since the
detail pane shows the full text on selection and a tree summary only has to
disambiguate `push` from `http-push`.

```
╭ git ───────────────────────────────────────────╮
│▾ git             the stupid content tracker    │
│    add           Add file contents to the ind… │
│  ▸ bisect        Use binary search to find th… │
│  ▾ stash         Stash the changes in a dirty… │
│      push        save your local modification… │
╰────────────────────────────────────────────────╯
```

- **Summaries align to a computed column**, `min(longest indent+name over
  the whole flattened row set, 40% of pane width)`, computed over all rows
  rather than the viewport, since a viewport-derived column jumps as you
  scroll. Stable until expand or collapse.
- **Truncate at a word boundary with `…`**, a real signal that the detail
  pane has more, since a mid-word cut just looks broken.
- **The name column never yields to the summary.** A long name truncates
  the summary to nothing before truncating itself: navigation without a
  summary is possible, without a name it is not.
- Width ladder: full layout above 60 columns; names only below it (drop
  summaries rather than showing a few useless characters); stacked panes
  below 50.
- **An `unverified` marker (§5.4) takes the summary's column ahead of the
  summary**, and the width ladder does not drop it, since a summary is a
  convenience and this is the row's claim about whether the command exists
  at all. A plain word, not a glyph, so it survives a terminal with no
  color and no Unicode (§9.2).

### 9.1a Flag rows: one table, one column

The detail pane's flag list is a two-column table: spelling, description. A
value placeholder belongs to the spelling it follows, measured with it one
space behind (`--env list`), never given its own aligned column, since such
a column has to be as wide as the section's widest placeholder and every
row pays that width whether or not it takes a value. Spelling and
placeholder are told apart by style (§9.2), not position.

- **The description column is one number for the whole list**, not a target
  some rows are allowed to miss. A column most rows share and some don't is
  noise that looks like alignment.
- **A row too wide for the column starts its own first description line one
  space past its head**, and returns to the column on every later line. It
  never pushes the column right for itself, and the spelling is never
  truncated to force alignment (§9.1: names win).
- **An outlier row is excluded from the measurement, not clamped to it.**
  A row wider than 45% of the pane, spelling and placeholder together, does
  not get a vote; excluding it lets it run past the column while every
  other row stays aligned.
- **A pane too narrow for the column brings the column down, never the
  layout.** The column is clamped until the description has 28 columns to
  wrap in — measured, not picked: at 20 columns a real six-word flag
  description breaks across six lines, one mid-word; at 28 it reads as
  prose. A layout that changed shape at some threshold width would make one
  list read as two different products either side of it.

### 9.2 The styling contract

One accent, spent only on information. Everything else is neutral.

| Element | Style |
|---|---|
| Node name | Default foreground |
| Selected row | Accent + reversed |
| Tree summary | Muted |
| Focused pane border | Accent; unfocused muted |
| Breadcrumb | Ancestors muted, leaf bold |
| Section heading (`DESCRIPTION`, `FLAGS`) | Its own rule's shade exactly, never bolder (§9.3) |
| Group divider | One step below the section heading, label and rule alike (§9.3) |
| **Flag spelling** | **Accent** — the payload the user came for |
| Value placeholder (`<FILE>`) | Muted italic |
| Flag description | Default foreground |
| Inherited flag group | Entire group muted |
| Deprecated | Muted + a `(deprecated)` tag |
| Search match characters | Underline, within the name only |
| Provenance footer | Muted |
| Low confidence, and an `unverified` node (§5.4) | Warning color, the one sanctioned exception to single-accent |

Four implementation rules that matter more than the palette:

- **ANSI indexed colors, not RGB.** Indexed colors resolve through the
  user's own terminal theme, so mandible looks native in Solarized,
  Gruvbox, or a light terminal with no detection logic; hardcoded RGB looks
  wrong in half of them. The accent stays configurable.
- **Prefer `DarkGray` over `Modifier::DIM` for muted text.** Several
  terminals ignore `DIM` outright or render it nearly invisible, a
  portability trap that only manifests on someone else's machine.
- **Respect `NO_COLOR` and `TERM=dumb`**, degrading to bold/reverse/
  underline only. There is no truecolor tier and no RGB anywhere: named
  ANSI colors work at every depth that has color at all. Depth is consulted
  in exactly one place, the detail pane's two rule shades (§9.3), which need
  two steps below the terminal's default foreground; those read the
  xterm-256 gray ramp where available and fall back to `DarkGray` where not.
  Depth is read from `COLORTERM`/`TERM`, never queried, since a query needs
  the tty in raw mode before the TUI has set it up and hangs on a terminal
  that does not answer; an unrecognized terminal takes the fallback rather
  than a guess.
- **Highlight search matches.** `nucleo` returns match indices for free;
  underlining matched characters is the difference between "the list
  changed" and "here is why this matched."

#### What may be drawn, and what may not

The rule: a glyph may only be used if there is something legible to fall
back to. This is about how each technique fails, not aesthetics.

| Technique | Fails on | Failure mode |
|---|---|---|
| Box-drawing, block elements | non-UTF-8 locale, bare Linux console | falls back to `+-\|` |
| Color (named ANSI) | `NO_COLOR`, `TERM=dumb`, no TERM | falls back to bold/reverse |
| Bold, reverse, underline | almost nothing | — |
| **Italic, `DIM`** | **many terminals silently ignore them** | **must never be the sole distinction between two kinds of text** |
| Sixel / Kitty graphics | most terminals, most tmux, many SSH sessions | raw bytes on screen |
| Nerd Font icons | any machine without the patched font | `□`, meaning nothing |

Two properties decide it: **detectability** — `NO_COLOR`, `TERM`, and the
locale can be inspected; a terminal can never be asked what font it is
using, which rules Nerd Fonts out permanently — and **how it degrades**:
losing color loses emphasis and the text remains, losing the font loses the
meaning and leaves a box.

This matters more here than for most TUIs because of where mandible gets
used: SSH'd into an unfamiliar machine, or inside a minimal container with
`LANG` unset, trying to work out a CLI you do not know. Polish that
evaporates exactly where the tool is most needed is not polish.

Implemented in `mandible-tui/src/glyphs.rs`: two glyph sets chosen at
startup from `LC_ALL`/`LC_CTYPE`/`LANG`, with `MANDIBLE_ASCII=1` as an
override for a terminal that claims UTF-8 and renders it badly anyway.
Enforced by a test that renders a full frame over ASCII-only content and
asserts no cell contains a non-ASCII symbol; content from the tool itself is
exempt, since reproducing a tool's own text exactly matters more than this.

Markup handling is staged: prose is flattened to plain text today
(`Text::sanitize_markdown`). The better end state keeps parsed spans in the
IR so inline code and link labels can be styled rather than stripped, once
the plain-text path is stable.

### 9.3 The detail pane is sections, not tabs

The right pane is one scrollable document of sections, rendered in this
order and only when non-empty:

1. `DESCRIPTION`
2. `USAGE`
3. `POSITIONALS`
4. `FLAGS`
5. `MODIFIERS`
6. `ENVIRONMENT`

Rules:

- **Empty sections do not render**, and list-section headers carry a count:
  `FLAGS (41)`, `MODIFIERS (17)`.
- **Spellings collapse to one row**: `-h, -?, -help, --help` is a single
  entry (§4.5).
- **Two spelling columns, set by the row's own shape.** A short (one dash,
  one character) starts at the content area's true left edge; every long
  starts one short-prefix in, at the width of `-X, `. A row with only long
  spellings is preindented to that same place, so longs run down one column
  whether or not a short precedes them. A dashless spelling (a positional,
  a modifier letter, a variable name), and any row with more than two
  spellings, sits at the short column.
- **A repeatable positional renders `name...`**, the POSIX synopsis
  ellipsis that says "one or more", the same signal `repeatable` was parsed
  from. Only POSITIONALS uses it this way: a flag says the same thing by
  being accepted again (`-v -v -v`), never by an ellipsis on its spelling.
- **The value placeholder is part of the spelling**, one space behind and
  measured with it (§9.1a), never its own aligned column.
- **A flag's `choices` render as their own `values:` line under the
  description**, indented two columns past the shared description column,
  never folded into the description text or the spelling column. A
  choice's own per-value description, when the tool documents one, renders
  one further indent past `values:`, one `name  description` row per
  choice, in the same style; a flag whose choices all lack one keeps the
  single-line `values: a, b, c` summary, and the two forms may mix within
  one flag's list. A tool's own scope-flag columns (ffmpeg's `ED.VAS.....`)
  stay verbatim inside the description; mandible parses no meaning out of
  them.
- **Capped shared column, per section.** Every list section computes its
  own column, fitted to roughly the p90 row width (the majority, not the
  outliers), measured from the pane's left edge through the placeholder's
  end. Every description line in the section, first line and continuation
  alike, begins at that column. Never a per-row column, never a global
  uncapped one. A wrapped entry is one logical row for selection and scroll
  math.
- **A head that reaches the column pushes its own first line, and only
  that**, never truncated and never moving the column for the section. A
  head too wide for the pane wraps within the head area, each line at its
  own spelling's column, description beginning on the line beneath at the
  shared column.
- **A narrow pane moves the column, not the layout** (§9.1a): clamped down
  until the description has its 28 columns, never below two past the long
  column. A 90-column terminal's 41-column detail pane clamps the column to
  13, still holding a short-and-long pair.
- **POSITIONALS is inset by two columns; the flag-shaped sections are
  not.** A positional's bare name carries no dashes to set it off from the
  pane's border; FLAGS, MODIFIERS, and ENVIRONMENT keep the edge, since
  their short and long columns are already structure the eye follows down
  the section.
- **The vertical gaps are the container hierarchy**: two blank rows above a
  section header, one above a ruled group divider, none below either, none
  above the first header on the page. A section is a chapter and a group a
  paragraph within it, so the wider gap marks the wider boundary. Each
  count is exact, not a minimum, and belongs to the block that opens, never
  to the one that closes, so a boundary never varies with how much content
  the block above it held.
- **ENVIRONMENT is display-only**: documented vars under an explicit
  heading only, no probing, no inferred cross-references (§4.5).
- **Group dividers are label-first, like the headers above them.** A
  `group` renders once as its label at column 0 followed by a rule to the
  pane's edge, mixed case; rows beneath sit at the section's normal margin.
  Section headers are CAPS with a count, group dividers mixed-case without
  one, a shape distinction that survives a terminal that ignores dimming
  (§9.2).
  - A label drops the terminator its source gave it (a heading's colon, or
    the full stop of a label that is a whole sentence), so the label runs
    straight into the rule beside it.
  - **Three neutral steps, brightest first: pane borders, section header,
    group divider.** Borders keep the terminal's own default foreground;
    the header's rule is a clear step below, the divider's a clear step
    below that.
  - The two rule shades come from the xterm-256 gray ramp, indices 246 and
    240, since the sixteen named colors cannot express this: `Gray` is
    ANSI 7, the default foreground in most themes, and below it there is
    only `DarkGray`, one step for two levels. Without the extended palette
    both levels collapse to `DarkGray`; the step below the borders
    survives, and §9.2's shape rule (CAPS-plus-count versus mixed-case)
    carries the distinction between the two inner levels on its own.
  - A label is drawn in exactly its own rule's style at both levels, same
    color, never bold, since a label in a different shade or weight from
    the line running out of it reads as two unrelated marks sharing a row.
  - A divider that opens its section drops its rule and its blank row,
    rendering its label alone at column 0 directly beneath the header,
    since a second rule immediately beneath the header's own would read as
    one doubled line. A divider later in the same section keeps both, since
    it genuinely ends one run of rows and starts another.
- **Descriptions always wrap.** Sections are mandible's own layout, so
  nothing in them is ever clipped or horizontally scrolled; `[ui]
  horizontal_scroll` governs only content whose layout is not ours (the raw
  view, verbatim USAGE synopsis lines, the `unparsed` fallback, which
  reaches the pane by the same path as the raw view). A description's
  preserved line breaks (§4.1) wrap too, each logical line wrapped on its
  own at its own indent.

---
---

## 10. Search

**`nucleo`** — the matcher behind Helix. Faster than `fuzzy-matcher`/`skim`,
correct on Unicode graphemes, and designed to match on a background thread pool
so typing never blocks.

**Index entries are `NodeRef`s, and every entity gets its own entry.**
Revision 1 folded flag names into the parent command's haystack, so searching
`--squash` selected `git rebase` rather than the flag. Since finding what a
tool documents is the product's core job (§1), and a tool documents more than
flags — `ar`'s modifier letters, `bpftrace`'s environment variables, a
command's positionals — every entity of every kind is its own index entry,
addressed by `NodeRef::Flag`, whose `key: FlagKey` now covers both shapes an
entity's name can take: `Long`/`Short` for a flag's dashed spelling, and
`Name` for a dashless entity's bare `primary_name()` (a positional's
placeholder, a modifier's letter, an environment variable's name — §4.5's
three dashless `EntityKind`s). Each entry's haystack is every documented
spelling's bare name, `value_name`, and description — the same shape for
every kind, dashless spellings indexed with no dash prefix. Selecting a
result selects the parent command and scrolls the detail pane to that
entity's own row, in whichever of FLAGS/POSITIONALS/MODIFIERS/ENVIRONMENT
section documents it (§9.3), exactly as a flag result always has.

**Two match modes, name-only by default.** Matching one combined haystack
(name + summary + description + entity value) is correct and looks
arbitrary: searching `branch` in `git` returns `switch` via "Switch
branches", and since only name matches are underlined, nothing on screen
explains why that row is there. `/` opens the box in name mode; pressing
`/` again toggles wide mode, the combined haystack, shown in the search
bar's title. Name mode is the default because its results explain
themselves. Name mode filters the index's own result set: a command
matches by a literal, case-insensitive substring of its own name; every
entity matches, with no per-kind branch, by a case-insensitive prefix of a
`-`/`_`-separated word of any of its spellings, the whole name counting as
its own first word, so `NODE_D` matches `NODE_DEBUG` from the start and
`debug` matches it from after the `_`. A looser subsequence test was tried
first and made the mode feel broken — searching `run` in `docker` surfaced
`--no-trunc`'s parent command, since `--no-trunc` contains r…u…n in
order — and the word-prefix rule refuses that case while still admitting
`no`, `trunc`, a bare modifier letter, or either half of an underscored env
var name. Wide mode's ranking is unaffected, and stays the fuzzy index,
where `gco` still finds `checkout`.

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

`cargo xtask coverage` runs extraction across every executable on `PATH` and
emits a scoreboard: tool, tier(s), nodes, flags, `%flags_text`, ms, status.
The scoreboard is checked into the repo and diffed on every parser change.

This is what makes "no per-tool adjustment" measurable rather than
aspirational. Without a fleet-wide scoreboard, a grammar change is judged
only against the tool someone happens to be looking at, and a fix to one
tool can silently regress another.

The regression gate: the `%flags_text` aggregate and the `no-tier` count may
never worsen. `%flags_text` is `described / describable`, not
`described / total` — a flag whose only source structurally cannot supply a
description (`Source::HelpTextSynopsis`) is excluded from the denominator
rather than counted as a miss. See Appendix B for the rename from
`pct_described`, and [M-15] for the measurement that forced it.

A **structure-sanity** column catches fabrication that a text-attachment
ratio cannot see on its own: it counts nodes whose name fails
`^[a-z][a-z0-9_.-]*$`, and nodes with no flags, no children, and no summary.
A tool with a nonzero count is marked `suspicious`, gated the same as
`no-tier`. See [M-10] for the defect this column exists to catch.

Two detectors re-examine text the pipeline already captured, adding no new
probes:

- **Misattribution** (`xtask/src/misattribution.rs`) flags a flag
  description that also contains another flag's literal spelling, attested
  at a column-aligned position elsewhere in the raw text — a multi-column
  options table read as a single column.
- **Existence** (`xtask/src/existence.rs`) checks that every help-text-sourced
  subcommand name and flag spelling occurs literally in the tool's own
  captured text, guarding against invented nodes.

Both report a scoreboard column and a footer field, and neither is gated:
a brand-new detector with no fleet baseline must not fail a build the first
time it runs. Every scoreboard also carries a literal `accuracy: unmeasured`
line, so a reader can never mistake presence-of-text for correctness. The
audit (§13.1c) is the only instrument that measures correctness.

A coverage metric that can be satisfied by the failure mode it exists to
detect is worse than no metric, because it converts a silent bug into a
confidently reported success. §13.1b states this as five rules; [M-21]
records the incidents that produced them.

### 13.1a The framework-support workflow

A CI workflow reports, per run, which frameworks mandible supports and how
well, rendered into the run summary. Two jobs:

1. **Framework matrix.** Install one representative tool per supported
   framework and assert that mandible identifies the framework and extracts
   a non-trivial tree.
2. **PATH sweep.** Run the coverage harness over the runner's own `PATH`,
   at zero installation cost.

The summary table carries, per framework: tools detected, flags extracted,
`%flags_text`, and pass/fail. The gate fails on regressions in `no-tier`,
`suspicious`, or framework-detection failures.

### 13.1b Metric design rules

A metric that is not monotone under added true information will eventually
punish a real improvement. `pct_flags_with_text` learned this the hard way
when a usage-synopsis grammar recovered thousands of real flags and the
ratio *fell*, because every recovered flag counted as undescribed against a
source that could never have described it ([M-15]). [M-21] records this
incident alongside four more of the same shape: an inflated ratio from
fabricated nodes ([M-10]), a conflated status from an unrelated property
([M-16]), a false regression from timing under load, and a name
(`%described`) that read as an accuracy claim it never earned.

Five rules follow, each keyed to one of those incidents:

1. **A gated metric must be monotone under added true information.** An
   improvement that adds correct data and loses nothing must never worsen a
   gated number.
2. **A denominator is conditioned on what the source could have provided.**
   A flag whose only source cannot supply a description is excluded from
   `%flags_text`'s denominator, not counted as a miss. Its spelling still
   counts in the raw, ungated flag total.
3. **A status derived under resource pressure states a fact about the
   machine, not the parser.** A wall-clock-derived signal must not silently
   flip a correctness gate. Where a timing assertion is not itself the
   safety property under test, it is demoted to a non-blocking warning with
   a wide margin; where it is (`exec::spawn`'s process-group-kill test), it
   stays blocking.
4. **A name is part of a metric's design, not decoration.** A name a reader
   could mistake for a stronger claim than the metric makes is a defect.
   `pct_described` was renamed `pct_flags_with_text` for this reason alone;
   the computation did not change.
5. **A mass status promotion must carry its own spot-audit stratum, drawn
   at random, never asserted from the aggregate that produced it.** A clean
   corpus and a clean sweep-diff prove nothing regressed; neither looks at a
   single promoted tool with a human eye. Any change promoting more than a
   handful of tools to `ok` must include a spot-audit of 5 to 10 randomly
   drawn promoted tools, recorded in the audit manifest as its own stratum.
   `xtask audit spot-audit --event <name> --promoted <tool,...> --sample <n>
   --draw-seed <seed>` draws reproducibly, via the same per-stratum seed mix
   the frozen queue uses, and tags each drawn tool with its own
   `spot-audit:<event>` row, distinct from the ordinary strata and from
   `forced-inclusion`. A promoted tool already present in the manifest is
   tagged into the new stratum without its prior verdict, note, or amendment
   history being touched; only `xtask audit amend` may change a verdict.

### 13.1c The audit instrument: comparing against truth

Misattribution and existence each compare the parser's output against
itself. `xtask audit` and `mandible --review` are the first instruments to
compare output against independently established truth: a human reads a
tool's own raw `--help` text beside the parsed tree and judges it.

Subcommands (`xtask audit <subcommand>`): `sample` draws and persists a
sample; `review` is the interactive terminal loop; `emit`/`ingest` are its
non-interactive twin, since CI has no tty (AGENTS.md §3.6) — `emit` writes
every pending pair to a file, `ingest` reads a verdicts file back; `report`
renders accuracy; `fixtures` turns a reviewed tool into a staged
`corpus/`-shaped fixture, a `correct` verdict becoming a real
`expected.snap` and a `wrong`/`incomplete` verdict becoming `[xfail]` with
the reviewer's note as `reason`. `mandible --review <SEED>` (§5.3) reviews
the same manifest inside the real TUI.

The draw is stratified, deterministic, and force-includable, via a frozen
queue (§13.1d): `xtask audit freeze` classifies every tool once and
shuffle-stratifies the result into an ordered queue, and `xtask audit
sample` advances that queue's cursor. A tool can additionally be
force-included, but only with a recorded reason
(`audit/force-include.txt`); force-included entries are tallied under their
own `forced-inclusion` stratum, never blended into the random draw.

Verdicts are `correct`, `incomplete`, `wrong`, or `skip`. A `wrong` or
`incomplete` verdict must carry a note — for those two verdicts the note is
the finding — enforced identically in the TUI and in `ingest`. `correct` and
`skip` do not require one. `skip` is recorded, occupying its slot and
appearing in `audit report`, excluded only from the accuracy ratio. Which of
`wrong` and `incomplete` a tool received is never load-bearing anywhere
downstream: `accuracy_over` collapses both into one judged-defect bucket,
the note requirement is identical for both, and a defect-family label is
derived from the note and the fixture, never from which word was chosen.

Three pre-tagged known-defect classes are computed at sample time and shown
to the reviewer before they record a verdict, so confirming is free and
overriding is one token:

- **K1**: a single-dash long option mis-parsed as a short flag plus a value
  (`-fdump-scos` stored as `-f` with value `dump-scos`). The same
  `short.is_some() && long.is_none() && value_name.is_some()` shape is also
  produced by a collapsed short-flag bundle and by a repeated flag letter
  (`-vv`); a detector for one fires on the other two unless it inspects what
  the value text actually is. All three now have a separate, calibrated
  detector and a shipped repair, ratchet-gated at zero. [M-21] has the
  fleet numbers.
- **K2**: the existence detector's own tokenizer gap, not a parser defect.
  Closed: characterized on a full sweep and repaired down to a small,
  genuine residual. [M-21] has the numbers.
- **K3**: a subcommand stub whose help was never fetched, because the
  attestation gate refused to probe a name with no recognized `--help`
  heading, or because the single-pass extraction never reached it.

**Display-only findings are excluded from the accuracy denominator, never
from the record.** A `wrong`/`incomplete` verdict sometimes lands on a
rendering defect (`mandible --review`'s own TUI mis-rendering a correct
extraction) rather than a parse defect. `skip` cannot record this, since the
defect was judged and real. The `display-only`
[`mandible_core::audit::DEFECT_FAMILIES`] label marks it, and
[`Entry::is_display_only`] excludes it from every accuracy view while the
verdict, note, and fixture stay exactly as recorded; `audit report` prints
excluded findings in their own section plus an `out-of-scope` column, so the
number cannot go quietly missing. `display-only` must be an entry's only
family: a genuine parse-shape family riding alongside it blocks the
exclusion rather than granting it.

`audit report` states accuracy per stratum with a Wilson 95% confidence
interval, never a bare percentage, and also reports accuracy with each known
class (K1/K2/K3) excluded, so a reader can see how much of a raw number is
attributable to an already-scheduled cause.

**Scope**: the audit measures flag accuracy and command/subcommand accuracy
only. A node's own prose description and usage-section formatting are out of
scope. A flag's description attached to the wrong flag is in scope, since
that is flag data misattribution; the node's own prose description is not.

`audit/<seed>.toml` is tracked, since an accuracy claim must carry its
evidence in git rather than depend on one contributor's machine.
`audit/<seed>/fixtures/` is not: it is staging output, reviewed and
deliberately promoted into `corpus/`.

The audit has not finished running. This section documents the instrument,
not a result: no accuracy number is stated here. The result belongs in
Appendix A as [M-20], once the audit completes.

### 13.1d The frozen sampling queue

Before this design, `xtask audit sample` reclassified the whole `PATH`
population on every draw, and because the strata were recomputed from
whatever the parser happened to be on the day, two draws taken apart in time
were stratifying against two different definitions of "ok" and were not
directly comparable.

The fix: freeze the tool list once, walk a cursor through it. `xtask audit
freeze` sweeps `PATH` (or a pinned `--tools` list) exactly once, classifies
every tool, shuffle-stratifies the result with a recorded seed, and writes
the ordered queue to `audit/queue.toml`. `xtask audit sample` only ever
advances that queue's cursor and merges the slice into a verdict file — no
re-probing, no reclassification, at draw time. The queue is ordered once and
a cursor advances through it; a draw never depends on which tools any
verdict file has already recorded, which is what keeps successive draws
comparable.

Three properties the design guarantees:

1. `queue.toml` records a freeze date and a population hash, so staleness can
   be detected (`xtask audit freeze --check`, a directory listing, no
   probing) without rewriting anything.
2. Each stratum is independently shuffled, then merged by a fractional rank
   within its own stratum, so any prefix of the frozen queue is itself a
   proportionally stratified sample, not just the queue as a whole.
3. `xtask audit freeze` persists every `(argv, output)` pair each tool's
   extraction pass recorded under `audit/queue-captures/`. `xtask audit
   reclassify` replays those bytes through the current parser via
   `mandible_extract::exec::Transcript`, with no `PATH` sweep and zero
   subprocess spawns, recomputing every tool's stratum in parallel. [M-21]
   has the measured cost.

`audit/queue.toml` is tracked; `audit/queue-captures/` is not — it is bulk,
machine-generated content, regenerable locally by re-running `xtask audit
freeze`. `--tools` lives on `freeze`, since `sample` no longer touches
`PATH` at all; `sample --seed` names which verdict file a slice merges into,
not a draw seed.

Honest caveats: a frozen population drifts from a machine's real installed
tools over time, and `freeze --check` detects drift without fixing it.
Reclassification updates a tool's reported stratum, never its position in
the queue, so a long-unfrozen queue's interleaving reflects its
freeze-time composition, not its current one — a real but much smaller
drift than the staleness this design replaces. Reclassification still reads
a tool's on-disk binary for framework fingerprinting, so it depends on the
binary resolving on `PATH` at the same path, even though it spawns nothing.

No new execution-safety surface: `freeze` issues exactly the probes the old
live sweep issued, all through `run_inert` (§6); `reclassify` spawns
nothing.

### 13.1e Family detectors and the calibration precondition

A **family detector** generalizes one human audit finding across the fleet:
the audit reads one tool at a time and is slow; a detector asks whether the
same shape occurs on every `PATH` tool, in seconds. `xtask detector`
(`xtask/src/detector.rs`) is the harness they register in.

A family detector is not a correctness instrument. The audit remains the
only instrument that touches truth; a detector's claim is narrower — this
same shape occurs here too — and that narrowness is exactly where the
danger is: a detector produces a confident fleet-wide count and nothing
inside that count knows whether the detector fires on the defect it names.

> A detector's fleet-wide number is not quotable until it has passed
> calibration against the human labels: it must fire on the known-bad tools
> and stay silent on the known-good ones. A detector that has not passed
> this check is measuring itself.

`mandible_core::audit::Entry` carries `families` — labels from the closed
`DEFECT_FAMILIES` set — alongside `families_derived`, an `Option<bool>`
recording that the labels are a machine reading of the reviewer's note and
fixture evidence, never the reviewer's own classification. A label with no
recorded provenance is refused, as is a label on a `correct` or `skip`
verdict.

**A family name that turns out to cover more than one shape must be split,
never detected.** A detector built over a symptom name rather than a shared
shape fires on whatever the author happened to encode and misses the rest,
naming a population no one can check — the same failure the calibration
precondition exists to prevent, arriving through the label instead of the
detector. Two names in this project's own defect backlog dissolved this way
once examined: each covered several unrelated defects sharing only a
symptom, and no detector was built for either. A third, `block-extent`,
turned out to be exactly one rule shared by two of its three candidate
tools, and a detector was built for those two. Per-family membership and
disposition are documented in `xtask/src/detector.rs`, not here.

**The confusion matrix has five cells, not four.** Beyond fires-on-bad,
misses, silence-on-good, and false alarms, there is *fires on a tool judged
defective of a different family* — neither a hit nor a false alarm, since
the human already said this parse is wrong. Every cell names its tools.
**Not-evaluable is counted, never dropped:** a labelled tool with no fixture
is listed by name; a matrix computed over part of the labelled set and
reported as complete is a worse claim than an incomplete one stated as such.
A detector may legitimately generalize no family the labelled set contains
(`Detector::family` returns `None`); forcing it onto the nearest family
would manufacture a matrix nobody verified.

**A fixed family inverts its own calibration.** The moment a family's fix
lands, its detector's recall on the labelled set drops to zero, because
those fixtures now parse correctly and the labelled set has nothing left to
confirm against. The precondition is a claim about labels recorded against a
particular parser, and it expires for a family on the commit that fixes it.
What carries the weight afterward is the detector's own hand-built tests
(`Detector::self_checks`, which construct the defective shape directly) and
`sweep-diff` against a fresh full sweep.

**`REPAIRED` is a third calibration verdict, reached only when calibration
has inverted and the detector's self-checks still hold**, covering both
directions: at least one case the detector must fire on and at least one it
must stay silent on. An empty self-check list is refused rather than passing
vacuously. `REPAIRED` is a stated claim, never a suppression: recall still
reads 0%, every missed tool stays named, and the self-check block prints on
every run, including runs that do not reach `REPAIRED`.

**A ratchet gate asserts the detector alongside the count.** Once a family
is repaired, its fleet count is gated at a literal zero (`coverage --check`,
`detector::ratchet_at_zero`), never against the checked-in scoreboard, which
a reintroducing commit could otherwise edit to raise its own baseline. The
gate requires the same self-check evidence `REPAIRED` does and refuses a
zero without it, so a gate asserting `count == 0` cannot be satisfied by
deleting the detector.

**A declared scope exclusion carries a structural predicate, not prose.**
`Scope::known_exclusions` is a closed `Ground` enum, each variant carrying a
witness token from the tool's own help text plus the constant it falls
below; the arithmetic is computed from the witness and has to agree. Prose
survives only as a `note` printed beside the generated sentence, never
instead of it.

**Calibration can find a mislabel, and finding one is the mechanism
working.** A false alarm is never waived: it is either a detector bug or a
label bug, and which one is argued in the commit that resolves it, using
`xtask audit amend` to correct the label with its reason recorded.

### 13.1f Residue ranking: a discovery instrument, deliberately not a metric

`cargo run -p xtask -- residue` (`xtask/src/residue.rs`) is existence's
complement: existence asks whether everything in the tree is attested by the
text and catches invention; residue asks what in the text the tree never
accounted for, and catches omission. It classifies each physical line of a
captured `--help` document by shape (a flag row, or an indented
`name<gutter>description` row) and reports the rows no spelling or name in
the parsed tree accounts for. It replays frozen fixture bytes and spawns
nothing.

It is not, and must never become, a gate or a quotable number: a wrong
residue candidate costs review time and cannot produce a wrong parse,
because nothing downstream reads it. The moment a residue count is treated
as a measurement, that asymmetry is gone. Nothing in `coverage --check`
consults it, it appears in no ratchet and no `corpus` contract, and a test
fails the build if `coverage.rs`, `corpus.rs`, or `status.rs` ever calls
into it. Its output is a reading queue for a human, who turns a confirmed
finding into a deterministic, calibrated, ratchet-gated rule the ordinary
way. [M-22] has what it found the one time it was run over the full audited
set, including a real four-flag gap in a fixture that had been green,
blessed, and contract-gated throughout.

### 13.2 Fixed corpus

A fixture (`corpus/<tool>/<version>/`) freezes both halves of one extraction
pass: the raw bytes a real probe produced, byte-exact
(`.gitattributes` marks everything under `corpus/` `-text`, so Git's own
line-ending normalization can never quietly alter a capture), and the
`CommandNode` tree the real pipeline produces from those bytes today,
replayed with zero subprocesses through the same `Transcript` seam §13.1c
and §13.1d use. Snapshotting only the tree is not enough: an IR-only
snapshot can only assert "the tree once looked like this," with nothing to
re-derive from after a tool version bump or a grammar rewrite. A fixture is
filed by tool and version only, never by tier. `corpus/README.md` has the
full layout and the `meta.toml` contract: a descriptive half
(`expected.snap`, rewritten wholesale by `--bless`) and a normative half
(`[contract]`, weakened only by an explicit, reviewed edit).

`[contract]` can state a negative as well as a positive: `must_not_contain_flags`
asserts that a spelling (a matched long name, short flag, or bare word) is
absent from the root, guarding against invention the same way
`must_contain_flags` guards against omission. A tree with no root satisfies
it vacuously and is not reported, so a missing tree cannot pass by accident
in the one gate whose authority depends on never doing that.

`verdict_scope` records which dimensions of the tree a human actually
looked at before blessing it, some subset of `"flags"`, `"subcommands"`,
`"descriptions"`, `"usage"`. Absent means no scope was claimed, never every
scope: a bless freezes every field whether or not a human read it, so
treating silence as "everything verified" would let the same overclaim that
cost this project a fixture (`lsof`) survive by omission.

Strict xfail: an `[xfail]` fixture whose snapshot and every `[contract]`
field now pass fails the run rather than passing quietly. A fixture marked
broken that stops being broken means the bug looks fixed while the label
still says otherwise, and the run demands the label be removed. Both
directions are checked on every run, not only "did it get fixed": a fixture
claiming to be broken while every check passes is as much a bug as an
unmarked regression, and a promoted fixture's contract is strengthened, not
merely unmarked, when the fix's own evidence supports a stronger claim.

Current scale and provenance are in [M-22].

### 13.3 Required test classes

- **Real-argv tests.** Every tier needs at least one test that exercises the
  actual argv construction, not just the parser behind it. A prior cobra
  implementation omitted the literal `__complete` from its argv and was
  silently dead in the real pipeline, because its unit tests injected a mock
  probe that bypassed argv construction entirely.
- **Execution-policy tests** (§6): a shim binary logs argv/env; any
  invocation outside the allowlist fails the suite.
- **Sanitization tests**: ANSI, C0, backspace-overstrike, tabs, embedded
  newlines, CJK/emoji width, and a 10 MB pathological string.
- **Render tests** against `ratatui::backend::TestBackend`, asserting that
  border cells stay intact for adversarial description text at several
  widths and scroll offsets.
- **Fuzzing** the Tier B grammar (`cargo-fuzz`), since it consumes untrusted
  text.
- **Merge property tests**: merge is associative over authority; a `None`
  never displaces a `Some`; alias pairing is idempotent.

The workspace runs under `cargo nextest run --workspace` in CI, never
`cargo test --workspace` piped into a text-processing tool, because
human-format test output must never be parsed by anyone for any reason: a
`grep -c FAILED` against `cargo test`'s output once false-positived on test
data that happened to contain the literal word "FAIL." `cargo nextest run`
reports a real exit code and can emit `--message-format libtest-json` when a
structured result is needed. Nextest cannot run doctests, so CI runs a
separate `cargo test --doc --workspace` step to cover them.

### 13.4 The detect-to-fix loop, end to end

§13.1–§13.2 introduce five instruments at five points. They compose, in this
order:

1. **Corpus fixtures** (§13.2) — per-document. Frozen bytes plus the tree
   they should produce; `cargo xtask corpus` catches a regression on one
   tool someone already looked at, with zero subprocesses.
2. **Sweep-diff** (`xtask sweep-diff`) — fleet-wide: a semantic diff between
   two full-`PATH` scoreboards, gains and losses always reported as two
   separate totals, never netted, since summing them hides exactly the
   losses that motivated building it. It answers whether a fix broke
   anything else, and is non-blocking by design: it always exits 0, and
   there is no flag to wire it to a nonzero exit by accident.
3. **Oracles** — existence and misattribution (§13.1) — fleet-wide
   self-consistency checks. Neither compares against a tool's real behavior;
   both re-examine text the pipeline already captured.
4. **Audit** (§13.1c) — sampled, and the only instrument in this list that
   touches truth: a human reads a tool's own raw `--help` text beside the
   parsed tree.
5. **Family detectors and calibration** (§13.1e) — generalize one human
   finding across the fleet, quotable only once calibrated against the
   audit's own labelled verdicts.

The loop: an audit finding gets a family label, derived from the reviewer's
note and the fixture. A detector generalizes that label's shape across the
fleet. The detector is calibrated against the labelled verdicts. Only once
calibrated does its fleet-wide count become quotable. The count motivates a
grammar fix. The fix flips the family's `[xfail]` fixtures to passing, which
strict xfail reads as a demand to promote them. Sweep-diff proves the fix
broke nothing else. The detector's fleet count is ratchet-gated at zero
going forward, so a future regression in that family is visible the moment
the count leaves zero. [M-21] has a worked example, start to finish.

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
packaging/         debian/, rpm/, shell/ (the --shell-init snippets),
                   mandible.1 (man page for mandible itself)
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
- Ship completions for mandible itself and `packaging/mandible.1`, installed
  to the standard paths per shell (zsh's path differs by distro: Debian
  carries `vendor-completions`, Fedora `site-functions`). Every channel
  generates them from the built binary's own `--completions <shell>`, so
  there is one generator and no packaging path that can install a file the
  shell will not find.
- Every argument that names a tool (the `TOOL` positional, `--doctor`'s
  value, `--report`'s value) completes to the command names on `$PATH`,
  never to filenames, since each one is a program mandible is about to run
  `--help` on. `SUBCOMMAND` words after `TOOL` are names inside one tool's
  tree and are not completed this way.
- The shell integration (§2's `--print-selection` binding) ships the same
  way and installs to no path at all: `mandible --shell-init <shell>`
  prints it, from a snippet compiled into the binary
  (`mandible/shell/`, inside the crate root so the published package
  carries it), and the user opts in with `eval "$(mandible --shell-init
  bash)"` in their rc file. The one-generator rule applies for the same
  reason it does to completions, but the install half does not: no shell
  auto-loads a key binding the way it auto-loads completions, and a
  package binding `Ctrl-X m` for every user of a machine would be taking a
  key nobody asked it to.
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
   extraction is eager [M-3]. Mitigated by lazy per-node extraction (§5.2),
   which must exist early, not in a polish phase; there is no cache (§11).
2. **Running other people's binaries can damage a machine.** Mitigated by §6, and
   §6 is only real because `exec/` is the sole module allowed to spawn processes
   and a test enforces it.
3. **Description coverage is the actual product value, and only per-framework
   grammars (§7 Tier B) and man-page enrichment (§7 Tier D, opt-in) supply it
   well.** A tool with no man page and an undetected framework — every
   internal company CLI, precisely the case "universal" is for — renders as
   names with sparse prose. This is the honest limit of the design and the UI
   must show it rather than hide it.
4. **Framework-profile drift across tool updates.** A framework's own
   `--help` template can change between versions; a profile has no
   automatic refresh beyond the fingerprint and grammar staying general
   (§7 Tier A′, Tier B).
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

  **Correction (2026-08-17): the "empty word returns subcommands only" half of
  this entry is wrong, and was only ever measured at nodes that *have*
  subcommands.** cobra answers `__complete <path> ""` by emitting the node's real
  subcommands **and then appending whatever that command's `ValidArgsFunction`
  returns** — application code that reads live state. At a leaf there are no
  subcommands, so the response is *entirely* argument data. Measured on docker
  29.7.2, this box, with three `hello-world` containers present:

  ```console
  $ docker __complete container ""   # a node with subcommands
  attach<TAB>Attach local standard input, output, and error streams to a container
  commit<TAB>Create a new image from a container's changes
  ...
  :4
  $ docker __complete stop ""        # a leaf: RUNNING CONTAINER NAMES, bare
  mandible-canary-1
  mandible-canary-2
  :4
  $ docker __complete run ""         # a leaf: image names, bare
  vsnote:latest
  hello-world:latest
  :4
  ```

  This was a live defect, reported from real use: the reporter's own container
  names were rendered in the tree as docker subcommands, and each fabricated node
  was then warmed like any other (§5.2 step 4), multiplying the probe count by
  the size of a set that scales with the user's data rather than with the tool.

- **[M-2a] Which cobra candidates are really subcommands.** Method: walk both
  installed cobra binaries breadth-first to depth 3 via `__complete <path> ""`,
  classifying every response list as all-described, all-undescribed, or mixed,
  and reading each list against the tool's real command tree. Result, over **631
  distinct command paths** (`docker` 29.7.2: 253; `gh` 2.45.0: 378):

  | list shape | count | what they actually were |
  |---|---|---|
  | every candidate described (`name<TAB>text`) | 85 | genuine subcommand lists, all 85 |
  | every candidate undescribed (bare `name`) | 45 | argument data, all 45 |
  | mixed | 5 | argument data, all 5 |

  The 50 non-subcommand lists were container names, image names and tags, network
  names, buildx builder names, docker context names, and `gh project
  field-create --data-type`'s enum values. **No real subcommand appeared
  undescribed, and no argument value appeared in an all-described list**, which
  is why Tier E accepts a candidate list only when every candidate in it carries
  a non-empty description (§7 Tier E). The mechanism behind the rule is cobra's
  own formatting: it writes subcommand rows with `fmt.Sprintf("%s\t%s",
  subCmd.Name(), subCmd.Short)`, while a `ValidArgsFunction` returning a plain
  `[]string` produces bare rows.

  The 5 mixed lists were all `docker context <verb> ""`, whose completer
  decorates only the *currently selected* context (`rootless<TAB>current`) and
  leaves the rest bare. They are why a single undescribed candidate has to
  condemn the whole list rather than only itself: cobra marks no boundary between
  its own subcommand block and the appended argument block, so a described entry
  sitting in a list that also carries bare entries cannot be attributed to
  either.

  **Known residual, measured, not closed.** A completer that describes *every*
  value it returns is indistinguishable from a subcommand list in cobra's wire
  format. `docker context use ""` hits this on a machine with exactly one
  context, where the sole candidate is `default<TAB>current`; with two or more
  contexts the list is mixed and the rule closes it. Closing the one-context case
  would need a signal cobra does not provide.

  Two alternative designs were measured and rejected. (a) Probing cobra's own
  `help` command — `__complete help <path> ""`, which enumerates the command tree
  and never runs a `ValidArgsFunction` — is exactly right on `gh` (`gh __complete
  help pr ""` returns `pr`'s 20 real subcommands) and returns **nothing at all**
  on `docker`, which replaces cobra's help command. (b) Differencing against a
  sentinel positional — `__complete <path> <sentinel> ""` suppresses cobra's
  subcommand block, so `A \ B` is the subcommand set — works for 6 of 8 sampled
  paths, costs a third spawn per node (+50%), and still leaks `docker context
  use`, whose completer returns the same list either way.
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
  every precision tightening in `help_text/sections/` (the apt-get prose
  rule, the mysqlslap same-indent rule, the curl usage-continuation rule)
  could pay for itself in recall elsewhere on `PATH` and nothing would say so.
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

- **[M-21] Metric-design incidents and the defect-family detector fixes**
  (2026-08 batch, seed-2 audit of 94 tools). Five incidents produced §13.1b's
  five rules: [M-10]'s fabricated `tar` nodes inflated `%described`; [M-16]'s
  `verbatim` status conflated "nothing parsed" with "printed a man page," a
  fifty-times overestimate (314 vs 6); `waagent2.0` flipped `ok` → `verbatim`
  between two runs of identical code as its elapsed time halved (41.9s →
  21.4s) near a 10s extraction cap, a machine-load artifact rather than a
  regression; [M-15]'s synopsis-flag recovery fell foul of an unconditioned
  denominator; and `pct_described` read as an accuracy claim it never made.
  After the fixes, `pct_flags_with_text` on a 2,266-tool `PATH` sweep reads
  94.19%, within 0.01 of the pre-[M-15] figure, with the recovered flags kept
  in the raw total and 204 tools moving `low-confidence` → `ok`.

  K1's three sub-shapes (single-dash-long, bundled-short-flag,
  repeated-char-flag) are now each detected and repaired separately, ratchet-
  gated at zero. Fleet counts at time of fix: single-dash-long 132 tools,
  8,784 flags (17.6% of all flags mandible extracted); bundled-short-flag 58
  tools, 465 flags, closing at 0/0 with 489 flags gained across 67 tools on
  the after-sweep; repeated-char-flag closed 5 fixtures
  (`killsnoop.bt`, `naptime.bt`, `opensnoop.bt`, `tcpaccept.bt`,
  `threadsnoop.bt`). K2 (the existence detector's own tokenizer gap) was
  characterized on a 2,302-tool sweep at 656 false fabrications, of which 613
  (93%) were three detector-artifact classes now fixed, leaving 43 genuine
  parser defects (42 a mis-merged GCC alias, one an invented `dockerd -h`).

  The bundled-short-flag fix is the worked example of the full detect-to-fix
  loop (§13.4): audited at 6 tools, detected fleet-wide at 58, fixed, and the
  detector's own calibration against the labelled set immediately inverted to
  0% recall as those fixtures started parsing correctly — the expected
  behavior a fixed family's calibration takes on (§13.1e).

  `xtask audit reclassify`'s replay-from-frozen-captures redesign was
  measured directly: a naive serial pass over a 500-tool `PATH` slice on a
  4-core machine took 135s, longer than the ~123s live-probing freeze it
  replaced; a `rayon`-parallel pass took ~65s, roughly half. The win is the
  removal of subprocess spawns and probe-timeout cost, not an unconditional
  "seconds regardless of scale" claim; what remains scales with population
  size and CPU, not with probe count times timeout.

- **[M-22] Residue ranking findings, and corpus fixture scale** (seed-2
  audit set, 84 tools with a verdict and a capture). Residue ranking found
  unaccounted rows on 12 tools; all 12 were already labelled `wrong` or
  `incomplete`, and none of the 32 tools judged `correct` left any residue —
  a low-recall, high-precision signal (8 of 43 defective tools reached the
  ranking threshold). It also found a real gap in a fixture that was green,
  snapshot-blessed, and contract-gated throughout: `corpus/tar/1.35` was
  missing four real flags (`--old-archive`, `--portability`, `--pax-option`,
  `--posix`), nested inside a choice block whose parser never dedented
  before resuming the options table — the `bare_block_end` fix (a bare block
  ends where a flag row resumes) closed it and was confirmed to change
  exactly two trees fleet-wide (`tar` and `sg_dd`) across all fixtures.

  Corpus scale: 82 fixtures, 68 passing, 14 `[xfail]`, 0 unexpectedly
  failing, as reported by `cargo run -p xtask -- corpus`. Eleven are
  hand-captured against a real installed version (`git`, `tar`, `curl` at two
  versions, `du`, `gcc`, `ffmpeg`, `lsof`, `unzip`, `zoxide`,
  `mariadb-check`); the other 71 are `audit-seed2` fixtures staged from the
  seed-2 human audit via `xtask audit fixtures`.

- **[M-23] Evidence-gating cost and benefit, Tiers C and E.** Tier C's
  completion-script probe, sent unconditionally before its evidence gate
  existed, cost a full-screen program with no `completion` subcommand two
  `PROBE_TIMEOUT` waits: `vim.basic` measured 20,304 ms before the gate,
  287 ms after; `bashbug` 20,657 ms → 49 ms; `jconsole` 40,081 ms →
  22,065 ms (the remainder is Tier B's own `--help`/`-h` pair, not this
  tier); `docker-proxy` 239 ms → 221 ms. Across 19 hand-picked real tools
  including those four, `--doctor` output was byte-for-byte identical
  before and after, and Tier C contributed to none of them either way in
  this sample — a sample, not a fleet measurement.

  Tier E's `__complete` probe, gated to cobra-identified binaries only:
  measured full-`PATH`, 2,248 tools joined, before and after, zero status
  transitions, zero flag-count changes, identical aggregate
  `pct_flags_with_text` (94.83% both sides). Tools eligible for the probe
  fell from every tool swept (2,302) to 5 (`docker`, `dockerd`, `gh`,
  `git-lfs`, `ollama`) — the speculative form was contributing nothing to
  extraction while carrying the whole risk of a bare invocation reaching an
  unrelated tool.

- **[M-24] Process-containment residuals.** A developer box accumulated 622
  orphaned processes from probes whose descendant daemonised and left the
  process group: `blkmapd` ×148, `rpc.idmapd` ×144, `rpc.gssd` ×144, plus
  `sudo_logsrvd` listening on `0.0.0.0:30343` and `[::]:30343`, `guacd` on
  `127.0.0.1:4822`, and `pam-auth-update` burning a full core for three
  days, the oldest five days old. Not a hang: all 2,302 `probe-start` lines
  in a traced sweep had a matching `probe-done`, so every probe returned
  normally. The child-subreaper reap (§6 rule 4) closed this.

  Separately, the scratch-directory redirect (§6 rule 8) does not stop a
  probe that daemonises from surviving past the timeout, since the timeout
  only kills the process group. A CI sweep loses roughly three shards in
  sixteen to the runner being reclaimed; instrumenting both sides named the
  tools that start and never finish: `chromedriver` (starts a
  browser-driver server), `vimtutor` (launches vim), `ghci` (opens a REPL),
  `syscount.bt` (attaches kernel probes). This residual is documented, not
  closed, and needs OS-level sandboxing to close fully.

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
