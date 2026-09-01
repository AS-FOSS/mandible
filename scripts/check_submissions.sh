#!/usr/bin/env bash
# CI check for a pull request that touches audit/submissions/** (CONTRIBUTING.md
# §2, "Audit mandible against your own tools").
#
# Three rules over every file the PR changed under audit/submissions:
#
#   1. Its path matches audit/submissions/<login>/<seed>.toml or
#      audit/submissions/<login>/<seed>-report.txt — nothing else belongs
#      there.
#   2. Every changed *.toml file is a well-formed verdict file: `cargo xtask
#      audit report` can load and render it.
#   3. <login> equals the pull request author's GitHub login — a typo in the
#      folder name must fail loudly rather than filing someone's audit under
#      the wrong account — unless the author is `sadigaxund` (the
#      maintainer, who may commit or repair a submission under any login as
#      part of maintenance).
#
# Usage: scripts/check_submissions.sh <base-rev> <pr-author-login>
#
# Diffs the *currently checked-out working tree* against <base-rev> — same
# convention as the corpus CI job's `--baseline-dir` population, so this
# reads real files off disk rather than reaching into git for their content.
# Run it after checking out the revision you want checked; in CI that is the
# pull request's head, checked out by actions/checkout before this runs.
#
# Testable with a throwaway git repo: init one, commit a base revision,
# commit changes under audit/submissions/, then run this script with that
# base revision's SHA and a login to assert against — see
# scripts/tests/check_submissions_test.sh.

set -euo pipefail

base_rev="${1:?usage: check_submissions.sh <base-rev> <pr-author-login>}"
pr_author="${2:?usage: check_submissions.sh <base-rev> <pr-author-login>}"

path_re='^audit/submissions/[A-Za-z0-9-]+/[0-9]+(\.toml|-report\.txt)$'

# `--diff-filter=ACMR`: added/copied/modified/renamed files only — a deleted
# submission has nothing left on disk to check, and re-checking a path that
# no longer exists would just fail on a missing file for no useful reason.
changed=$(git diff --name-only --diff-filter=ACMR "$base_rev" -- audit/submissions || true)

if [ -z "$changed" ]; then
    echo "check_submissions: no changed files under audit/submissions"
    exit 0
fi

status=0

while IFS= read -r path; do
    [ -z "$path" ] && continue

    if ! [[ "$path" =~ $path_re ]]; then
        echo "error: $path — does not match audit/submissions/<login>/<seed>.toml or -report.txt" >&2
        status=1
        continue
    fi

    login=$(echo "$path" | cut -d/ -f3)
    if [ "$pr_author" != "sadigaxund" ] && [ "$login" != "$pr_author" ]; then
        echo "error: $path — folder is $login but this pull request is from $pr_author" >&2
        status=1
        continue
    fi

    case "$path" in
        *.toml)
            seed="$(basename "$path" .toml)"
            folder="$(dirname "$path")"
            if ! cargo xtask audit report --dir "$folder" --seed "$seed" >/dev/null; then
                echo "error: $path — cargo xtask audit report --dir $folder --seed $seed failed" >&2
                status=1
            fi
            ;;
    esac
done <<<"$changed"

if [ "$status" -eq 0 ]; then
    echo "check_submissions: every changed audit/submissions file passed"
fi
exit "$status"
