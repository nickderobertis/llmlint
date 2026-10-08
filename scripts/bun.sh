#!/usr/bin/env bash
# Resolve (and, for bootstrap, install) the bun version `.tool-versions` pins.
#
#   scripts/bun.sh ensure   install the pinned bun if no pinned copy is available
#   scripts/bun.sh path     print the pinned bun's path; fail if there is none
#
# Nx — the orchestrator every gate recipe delegates to — is installed by bun from
# the locked bun.lock, so the pin is what makes a clean clone and CI resolve the
# same toolchain. A `bun` already first on PATH is used only when it reports the
# pinned version; otherwise the pinned release is downloaded from bun's GitHub
# release, verified against its published SHA-256 sums, and unpacked into a
# per-version cache directory outside the clone (never onto PATH, and no shell rc
# file is edited). CI provisions the pin itself (oven-sh/setup-bun reading
# .tool-versions), so `ensure` finds it on PATH there and installs nothing.
#
# BUN_SH_DOWNLOAD_BASE replaces the release URL prefix (an https:// or file://
# URL; anything else is refused) so the repo-tooling journeys can drive the
# download-and-verify path offline; it exists for those tests only.
#
# Exit status: 0 the pinned bun is available (`path` prints it); 1 it is not and
# could not be installed (the message says why); 2 a usage error (no mode, more
# than one, or an unknown one).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" \
  || {
    echo "bun.sh: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout whose directories you can read and enter." >&2
    exit 1
  }
readonly ROOT
readonly MODE="${1:-}"

fail() {
  echo "bun.sh: $*" >&2
  exit 1
}

usage() {
  echo "bun.sh: $*; usage: scripts/bun.sh ensure | path" >&2
  exit 2
}

[ "$#" -eq 1 ] || usage "expected exactly one mode (got $#)"
[ -r "$ROOT/.tool-versions" ] || fail "cannot read $ROOT/.tool-versions; restore it (it pins bun) from git."
# Exactly one `bun X.Y.Z` line: the value lands in a cache path and a download
# URL, so a second pin or anything but a plain version is refused, not guessed at.
pins="$(awk '$1 == "bun" { print $2 }' "$ROOT/.tool-versions")" \
  || fail "cannot read the bun pin from $ROOT/.tool-versions; restore it from git."
[ "$(printf '%s\n' "$pins" | grep -c .)" -eq 1 ] \
  || fail ".tool-versions must pin bun exactly once, as 'bun X.Y.Z' (found $(printf '%s\n' "$pins" | grep -c .) bun lines); restore the line from git (git checkout -- .tool-versions)."
VERSION="$pins"
readonly VERSION
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || fail ".tool-versions must pin bun as 'bun X.Y.Z' (got '${VERSION}'); restore the line from git (git checkout -- .tool-versions)."

# The cache root is where `ensure` creates and replaces the bun binary, so it must
# be an absolute path (XDG_CACHE_HOME when set, else HOME/.cache), never one
# resolved against whatever directory the recipe happens to run from.
if [ -n "${XDG_CACHE_HOME:-}" ]; then
  case "$XDG_CACHE_HOME" in
    /*) cache_root="$XDG_CACHE_HOME" ;;
    *) fail "XDG_CACHE_HOME must be an absolute path (got '${XDG_CACHE_HOME}'); fix it or unset it to use \$HOME/.cache." ;;
  esac
else
  case "${HOME:-}" in
    /*) cache_root="$HOME/.cache" ;;
    *) fail "HOME must be an absolute path to find the bun cache (got '${HOME:-}'), or set XDG_CACHE_HOME to an absolute directory." ;;
  esac
fi
readonly CACHE_DIR="$cache_root/llmlint-dev/bun-$VERSION"

# The pinned bun, if one is available: PATH first, then the cache.
pinned_bun() {
  local candidate
  for candidate in "$(command -v bun 2>/dev/null || true)" "$CACHE_DIR/bin/bun"; do
    if [ -z "$candidate" ] || [ ! -x "$candidate" ]; then
      continue
    fi
    if [ "$("$candidate" --version 2>/dev/null)" = "$VERSION" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

asset_name() {
  local os arch
  os="$(uname -s)" || fail "'uname -s' failed (above); cannot pick a bun build for this host. Check the uname first on PATH (command -v uname), or install bun $VERSION first on PATH yourself, then re-run 'just bootstrap'."
  arch="$(uname -m)" || fail "'uname -m' failed (above); cannot pick a bun build for this host. Check the uname first on PATH (command -v uname), or install bun $VERSION first on PATH yourself, then re-run 'just bootstrap'."
  case "$os/$arch" in
    Linux/x86_64) echo "bun-linux-x64" ;;
    Linux/aarch64 | Linux/arm64) echo "bun-linux-aarch64" ;;
    Darwin/arm64) echo "bun-darwin-aarch64" ;;
    Darwin/x86_64) echo "bun-darwin-x64" ;;
    *) fail "no bun build for $os/$arch here; install bun $VERSION yourself (https://bun.sh) so it is first on PATH." ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "no SHA-256 tool to verify the download; install coreutils (sha256sum) or perl (shasum), then re-run 'just bootstrap'."
  fi
}

remove_tmp() {
  rm -rf "$BUN_SH_TMP" || echo "bun.sh: could not remove $BUN_SH_TMP; delete it by hand" >&2
}

install_pinned() {
  local asset tmp base expected actual
  asset="$(asset_name)"
  command -v curl >/dev/null 2>&1 \
    || fail "curl is required to install bun $VERSION; install it with your package manager (e.g. 'apt-get install curl'), then re-run 'just bootstrap'."
  command -v unzip >/dev/null 2>&1 \
    || fail "unzip is required to install bun $VERSION; install it with your package manager (e.g. 'apt-get install unzip'), then re-run 'just bootstrap'."
  tmp="$(mktemp -d)" || fail "could not create a temporary directory; check TMPDIR is writable, then re-run 'just bootstrap'."
  # Held in a global the trap reads when it fires (this function's locals are
  # gone by then), so the path is never re-parsed as shell text: any TMPDIR,
  # quotes and spaces included, is removed as given.
  BUN_SH_TMP="$tmp"
  trap remove_tmp EXIT
  base="${BUN_SH_DOWNLOAD_BASE:-https://github.com/oven-sh/bun/releases/download}"
  # The whole shape, not just the scheme: a host (or, for file://, a path) must
  # follow it, and no whitespace may appear anywhere in the value.
  if ! [[ $base =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?(/[^[:space:]]*)?$ || $base =~ ^file:///[^[:space:]]+$ ]]; then
    fail "BUN_SH_DOWNLOAD_BASE must be an https:// or file:// URL: https://<host>[/path] or file:///<path>, with no whitespace (got '${base}'); unset it to download from bun's GitHub release."
  fi
  base="$base/bun-v$VERSION"
  echo "bun.sh: installing bun $VERSION into $CACHE_DIR" >&2
  curl -fsSL --retry 3 -o "$tmp/$asset.zip" "$base/$asset.zip" \
    || fail "downloading $base/$asset.zip failed; check your network and re-run 'just bootstrap'."
  curl -fsSL --retry 3 -o "$tmp/SHASUMS256.txt" "$base/SHASUMS256.txt" \
    || fail "downloading $base/SHASUMS256.txt failed; check your network and re-run 'just bootstrap'."
  expected="$(awk -v f="$asset.zip" '$2 == f { print $1 }' "$tmp/SHASUMS256.txt")" \
    || fail "could not read $tmp/SHASUMS256.txt; re-run 'just bootstrap'."
  actual="$(sha256_of "$tmp/$asset.zip")" \
    || fail "could not hash $tmp/$asset.zip; re-run 'just bootstrap'."
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    fail "checksum mismatch for $asset.zip (expected '${expected}', got '${actual}'); not installing. Re-run 'just bootstrap' (a truncated download heals); if it persists, report it — do not bypass the check."
  fi
  unzip -q "$tmp/$asset.zip" -d "$tmp" \
    || fail "could not unpack $asset.zip (above); re-run 'just bootstrap', and report it if it persists."
  [ -f "$tmp/$asset/bun" ] \
    || fail "$asset.zip holds no $asset/bun (the release layout changed); install bun $VERSION first on PATH instead, and report it."
  { mkdir -p "$CACHE_DIR/bin" \
    && mv "$tmp/$asset/bun" "$CACHE_DIR/bin/bun.partial" \
    && chmod +x "$CACHE_DIR/bin/bun.partial" \
    && mv "$CACHE_DIR/bin/bun.partial" "$CACHE_DIR/bin/bun"; } \
    || fail "could not install into $CACHE_DIR (above); make it writable (or set XDG_CACHE_HOME elsewhere), then re-run 'just bootstrap'."
}

case "$MODE" in
  path)
    pinned_bun || fail "bun $VERSION (pinned in .tool-versions) is not installed; run 'just bootstrap'."
    ;;
  ensure)
    if ! pinned_bun >/dev/null; then
      case "${OS:-}${OSTYPE:-}" in
        *Windows_NT* | *msys* | *cygwin* | *win32*)
          fail "install bun $VERSION first on PATH (powershell -c \"irm bun.sh/install.ps1 | iex\" with BUN_VERSION), then re-run 'just bootstrap'."
          ;;
      esac
      install_pinned
      pinned_bun >/dev/null || fail "installed bun does not report $VERSION; remove $CACHE_DIR and re-run 'just bootstrap'."
    fi
    ;;
  *)
    usage "unknown mode '${MODE}'"
    ;;
esac
