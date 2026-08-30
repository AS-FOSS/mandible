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

Linux and macOS, on x86_64 and arm64. Windows is not supported: the containment above
relies on POSIX process groups, and Windows tools use conventions (`/?`, PowerShell's
own help system) this project does not speak.

**Cargo**

```console
# fetch prebuilt binary
cargo binstall mandible
```

```console
# build from source
cargo install mandible
```

**Fedora / EPEL** (COPR repo)

```console
sudo dnf copr enable as-foss/mandible
sudo dnf install mandible
```

**Nix**

```console
nix run github:AS-FOSS/mandible
```

**Homebrew** (macOS or Linux)

```console
brew install as-foss/mandible/mandible
```

**Debian / Ubuntu** (signed apt repo)

```console
sudo curl -fsSL https://as-foss.github.io/mandible-apt/mandible-archive-keyring.gpg \
  -o /usr/share/keyrings/mandible-archive-keyring.gpg
sudo tee /etc/apt/sources.list.d/mandible.sources >/dev/null <<'EOF'
Types: deb
URIs: https://as-foss.github.io/mandible-apt
Suites: stable
Components: main
Architectures: amd64 arm64
Signed-By: /usr/share/keyrings/mandible-archive-keyring.gpg
EOF
sudo apt-get update && sudo apt-get install mandible
```


Standalone binaries, `.deb` and `.rpm` packages, each with a `.sha256`, are attached
to every [release](https://github.com/AS-FOSS/mandible/releases/latest).

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

> [!TIP]
> Open straight at a subcommand: `mandible cargo clippy`. Commands the parent's
> own help never lists — `cargo-clippy`, `git-lfs` and the like — are found on
> `PATH` and shown marked `unverified`, since the naming convention is evidence
> about the filesystem and a guess about the tool.

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
suggests. Accuracy comes from a human-reviewed audit, where a person checks a randomly
drawn sample of real tools against each tool's own `--help` text. The sample is
committed to the repo before any verdict is recorded, so it cannot be quietly redrawn
once the results look bad. In the most recent audit, **25 of 43 tools parsed fully
correctly, about 58%**. A sample that small leaves real statistical slack, so the true
rate across all tools could sit anywhere between about 43% and 72%. Tools that
mandible itself marks `ok` do better, at 80% correct with a plausible range of 61%
to 91%.

mandible is useful today and wrong often enough that you should check anything
surprising against the tool's own `--help`.


> [!TIP]
> Something parses wrong? Run `mandible --report <tool>` and paste the output
> into [an issue](../../issues/new?template=parsing-issue.yml). That takes two
> minutes and is a complete contribution on its own.

<details>
<summary><h2>Configuration</h2></summary>

Settings live in `~/.config/mandible/config.toml`. The file doesn't need to
exist — everything has a default.

### Settings

Long preformatted lines (the raw `--help` view, USAGE synopses) scroll
sideways with `←`/`→` (or `h`/`l`) instead of wrapping, and a dim `<`/`>`
marker sits beside each line that continues past the pane edge. Set
`horizontal_scroll = false` to wrap everything instead.

```toml
# ~/.config/mandible/config.toml

[ui]
horizontal_scroll = true  # the default
```

Getting a tool's *documentation* right is mandible's job, not yours — if
something parses wrong, `mandible --report <tool>` and an issue is the fix that
helps everyone. (Local per-tool corrections do exist for the impatient:
`~/.config/mandible/overrides/<tool>.toml`.)

### Environment variables

| Variable | Effect |
|---|---|
| `NO_COLOR` | Disable colour. `TERM=dumb` and piped output do the same |
| `MANDIBLE_ASCII=1` | Force the ASCII glyph set, for terminals that mangle Unicode |
| `MANDIBLE_CONFIG_DIR` | Read config and overrides from a different directory |
| `MANDIBLE_LOG` | Tracing filter, written to stderr |

### Keys

`?` inside mandible lists every binding, and the footer keeps the important ones
on screen: arrows or `hjkl` to move, `/` to search, `Tab` between panes, `←`/`→`
to scroll wide lines sideways, `t` for the tool's own `--help`, `y` to copy the
selected flag, `q` to quit.

Search has two modes: `names` matches command names literally; `everything`
searches flags and descriptions fuzzily, so `gco` finds `checkout`. `/` opens
the first, pressing it again switches to the second.

### Completions

The packages install shell completions for you. For a hand-built binary,
`mandible --completions <shell>` prints the script (bash, zsh, fish, and more)
— drop it wherever your shell looks for completions.

They cover more than flags: the tool argument completes to the commands on
your `PATH`, so `mandible gi<TAB>` offers `git`, not the files in the current
directory. (zsh and fish today; bash's script format can't express this yet.)

### Onto the prompt

`y` gets a spelling as far as the clipboard. To land it on the command line
instead, add the shell integration:

```console
$ eval "$(mandible --shell-init bash)"   # or: zsh — in your ~/.bashrc, ~/.zshrc
```

Now type a tool name, press `Ctrl-X m`, browse, and press `Enter`: the command
you selected — `git commit --amend`, say — replaces the line, ready to edit.
Quit with `q` and the line is left exactly as it was.

The binding is a few lines of shell around `mandible --print-selection <tool>`,
which browses as usual but makes `Enter` print the selection instead of
expanding the row (the UI draws on stderr, so stdout carries just that one
line). Bind it to a different key, or wrap it in your own widget, by reading
what `--shell-init` prints.

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
