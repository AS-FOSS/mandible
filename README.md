<!-- Logo goes here. Suggested: a 120-160px mark, centered, above the title. -->
<p align="center">
  <!-- <img src="docs/logo.png" alt="mandible" width="140"> -->
</p>

<h1 align="center">mandible</h1>

<p align="center">
  <strong>A TUI manual for every command-line tool you have.</strong>
</p>

<p align="center">
  <a href="https://github.com/sadigaxund/mandible/actions/workflows/ci.yml"><img src="https://github.com/sadigaxund/mandible/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/sadigaxund/mandible/actions/workflows/frameworks.yml"><img src="https://github.com/sadigaxund/mandible/actions/workflows/frameworks.yml/badge.svg" alt="framework support"></a>
  <a href="https://crates.io/crates/mandible"><img src="https://img.shields.io/crates/v/mandible.svg" alt="crates.io"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="license"></a>
  <img src="https://img.shields.io/badge/platform-linux%20%7C%20macOS-lightgrey.svg" alt="platforms">
</p>

---

`man` tells you about one command. `--help` tells you about one invocation.
Neither lets you *explore* a tool you don't already know.

```console
$ mandible docker
```

opens an explorable tree of every command, subcommand, and flag — with
descriptions — and a search bar over all of it.

<!-- Screenshot goes here. -->

## Install

```console
cargo install mandible
```

Packages for `.deb` and `.rpm` are built from the same metadata; see
[`packaging/`](./packaging). Linux and macOS.

## Why it works on tools it has never seen

**No per-tool logic, ever.** No `if tool == "docker"`, no vendored catalog of
hand-written definitions. That approach starts out convenient and ends as an
unmaintainable pile that is always slightly out of date.

The insight it runs on instead: **help text isn't written by hand, it's
generated** — and only a small, closed set of generators exists. mandible
identifies the *framework* behind a tool's output (clap, cobra, argparse, click,
urfave/cli, GNU argp, busybox, picocli, …) and applies that framework's grammar.

Fixing the argparse grammar improves every Python CLI ever written. A catalog
entry improved exactly one tool, until it went stale.

Identification is artifact-first: `spf13/cobra` appears 583× in `docker`'s own
bytes, which is ground truth rather than a guess about section headings.

**When it can't parse something, it says so.** A tool matching no known grammar
is rendered verbatim — the author's own text, untouched, labelled `unparsed`.
Inventing structure a user can't tell is wrong is worse than admitting defeat.

Detection is not the same as coverage: only a minority of tools are matched to a
specific framework, and the rest still parse through the general layout engine.
`mandible --doctor <tool>` tells you which happened for any given tool.

## Speed

Startup does no extraction at all: the UI is on screen immediately, and the tree
fills in behind it on a background pool, showing `⋯ loading` where it hasn't
arrived yet. There is deliberately **no cache** — a cache can't see `docker`
gaining a plugin or `git` gaining an alias from `~/.gitconfig`, and being
confidently stale is worse than being fast.

## Keys

| | |
|---|---|
| `↑`/`↓`, `j`/`k` | move |
| `→`/`Enter`, `←` | expand / collapse |
| `/` | search. Press again to switch between **names** (command names, literal substring) and **everything** (flags, summaries and descriptions, fuzzy) |
| `Esc` | leave search, keeping the filter; again to clear |
| `Tab` | switch pane |
| `y` | copy the selected flag or command path |
| `.` | show hidden and deprecated items |
| `r` | re-extract |
| `?` | all keys |
| `q` | quit (from the tree; `Ctrl-C` quits from anywhere, including mid-search) |

## Is it actually universal?

That claim is measured, not asserted. `cargo xtask coverage` runs the pipeline
against **every executable on your `PATH`** and writes a scoreboard — tiers hit,
framework detected, nodes, flags, % described, and a structure-sanity column that
catches *fabricated* output. That column exists because `%described` alone once
reported a tool as `ok` at 100% while 39 of its 40 subcommands were invented: a
coverage metric that can be gamed by the failure it should detect is worse than
no metric.

CI gates every change against a fixed tool list, and sweeps the whole `PATH`
separately for the broad picture.

For a single tool:

```console
$ mandible --doctor gh
framework:  cobra (from artifact)
nodes:      29
flags:      2 (100.0% described)
```

## Docs

- [`spec.md`](./spec.md) — design authority: the tier model, execution-safety
  policy, and the measured baselines behind every non-obvious decision.
- [`AGENTS.md`](./AGENTS.md) — working agreements and the invariants table, each
  entry naming the failure it prevents.
- [`CONTRIBUTING.md`](./CONTRIBUTING.md)

## Safety

mandible finds out what a tool does by **running it** — `docker --help`,
`git rebase --help`, and so on. Running other people's programs to read their
documentation deserves some care, so:

**Some programs are never run at all.** `kill`, `pkill`, `killall`, `fuser`,
`reboot`, `shutdown` and their relatives exist to terminate things. There is no
safe way to ask them for help, because mandible's questions take the shape
`pkill something --help` — and to `pkill`, that first word is *a process to
kill*, not a subcommand. So it doesn't ask. Use `man pkill` for those.

**Everything else is asked only in a few fixed, harmless ways** — `--help`,
`-h`, `<tool> help`, and the completion commands some tools support. mandible
never passes an argument that could name a file to write to, never runs a tool
with no arguments at all, and gives up on anything that hangs.

**Whatever a tool writes goes somewhere disposable.** Some programs create files
just because you asked for help. One real example found while testing: running
`mysql_secure_installation --help` drops a MySQL config file into your home
directory containing a blank database password. So every probe runs with its
home, temp and config directories pointed at a throwaway folder that is deleted
straight afterwards, and with its working directory there too.

**It is all in one place.** Every one of these rules lives in a single module,
and a test fails the build if any other part of the codebase learns how to
launch a program — so the boundary is enforced rather than merely intended.

Full isolation would need OS-level sandboxing, which mandible does not yet do;
[`spec.md`](./spec.md) §6 states plainly what is and isn't covered.

## License

MIT OR Apache-2.0, at your option — the Rust ecosystem's standard dual license.
See [LICENSE-MIT](./LICENSE-MIT) and [LICENSE-APACHE](./LICENSE-APACHE).
