#!/usr/bin/env bash
# Everything you are supposed to have done, in one command.
#
# Prints the pull-request class of the current diff, prints the change-trigger
# rows that the diff actually fires, then runs every gate. Run it before you
# report a unit of work done, and again after any context compaction: the
# state that matters lives in tracked files, and this is how you read it back.
#
# `--fast` skips the release build and the corpus replay, for a quick loop.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

base="${BASE:-main}"
fast=0
[ "${1:-}" = "--fast" ] && fast=1

merge_base=$(git merge-base "$base" HEAD 2>/dev/null || echo "")
if [ -n "$merge_base" ]; then
    changed=$(git diff --name-only "$merge_base"...HEAD; git diff --name-only; git ls-files --others --exclude-standard)
else
    changed=$(git diff --name-only; git ls-files --others --exclude-standard)
fi
changed=$(printf '%s\n' "$changed" | sort -u | grep -v '^$' || true)

echo "== pull-request class =="
./scripts/pr_class.sh "$base"
echo

echo "== change-trigger rows this diff fires =="
# The rows live in AGENTS.md between the two directory-trigger markers, one
# per line as `| <path glob> | <what must move with it> |`. Parsing them from
# the document keeps this script from becoming a second copy of the matrix.
awk '/<!-- directory-triggers:start -->/{on=1;next} /<!-- directory-triggers:end -->/{on=0} on' AGENTS.md 2>/dev/null \
| grep '^|' | grep -v '^|---' | grep -v '^| Path' \
| while IFS='|' read -r _ pattern action _; do
    pattern=$(echo "$pattern" | tr -d '` ' )
    action=$(echo "$action" | sed 's/^ *//;s/ *$//')
    [ -z "$pattern" ] && continue
    if printf '%s\n' "$changed" | grep -q "^${pattern%\*}"; then
        printf '  %-22s %s\n' "$pattern" "$action"
    fi
done
echo

echo "== gates =="
set -x
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/ratchet_check.sh
./scripts/shape_guard.sh
cargo nextest run --workspace
if [ "$fast" -eq 0 ]; then
    cargo run -p xtask -- corpus
    cargo build --release
fi
set +x

echo
echo "preflight: ok"
