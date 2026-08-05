# mantui

**A universal, interactive TUI reference for CLI tools, in Rust.**

```
$ mantui git
```

opens a full-screen, explorable tree of every command, subcommand, and flag
`git` has — with descriptions, not just names — plus a search bar. It is a
*reference browser*, not a command builder: the product's job ends the
moment you've found the flag and can `y`-copy its exact spelling.

The full design rationale, measured baselines, and the reasoning behind every
non-obvious decision below live in [`spec.md`](./spec.md). This README is the
short version.

## Status

This is an early, in-progress implementation (spec roadmap phases 0-1 of 6).
**What works today:**

- A complete intermediate representation (`mantui-core`) with sanitized text,
  per-field provenance, and two-axis authority merging.
- **Tier A**: the [carapace-spec](https://github.com/carapace-sh/carapace-bin)
  catalog — 740 tools, tens of thousands of flag descriptions — served from a
  byte-indexed vendored snapshot with no subprocess cost.
- A full `ratatui` TUI: tree pane, detail pane, search bar, status bar,
  keybinding overlay, mouse support, responsive layout.
- An on-disk cache (spec §11) so repeat launches are near-instant.
- `mantui --doctor <tool>`, a non-interactive diagnostic.

**What's explicitly not built yet** (see spec.md §12 for the roadmap):
`--help` text parsing (Tier B), completion-script parsing (Tier C), man page
extraction (Tier D), native dynamic probes (Tier E), user overrides (Tier F),
lazy/incremental extraction, and real fuzzy/flag-aware search (`mantui-search`
is currently a stub — the search bar does simple local substring filtering).

The honest coverage story: **today, mantui is genuinely useful for the ~740
tools in the carapace catalog** (`git`, `docker`, `kubectl`, `gh`, `curl`,
and hundreds more) and shows an honest "no tier could extract this" for
everything else. Later phases close that gap without ever special-casing a
tool by name — see the invariant below.

## The invariant

> The mantui repository will never contain per-tool logic. No
> `if tool == "docker"`, no vendored per-tool patch file, no tool-name-keyed
> special case in any tier. Tool-specific knowledge lives in exactly two
> places: (a) third-party structured catalogs consumed wholesale as *data*,
> and (b) user-local override files that are never checked into this repo.

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Install

```
cargo install --path mantui
```

(Not yet published to crates.io — see spec.md §12 phase 6.)

## Usage

```
mantui <tool>              # open the interactive tree for <tool>
mantui <tool> --refresh    # bypass the cache and re-extract
mantui --doctor <tool>     # non-interactive diagnostic: tiers, counts, timing
```

Keybindings (also shown in-app via `?`):

| Key | Action |
|---|---|
| `↑`/`↓`, `k`/`j` | Move tree selection |
| `→`/`Enter`/`l` | Expand |
| `←`/`h` | Collapse, or jump to parent |
| `/` | Focus search |
| `Esc` | Leave search (pin filter), `Esc` again clears it |
| `Tab` | Switch focus between tree and detail pane |
| `y` | Copy the selected flag's spelling or the node's command path |
| `?` | Keybinding overlay |
| `r` | Re-extract, bypassing cache |
| `.` | Toggle hidden/deprecated items |
| `q`, `Ctrl-C` | Quit |

## Building from source

```
git clone <this repo>
cd mantui
cargo build --release
./target/release/mantui git
```

Default features build with no network access and no C toolchain (spec
§15). The `manpage` feature (Tier D, deferred) needs a C toolchain and is
off by default.

## Where the data comes from

`vendor/carapace-specs.json` is a vendored, point-in-time snapshot of the
[carapace-bin](https://github.com/carapace-sh/carapace-bin) project's
declarative command specs, produced by `scripts/vendor_carapace_specs.py`.
See [NOTICE](./NOTICE) for full attribution and license text, and
`mantui --doctor <tool>` for the snapshot's vendoring date and commit.

## License

MIT — see [LICENSE](./LICENSE). Vendored third-party data is separately
attributed in [NOTICE](./NOTICE).
