#!/usr/bin/env bash
# Print one version's section of CHANGELOG.md, for use as a GitHub Release
# body.
#
#   scripts/changelog_section.sh 0.1.0
#
# Looks for a `## [0.1.0]` heading and prints everything up to the next
# `## ` heading. Falls back to `## [Unreleased]` when the version has no
# section of its own — which is the normal state for a first release, where
# the work has accumulated under Unreleased and the tag is what names it.
#
# Exists as a file rather than inline YAML for the same reason
# framework_matrix.sh does: it can be run and checked locally, so a broken
# release body is not something you discover only after tagging.
set -euo pipefail

version="${1:?usage: changelog_section.sh <version>}"
changelog="${2:-CHANGELOG.md}"

extract() {
  awk -v want="$1" '
    /^## / {
      if (found) exit
      # Match "## [1.2.3]" or "## 1.2.3", with or without a trailing date.
      heading = $0
      gsub(/^## +/, "", heading)
      gsub(/^\[/, "", heading)
      sub(/\].*$/, "", heading)
      sub(/ +-.*$/, "", heading)
      if (heading == want) { found = 1; next }
      next
    }
    found { print }
  ' "$changelog"
}

body="$(extract "$version")"
if [[ -z "${body//[[:space:]]/}" ]]; then
  body="$(extract "Unreleased")"
fi

if [[ -z "${body//[[:space:]]/}" ]]; then
  echo "no changelog section found for '$version' or 'Unreleased'" >&2
  exit 1
fi

# Trim leading/trailing blank lines.
printf '%s\n' "$body" | sed -e '/./,$!d' | tac | sed -e '/./,$!d' | tac
