#!/usr/bin/env bash
# Print the install-options block that heads every release body.
#
#   scripts/install_block.sh
#
# Kept beside changelog_section.sh, and a file for the same reason: the
# release body is assembled from checked-in scripts so it can be rendered
# and read locally, rather than being discovered after a tag is pushed.
#
# Every entry below is a channel that works today. A package manager
# advertised before it can actually serve the release is worse than one
# not mentioned at all — the reader tries it, it fails, and nothing on the
# page tells them the channel was aspirational. Adding a channel is one
# line in the array; the widths line up because the comment column is
# padded, so keep the padding when adding to it.
set -euo pipefail

channels=(
  'brew install as-foss/mandible/mandible   # Homebrew, macOS or Linux'
  'cargo binstall mandible                  # prebuilt binary, no compiling'
  'cargo install mandible                   # from source'
  'nix run github:AS-FOSS/mandible          # without installing'
  'dnf copr enable as-foss/mandible         # Fedora and EPEL: enable the COPR repo'
  'dnf install mandible                     # ...and install from it'
)

echo "## Install"
echo
echo '```console'
printf '%s\n' "${channels[@]}"
echo '```'
echo
echo "Debian and Ubuntu: the signed [apt repository](https://as-foss.github.io/mandible-apt)"
echo "carries every released version for amd64 and arm64, with setup instructions."
echo
echo "Binaries, \`.deb\` and \`.rpm\` packages, each with a matching \`.sha256\`, are"
echo "attached below."
