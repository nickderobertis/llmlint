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
# `ci_install_freeze_*` journeys in tests/visual_guard.rs gate them against each other.
set -euo pipefail

freeze_version="0.2.2"

# freeze's Linux release assets are named for the arch the way screencomp names
# its lanes (x86_64 / arm64); map explicitly anyway, since that is a coincidence
# of two vocabularies rather than one shared name.
host="$(uname -m)" || {
  echo "ci-install-freeze: 'uname -m' failed (above); cannot pick the freeze asset for this runner. Check that the uname first on PATH (command -v uname) is the system one, then re-run." >&2
  exit 1
}
case "$host" in
  x86_64 | amd64) asset_arch="x86_64" ;;
  arm64 | aarch64) asset_arch="arm64" ;;
  *)
    echo "ci-install-freeze: no pinned freeze build for this architecture: $host" >&2
    echo "                   freeze v$freeze_version ships Linux x86_64 and arm64 only." >&2
    echo "                   Run the capture on one of those, or install freeze from" >&2
    echo "                   source first: just screenshots-tools" >&2
    exit 1
    ;;
esac

# Overridable so the journeys in tests/visual_guard.rs can drive the real script
# against a stand-in release tree and digest pin instead of the network; CI uses the defaults. `-`
# rather than `:-`: an override that is SET but empty is a misconfigured caller
# (an unset variable expanded into it), which the checks below reject — silently
# falling back to the default would install somewhere nobody asked for.
base_url="${FREEZE_BASE_URL-https://github.com/charmbracelet/freeze/releases/download}"
install_dir="${FREEZE_INSTALL_DIR-/usr/local/bin}"
sums_file="${FREEZE_SHA256_FILE-$(dirname "$0")/freeze.sha256}"

# Validate the three overrides before anything is fetched or written: each one
# steers a download or a filesystem write, so a malformed value must fail here
# with its own name attached, not deep inside curl/awk/install.
# The whole shape, not just the scheme: a host (or, for file://, a path) must
# follow it, and no whitespace may appear anywhere in the value.
if ! [[ $base_url =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?(/[^[:space:]]*)?$ || $base_url =~ ^file:///[^[:space:]]+$ ]]; then
  echo "ci-install-freeze: FREEZE_BASE_URL must be an https:// or file:// URL:" >&2
  echo "                   https://<host>[/path] or file:///<path>, with no whitespace" >&2
  echo "                   got: ${base_url:-<empty>}" >&2
  echo "                   Unset it to use the default release base." >&2
  exit 1
fi
case "$install_dir" in
  /*) ;;
  *)
    echo "ci-install-freeze: FREEZE_INSTALL_DIR must be an absolute directory path" >&2
    echo "                   to install into; got: ${install_dir:-<empty>}" >&2
    echo "                   Unset it to use /usr/local/bin." >&2
    exit 1
    ;;
esac
if [ ! -r "$sums_file" ]; then
  echo "ci-install-freeze: no readable digest pin file at $sums_file" >&2
  echo "                   Restore screenshots/freeze.sha256, or point" >&2
  echo "                   FREEZE_SHA256_FILE at a copy of it." >&2
  exit 1
fi

stem="freeze_${freeze_version}_Linux_${asset_arch}"
tmp="$(mktemp -d)" || {
  echo "ci-install-freeze: could not create a temporary directory (check TMPDIR)" >&2
  exit 1
}
trap 'rm -rf "$tmp" || echo "ci-install-freeze: could not remove $tmp; delete it by hand" >&2' EXIT

# Portable SHA-256 (Linux coreutils vs macOS/BSD), as in screenshots/screenshots.sh.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

if ! curl -fsSL -o "$tmp/freeze.tar.gz" "$base_url/v${freeze_version}/${stem}.tar.gz"; then
  echo "ci-install-freeze: could not download $base_url/v${freeze_version}/${stem}.tar.gz" >&2
  echo "                   Check the network and that freeze v${freeze_version} publishes it, then re-run." >&2
  exit 1
fi

# Validate the archive before unpacking it: the expected digest is pinned in THIS
# repository, not fetched beside the archive — a checksum served from the
# download's own origin vouches for nothing (the reasoning scripts/install.sh's
# `sum_trusted` applies). $sums_file holds the relevant lines of the release's
# checksums.txt verbatim, so refreshing it on a version bump is a copy.
if ! expected="$(awk -v want="${stem}.tar.gz" '$2 == want { print $1 }' "$sums_file")"; then
  echo "ci-install-freeze: could not read $sums_file; restore it from git." >&2
  exit 1
fi
if [ -z "$expected" ]; then
  echo "ci-install-freeze: no pinned sha256 for ${stem}.tar.gz in $sums_file" >&2
  echo "                   Add its line from" >&2
  echo "                   $base_url/v${freeze_version}/checksums.txt" >&2
  exit 1
fi
if ! actual="$(sha256 "$tmp/freeze.tar.gz")"; then
  echo "ci-install-freeze: could not hash the downloaded ${stem}.tar.gz (error above)" >&2
  echo "                   Install sha256sum (coreutils) or shasum (perl), then re-run." >&2
  exit 1
fi
if [ "$actual" != "$expected" ]; then
  echo "ci-install-freeze: sha256 mismatch for ${stem}.tar.gz — NOT installing" >&2
  echo "                   expected $expected (pinned in $sums_file)" >&2
  echo "                   got      $actual" >&2
  echo "                   If the pin is stale, refresh $sums_file from" >&2
  echo "                   $base_url/v${freeze_version}/checksums.txt; otherwise treat" >&2
  echo "                   the download as untrusted and do not retry blindly." >&2
  exit 1
fi

if ! tar -xzf "$tmp/freeze.tar.gz" -C "$tmp"; then
  echo "ci-install-freeze: ${stem}.tar.gz matched its pinned digest but would not unpack" >&2
  echo "                   (tar's error above); check $tmp has space, then re-run." >&2
  exit 1
fi
if [ ! -f "$tmp/$stem/freeze" ]; then
  echo "ci-install-freeze: ${stem}.tar.gz matched its pinned digest but holds no" >&2
  echo "                   $stem/freeze — upstream changed the archive layout." >&2
  echo "                   Re-pin freeze_version here and in the justfile against" >&2
  echo "                   the new layout." >&2
  exit 1
fi

if ! install -d "$install_dir" || ! install "$tmp/$stem/freeze" "$install_dir/freeze"; then
  echo "ci-install-freeze: could not install freeze into $install_dir (error above)" >&2
  echo "                   Make it writable, or point FREEZE_INSTALL_DIR elsewhere, then re-run." >&2
  exit 1
fi
echo "ci-install-freeze: installed freeze v$freeze_version ($asset_arch) to $install_dir"
