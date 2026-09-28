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
# shellcheck source=scripts/setup-lib.sh
. "$ROOT/scripts/setup-lib.sh"
version="$(cd "$ROOT" && _justfile_pin actionlint)"
if [ -z "$version" ]; then
  echo "install-actionlint: no actionlint-version pin in $ROOT/justfile" >&2
  echo "                    Restore the line: actionlint-version := \"<version>\"" >&2
  exit 1
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
if [ ! -r "$sums_file" ]; then
  echo "install-actionlint: no readable digest pin file at $sums_file" >&2
  echo "                    Restore scripts/actionlint.sha256, or point" >&2
  echo "                    ACTIONLINT_SHA256_FILE at a copy of it." >&2
  exit 1
fi

# Idempotent: `just setup` runs this on every provision, so an install dir that
# already holds the pinned version is left alone (and needs no network).
if [ "$("$install_dir/actionlint" -version 2>/dev/null | head -n1 || true)" = "$version" ]; then
  echo "install-actionlint: actionlint $version already in $install_dir"
  exit 0
fi

asset="actionlint_${version}_${asset_os}_${asset_arch}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$asset" "$base_url/v${version}/${asset}"

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
if [ "$actual" != "$expected" ]; then
  echo "install-actionlint: sha256 mismatch for $asset — NOT installing" >&2
  echo "                    expected $expected (pinned in $sums_file)" >&2
  echo "                    got      $actual" >&2
  echo "                    If the pin is stale, refresh $sums_file from" >&2
  echo "                    $base_url/v${version}/actionlint_${version}_checksums.txt;" >&2
  echo "                    otherwise treat the download as untrusted and do not retry blindly." >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
if [ ! -f "$tmp/actionlint" ]; then
  echo "install-actionlint: $asset matched its pinned digest but holds no" >&2
  echo "                    top-level actionlint — upstream changed the archive layout." >&2
  echo "                    Update the extraction above to the new layout, or pin an" >&2
  echo "                    earlier actionlint-version in the justfile." >&2
  exit 1
fi

install -d "$install_dir"
install "$tmp/actionlint" "$install_dir/actionlint"
echo "install-actionlint: installed actionlint $version ($asset_os/$asset_arch) to $install_dir"
