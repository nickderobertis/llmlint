#!/usr/bin/env bash
# Choose the gate tier for the justfile's gate recipes (check, test, lint,
# lint-sh, lint-workflows, fmt-check, format, doc), so the tier is a flag on the
# same command rather than a second gate. Prints one line:
#
#   all        under --all: the FULL SWEEP — the recipe runs `nx run-many --all`
#              over every project (what the release PR runs; AGENTS.md,
#              "Commits, releases, and merging");
#   <base>     with no flag: the AFFECTED tier — the explicit base the recipe
#              hands to `nx affected --base=`, from scripts/nx-base.sh (NX_BASE,
#              validated, else the merge base with origin/main).
#
# Usage: scripts/nx-tier.sh [--all]
# Exit status: 0 with a tier; 1 when the base cannot be derived (nx-base.sh says
# why); 2 on any other flag — a mistyped tier never quietly buys a weaker one.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || { echo "nx-tier: cannot enter the repository root; run it from an intact checkout whose directories you can read and enter." >&2; exit 1; }

if [ "$#" -eq 0 ]; then
  bash scripts/nx-base.sh
elif [ "$#" -eq 1 ] && [ "$1" = --all ]; then
  echo all
else
  echo "nx-tier: unknown flag(s) '$*' — pass --all for the full sweep, or nothing for the affected tier." >&2
  exit 2
fi
