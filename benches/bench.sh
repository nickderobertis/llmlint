#!/usr/bin/env bash
#
# End-to-end CLI latency benchmark. Drives the optimized release binary the way
# a developer or CI invokes it — one process per command — and measures
# wall-clock time with hyperfine across every verb. This captures the cost that
# matters in practice: process startup + config discovery/merge (fs) + file
# globbing + plan/render/schema + spawning oneharness + verdict parse + vote +
# report, which the in-process Criterion benches (`benches/engine.rs`)
# deliberately exclude.
#
# `oneharness` is the genuinely-external boundary (network + a real model), so —
# exactly as the e2e suite does — this points `llmlint` at the deterministic
# `llmlint-mock-oneharness` fixture via `--oneharness-bin`. The numbers are
# therefore reproducible and measure llmlint's own work plus one child-process
# spawn, not model latency. The mock is its own workspace crate, so a release
# build of `-p llmlint -p llmlint-mock-oneharness` produces both the real
# `llmlint` binary (release profile, no mock code in it) and the mock alongside it.
#
# Usage:
#   benches/bench.sh            Full run (warmup + adaptive sampling).
#   benches/bench.sh --dry-run  One run, no warmup — a fast smoke check that the
#                               harness and every command still work (used by CI
#                               and `just`), without depending on stable numbers.
#
# Results: human table on stdout plus machine-readable exports under
# ${BENCH_OUT:-target/bench} (results.json, results.md).
#
# Environment overrides:
#   BENCH_OUT     output directory (default: <repo>/target/bench)
#   BENCH_WARMUP  warmup runs before timing (default: 10)
#   BENCH_KEEP    set to 1 to keep the temp sandbox for inspection

# llmlint: ignore-file[robust_shell, tool_output_is_signal] an interactive, informational performance harness (benches/AGENTS.md: it measures, never gates): its stage-by-stage progress is what a person running it watches, and it measures commands that exit non-zero by design (a failing lint verdict), so their statuses are deliberately not required
set -euo pipefail

# Measure llmlint's own work, not disk logging: results logging is on by default
# and would write a record per invocation (perturbing timings + touching the real
# data dir). Off for a clean, side-effect-free benchmark.
export LLMLINT_NO_HISTORY=1

mode="${1:-run}"
case "$mode" in
    run | --dry-run) ;;
    *)
        echo "usage: bench.sh [--dry-run]" >&2
        exit 2
        ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$repo_root/target/release/llmlint"
mock="$repo_root/target/release/llmlint-mock-oneharness"
out="${BENCH_OUT:-$repo_root/target/bench}"
warmup="${BENCH_WARMUP:-10}"

note() { printf '%s\n' "$*"; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# Validate the overrides before anything is built or run: the warmup count
# reaches hyperfine's numeric flag, BENCH_KEEP decides whether the sandbox is
# deleted, and BENCH_OUT is where the exports are written — so a bad value fails
# here, naming its variable, rather than deep inside hyperfine or mkdir.
[[ "$warmup" =~ ^(0|[1-9][0-9]{0,3})$ ]] ||
    fail "BENCH_WARMUP must be a whole number of warmup runs from 0 to 9999 (got '$warmup'); unset it to use the default (10)."
case "${BENCH_KEEP:-0}" in
    0 | 1) ;;
    *) fail "BENCH_KEEP must be 1 (keep the sandbox) or 0 (got '${BENCH_KEEP}'); unset it to delete the sandbox." ;;
esac
case "$out" in
    -*) fail "BENCH_OUT must be a directory path, not an option (got '$out'); unset it to use $repo_root/target/bench." ;;
esac
if [ -e "$out" ] && [ ! -d "$out" ]; then
    fail "BENCH_OUT must name a directory (got '$out', which exists and is not one); unset it to use $repo_root/target/bench."
fi

if ! command -v hyperfine >/dev/null 2>&1; then
    fail "hyperfine not found on PATH. Install it with 'just bench-tools' (or 'cargo binstall hyperfine')."
fi

# A `--dry-run` proves the harness and commands work without spending time on
# statistics; the full run warms up and lets hyperfine sample adaptively.
runs_opt=()
if [[ "$mode" == "--dry-run" ]]; then
    warmup=0
    runs_opt=(--runs 1)
fi

note "» building release binary + mock-oneharness fixture"
(cd "$repo_root" && cargo build --release --locked --quiet -p llmlint -p llmlint-mock-oneharness)
[ -x "$bin" ] || fail "release binary not found at $bin"
[ -x "$mock" ] || fail "mock-oneharness fixture not found at $mock"

# Hermetic config sandbox, mirroring tests/e2e: a project with its own
# `llmlint.yml` and source files, so config discovery + globbing + the rendered
# verdicts are reproducible and the host machine's own config never leaks in.
sandbox="$(mktemp -d)"
cleanup() { [ "${BENCH_KEEP:-0}" = "1" ] || rm -rf "$sandbox"; }
trap cleanup EXIT

proj="$sandbox/project"
initdir="$sandbox/initdir"
mkdir -p "$proj/src" "$initdir"

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
  - name: layered_architecture
    description: "true when domain logic stays free of I/O; false otherwise."
YAML
printf '// sample source for the benchmark sandbox\npub fn answer() -> u32 { 42 }\n' \
    >"$proj/src/lib.rs"

# Two verdict fixtures: an all-pass map (unlisted rules default to holds=true in
# the mock) and one that forces a single failure so the `lint:fail` row exercises
# the violation-formatting path (exit 1, wrapped with `|| true`).
pass_verdicts="$sandbox/pass.json"
fail_verdicts="$sandbox/fail.json"
printf '{}\n' >"$pass_verdicts"
printf '{"no_unwrap_in_library": {"holds": false, "violations": [{"file": "src/lib.rs", "line": 1, "message": "unwrap used"}]}}\n' \
    >"$fail_verdicts"

# Point every spawned process at the mock harness (covers `lint`'s flag/env
# resolution and `doctor`, which reads only the env var) and keep config
# discovery off the host.
export LLMLINT_ONEHARNESS_BIN="$mock"
export HOME="$sandbox"

mkdir -p "$out"

note "» benchmarking $bin"
# One invocation so a single export holds every command. `--prepare` clears the
# `init` target before each run (init refuses to overwrite, and the create path
# is what should be measured each time); harmless for the read-only commands.
# `lint:fail` exits 1 by design, so it is wrapped with `|| true`.
hyperfine \
    --warmup "$warmup" ${runs_opt[@]+"${runs_opt[@]}"} \
    --prepare "rm -f '$initdir/llmlint.yml'" \
    --export-json "$out/results.json" \
    --export-markdown "$out/results.md" \
    -n "version" "'$bin' --version" \
    -n "help" "'$bin' --help" \
    -n "doctor" "'$bin' doctor" \
    -n "config" "'$bin' config --cwd '$proj' > /dev/null" \
    -n "init" "cd '$initdir' && '$bin' init > /dev/null" \
    -n "lint:pass" "LLMLINT_MOCK_VERDICTS='$pass_verdicts' '$bin' lint --cwd '$proj' > /dev/null" \
    -n "lint:fail" "LLMLINT_MOCK_VERDICTS='$fail_verdicts' '$bin' lint --cwd '$proj' > /dev/null || true" \
    -n "lint:json" "LLMLINT_MOCK_VERDICTS='$pass_verdicts' '$bin' lint --format json --cwd '$proj' > /dev/null"

note ""
note "✓ wrote $out/results.json"
note "       $out/results.md"
