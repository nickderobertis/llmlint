#!/usr/bin/env bash
# Print the base commit the AFFECTED gate tier keys off — always explicit, never
# Nx's implicit default. The justfile's gate recipes reach this through
# scripts/nx-tier.sh and hand the result to `nx affected --base=...`.
#
#   * NX_BASE when set (CI exports the base .github/scripts/ci-gate.sh derived).
#     Only a plain ref name or a commit SHA is accepted — letters, digits and
#     `. _ / -`, not starting with `-` and without `..` — and it must resolve to
#     a commit; any other value is refused here, before a single target runs.
#   * otherwise `git merge-base origin/main HEAD`, the fork point from the
#     default branch (fetch origin/main first in a clone that lacks it).
#
# Prints the base on stdout and one line saying where it came from on stderr.
# Exit status: 0 with a base; 1 when NX_BASE is refused or no base can be derived.
set -euo pipefail

fail() {
  echo "nx-base: $*" >&2
  exit 1
}

if [ -n "${NX_BASE+set}" ]; then
  case "$NX_BASE" in
    "" | -* | *..*) fail "NX_BASE must be a plain git ref name or commit SHA (got '$NX_BASE'); set it to e.g. origin/main or a SHA, or unset it to use the merge base with origin/main." ;;
  esac
  printf '%s' "$NX_BASE" | grep -Eq '^[A-Za-z0-9._/-]+$' \
    || fail "NX_BASE must be a plain git ref name or commit SHA — letters, digits and . _ / - only (got '$NX_BASE'); set it to e.g. origin/main or a SHA, or unset it."
  git rev-parse --verify --quiet "$NX_BASE^{commit}" >/dev/null \
    || fail "NX_BASE '$NX_BASE' does not resolve to a commit in this clone; fetch it or unset NX_BASE."
  echo "nx-base: affected since $NX_BASE (NX_BASE)" >&2
  printf '%s\n' "$NX_BASE"
else
  git rev-parse --verify --quiet "origin/main^{commit}" >/dev/null \
    || fail "no origin/main in this clone to derive the merge base from; run 'git fetch origin main' or set NX_BASE."
  base="$(git merge-base origin/main HEAD)" \
    || fail "HEAD shares no history with origin/main; set NX_BASE to the commit to compare against."
  echo "nx-base: affected since $base (merge-base with origin/main)" >&2
  printf '%s\n' "$base"
fi
