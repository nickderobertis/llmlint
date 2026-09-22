#!/usr/bin/env bash
# Refresh THIS host's committed screencomp baseline from the capture in
# shots/current — the one place that decides which lane gets rewritten and where.
#
# Shared by `just screenshots-bless` (an intended output change) and the pre-push
# guard's drift path (.githooks/pre-push), so the two can never disagree about the
# lane or the manifest path; the guard's drift journey in tests/e2e/main.rs drives
# this script through the real hook. $SHOTS_CURRENT overrides the capture root.
#
# llmlint: ignore-file[new_code_lands_in_a_project] a single binary crate with no Nx project graph (AGENTS.md) has no project for a shell script to belong to
set -euo pipefail

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

lane="$(bash "$(dirname "$0")/host-arch.sh")"
screencomp manifest --input "$current" --arch "$lane" --output "shots/baseline/${lane}.json"
