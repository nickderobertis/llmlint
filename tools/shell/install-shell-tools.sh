#!/usr/bin/env bash
# Install the pinned shfmt and shellcheck (the shell format and lint every shell
# project's `format` and `lint` targets run through tools/shell/shell.sh) from
# their prebuilt releases, choosing the build for this host's OS and architecture
# and verifying each against a digest pinned in this repository. `just bootstrap`
# and `just shell-tools` run it.
#
# The versions are `shfmt-version` and `shellcheck-version` in the justfile — the
# one pin, which shell.sh also checks the installed tools against.
#
# LLMLINT_SHELL_TOOLS=off (the macOS/Windows `cross` jobs, whose gate leaves shell
# format and lint to Linux) makes this a no-op with a notice.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
  || {
    echo "install-shell-tools: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout." >&2
    exit 1
  }
readonly ROOT

case "${LLMLINT_SHELL_TOOLS:-on}" in
  on) ;;
  off)
    echo "install-shell-tools: LLMLINT_SHELL_TOOLS=off — shfmt and shellcheck not installed; the Linux gate runs them." >&2
    exit 0
    ;;
  *)
    echo "install-shell-tools: LLMLINT_SHELL_TOOLS must be 'on' (the default) or 'off' (got '${LLMLINT_SHELL_TOOLS}')" >&2
    exit 2
    ;;
esac

# Overridable so the journeys in tools/shell/tests drive the real script against
# a stand-in release tree and digest pin instead of the network; bootstrap and CI
# use the defaults. `-` rather than `:-`: an override that is SET but empty is a
# misconfigured caller, which the checks below reject.
shfmt_base="${SHFMT_BASE_URL-https://github.com/mvdan/sh/releases/download}"
shellcheck_base="${SHELLCHECK_BASE_URL-https://github.com/koalaman/shellcheck/releases/download}"
install_dir="${SHELL_TOOLS_INSTALL_DIR-$HOME/.local/bin}"
sums_file="${SHELL_TOOLS_SHA256_FILE-$ROOT/tools/shell/shell-tools.sha256}"

fail() {
  local line
  for line in "$@"; do
    echo "install-shell-tools: $line" >&2
  done
  exit 1
}

for url in "$shfmt_base" "$shellcheck_base"; do
  if ! [[ $url =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?(/[^[:space:]]*)?$ || $url =~ ^file:///[^[:space:]]+$ ]]; then
    fail "a release base URL must be https://<host>[/path] or file:///<path>, with no whitespace;" \
      "got: ${url:-<empty>} (from SHFMT_BASE_URL or SHELLCHECK_BASE_URL — unset it to use the default)"
  fi
done
[ -n "$install_dir" ] || fail "SHELL_TOOLS_INSTALL_DIR is empty; it must name a directory to install into (unset it to use ~/.local/bin)."
# `install` would read a leading `-` as an option, not a directory.
case "$install_dir" in
  -*) fail "SHELL_TOOLS_INSTALL_DIR must not start with '-' (got: $install_dir); give the directory as ./$install_dir or an absolute path." ;;
esac
[ -f "$sums_file" ] && [ -r "$sums_file" ] \
  || fail "no readable digest pin file at $sums_file;" "restore tools/shell/shell-tools.sha256, or point SHELL_TOOLS_SHA256_FILE at a copy of it."

# A pin becomes part of a download URL and an archive path, so it must be a
# plain release version.
pin() {
  local v
  v="$({ grep -E "^$1-version :=" "$ROOT/justfile" 2>/dev/null || true; } | head -n1 | cut -d'"' -f2)"
  [ -n "$v" ] || fail "no $1-version pin in $ROOT/justfile;" "restore the line: $1-version := \"<version>\""
  [[ $v =~ ^[0-9]+(\.[0-9]+)*$ ]] \
    || fail "the $1-version pin in $ROOT/justfile is not a release version (got: $v);" "set it to the release's dotted number, e.g. $1-version := \"1.2.3\""
  printf '%s\n' "$v"
}
shfmt_version="$(pin shfmt)" || exit 1
shellcheck_version="$(pin shellcheck)" || exit 1

if command -v sha256sum >/dev/null 2>&1; then
  sha_tool=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
  sha_tool=(shasum -a 256)
else
  fail "no SHA-256 tool (sha256sum or shasum) on PATH, so the downloads cannot be verified — NOT installing." \
    "Install coreutils (sha256sum) and re-run."
fi

if ! host_os="$(uname -s)" || ! host_arch="$(uname -m)"; then
  fail "uname failed, so the host's OS and architecture are unknown;" "check that uname is on PATH and runs, then re-run."
fi
case "$host_os" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) fail "no pinned shfmt/shellcheck build for this OS: $host_os;" "install shfmt $shfmt_version and shellcheck $shellcheck_version yourself and put them on PATH." ;;
esac
case "$host_arch" in
  x86_64 | amd64) go_arch=amd64 sc_arch=x86_64 ;;
  arm64 | aarch64) go_arch=arm64 sc_arch=aarch64 ;;
  *) fail "no pinned shfmt/shellcheck build for this architecture: $host_arch;" "install shfmt $shfmt_version and shellcheck $shellcheck_version yourself and put them on PATH." ;;
esac

if ! tmp="$(mktemp -d)"; then
  fail "could not create a temporary directory; check that ${TMPDIR:-/tmp} exists, is writable, and has free space."
fi
trap 'rm -rf "$tmp" || echo "install-shell-tools: could not remove the scratch directory $tmp; delete it by hand." >&2' EXIT

fetch() {
  local url="$1" asset="$2" expected actual
  if ! curl -fsSL -o "$tmp/$asset" "$url"; then
    fail "could not download $url;" "check the network (or the base URL override) and re-run."
  fi
  if ! expected="$(awk -v want="$asset" '$2 == want { print $1 }' "$sums_file")"; then
    fail "could not read $sums_file;" "check that it is readable, then re-run."
  fi
  [ -n "$expected" ] || fail "no pinned sha256 for $asset in $sums_file;" "add its line from the release's published digests."
  if ! actual="$("${sha_tool[@]}" "$tmp/$asset" | awk '{print $1}')" || [ -z "$actual" ]; then
    fail "${sha_tool[*]} could not hash the downloaded $asset — NOT installing;" "check ${TMPDIR:-/tmp} is readable and re-run."
  fi
  if [ "$actual" != "$expected" ]; then
    fail "sha256 mismatch for $asset — NOT installing" "expected $expected (pinned in $sums_file)" "got      $actual" \
      "If the pin is stale, refresh it from the release; otherwise treat the download as untrusted and do not retry blindly."
  fi
}

place() {
  if ! { install -d "$install_dir" && install "$1" "$install_dir/$2"; }; then
    fail "could not install $2 into $install_dir;" "point SHELL_TOOLS_INSTALL_DIR at a writable directory on PATH."
  fi
}

# Idempotent: the pinned version already in the install dir is left alone (and
# needs no network); anything else there is replaced.
current() {
  [ -x "$install_dir/$1" ] || return 0
  case "$1" in
    shfmt) { "$install_dir/shfmt" --version 2>/dev/null || true; } | head -n1 | sed 's/^v//' ;;
    shellcheck) { "$install_dir/shellcheck" --version 2>/dev/null || true; } | sed -n 's/^version: //p' ;;
  esac
}

if [ "$(current shfmt)" = "$shfmt_version" ]; then
  shfmt_note="already there"
else
  asset="shfmt_v${shfmt_version}_${os}_${go_arch}"
  fetch "$shfmt_base/v${shfmt_version}/$asset" "$asset"
  place "$tmp/$asset" shfmt
  shfmt_note="installed for $os/$go_arch"
fi

if [ "$(current shellcheck)" = "$shellcheck_version" ]; then
  shellcheck_note="already there"
else
  asset="shellcheck-v${shellcheck_version}.${os}.${sc_arch}.tar.gz"
  fetch "$shellcheck_base/v${shellcheck_version}/$asset" "$asset"
  if ! tar -xzf "$tmp/$asset" -C "$tmp"; then
    fail "$asset matched its pinned digest but did not unpack;" "if ${TMPDIR:-/tmp} is full, free space and re-run."
  fi
  [ -f "$tmp/shellcheck-v${shellcheck_version}/shellcheck" ] \
    || fail "$asset matched its pinned digest but holds no shellcheck-v${shellcheck_version}/shellcheck —" \
      "upstream changed the archive layout; update the extraction above."
  place "$tmp/shellcheck-v${shellcheck_version}/shellcheck" shellcheck
  shellcheck_note="installed for $os/$sc_arch"
fi

echo "install-shell-tools: shfmt $shfmt_version ($shfmt_note), shellcheck $shellcheck_version ($shellcheck_note) in $install_dir"
