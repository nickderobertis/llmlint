#!/usr/bin/env bash
#
# Sampling profiler for finding bottlenecks, built on samply (records a trace
# you open in the Firefox Profiler UI), with a deterministic callgrind fallback
# for containers and CI where perf-event access is withheld.
#
# All modes build the dedicated `profiling` profile (Cargo.toml): the shipped
# release optimizations, but with symbols kept so the profiler can attribute
# time/instructions to functions. The real `[profile.release]` artifact stays
# stripped.
#
# Usage:
#   scripts/profile.sh                      Profile the whole engine hot path.
#   scripts/profile.sh engine [FILTER]      Profile one or more Criterion
#                                           benchmarks (e.g. schema_build).
#   scripts/profile.sh cli lint             Profile a real CLI invocation
#   scripts/profile.sh cli config           (startup + config + glob + plan +
#                                           render + schema + spawn + vote),
#                                           looped so the short process yields
#                                           enough samples.
#   scripts/profile.sh callgrind lint       Deterministic per-function
#                                           attribution of ONE CLI invocation
#                                           (valgrind callgrind; Linux-only).
#
# A single CLI run is far too short to sample, which is why the engine mode
# (Criterion's `--profile-time`, a long-running in-process loop) is the right
# tool for optimizing the engine, and the CLI mode loops the binary. The
# callgrind mode runs ONE invocation (no looping — counts are exact, not
# sampled), writes the raw output under target/profile/, and prints the top
# functions by instruction count; it attributes the same totals
# `just bench-instructions` reports, so a regression found there can be dug into
# here.
#
# The cli/callgrind modes set up the same hermetic sandbox the bench scripts use
# (a project config + source + the mock-oneharness fixture wired via
# LLMLINT_ONEHARNESS_BIN), then run `llmlint <your args>` inside it — so
# `cli lint` and `callgrind lint` work without any model or network.
#
# Environment overrides:
#   PROFILE_SECONDS   engine mode: seconds to sample (default: 10)
#   PROFILE_REPEAT    cli mode: invocations to loop under the profiler (default: 3000)
#   PROFILE_TOP       callgrind mode: function rows to print (default: 30)
#   SAMPLY_ARGS       extra args passed to `samply record` (e.g. --save-only)

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
seconds="${PROFILE_SECONDS:-10}"
repeat="${PROFILE_REPEAT:-3000}"
# shellcheck disable=SC2206  # intentional word-splitting of optional flags.
samply_args=(${SAMPLY_ARGS:-})

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# Build the profiling binary + mock fixture and set up a hermetic sandbox that
# the cli/callgrind modes run `llmlint` inside. Exports `bin`, `proj`, and the
# harness env; registers a cleanup trap.
setup_cli_sandbox() {
    bin="$repo_root/target/profiling/llmlint"
    local mock="$repo_root/target/profiling/llmlint-mock-oneharness"
    echo "» building binary + mock-oneharness fixture (profiling profile)"
    (cd "$repo_root" && cargo build --profile profiling --locked --quiet --features mock-oneharness)
    [ -x "$bin" ] || fail "profiling binary not found at $bin"
    [ -x "$mock" ] || fail "profiling mock-oneharness fixture not found at $mock"

    sandbox="$(mktemp -d)"
    # shellcheck disable=SC2317  # invoked via the EXIT trap.
    cleanup() { rm -rf "$sandbox"; }
    trap cleanup EXIT

    proj="$sandbox/project"
    mkdir -p "$proj/src"
    cat >"$proj/llmlint.yml" <<'YAML'
version: 1
files:
  include: ["src/**"]
agents:
  default:
    harness: claude-code
rules:
  - name: public_items_are_documented
    description: "true when every public item has a doc comment; false otherwise."
  - name: no_unwrap_in_library
    description: "true when no library code calls unwrap/expect; false otherwise."
YAML
    printf '// sample source\npub fn answer() -> u32 { 42 }\n' >"$proj/src/lib.rs"
    printf '{}\n' >"$sandbox/verdicts.json"
    export LLMLINT_ONEHARNESS_BIN="$mock"
    export LLMLINT_MOCK_VERDICTS="$sandbox/verdicts.json"
    export HOME="$sandbox"
}

mode="${1:-engine}"

# Deterministic per-function attribution of a single CLI invocation. No samply
# (and no perf-event access) needed, so it works in containers and CI.
if [[ "$mode" == "callgrind" ]]; then
    shift
    [[ $# -ge 1 ]] || fail "usage: profile.sh callgrind <llmlint args…> (e.g. callgrind lint)"
    command -v valgrind >/dev/null 2>&1 ||
        fail "valgrind not found on PATH (Linux-only; install it with your package manager)."
    command -v callgrind_annotate >/dev/null 2>&1 ||
        fail "callgrind_annotate not found on PATH (ships with valgrind)."
    setup_cli_sandbox
    outdir="$repo_root/target/profile"
    mkdir -p "$outdir"
    cg_out="$outdir/callgrind.out"
    echo "» running '$bin $* --cwd $proj' under callgrind"
    # Non-zero exits are tolerated: a fail verdict exits 1 by design and the
    # profile of that path is exactly what was asked for.
    valgrind --tool=callgrind --callgrind-out-file="$cg_out" -- \
        "$bin" "$@" --cwd "$proj" >/dev/null || true
    echo
    echo "» top ${PROFILE_TOP:-30} functions by instruction count (full data: $cg_out)"
    callgrind_annotate --threshold=99 "$cg_out" | head -n "$((${PROFILE_TOP:-30} + 12))"
    exit 0
fi

command -v samply >/dev/null 2>&1 ||
    fail "samply not found on PATH. Install it with 'just bench-tools' (or 'cargo install --locked samply')."

if [[ "$mode" == "engine" ]]; then
    shift || true
    filter="${1:-}"
    echo "» building bench (profiling profile)"
    # Build the bench with symbols, then read its executable path from cargo's
    # JSON output (no jq dependency).
    artifact="$(cargo build --profile profiling --bench engine --locked --message-format=json -q |
        grep -F '"name":"engine"' | grep -F '"executable":' | tail -1)"
    bench_exe="$(printf '%s' "$artifact" | grep -o '"executable":"[^"]*"' | cut -d'"' -f4)"
    [ -n "$bench_exe" ] && [ -x "$bench_exe" ] || fail "could not locate the profiling bench executable"
    echo "» profiling engine for ${seconds}s (${filter:-all benchmarks})"
    # `--profile-time` makes Criterion run the bench in a plain loop with no
    # statistical analysis — exactly what an external sampler wants.
    samply record "${samply_args[@]}" -- \
        "$bench_exe" --bench --profile-time "$seconds" ${filter:+"$filter"}
    exit 0
fi

if [[ "$mode" == "cli" ]]; then
    shift
    [[ $# -ge 1 ]] || fail "usage: profile.sh cli <llmlint args…> (e.g. cli lint)"
    setup_cli_sandbox
    echo "» profiling '$bin $* --cwd $proj' over $repeat invocations"
    samply record "${samply_args[@]}" -- \
        bash -c 'n="$1"; shift; proj="$1"; shift; for ((i = 0; i < n; i++)); do "$@" --cwd "$proj" >/dev/null 2>&1 || true; done' \
        _ "$repeat" "$proj" "$bin" "$@"
    exit 0
fi

fail "unknown mode '$mode' (expected: engine | cli | callgrind)"
