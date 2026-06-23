#!/usr/bin/env bash
# Lightweight, fast check of whether this machine is set up to develop the repo.
# Exit 0 = ready; exit 1 = needs `just setup`. No installs, no network: a few
# `command -v` calls plus one fingerprint hash of small config files.
set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
_load_tool_env

if _check_ready; then
  echo "✓ dev environment ready"
  optional_missing="$(_missing_bins "$OPTIONAL_BINS")"
  [ -n "$optional_missing" ] \
    && echo "note: optional tools unavailable:${optional_missing} (runtime-only; gate unaffected)"
  exit 0
fi

echo "dev environment needs setup: ${REASON}"
echo "run: just setup   (or, without just: ./scripts/setup.sh)"
exit 1
