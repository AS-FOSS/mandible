# Contributing to mandible

Thank you for considering a contribution. Please read this before opening a
PR — the project has one rule that overrides most instincts about how to fix
a bug.

## The invariant (read this first)

> **The mandible repository will never contain per-tool logic.** No
> `if tool == "docker"`, no vendored per-tool catalog, no tool-name-keyed
> special case in any extraction tier. Tool-specific knowledge lives in
> exactly one place: user-local override files
> (`~/.config/mandible/overrides/<tool>.toml`) that are **never** checked into
> this repository.

The unit of knowledge here is the **framework**, not the tool. Help text is
generated, not hand-written, and only a small closed set of generators
exists — so a grammar fix for argparse improves every Python CLI ever
written, while a catalog entry improved exactly one tool until it went
stale. Adding framework support is one `match` arm in
`mandible-extract/src/help_text/profile.rs` plus one fingerprint in
`mandible-extract/src/framework/`.

If a change would violate that invariant, the correct fix is one of:

1. Improve a general parser (the Tier B `--help` grammar) so it handles a
   whole *class* of tools better, not just the one you're looking at.
2. Teach it a new *framework* — bounded, ~18 entries, and the right
   granularity.
3. Add or extend a general extraction tier.
4. Accept the gap and let it degrade honestly: an unparseable tool is
   rendered verbatim and labelled, never given invented structure.

A single tool-name-keyed exception starts the erosion — the next contributor
who hits a hard tool will reasonably assume it's the established pattern.
See spec.md §1 and §3 for the full reasoning.

## Before you start

Read [`spec.md`](./spec.md) in full. It is the authoritative design
reference — every non-obvious decision in this codebase traces back to a
measurement or a stated tradeoff in that document (see its Appendix A for
the measured baseline, and Appendix B for what changed between revisions).
If your change disagrees with something in spec.md, that's worth raising as
its own discussion before code — don't let the code and the spec drift apart
silently.

## Workspace layout

See spec.md §8 for the full crate architecture and the reasoning behind it.
Key structural rules, enforced by tests, not just convention:

- **`std::process` may only appear in `mandible-extract/src/exec/`.** A test
  (`mandible-extract/tests/no_process_outside_exec.rs`) greps the whole
  workspace source tree and fails the build if this is violated anywhere
  else. This is what makes the execution-safety policy (spec §6) auditable.
- **Every string from outside this process must go through
  `mandible_core::Text::sanitize`.** `Text`'s field is private with no
  `From<String>`, so this is structurally enforced, not just documented.
  Widgets are allowed to assume a `Text` is clean.
- **Provenance lives on `CommandNode`, `Flag`, and `Positional`
  individually** — never one badge for a whole tree. See spec §4.2 for why
  a single per-node badge is actively misleading after a multi-tier merge.

## Execution safety (spec §6)

Any code that spawns a subprocess against a user's installed tools must:

1. Never invoke a bare binary — only the fixed set of inert argv shapes in
   `mandible_extract::exec::InertArgv`.
2. Set stdin to `/dev/null`.
3. Enforce a wall-clock timeout and kill the whole process group on expiry.
4. Cap combined stdout+stderr at 8 MiB.
5. Run under a sanitized environment (no `PAGER`/`LESS`/`MANPAGER`, etc.).
6. Never pass an argument that could cause the tool to write a file.

Adding a new inert argv shape requires touching `InertArgv` deliberately (a
closed enum, by design) — that friction is intentional per spec §6 rule 2.

## Testing expectations

- Unit tests live next to the code (`#[cfg(test)] mod tests`); integration
  tests live in each crate's `tests/`, with large fixtures in
  `tests/fixtures/`, not inlined as string literals.
- A change to sanitization, tree rendering, or extraction merging should
  come with a test that would have caught the bug it fixes — several such
  tests exist specifically because of prior regressions (see spec §13.3,
  "Real-argv tests" and the border-integrity render tests).
- Run before opening a PR:

  ```
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

## Style

- `#![forbid(unsafe_code)]` on every crate. No exceptions in this codebase;
  if you believe you need `unsafe`, that's a discussion, not a PR.
- `#![warn(missing_docs)]` on library crates — every public item is
  documented.
- `thiserror` in libraries, `anyhow` only in the `mandible` binary.
- No `unwrap()`/`expect()` in a library code path reachable by tool input.
  `expect()` is fine only for genuinely-infallible cases, with a comment
  explaining why.

## Releasing (maintainers)

Releases are cut **manually**, on purpose. Tagging is the one step where a
human decides that a given commit is worth making permanent — publishing to
crates.io cannot be undone, only yanked — and no bot has better judgement
about that than you do.

```console
# 1. Move the accumulated Unreleased notes under a new version heading in
#    CHANGELOG.md. The release body is generated from that section:
scripts/changelog_section.sh 0.2.0        # check what it will say

# 2. Bump `version` in the workspace Cargo.toml (all five crates share it),
#    commit, and let CI go green on main.

# 3. Tag — signed, so the release shows as Verified on GitHub:
git tag -s v0.2.0 -m "v0.2.0" && git push origin v0.2.0
```

The tag triggers `.github/workflows/release.yml`, which will not build
anything until CI and the framework matrix both pass, then produces
binaries for four targets, `.deb`/`.rpm`, a GitHub Release, and finally
publishes all five crates to crates.io in dependency order (`mandible-core`
→ `mandible-search` → `mandible-extract` → `mandible-tui` → `mandible`).

Two things that will bite otherwise:

- **All five crates must share a version and be published together.**
  Publishing `mandible-tui 0.2.0` while it still depends on
  `mandible-core 0.1.0` is accepted by crates.io permanently.
- **Re-pushing a tag supersedes the previous run** (the workflow has a
  `concurrency` group), so fixing a broken release is: fix, force-tag,
  push. Nothing is published until every earlier job has passed.

## License

This project is dual-licensed under [MIT](./LICENSE-MIT) or
[Apache License, Version 2.0](./LICENSE-APACHE), at your option. By
contributing, you agree your contribution is licensed under both, without
any additional terms or conditions, unless you explicitly state otherwise.
If your change vendors new third-party *data* (not just a crate
dependency), it needs an entry in [NOTICE](./NOTICE) with the source,
commit, and verified license text — see spec.md §14/§15 for why this
matters as much as crate licensing.
