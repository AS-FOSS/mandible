#!/usr/bin/env bash
# Generate mandible's shell completion files from the built binary, into the
# stable directory the .deb and .rpm manifests install them from.
#
# The binary is the only generator. `mandible --completions <shell>` is
# already what the nix derivation and the Homebrew formula call, so every
# packaging channel emits the same bytes from the same clap definition.
#
#   cargo build --release --bin mandible
#   scripts/gen_completions.sh
#   cargo deb --no-build -p mandible
#   cargo generate-rpm -p mandible
#
# Each file is written under the name the shell actually looks for, so the
# manifests map one source file to one destination path and never glob:
# bash-completion loads `completions/<command>`, zsh loads `_<command>`.
set -euo pipefail

bin="${1:-target/release/mandible}"
out="${2:-target/release/completions}"

mkdir -p "$out"
"$bin" --completions bash >"$out/mandible"
"$bin" --completions zsh >"$out/_mandible"
"$bin" --completions fish >"$out/mandible.fish"
