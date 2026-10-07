#!/usr/bin/env bash
# The line-coverage gate, in three steps over cargo-llvm-cov's one profile
# directory (target/llvm-cov-target):
#
#   coverage.sh clear          drop every raw profile and instrumented artifact
#   coverage.sh test <crate>   run one crate's tests instrumented, keep the profiles
#   coverage.sh report         merge every crate's profiles; fail below the floor
#
# Each coverage-measured project's `test` target is step two,
# `coverage:coverage-clear` is step one and `coverage:coverage` is step three, so
# the floor is enforced once over the union of every crate's run: the e2e crate's
# journeys count toward the lines of the `llmlint` crate whose binary they drive.
# `--no-report` is what lets the crates share the directory: a reporting run
# clears every profile in it first.
#
# The floor covers the `llmlint` crate's own sources (src/) — the code the
# single-crate gate measured before the split. Every other workspace member is a
# test, fixture, bench or tooling crate whose own lines are not product code, so
# the report ignores their directories.
#
# LLMLINT_COVERAGE=off (the macOS/Windows `cross` jobs) runs `test` with plain
# nextest, uninstrumented, and makes `report` a no-op with a notice: those jobs
# prove the tests pass on the platform, and the floor is enforced on Linux.
#
# Exit status: 0 success; 1 a test run, the report, or the floor failed; 2 a
# usage error (unknown step, wrong argument count, a crate that is not a
# workspace member, or an unknown LLMLINT_COVERAGE value).
set -euo pipefail

readonly MIN_LINES=95
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
  || { echo "coverage: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout whose directories you can read and enter." >&2; exit 1; }
readonly ROOT
cd "$ROOT" || { echo "coverage: cannot enter $ROOT; run it from an intact checkout whose directories you can read and enter." >&2; exit 1; }

usage() {
  echo "coverage: $1" >&2
  echo "usage: tools/coverage/coverage.sh clear | test <crate> | report" >&2
  exit 2
}

[ "$#" -ge 1 ] || usage "no step given"
readonly STEP="$1"
case "$STEP" in
  clear | report) [ "$#" -eq 1 ] || usage "'$STEP' takes no arguments (got $(($# - 1)))" ;;
  test) [ "$#" -eq 2 ] || usage "'test' takes exactly one crate name (got $(($# - 1)) arguments)" ;;
  *) usage "unknown step '$STEP'" ;;
esac

case "${LLMLINT_COVERAGE:-on}" in
  on) instrumented=true ;;
  off) instrumented=false ;;
  *) usage "LLMLINT_COVERAGE must be 'on' (the default) or 'off' (got '${LLMLINT_COVERAGE}')" ;;
esac

require() {
  local out
  if ! out="$(cargo "$1" --version 2>&1)"; then
    printf '%s\n' "$out" >&2
    echo "coverage: 'cargo $1 --version' failed (above) — cargo-$1 is missing or broken; run 'just setup' (or 'cargo install cargo-$1 --locked')." >&2
    exit 1
  fi
}

# The crate selector must name a real workspace member: unchecked, a typo would
# measure nothing and pass. Only plain package names are accepted, checked
# against the members `cargo metadata` lists.
validate_crate() {
  local crate="$1" metadata members
  if ! printf '%s' "$crate" | grep -Eq '^[a-z0-9][a-z0-9-]*$'; then
    echo "coverage: '$crate' is not a valid crate name; pass a workspace package name (lowercase letters, digits, -)." >&2
    exit 2
  fi
  if ! metadata="$(cargo metadata --format-version 1 --no-deps --locked 2>&1)"; then
    printf '%s\n' "$metadata" >&2
    echo "coverage: 'cargo metadata' failed (above); fix the manifests so it resolves, then re-run." >&2
    exit 1
  fi
  # With --no-deps the packages are exactly the workspace members.
  members="$(printf '%s' "$metadata" | grep -o '"name":"[a-z0-9-]*","version"' | cut -d'"' -f4 | sort -u)"
  if ! printf '%s\n' "$members" | grep -qxF -- "$crate"; then
    echo "coverage: '$crate' is not a member of this Cargo workspace; pass one of: $(printf '%s\n' "$members" | tr '\n' ' ')" >&2
    exit 2
  fi
}

case "$STEP" in
  clear)
    "$instrumented" || exit 0
    require llvm-cov
    if ! out="$(cargo llvm-cov clean --workspace 2>&1)"; then
      printf '%s\n' "$out" >&2
      echo "coverage: could not clear target/llvm-cov-target; fix the error above and re-run." >&2
      exit 1
    fi
    ;;

  test)
    readonly CRATE="$2"
    validate_crate "$CRATE"
    require nextest
    if ! "$instrumented"; then
      exec cargo nextest run -p "$CRATE" --locked
    fi
    require llvm-cov
    # The e2e journeys find `llmlint` and the mock fixture beside their own test
    # executable, i.e. in target/llvm-cov-target under coverage; build the
    # instrumented copies there first (the `build` targets the e2e `test`
    # dependsOn produce the uninstrumented ones in target/debug). A no-op when
    # they are fresh. Each one `--version` run adds a profile covering only the
    # argument parsing every journey executes anyway.
    if [ "$CRATE" = "llmlint-e2e" ]; then
      for bin in llmlint llmlint-mock-oneharness; do
        if ! out="$(cargo llvm-cov --no-report run -p "$bin" --bin "$bin" --locked -- --version 2>&1)"; then
          printf '%s\n' "$out" >&2
          echo "coverage: building the instrumented $bin binary failed; fix the error above." >&2
          exit 1
        fi
      done
    fi
    exec cargo llvm-cov --no-report nextest -p "$CRATE" --locked
    ;;

  report)
    if ! "$instrumented"; then
      echo "coverage: LLMLINT_COVERAGE=off — report skipped; the Linux gate enforces ${MIN_LINES}%." >&2
      exit 0
    fi
    require llvm-cov
    # Every workspace member other than the `llmlint` crate lives under one of
    # these directories; they are the only paths left out of the report.
    ignore="^$(printf '%s' "$ROOT" | sed 's/[][\.*^$+?(){}|]/\\&/g')/(tests|benches|scripts|screenshots|\.github|tools)/"
    if ! out="$(cargo llvm-cov report --ignore-filename-regex "$ignore" --fail-under-lines "$MIN_LINES" 2>&1)"; then
      printf '%s\n' "$out" >&2
      if printf '%s\n' "$out" | grep -q '^TOTAL '; then
        echo "coverage: the llmlint crate is below ${MIN_LINES}% line coverage over every crate's run." >&2
        echo "coverage: the per-file table is above — cover the missed lines with a test that drives the real behaviour." >&2
      else
        echo "coverage: no report could be produced (reason above). Run the tests first: 'just check' or 'just coverage'." >&2
      fi
      exit 1
    fi
    if ! total="$(printf '%s\n' "$out" | grep '^TOTAL ')"; then
      printf '%s\n' "$out" >&2
      echo "coverage: the report passed but has no TOTAL row (format above); check the cargo-llvm-cov version against tools/coverage/coverage.sh." >&2
      exit 1
    fi
    printf '%s\n' "$total" | awk -v min="$MIN_LINES" '{ print "coverage: " $(NF-3) " lines covered (floor " min "%)" }' >&2
    ;;
esac
