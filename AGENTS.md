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

Tool-specific knowledge lives in exactly two places: third-party catalogs
consumed wholesale as *data*, and user-local override files under
`~/.config/mantui/overrides/` that are never committed here.

If a tool renders badly, the fix is a better general parser, a new general tier,
or an honest low-confidence badge in the UI. It is never a special case. This is
the entire reason the tiered architecture exists; one exception starts the
erosion that the architecture was built to prevent.

---

## 2. Architectural invariants

Breaking any of these produces a bug that tests will not catch.

| Invariant | Where | Failure it prevents |
|---|---|---|
| `Text::sanitize` (or `sanitize_markdown`) is the **only** way untrusted text enters the IR | `mantui-core/src/text.rs` | Control chars and markup reaching a `ratatui::Span`, which corrupts pane borders. Two widget-level fixes for this failed before the boundary fix worked. |
| Widgets may **assume** `Text` is clean | `mantui-tui` | Re-implementing defenses in each of the three consumers (tree, detail, clipboard), inconsistently |
| `std::process` appears **only** in `mantui-extract/src/exec/` | enforced by `tests/no_process_outside_exec.rs` | Unaudited subprocess spawning; §6 of the spec becomes unenforceable |
| Provenance is **per field**, never per tree | `mantui-core/src/provenance.rs` | A trust badge that lies after a multi-tier merge — worse than no badge |
| Extraction is **node-scoped** (`extract_node`), never whole-tree | `mantui-extract/src/tier.rs` | Eager extraction: 232 subprocesses and 10.5s for `docker`. Do not reintroduce a whole-tree `extract()`. |
| **One node = exactly one tree row.** No wrapping in the tree pane | `mantui-tui/src/render/tree_pane.rs` | Row index ↔ node stops being a bijection, breaking selection, scrolling, mouse hit-testing, and filtering all at once |
| Truncate by **display width** (`unicode-width`), never `char` or byte count | `mantui-tui` | CJK/emoji overflow the border by one cell per wide character |
| Cache keys must depend on **extraction logic**, not just crate version | `mantui-cache/src/key.rs` | See §3.2 — this shipped as a real bug |

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

**Use the pty harness** — it forks a real pseudo-terminal, sets an explicit
window size (this is the part naive attempts miss; without `TIOCSWINSZ` the pty
is 0×0 and ratatui renders nothing), and replays output through a terminal
emulator to give you the actual screen:

```bash
python3 -m venv /tmp/ptyvenv && /tmp/ptyvenv/bin/pip install pyte
/tmp/ptyvenv/bin/python scripts/pty_screenshot.py 100 28 ./target/release/mantui git
```

This is the only way to see rendering defects that depend on real data. It is
how the markdown leak and the ragged re-wrap were found — both invisible to
`TestBackend` tests, which use synthetic fixtures.

`TestBackend` tests are still required (see the border-integrity suite). They
catch structural regressions; the pty catches *content* regressions.

### 3.3 The `--refresh` trap

When changing extraction, parsing, or sanitization, **always verify with
`--refresh`** until the cache key is derived from extraction logic. Otherwise
you are looking at output produced by the code you just replaced, and you will
conclude a correct fix is broken.

*Delete this section once the cache key includes a source fingerprint.*

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
- The vendored catalog is **739 tools** with ~48k flag descriptions.

If you measure something that contradicts Appendix A, the measurement wins —
update Appendix A in the same commit, with the method.

---

## 5. Working agreements

- **Commit per unit of work, not per session.** A session limit once killed 220
  uncommitted lines and left the tree not building. An interim commit that
  compiles beats an uncommitted one that does not.
- **Never regenerate or reformat `vendor/carapace-specs.json`.** It is an 11 MB
  third-party snapshot; reproducing it needs a `carapace-bin` clone. Its
  `_meta.commit` is load-bearing for attribution and cache keying.
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
is a completed task, not a loss — §3.3 is already marked for deletion. Review
the whole file whenever you finish a batch of work.

**Do not duplicate `spec.md`.** Link to it. Duplication means two sources that
will disagree, and the disagreement will be discovered at the worst time.

**Keep it under ~200 lines.** If it grows past that, something belongs in
`spec.md` (design), `CONTRIBUTING.md` (human process), or the bin.

**Date-stamp anything environment-dependent**, and re-verify rather than trust
it. Facts about other people's tools go stale.
