#!/usr/bin/env bash
# Fail if a released CHANGELOG section has been edited.
#
# The failure this prevents, observed 2026-08-12: a change appended its notes
# under `## [0.2.2]`, a heading whose release had already been tagged and
# published weeks earlier. Nothing complained. Roughly 56 lines describing
# unreleased work sat under a shipped version's notes, and because the release
# body is generated from a changelog section (scripts/changelog_section.sh),
# that misattribution would have gone out with the next release.
#
# A released section is history. Once `vX.Y.Z` exists as a tag, the text under
# `## [X.Y.Z]` describes what that tag contains and must never change again.
# New work belongs under `## [Unreleased]`, or under a new version heading at
# release time.
#
# The check: for every `## [X.Y.Z]` heading that has a matching `vX.Y.Z` tag,
# compare that section against the same section as it existed in the tag
# itself. Any difference is an edit to published history.
#
# Typo fixes to a released section are the one legitimate case this blocks.
# That is deliberate: they are rare, and the reviewer should see the diff and
# say so out loud rather than have it slip through unnoticed.

set -euo pipefail

changelog="${1:-CHANGELOG.md}"
status=0

# Section text for a version, from a given git revision (or the working tree
# when rev is empty). Prints the lines under `## [X.Y.Z]` up to the next
# `## [` heading.
section() {
    local rev="$1" version="$2" source
    if [ -z "$rev" ]; then
        source=$(cat "$changelog")
    else
        source=$(git show "$rev:$changelog" 2>/dev/null || true)
    fi
    printf '%s\n' "$source" | awk -v v="## [$version]" '
        $0 == v { inside = 1; next }
        inside && /^## \[/ { exit }
        inside { print }
    ' | normalize
}

# The one rewrite of published notes that is legitimate, and why.
#
# This repository moved from github.com/sadigaxund/mandible to
# github.com/AS-FOSS/mandible, and the issue links inside already-released
# sections were rewritten to follow it. Two sections carry that edit, [0.1.6]
# and [0.1.0], and leaving the guard red over it would teach everyone to
# ignore the guard, which defeats it entirely.
#
# So the owner segment of a mandible GitHub URL is normalized away before
# comparing. Everything else about a released section, including the rest of
# the URL and the issue number, is still compared exactly.
normalize() {
    sed -E 's#github\.com/[A-Za-z0-9_.-]+/mandible#github.com/OWNER/mandible#g'
}

versions=$(grep -oE '^## \[[0-9]+\.[0-9]+\.[0-9]+\]' "$changelog" | tr -d '#[] ')

for version in $versions; do
    tag="v$version"
    if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
        # Not released yet. This is the section under active development.
        continue
    fi
    if ! diff -q <(section "$tag" "$version") <(section "" "$version") >/dev/null; then
        echo "error: the [$version] section differs from what tag $tag published." >&2
        echo "       That section is history. Put new notes under [Unreleased]." >&2
        echo >&2
        diff <(section "$tag" "$version") <(section "" "$version") | head -40 >&2
        echo >&2
        status=1
    fi
done

if [ "$status" -eq 0 ]; then
    echo "changelog guard: released sections match their tags"
fi
exit "$status"
