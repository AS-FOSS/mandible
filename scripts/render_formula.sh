#!/usr/bin/env bash
# Render the Homebrew formula for one released version.
#
#   scripts/render_formula.sh 0.4.5 <dir-with-sha256-files> [template] > mandible.rb
#
# `<dir-with-sha256-files>` holds the `.sha256` files that the release
# workflow already published as assets, one per target:
#
#   mandible-v0.4.5-aarch64-apple-darwin.tar.gz.sha256
#   mandible-v0.4.5-x86_64-apple-darwin.tar.gz.sha256
#   mandible-v0.4.5-aarch64-unknown-linux-gnu.tar.gz.sha256
#   mandible-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
#
# The checksums are *read from those assets*, never recomputed here. A
# second build to re-hash the tarballs would be a second build: if it ever
# disagreed with the one the release published, the formula would describe
# a binary nobody can download. Reading the published `.sha256` makes the
# formula a statement about the release, which is what a user verifying a
# download is actually checking against.
#
# Exists as a file rather than inline YAML for the same reason
# changelog_section.sh does: it can be run and checked locally, so a broken
# formula is not something you discover only after tagging.
set -euo pipefail

version="${1:?usage: render_formula.sh <version> <sha256-dir> [template]}"
sums_dir="${2:?usage: render_formula.sh <version> <sha256-dir> [template]}"
template="${3:-packaging/homebrew/mandible.rb.tmpl}"

version="${version#v}"

[[ -f "$template" ]] || { echo "no such template: $template" >&2; exit 1; }
[[ -d "$sums_dir" ]] || { echo "no such directory: $sums_dir" >&2; exit 1; }

# Read one target's checksum out of its published `.sha256` asset.
#
# `shasum -a 256 <file>` writes "<64 hex>  <filename>", so both fields are
# checked: the digest must look like a sha256, and the filename it was
# computed over must be the asset this formula is about to point a URL at.
# A `.sha256` naming some other file is the one way this could silently
# publish a correct-looking hash for the wrong artifact.
read_sum() {
  local target="$1"
  local asset="mandible-v${version}-${target}.tar.gz"
  local file="${sums_dir}/${asset}.sha256"

  [[ -f "$file" ]] || { echo "missing checksum asset: ${file}" >&2; exit 1; }

  local digest name
  read -r digest name < "$file"

  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] ||
    { echo "${file}: '${digest}' is not a sha256 digest" >&2; exit 1; }
  # `shasum` may prefix the name with '*' in binary mode; strip it before
  # comparing rather than requiring one mode or the other.
  [[ "${name#\*}" == "$asset" ]] ||
    { echo "${file}: digest is over '${name}', expected '${asset}'" >&2; exit 1; }

  printf '%s' "$digest"
}

rendered="$(cat "$template")"
substitute() {
  # Bash parameter expansion, not sed: a digest can never contain a sed
  # delimiter, and this keeps the replacement free of escaping questions.
  #
  # The empty check is not defensive noise. `read_sum` reports its own
  # failures and exits, but it is called from a command substitution, and
  # an `exit` there ends only the subshell — so a value that failed to be
  # read arrives here as the empty string. Substituting it would replace
  # `@SHA256_…@` with nothing, leave no placeholder for the check below to
  # find, and emit a formula reading `sha256 ""`. Measured, not imagined:
  # before this check the three read_sum failure paths all exited 0 with a
  # formula written.
  [[ -n "$2" ]] || { echo "empty replacement for ${1}; refusing to emit the formula" >&2; exit 1; }
  rendered="${rendered//"$1"/"$2"}"
}

# One assignment per line, deliberately. `set -e` aborts on a failing
# command substitution only when the assignment is the whole simple
# command; inlining these into the `substitute` calls as arguments would
# discard read_sum's exit status entirely.
sum_arm_mac="$(read_sum aarch64-apple-darwin)"
sum_x86_mac="$(read_sum x86_64-apple-darwin)"
sum_arm_linux="$(read_sum aarch64-unknown-linux-gnu)"
sum_x86_linux="$(read_sum x86_64-unknown-linux-gnu)"

substitute '@VERSION@' "$version"
substitute '@SHA256_AARCH64_APPLE_DARWIN@' "$sum_arm_mac"
substitute '@SHA256_X86_64_APPLE_DARWIN@' "$sum_x86_mac"
substitute '@SHA256_AARCH64_UNKNOWN_LINUX_GNU@' "$sum_arm_linux"
substitute '@SHA256_X86_64_UNKNOWN_LINUX_GNU@' "$sum_x86_linux"

# Nothing may reach the tap with a placeholder still in it. A formula
# carrying a literal `@SHA256_…@` installs nothing and would sit in the tap
# until a user reported it.
if grep -n '@[A-Z0-9_]\+@' <<< "$rendered" >&2; then
  echo "unsubstituted placeholder(s) above; refusing to emit the formula" >&2
  exit 1
fi

printf '%s\n' "$rendered"
