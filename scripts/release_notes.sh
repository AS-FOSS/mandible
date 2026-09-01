#!/usr/bin/env bash
# Print the complete GitHub Release body for one version.
#
#   scripts/release_notes.sh 0.6.1 > release-notes.md
#
# The structure is the one the maintainer settled on by hand-editing the
# 0.5.0 release notes, now reproduced mechanically for every tag:
#
#   # Release Notes - Version X.Y.Z
#   ## Install                     (scripts/install_block.sh)
#   ---
#   ### Added / Changed / Fixed    (scripts/changelog_section.sh — the
#                                   CHANGELOG section, verbatim)
#
# One script rather than shell in the workflow YAML, so the exact body a
# tag will publish can be rendered and read locally first — a broken
# release body should not be something discovered after tagging.
set -euo pipefail

version="${1:?usage: release_notes.sh <version>}"
version="${version#v}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "# Release Notes - Version ${version}"
echo
"$here/install_block.sh"
echo
echo "---"
echo
"$here/changelog_section.sh" "$version"
