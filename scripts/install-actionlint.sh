#!/usr/bin/env bash
# Install the pinned `actionlint` (the workflow linter `just lint-workflows` runs
# inside `just check`) from its prebuilt release, choosing the build for this
# host's OS and architecture and verifying it against a digest pinned in this
# repository. Run through `just actionlint-tools` (by `just setup` and CI's gate).
#
# The version is `actionlint-version` in the justfile — the one pin, which
# scripts/lint-workflows.sh also checks the installed binary against.
# llmlint: ignore-file[new_code_lands_in_a_project] a single binary crate with no Nx project graph (AGENTS.md) has no project for a shell script to belong to
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
# `|| true`: under pipefail a missing pin would otherwise exit here silently.
version="$(_justfile_pin actionlint || true)"
if [ -z "$version" ]; then
  echo "install-actionlint: no actionlint-version pin in $ROOT/justfile" >&2
  echo "                    Restore the line: actionlint-version := \"<version>\"" >&2
  exit 1
fi

# Overridable so the e2e journeys can drive the real script against a stand-in
# release tree and digest pin instead of the network; setup and CI use the
# defaults. `-` rather than `:-`: an override that is SET but empty is a
# misconfigured caller, which the checks below reject.
base_url="${ACTIONLINT_BASE_URL-https://github.com/rhysd/actionlint/releases/download}"
install_dir="${ACTIONLINT_INSTALL_DIR-$LOCAL_BIN}"
sums_file="${ACTIONLINT_SHA256_FILE-$ROOT/scripts/actionlint.sha256}"

case "$base_url" in
https://* | file://*) ;;
*)
  echo "install-actionlint: ACTIONLINT_BASE_URL must be an https:// or file:// URL" >&2
  echo "                    got: ${base_url:-<empty>}" >&2
  echo "                    Unset it to use the default release base." >&2
  exit 1
  ;;
esac
if [ -z "$install_dir" ]; then
  echo "install-actionlint: ACTIONLINT_INSTALL_DIR is empty; it must name a directory" >&2
  echo "                    to install into. Unset it to use $LOCAL_BIN." >&2
  exit 1
fi
if [ ! -f "$sums_file" ] || [ ! -r "$sums_file" ]; then
  echo "install-actionlint: no readable digest pin file at $sums_file" >&2
  echo "                    Restore scripts/actionlint.sha256, or point" >&2
  echo "                    ACTIONLINT_SHA256_FILE at a copy of it." >&2
  exit 1
fi

# Idempotent: `just setup` runs this on every provision, so the pinned version
# already in the install dir is left alone (and needs no network). With nothing
# in the install dir, a pinned actionlint on PATH (installed by hand on a platform
# this script has no build for) counts too; a stale one in the install dir is
# always replaced, since the lint step may find it first.
existing="$install_dir/actionlint"
[ -e "$existing" ] || existing="$(command -v actionlint || true)"
if [ -n "$existing" ] && [ "$("$existing" -version 2>/dev/null | head -n1 || true)" = "$version" ]; then
  echo "install-actionlint: actionlint $version already at $existing"
  exit 0
fi

os="$(uname -s)"
case "$os" in
Linux) asset_os="linux" ;;
Darwin) asset_os="darwin" ;;
*)
  echo "install-actionlint: no pinned actionlint build for this OS: $os" >&2
  echo "                    Install actionlint $version yourself (see" >&2
  echo "                    https://github.com/rhysd/actionlint/blob/main/docs/install.md)" >&2
  echo "                    and put it on PATH." >&2
  exit 1
  ;;
esac
host="$(uname -m)"
case "$host" in
x86_64 | amd64) asset_arch="amd64" ;;
arm64 | aarch64) asset_arch="arm64" ;;
*)
  echo "install-actionlint: no pinned actionlint build for this architecture: $host" >&2
  echo "                    Install actionlint $version yourself (see" >&2
  echo "                    https://github.com/rhysd/actionlint/blob/main/docs/install.md)" >&2
  echo "                    and put it on PATH." >&2
  exit 1
  ;;
esac

asset="actionlint_${version}_${asset_os}_${asset_arch}.tar.gz"
if ! tmp="$(mktemp -d)"; then
  echo "install-actionlint: could not create a temporary directory; check that" >&2
  echo "                    ${TMPDIR:-/tmp} exists, is writable, and has free space." >&2
  exit 1
fi
trap 'rm -rf "$tmp"' EXIT

if ! curl -fsSL -o "$tmp/$asset" "$base_url/v${version}/${asset}"; then
  echo "install-actionlint: could not download $base_url/v${version}/${asset}" >&2
  echo "                    Check the network (or ACTIONLINT_BASE_URL) and re-run." >&2
  exit 1
fi

# Validate before unpacking: the expected digest is pinned in THIS repository,
# not fetched beside the archive (the reasoning scripts/install.sh's
# `sum_trusted` applies).
expected="$(awk -v want="$asset" '$2 == want { print $1 }' "$sums_file")"
if [ -z "$expected" ]; then
  echo "install-actionlint: no pinned sha256 for $asset in $sums_file" >&2
  echo "                    Add its line from" >&2
  echo "                    $base_url/v${version}/actionlint_${version}_checksums.txt" >&2
  exit 1
fi
actual="$(_sha256_stdin <"$tmp/$asset")"
if [ "$actual" = "no-sha256-tool" ]; then
  echo "install-actionlint: no SHA-256 tool (sha256sum, shasum, or openssl) on PATH," >&2
  echo "                    so $asset cannot be verified — NOT installing." >&2
  echo "                    Install coreutils (sha256sum) or openssl and re-run." >&2
  exit 1
fi
if [ "$actual" != "$expected" ]; then
  echo "install-actionlint: sha256 mismatch for $asset — NOT installing" >&2
  echo "                    expected $expected (pinned in $sums_file)" >&2
  echo "                    got      $actual" >&2
  echo "                    If the pin is stale, refresh $sums_file from" >&2
  echo "                    $base_url/v${version}/actionlint_${version}_checksums.txt;" >&2
  echo "                    otherwise treat the download as untrusted and do not retry blindly." >&2
  exit 1
fi

if ! tar -xzf "$tmp/$asset" -C "$tmp"; then
  echo "install-actionlint: $asset matched its pinned digest but did not unpack." >&2
  echo "                    If ${TMPDIR:-/tmp} is full, free space and re-run; otherwise the" >&2
  echo "                    pinned release itself is not a .tar.gz — re-pin actionlint-version" >&2
  echo "                    and scripts/actionlint.sha256 to a release that is." >&2
  exit 1
fi
if [ ! -f "$tmp/actionlint" ]; then
  echo "install-actionlint: $asset matched its pinned digest but holds no" >&2
  echo "                    top-level actionlint — upstream changed the archive layout." >&2
  echo "                    Update the extraction above to the new layout, or pin an" >&2
  echo "                    earlier actionlint-version in the justfile." >&2
  exit 1
fi

if ! { install -d "$install_dir" && install "$tmp/actionlint" "$install_dir/actionlint"; }; then
  echo "install-actionlint: could not install into $install_dir" >&2
  echo "                    Point ACTIONLINT_INSTALL_DIR at a writable directory on PATH." >&2
  exit 1
fi
echo "install-actionlint: installed actionlint $version ($asset_os/$asset_arch) to $install_dir"
