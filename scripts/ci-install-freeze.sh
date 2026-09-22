#!/usr/bin/env bash
# Install the pinned `freeze` (the screenshot renderer) from its prebuilt release,
# choosing the build that matches the RUNNER's architecture.
#
# CI's Visual-docs workflow fans out one job per lane in [capture].arches of
# screencomp.toml, and screencomp's reusable workflow runs the arm64 lane on
# `ubuntu-24.04-arm` — so the capture cannot hard-code one asset name. This picks
# it from `uname -m` instead. Local installs go through `just screenshots-tools`
# (Go), which is why this lives beside CI rather than in that recipe.
#
# Keep `freeze_version` below in sync with `freeze-version` in the justfile — the
# `ci_install_freeze_*` journeys in tests/e2e/main.rs gate them against each other.
set -euo pipefail

freeze_version="0.2.2"

# freeze's Linux release assets are named for the arch the way screencomp names
# its lanes (x86_64 / arm64); map explicitly anyway, since that is a coincidence
# of two vocabularies rather than one shared name.
host="$(uname -m)"
case "$host" in
x86_64 | amd64) asset_arch="x86_64" ;;
arm64 | aarch64) asset_arch="arm64" ;;
*)
  echo "ci-install-freeze: no pinned freeze build for this architecture: $host" >&2
  echo "                   (freeze v$freeze_version ships Linux x86_64 and arm64)" >&2
  exit 1
  ;;
esac

# Overridable so the e2e journeys can drive the real script against a stand-in
# release tree instead of the network; CI uses the defaults.
base_url="${FREEZE_BASE_URL:-https://github.com/charmbracelet/freeze/releases/download}"
install_dir="${FREEZE_INSTALL_DIR:-/usr/local/bin}"

stem="freeze_${freeze_version}_Linux_${asset_arch}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/freeze.tar.gz" "$base_url/v${freeze_version}/${stem}.tar.gz"
tar -xzf "$tmp/freeze.tar.gz" -C "$tmp"

bin="$tmp/$stem/freeze"
if [ ! -f "$bin" ]; then
  bin="$(find "$tmp" -type f -name freeze | head -n 1)"
fi
if [ -z "$bin" ] || [ ! -f "$bin" ]; then
  echo "ci-install-freeze: no 'freeze' binary inside ${stem}.tar.gz" >&2
  exit 1
fi

install -d "$install_dir"
install "$bin" "$install_dir/freeze"
echo "ci-install-freeze: installed freeze v$freeze_version ($asset_arch) to $install_dir"
