#!/usr/bin/env bash
# Install the pinned released oneharness (`oneharness-cli` on PyPI, the prebuilt
# binary wheel) into a private venv under .dev/ and print the binary's path on
# stdout — nothing else. `just test-oneharness` runs its real-oneharness tier
# against that binary. The version is `oneharness-cli-version` in the justfile —
# the one pin, which `tests/real_oneharness.rs` holds to llmlint's multi-file
# floor. Idempotent: an existing install at the pin is reused. Needs network
# (PyPI) the first time.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
# `|| true`: under pipefail a missing pin would otherwise exit here silently.
version="$(_justfile_pin oneharness-cli || true)"
if [ -z "$version" ]; then
  echo "install-oneharness: no oneharness-cli-version pin in $ROOT/justfile" >&2
  echo "                    Restore the line: oneharness-cli-version := \"<version>\"" >&2
  exit 1
fi

venv="$ROOT/.dev/oneharness-cli-$version"
# A venv lays its executables out in `bin/` on Unix and `Scripts/` on Windows.
find_bin() {
  for b in "$venv/bin/oneharness" "$venv/Scripts/oneharness.exe"; do
    [ -x "$b" ] && { printf '%s\n' "$b"; return 0; }
  done
  return 1
}

if ! bin="$(find_bin)" || ! "$bin" --version 2>/dev/null | grep -qF "oneharness $version"; then
  py="$(command -v python3 || command -v python || true)"
  if [ -z "$py" ]; then
    echo "install-oneharness: python3 is required to install oneharness-cli==$version" >&2
    echo "                    Install Python 3 (with its venv module), then re-run just test-oneharness." >&2
    exit 1
  fi
  rm -rf "$venv"
  if ! "$py" -m venv "$venv" >&2; then
    echo "install-oneharness: $py -m venv could not create $venv" >&2
    echo "                    Install Python's venv module (e.g. python3-venv), then re-run just test-oneharness." >&2
    exit 1
  fi
  pip_py="$venv/bin/python"
  [ -x "$pip_py" ] || pip_py="$venv/Scripts/python.exe"
  if ! "$pip_py" -m pip install --quiet --disable-pip-version-check "oneharness-cli==$version" >&2; then
    echo "install-oneharness: pip could not install oneharness-cli==$version from PyPI" >&2
    echo "                    Check network access to pypi.org, then re-run just test-oneharness." >&2
    exit 1
  fi
  if ! bin="$(find_bin)"; then
    echo "install-oneharness: oneharness-cli==$version installed no oneharness binary in $venv" >&2
    echo "                    Check that PyPI has a wheel for this platform, then delete $venv and re-run just test-oneharness." >&2
    exit 1
  fi
fi
printf '%s\n' "$bin"
