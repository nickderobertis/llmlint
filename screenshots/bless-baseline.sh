#!/usr/bin/env bash
# Refresh THIS host's committed screencomp baseline from the capture in
# shots/current — the one place that decides which lane gets rewritten and where.
#
# Shared by `just screenshots-bless` (an intended output change) and the pre-push
# guard's drift path (.githooks/pre-push), so the two can never disagree about the
# lane or the manifest path; the guard's drift journey in tests/visual_guard.rs drives
# this script through the real hook. $SHOTS_CURRENT overrides the capture root.
#
set -euo pipefail

# The baseline paths below are relative to the repository root.
cd "$(dirname "${BASH_SOURCE[0]}")/.." || {
  echo "bless-baseline: cannot enter the repository root; run it by its path from a readable checkout." >&2
  exit 1
}

if ! command -v screencomp >/dev/null 2>&1; then
  echo "bless-baseline: screencomp is not installed, so the baseline cannot be" >&2
  echo "                refreshed. Install it and retry:" >&2
  echo "                https://github.com/nickderobertis/screencomp#install" >&2
  exit 1
fi

current="${SHOTS_CURRENT:-shots/current}"
if [ ! -d "$current" ]; then
  echo "bless-baseline: no capture to bless at $current" >&2
  echo "                Capture one first: just screenshots" >&2
  exit 1
fi

if ! lane="$(bash screenshots/host-arch.sh)"; then
  echo "bless-baseline: could not name this host's lane (host-arch.sh's error above)" >&2
  echo "                It reads 'uname -m'; make that work on this host, then re-run" >&2
  echo "                just screenshots-bless." >&2
  exit 1
fi
if ! screencomp manifest --input "$current" --arch "$lane" --output "shots/baseline/${lane}.json"; then
  echo "bless-baseline: screencomp could not write shots/baseline/${lane}.json (error above)" >&2
  echo "                Check shots/baseline/ is writable and screencomp is installed, then re-run." >&2
  exit 1
fi
