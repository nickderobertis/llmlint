#!/usr/bin/env bash
# Print this host's screencomp capture lane: the normalized CPU architecture that
# names shots/current/<arch>/ and shots/baseline/<arch>.json.
#
# One place, so the three consumers can never disagree about what a lane is
# called: the capture (screenshots/screenshots.sh), the local pre-push guard
# (.githooks/pre-push), and `just screenshots-bless`. The names match
# [capture].arches in screencomp.toml, which is what screencomp fans CI out over.
set -euo pipefail

arch="$(uname -m)" || { echo "host-arch: 'uname -m' failed (above); this host's lane cannot be named until it works." >&2; exit 1; }
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="arm64" ;;
esac
printf '%s\n' "$arch"
