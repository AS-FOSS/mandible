<div align="center">
     
<img width="256" height="256" alt="logo" src="https://github.com/user-attachments/assets/c1f6d254-1488-4d4d-ac94-b3ad273d7bb7" />

<!-- LOGO: a ~96px mark goes here. Something jaw/mandible-shaped reads well at this
     size. Replace this comment with: <img src="docs/logo.png" width="96" alt="mandible logo"> -->

# mandible

*A TUI manual for every command-line tool you have*

[![CI](https://github.com/AS-FOSS/mandible/actions/workflows/ci.yml/badge.svg)](https://github.com/AS-FOSS/mandible/actions/workflows/ci.yml)
[![framework support](https://github.com/AS-FOSS/mandible/actions/workflows/frameworks.yml/badge.svg)](https://github.com/AS-FOSS/mandible/actions/workflows/frameworks.yml)
[![crates.io](https://img.shields.io/crates/v/mandible.svg?style=flat-square)](https://crates.io/crates/mandible)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg?style=flat-square)](#)
![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS-lightgrey.svg?style=flat-square)

[Install](#install) • [How it works](#how-it-works) • [Coverage](#is-it-actually-universal) • [Keys](#keys)

</div>

`man` tells you about one command. `--help` tells you about one invocation. Neither explores a tool you don't already know.

```console
$ mandible docker
```

<p align="center">
     
<img width="2560" height="1440" alt="output" src="https://github.com/user-attachments/assets/8fa831a6-c0e6-472f-834e-b844d7c49792" />

  <br>
  <em>A tree of every command, subcommand and flag on the left. The selected one's documentation on the right.</em>
</p>





> [!TIP]
> Try running `mandible mandible`

## Install

### Pre-built binaries

Linux and macOS, on both x86_64 and arm64. Windows is not supported. The process
containment described above relies on POSIX process groups, and native Windows tools
use conventions (`/?`, PowerShell's own help system) that this project does not yet
speak.

<div align="center">

| Platform              | Download |
|:----------------------|:---------|
| Linux x86_64          | [`tar.gz`](https://github.com/AS-FOSS/mandible/releases/latest/download/mandible-x86_64-unknown-linux-gnu.tar.gz) |
| Linux arm64           | [`tar.gz`](https://github.com/AS-FOSS/mandible/releases/latest/download/mandible-aarch64-unknown-linux-gnu.tar.gz) |
| macOS Apple Silicon   | [`tar.gz`](https://github.com/AS-FOSS/mandible/releases/latest/download/mandible-aarch64-apple-darwin.tar.gz) |
| macOS Intel           | [`tar.gz`](https://github.com/AS-FOSS/mandible/releases/latest/download/mandible-x86_64-apple-darwin.tar.gz) |

`.deb` and `.rpm` packages are attached to every [release](https://github.com/AS-FOSS/mandible/releases).  
Each archive ships a matching `.sha256` checksum.
</div>

### Using Cargo

```console
cargo install mandible
```



## How it works

There is no per-tool logic anywhere in this project. No `if tool == "docker"`, no
vendored catalogue of hand-written definitions. That approach is convenient for a
week and unmaintainable ever after. It is always slightly out of date, and it is
wrong in ways you cannot see from the outside.

The insight it runs on instead: help text isn't written by hand, it's generated, and
only a small closed set of generators exists. mandible works out which *framework*
produced a tool's output, then applies that framework's grammar. For example:
<div align="center">
     <table>
       <tr><td><b>Rust</b></td><td>clap (v2, v3/v4)</td></tr>
       <tr><td><b>Go</b></td><td>cobra, urfave/cli, stdlib flag</td></tr>
       <tr><td><b>Python</b></td><td>argparse, click, docopt</td></tr>
       <tr><td><b>JavaScript</b></td><td>commander, yargs, oclif</td></tr>
       <tr><td><b>Java / .NET</b></td><td>picocli, System.CommandLine</td></tr>
       <tr><td><b>Others</b></td><td>GNU argp/getopt_long, busybox, Symfony Console, OptionParser/Thor, BSD-terse</td></tr>
     </table>
</div>

> [!TIP]
> You can also try to probe executable files: `mandible scripts/custom.py`

## Is it actually universal?

That claim is measured, not asserted. `cargo xtask coverage` runs the pipeline against
every executable on your `PATH` and scores each one: sources used, framework detected,
nodes, flags, percentage described.

It also carries a structure-sanity column, which exists because a coverage number
alone can be gamed by the very failure it should catch. `%described` once reported a
tool as fine at 100% while 39 of its 40 subcommands had been fabricated out of wrapped
prose. A metric that improves when the tool gets worse is worse than no metric.

CI gates every change against a fixed tool list, and sweeps the whole `PATH`
separately for the broad picture.

Coverage is not accuracy, though, and the honest number is lower than a green badge
suggests. To measure accuracy, a human reviews a randomly drawn sample of real tools
against each tool's own `--help` text. The sample is committed to the repo *before*
any verdict is recorded, so it cannot be quietly redrawn once the results look bad.

In the most recent sample, **25 of 43 tools parsed fully correctly — about 58%**.
A sample of 43 is small, so the true rate across all tools could plausibly be
anywhere from roughly 43% to 72% — a statistical margin of error, not a second
measurement. Tools that mandible itself marks `ok` do better: 80% correct, with a
margin of roughly 61% to 91%. One more caveat: when the same tools were reviewed a
second time, the two human reviews only agreed 11 times out of 16 — so every figure
here is approximate, and differences between measurement rounds are partly the
reviewer changing their mind, not the parser changing quality.

mandible is useful today and wrong often enough that you should check anything
surprising against the tool's own `--help`.


> [!TIP]
> Something parses wrong? Run `mandible --report <tool>` and paste the output
> into [an issue](../../issues/new?template=parsing-issue.yml). That takes two
> minutes and is a complete contribution on its own.

<details>
<summary><h2>Configurations</h2></summary>
     
### Keys

`?` lists every binding and the footer keeps the important ones on screen, so this
section is deliberately short: arrows or `hjkl` to move, `/` to search, `Tab`
between panes, `y` to copy the selected flag, `q` to quit.

Search is the part that is not self-evident, because its two modes answer
different questions. `names` matches command names literally, so every row you see
contains what you typed. `everything` searches flags, summaries and descriptions
fuzzily, so `gco` finds `checkout`. `/` opens the first, and pressing it again
switches to the second.

### Overrides

Anything mandible gets wrong about a tool, you can correct locally. Drop a TOML file
at `~/.config/mandible/overrides/<tool>.toml`:

```toml
summary = "my better one-line description"

[[flags]]
long = "verbose"
short = "v"
description = "a description that actually explains it"

[[node]]
path = ["build"]
summary = "corrections apply to subcommands too"
```

These are yours and are never committed to this repository.

> [!TIP]
> An override fixes a tool for you today. Consider also opening an issue: the real
> fix belongs in a framework grammar, where it improves every tool built with that
> framework at once.

### Environment

| Variable | Effect |
|---|---|
| `NO_COLOR` | Disable colour. `TERM=dumb` and piped output do the same |
| `MANDIBLE_ASCII=1` | Force the ASCII glyph set, for terminals that mangle Unicode |
| `MANDIBLE_CONFIG_DIR` | Override the config directory outright |
| `MANDIBLE_LOG` | Tracing filter, written to stderr |

### Diagnostics

```console
$ mandible --doctor gh
framework: cobra (from artifact)
nodes: 29
flags: 2 (100.0% described)
```

`--doctor` reports which framework was identified, which sources contributed, and how
much of the tool was understood. It turns "mandible is wrong about tool X" into "the
cobra grammar mishandles Y", which is a bug someone can actually fix.

</details>

## Documentation

| | |
|---|---|
| [`spec.md`](./spec.md) | Design authority: the source model, the safety policy, and the measurement behind every non-obvious decision |
| [`AGENTS.md`](./AGENTS.md) | The invariants table. Every entry names the failure it prevents |


## Contributing

If a tool renders wrong, that is worth reporting even if you never look at the
code. Run `mandible --report <tool>` and paste the output into an issue. Your
tool's version and its exact help text vanish when you upgrade, and nobody else
can recover them. Everything after that can be done later by anyone.

There is a second thing you can do that is hard for us to do ourselves. Every
accuracy figure here comes from one Ubuntu machine on ARM, so whatever is
installed on your `PATH` is probably software this project has never seen. The
built-in audit tool samples your own commands, shows you the parse next to the
real help text, and records what you think of each one:

```console
$ cargo run -p xtask -- audit sample --seed 42 --sample 20
$ mandible --review 42
```

Attach the resulting `audit/42.toml` to an issue. Twenty tools is plenty.

[`CONTRIBUTING.md`](./CONTRIBUTING.md) covers both, plus writing tests and
changing the parser.
