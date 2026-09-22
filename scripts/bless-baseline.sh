#!/usr/bin/env bash
# Refresh THIS host's committed screencomp baseline from the capture in
# shots/current — the one place that decides which lane gets rewritten and where.
#
# Shared by `just screenshots-bless` (an intended output change) and the pre-push
# guard's drift path (.githooks/pre-push), so the two can never disagree about the
# lane or the manifest path; the guard's drift journey in tests/e2e/main.rs drives
# this script through the real hook. $SHOTS_CURRENT overrides the capture root.
#
# llmlint: ignore-file[new_code_lands_in_a_project] llmlint is deliberately a single binary crate with no monorepo and no Nx project graph (AGENTS.md, "Stack and composition"), so there is no project definition for a shell helper to land in; its owning surface is the justfile recipe + the pre-push guard that call it, and pre_push_guard_blocks_on_drift_and_refreshes_the_lane_baseline drives it end to end
set -euo pipefail

if ! command -v screencomp >/dev/null 2>&1; then
  echo "bless-baseline: screencomp is not installed, so the baseline cannot be" >&2
  echo "                refreshed. Install it and retry:" >&2
  echo "                https://github.com/nickderobertis/screencomp#install" >&2
  exit 1
fi

lane="$(bash "$(dirname "$0")/host-arch.sh")"
screencomp manifest \
  --input "${SHOTS_CURRENT:-shots/current}" \
  --arch "$lane" \
  --output "shots/baseline/${lane}.json"
