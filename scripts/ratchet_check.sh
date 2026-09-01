#!/usr/bin/env bash
# Ratchet on the size-lint exemptions.
#
# `clippy::too_many_lines` and `clippy::cognitive_complexity` are warnings in
# every crate root, and CI's `-D warnings` makes them gates. A function that
# predates the ceiling carries a scoped `#[allow]` with a one-line reason, and
# every one of those allows is listed in `scripts/ratchet.txt`.
#
# This fails when the tree carries more allows than the baseline lists. Adding
# a line to the baseline is not a fix; splitting the function and deleting the
# line is. The baseline is only ever supposed to shrink.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

baseline_file="scripts/ratchet.txt"
baseline=$(grep -cvE '^\s*(#|$)' "$baseline_file" || true)

actual=$(git grep -hoE '#\[allow\(clippy::(too_many_lines|cognitive_complexity)\)\]' -- '*.rs' | wc -l | tr -d ' ')

echo "ratchet: $actual allows in the tree, baseline allows $baseline"

if [ "$actual" -gt "$baseline" ]; then
    echo
    echo "FAIL: the tree gained a size-lint exemption."
    echo "Split the function instead. If the split is genuinely out of reach,"
    echo "the baseline change needs to be argued in the pull request."
    echo
    git grep -nE '#\[allow\(clippy::(too_many_lines|cognitive_complexity)\)\]' -- '*.rs'
    exit 1
fi

if [ "$actual" -lt "$baseline" ]; then
    echo "note: $((baseline - actual)) exemption(s) gone; shrink $baseline_file"
fi

echo "ratchet: ok"
