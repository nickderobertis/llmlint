#!/usr/bin/env bash
# The shell line-coverage gate, measured with bashcov (pinned in the root
# Gemfile.lock) over the scripts that `tools/shell/shell.sh files` finds:
#
#   shcov.sh install                    install the locked bashcov (just bootstrap)
#   shcov.sh run <project> -- CMD...    run a project's test command, recording
#                                       target/shcov/<project>.json
#   shcov.sh report <project>...        merge those projects' records; fail below
#                                       the floor
#
# Each shell-measured project's `test` target (tag coverage:shell) is `run` around
# its own test command, and the shell-coverage project's `coverage` target is
# `report` over every one of them, so the floor is enforced once on the merged
# result: a journey in one project counts toward the script of another it drives.
# How a deep child process's trace is captured is in shcov.rb.
#
# LLMLINT_COVERAGE=off (the macOS/Windows `cross` jobs) runs CMD unmeasured and
# makes `install` and `report` a no-op with a notice, as tools/coverage/coverage.sh
# does: the floor is enforced on Linux.
#
# Exit status: CMD's own for `run`; 0 success; 1 the floor, a missing record or a
# missing tool; 2 a usage error.
set -euo pipefail

# The floor (whole percent of the shell lines bashcov counts as relevant), and
# why it is not 95, are in AGENTS.md ("Shell").
readonly MIN_LINES=50
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
  || {
    echo "shcov: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout." >&2
    exit 1
  }
readonly ROOT
export BUNDLE_GEMFILE="$ROOT/Gemfile"

usage() {
  echo "shcov: $1" >&2
  echo "usage: tools/coverage/shcov.sh install | run <project> -- CMD... | report <project>..." >&2
  exit 2
}

# The whole argument, not any one line of it: a name becomes a record path.
valid_project() {
  [[ $1 =~ ^[a-z0-9][a-z0-9-]*$ ]] \
    || usage "'$1' is not a project name (lowercase letters, digits, -)"
}

case "${LLMLINT_COVERAGE:-on}" in
  on) measured=true ;;
  off) measured=false ;;
  *) usage "LLMLINT_COVERAGE must be 'on' (the default) or 'off' (got '${LLMLINT_COVERAGE}')" ;;
esac

# Ruby and Bundler are host prerequisites (the .tool-versions ruby pin); the gems
# are this repository's, installed by `install`.
require_ruby() {
  local tool
  for tool in ruby bundle; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "shcov: $tool not found on PATH; install Ruby (the .tool-versions pin, with Bundler), then run: just bootstrap" >&2
      exit 1
    fi
  done
}

[ "$#" -ge 1 ] || usage "no step given"
readonly STEP="$1"
shift

case "$STEP" in
  install)
    [ "$#" -eq 0 ] || usage "'install' takes no arguments"
    if ! "$measured"; then
      echo "shcov: LLMLINT_COVERAGE=off — bashcov not installed; the Linux gate measures shell coverage." >&2
      exit 0
    fi
    require_ruby
    if ! out="$(cd "$ROOT" && bundle install 2>&1)"; then
      printf '%s\n' "$out" >&2
      echo "shcov: bundle install failed (above); fix it and re-run: just bootstrap" >&2
      exit 1
    fi
    ;;

  run)
    [ "$#" -ge 3 ] && [ "$2" = "--" ] || usage "'run' takes a project, then -- and the command to measure"
    valid_project "$1"
    project="$1"
    shift 2
    if ! "$measured"; then
      exec "$@"
    fi
    require_ruby
    exec bundle exec ruby "$ROOT/tools/coverage/shcov.rb" run "$project" "target/shcov/$project.json" -- "$@"
    ;;

  report)
    [ "$#" -ge 1 ] || usage "'report' takes the projects whose records it merges"
    if ! "$measured"; then
      echo "shcov: LLMLINT_COVERAGE=off — report skipped; the Linux gate enforces ${MIN_LINES}%." >&2
      exit 0
    fi
    records=()
    for project in "$@"; do
      valid_project "$project"
      records+=("target/shcov/$project.json")
    done
    require_ruby
    cd "$ROOT" || {
      echo "shcov: cannot enter $ROOT; run it from an intact checkout whose directories you can read and enter." >&2
      exit 1
    }
    exec bundle exec ruby "$ROOT/tools/coverage/shcov.rb" report "$MIN_LINES" "${records[@]}"
    ;;

  *) usage "unknown step '$STEP'" ;;
esac
