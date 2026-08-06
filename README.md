# mandible

**A universal, interactive TUI reference for CLI tools, in Rust.**

```
$ mandible git
```

opens a full-screen, explorable tree of every command, subcommand, and flag
`git` has — with descriptions, not just names — plus a search bar. It is a
*reference browser*, not a command builder: the product's job ends the
moment you've found the flag and can `y`-copy its exact spelling.

The full design rationale, measured baselines, and the reasoning behind every
non-obvious decision below live in [`spec.md`](./spec.md). This README is the
short version.

## Screenshot

A real capture (`scripts/pty_screenshot.py`, not fabricated output) of
`mandible git`, after expanding into `git cat-file`:

```
╭ search ──────────────────────────────────────────────────────────────────────────────────────────╮
│›                                                                                                 │
╰──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ git ───────────────────────────────────────────╮╭ git › cat-file ────────────────────────────────╮
│▾ git                the stupid content tracker ││Provide content or type and size information for│
│    add              Add file contents to the…  ││repository objects                              │
│    am               Apply a series of patches… ││                                                │
│    apply            Apply a patch to files…    ││DESCRIPTION                                     │
│    archimport       Import a GNU Arch…         ││Output the contents or other properties such as │
│    archive          Create an archive of…      ││size, type or delta information of one or more  │
│    backfill         backfill missing objects…  ││objects.                                        │
│  ▸ bisect           Use binary search to find… ││                                                │
│    blame            Show what revision and…    ││This command can operate in two modes, depending│
│    branch           List, create, or delete…   ││on whether an option from the --batch family is │
│    bugreport        Collect information for…   ││specified.                                      │
│  ▸ bundle           Move objects and refs by…  ││                                                │
│    cat-file         Provide content or type…   ││In non-batch mode, the command provides         │
│    check-attr       Display gitattributes…     ││information on an object named on the command   │
│    check-ignore     Debug gitignore / exclude… ││line.                                           │
│    check-mailmap    Show canonical names and…  ││                                                │
│    check-ref-format Ensures that a reference…  ││In batch mode, arguments are read from standard │
│    checkout-index   Copy files from the index… ││input.                                          │
│    checkout         Switch branches or…        ││                                                │
│    cherry-pick      Apply the changes…         ││FLAGS                                           │
│    cherry           Find commits yet to be…    ││  --allow-unknown-type  allow -s and -t to work │
│    citool           Graphical alternative to…  ││                        with broken/corrupt     │
╰────────────────────────────────────────────────╯╰────────────────────────────────────────────────╯
↑↓ move   → expand   / search   y copy   ? help   q quit
```

## Status

This is an in-progress implementation (spec roadmap phases 0-3 of 6).
**What works today:**

- A complete intermediate representation (`mandible-core`) with sanitized text
  (including a conservative markdown normalizer for catalog prose),
  per-field provenance, and two-axis authority merging.
- **Tier A**: a curated third-party command-spec catalog, served from a
  byte-indexed vendored snapshot with no subprocess cost.
- **Tier B**: a `winnow`-based `--help`/`-h` grammar for everything not in
  the catalog — layout-driven section parsing (not keyed on specific heading
  text), reads stdout and stderr without requiring exit 0, recovers
  same-indent "word grid" listings (`openssl`-style) as well as indented
  blocks.
- Lazy, node-at-a-time extraction with bounded background depth-warming
  (spec §5.2), so expanding into a large tree stays fast.
- Real fuzzy search (`mandible-search`, backed by `nucleo`): commands and
  flags are both independently searchable and ranked.
- A full `ratatui` TUI: tree pane, detail pane, search bar, status bar,
  keybinding overlay, mouse support, responsive layout.
- An on-disk cache (spec §11) keyed on a build-time fingerprint of the
  extraction logic itself, so a code change or a re-vendored catalog
  invalidates old entries automatically.
- `mandible --doctor <tool>` (non-interactive diagnostic) and
  `cargo xtask coverage` (the extraction coverage harness, spec §13.1).

**What's explicitly not built yet** (see spec.md §12 for the roadmap):
completion-script parsing (Tier C), man page extraction (Tier D, deferred
entirely), native dynamic probes (Tier E), user overrides (Tier F).

**The honest coverage story:** mandible is genuinely useful today for the
catalog's tools plus essentially any tool with a parseable `--help`/`-h`.
Real, current numbers vary as the extraction pipeline evolves, so this
README deliberately doesn't quote a specific tool count or percentage —
run `cargo xtask coverage` yourself (it scans every executable on your own
`PATH` and writes a scoreboard) or `mandible --doctor <tool>` for one
tool's exact tier-by-tier breakdown. Later phases close remaining gaps
without ever special-casing a tool by name — see the invariant below.

## Supported platforms

**Linux and macOS.** CI runs the full test suite natively on both
(`.github/workflows/ci.yml`, `ubuntu-latest` + `macos-latest`). **Windows is
not currently supported** — the execution-safety layer's process-group
handling (spec §6 rule 4) is POSIX-specific, and Windows support hasn't been
built or tested (spec §16 [M-8]).

Cache and config locations follow OS convention via the `directories` crate:
`$XDG_CACHE_HOME/mandible/` and `$XDG_CONFIG_HOME/mandible/overrides/` (or
their `~/.cache`/`~/.config` fallbacks) on Linux; `~/Library/Caches/mandible/`
and `~/Library/Application Support/mandible/overrides/` on macOS.

## The invariant

> The mandible repository will never contain per-tool logic. No
> `if tool == "docker"`, no vendored per-tool patch file, no tool-name-keyed
> special case in any tier. Tool-specific knowledge lives in exactly two
> places: (a) third-party structured catalogs consumed wholesale as *data*,
> and (b) user-local override files that are never checked into this repo.

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Install

```
cargo install --path mandible
```

(Not yet published to crates.io — see spec.md §12 phase 6.)

Packaging metadata for `.deb` (`cargo-deb`) and `.rpm`
(`cargo-generate-rpm`) lives in `mandible/Cargo.toml`; a packaged install
places the binary, a man page (`packaging/mandible.1`), and shell
completions (bash, zsh, fish — generated at build time from the same
`clap` definition the binary parses against, so they can't drift) at the
standard system paths. If you install via `cargo install` instead, generate
completions yourself:

```
mandible --completions zsh > ~/.zfunc/_mandible   # or bash / fish / elvish / powershell
```

## Usage

```
mandible <tool>                 # open the interactive tree for <tool>
mandible <tool> --refresh       # bypass the cache and re-extract
mandible --doctor <tool>        # non-interactive diagnostic: tiers, counts, timing
mandible --completions <shell>  # print a shell completion script to stdout
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
cd mandible
cargo build --release
./target/release/mandible git
```

Default features build with no network access and no C toolchain (spec
§15). The `manpage` feature (Tier D, deferred) needs a C toolchain and is
off by default.

## Where the data comes from

`vendor/carapace-specs.json` is a vendored, point-in-time snapshot of the
[carapace-bin](https://github.com/carapace-sh/carapace-bin) project's
declarative command specs, produced by `scripts/vendor_carapace_specs.py`.
See [NOTICE](./NOTICE) for full attribution and license text, and
`mandible --doctor <tool>` for the snapshot's vendoring date and commit.

## License

Dual-licensed under either [MIT](./LICENSE-MIT) or
[Apache License, Version 2.0](./LICENSE-APACHE), at your option — the
Rust ecosystem standard, chosen so the Apache half's explicit patent
grant is available to corporate users who require it. Vendored
third-party *data* is separately attributed in [NOTICE](./NOTICE).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you shall be dual-licensed as
above, without any additional terms or conditions.
