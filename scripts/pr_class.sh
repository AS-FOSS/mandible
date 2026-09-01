#!/usr/bin/env bash
# Print the pull-request class of the current branch's diff against `main`.
#
#   code     the diff touches something a reviewer can catch a bug in
#   direct   the diff touches only prose, scripts, workflows or config
#
# A `direct` diff goes straight to main. A self-opened, self-merged pull
# request for prose adds a merge commit and a dead branch while gating
# nothing, because a docs-only push skips CI entirely (`paths-ignore`).
#
# Code paths are `mandible*/`, `xtask/src` and the Cargo manifests. Everything
# else is prose, data or process.
#
# `corpus/` is deliberately NOT a code path. A fixture is captured bytes plus a
# contract, so it is data, and the maintainer's standing rule reserves pull
# requests for parser logic and releases. What guards a fixture is `xtask
# corpus` on push, and the `git status` check for an invented xfail snapshot
# (AGENTS §3.4), not a reviewer.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

base="${1:-main}"
merge_base=$(git merge-base "$base" HEAD)
changed=$(git diff --name-only "$merge_base"...HEAD)

if printf '%s\n' "$changed" | grep -qE '^(mandible[^/]*/|xtask/src/|Cargo\.(toml|lock)$)'; then
    echo code
else
    echo direct
fi
