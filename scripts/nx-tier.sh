#!/usr/bin/env bash
# Run Nx targets at a gate tier — the one implementation behind the justfile's
# gate recipes (check, test, lint, format, doc), so the tier is a flag on the
# same command rather than a second gate:
#
#   (no flag)  the AFFECTED tier: `nx affected` keyed off the explicit base
#              scripts/nx-base.sh prints (NX_BASE, validated, else the merge base
#              with origin/main) — what development, review and CI's pull-request
#              and merge-to-main runs pay for;
#   --all      the FULL SWEEP: `nx run-many --all` over every project — what the
#              release PR runs (AGENTS.md, "Commits, releases, and merging").
#
# Usage: scripts/nx-tier.sh [--all] -- <nx target arguments...>
#   e.g. scripts/nx-tier.sh -- -t format lint build test doc coverage
#
# A flag other than --all is refused before anything runs, as is a missing `--`:
# a mistyped tier must never quietly buy a weaker one.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "nx-tier: $*" >&2
  exit 2
}

tier=affected
while [ $# -gt 0 ]; do
  case "$1" in
    --all) tier=all ;;
    --) shift; break ;;
    *) fail "unknown flag '$1' — pass --all for the full sweep, or nothing for the affected tier." ;;
  esac
  shift
done
[ $# -gt 0 ] || fail "no Nx target arguments after '--' (e.g. -- -t lint); this is the justfile's helper, run it through a gate recipe."

if [ "$tier" = all ]; then
  exec bash scripts/nx run-many --all "$@"
fi
base="$(bash scripts/nx-base.sh)" || exit 1
exec bash scripts/nx affected --base="$base" "$@"
