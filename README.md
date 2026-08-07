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

Extraction runs real tools, so it is fenced: an allowlist of inert argv forms,
`std::process` confined to one audited module and enforced by a test, and every
probe's CWD, `HOME`, `TMPDIR` and `XDG_*` pointed at a scratch directory created
per invocation. That last one is not paranoia — `mysql_secure_installation
--help` was measured writing a `.my.cnf` with an empty root password.

## License

MIT OR Apache-2.0, at your option — the Rust ecosystem's standard dual license.
See [LICENSE-MIT](./LICENSE-MIT) and [LICENSE-APACHE](./LICENSE-APACHE).
