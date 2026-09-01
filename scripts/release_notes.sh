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
#   ---
#   ### Contributors               (external PR authors merged since the
#                                   previous tag, and reporters of the issues
#                                   the section cites — omitted when there are
#                                   none, or when `gh` cannot answer)
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

# Contributors: people who are not the project's own maintainers — anyone
# with push access to the repository (the `collaborators` API, readable by
# the release job's token and by a maintainer locally), falling back to the
# owner alone when that list cannot be read — and not bots, whose PRs merged since the previous tag, plus the reporters of the
# issues the section cites as `#N` (a cited PR number is not a report).
# Best-effort: a missing `gh` or token prints nothing rather than failing
# the release.
contributors() {
  command -v gh >/dev/null 2>&1 || return 0
  local repo owner prev since insiders
  repo="$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null)" || return 0
  owner="${repo%%/*}"
  insiders="$(printf '%s\n' "$owner"; gh api "repos/${repo}/collaborators?per_page=100" \
    -q '.[] | select(.permissions.push) | .login' 2>/dev/null || true)"
  prev="$(git tag --sort=-v:refname | grep -A1 -x "v${version}" | tail -n1)"
  [[ "$prev" == "v${version}" ]] && prev=""
  since="1970-01-01T00:00:00Z"
  [[ -n "$prev" ]] && since="$(git log -1 --format=%cI "$prev" 2>/dev/null || echo "$since")"
  is_insider() { grep -qx -- "$1" <<<"$insiders"; }
  {
    gh pr list --repo "$repo" --state merged --limit 200 --json number,title,author,mergedAt \
      -q ".[] | select(.mergedAt > \"${since}\") | select(.author.is_bot | not) | \
          \"\\(.author.login)\\t* @\\(.author.login) — #\\(.number) \\(.title)\"" 2>/dev/null
    "$here/changelog_section.sh" "$version" | grep -oE '#[0-9]+' | sort -u | tr -d '#' \
      | while read -r n; do
          gh api "repos/${repo}/issues/${n}" \
            -q "select(.pull_request == null) | \
                \"\\(.user.login)\\t* @\\(.user.login) — reported #\\(.number) \\(.title)\"" 2>/dev/null
        done
  } | while IFS=$'\t' read -r login line; do
        [[ -z "$login" ]] && continue
        is_insider "$login" || printf '%s\n' "$line"
      done | awk '!seen[$0]++'
}

body="$(contributors)"
if [[ -n "$body" ]]; then
  echo
  echo "---"
  echo
  echo "### Contributors"
  echo
  printf '%s\n' "$body"
fi
