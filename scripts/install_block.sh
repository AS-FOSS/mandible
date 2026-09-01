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
# page tells them the channel was aspirational. Commands stay bare — no
# trailing comments; the label above each block does the explaining, so
# what a reader copies is exactly what they run (maintainer rule,
# 2026-08-30).
set -euo pipefail

block() { # label, command...
  echo "**$1**"
  echo
  echo '```console'
  shift
  printf '%s\n' "$@"
  echo '```'
  echo
}

# Labels are short, with the qualifier in parentheses and no explanatory
# clause — the shape the maintainer settled on by hand-editing the 0.5.0
# release notes, which this script reproduces from now on.
echo "## Install"
echo
block "Homebrew (macOS or Linux)" \
  'brew install as-foss/mandible/mandible'
block "Fedora / EPEL (COPR repository)" \
  'sudo dnf copr enable as-foss/mandible' \
  'sudo dnf install mandible'
block "Nix" \
  'nix run github:AS-FOSS/mandible'
block "Cargo" \
  'cargo binstall mandible' \
  'cargo install mandible'
echo "Debian and Ubuntu: the signed [apt repository](https://as-foss.github.io/mandible-apt) carries every released version for amd64 and arm64, with setup instructions."
echo
echo "*Or download a binary below. Verify with the accompanying \`.sha256\`.*"
