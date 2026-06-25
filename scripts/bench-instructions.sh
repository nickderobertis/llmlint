#!/usr/bin/env bash
#
# Deterministic end-to-end CLI cost via instruction counts (valgrind's
# cachegrind, no cache simulation). Wall-clock timings (scripts/bench.sh) are
# noisy on shared hardware, so a small regression hides inside the jitter;
# instruction counts are reproducible to within ~0.1% (ASLR and environment
# size leave a little), which makes a base-vs-PR delta trustworthy where a
# hyperfine delta is not. Linux-only: it needs valgrind on PATH.
#
# Counts come from the `profiling` Cargo profile — codegen-matched to the
# shipped release profile, with symbols kept so a regression can be dug into
# with callgrind/cachegrind annotation tools afterwards. The mock-oneharness
# fixture (feature-gated) stands in for the real harness, exactly as in
# scripts/bench.sh; cachegrind does not trace child processes, so a `lint` count
# is llmlint's own instructions plus the spawn, never the mock's.
#
# Usage:
#   scripts/bench-instructions.sh                   Run the suite.
#   scripts/bench-instructions.sh report BASE HEAD  Print a markdown delta table
#                                                   from two instructions.tsv files.
#
# Results: markdown table on stdout plus machine-readable exports under
# ${BENCH_OUT:-target/bench} (instructions.tsv, instructions.md).
#
# Environment overrides:
#   BENCH_OUT   output directory (default: <repo>/target/bench)

set -euo pipefail

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# `report` joins a base and a head TSV (case<TAB>instructions) into a markdown
# delta table; it needs no valgrind, so CI can run it after checking back out of
# the base revision.
if [[ "${1:-}" == "report" ]]; then
    [[ $# -eq 3 && -s "$2" && -s "$3" ]] ||
        fail "usage: bench-instructions.sh report BASE.tsv HEAD.tsv (both non-empty)"
    awk -F'\t' '
        NR == FNR { base[$1] = $2; next }
        FNR == 1 {
            print "| command | base | head | Δ instructions |"
            print "|---|---:|---:|---:|"
        }
        {
            if ($1 in base && base[$1] > 0) {
                delta = ($2 - base[$1]) / base[$1] * 100
                printf "| %s | %s | %s | %+.2f%% |\n", $1, base[$1], $2, delta
            } else {
                printf "| %s | — | %s | new |\n", $1, $2
            }
        }
    ' "$2" "$3"
    exit 0
fi

[[ "${1:-}" == "" ]] || fail "usage: bench-instructions.sh [report BASE.tsv HEAD.tsv]"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$repo_root/target/profiling/llmlint"
mock="$repo_root/target/profiling/llmlint-mock-oneharness"
out="${BENCH_OUT:-$repo_root/target/bench}"

note() { printf '%s\n' "$*"; }

command -v valgrind >/dev/null 2>&1 ||
    fail "valgrind not found on PATH (Linux-only; install it with your package manager, e.g. 'apt-get install valgrind')."

note "» building binary + mock-oneharness fixture (profiling profile)"
(cd "$repo_root" && cargo build --profile profiling --locked --quiet --features mock-oneharness)
[ -x "$bin" ] || fail "profiling binary not found at $bin"
[ -x "$mock" ] || fail "profiling mock-oneharness fixture not found at $mock"

# Hermetic config sandbox, mirroring scripts/bench.sh: a project with its own
# config + source so counts are reproducible and the host config never leaks in.
sandbox="$(mktemp -d)"
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
  - name: layered_architecture
    description: "true when domain logic stays free of I/O; false otherwise."
YAML
printf '// sample source for the benchmark sandbox\npub fn answer() -> u32 { 42 }\n' \
    >"$proj/src/lib.rs"

pass_verdicts="$sandbox/pass.json"
fail_verdicts="$sandbox/fail.json"
printf '{}\n' >"$pass_verdicts"
printf '{"no_unwrap_in_library": {"holds": false, "violations": [{"file": "src/lib.rs", "line": 1, "message": "unwrap used"}]}}\n' \
    >"$fail_verdicts"

export LLMLINT_ONEHARNESS_BIN="$mock"
export HOME="$sandbox"

mkdir -p "$out"
tsv="$out/instructions.tsv"
md="$out/instructions.md"
: >"$tsv"

# Run one case under cachegrind and append its instruction count to the TSV. The
# first argument names the case; the rest is the command. Non-zero exits (the
# fail case exits 1 by design) still produce a count, so they are tolerated.
measure() {
    local name="$1"
    shift
    local log="$sandbox/cachegrind.log"
    set +e
    valgrind --tool=cachegrind --cache-sim=no \
        --cachegrind-out-file="$sandbox/cachegrind.out" \
        --log-file="$log" -- "$@" >/dev/null
    set -e
    local refs
    refs="$(awk '/I +refs:/ { gsub(",", "", $4); print $4; exit }' "$log")"
    [ -n "$refs" ] || fail "no instruction count for '$name' (see $log)"
    printf '%s\t%s\n' "$name" "$refs" >>"$tsv"
    note "  $name: $refs instructions"
}

note "» counting instructions ($bin)"
measure "version" "$bin" --version
measure "doctor" "$bin" doctor
measure "config" "$bin" config --cwd "$proj"
LLMLINT_MOCK_VERDICTS="$pass_verdicts" measure "lint:pass" "$bin" lint --cwd "$proj"
LLMLINT_MOCK_VERDICTS="$fail_verdicts" measure "lint:fail" "$bin" lint --cwd "$proj"
LLMLINT_MOCK_VERDICTS="$pass_verdicts" measure "lint:json" "$bin" lint --format json --cwd "$proj"

{
    echo "| command | instructions |"
    echo "|---|---:|"
    awk -F'\t' '{ printf "| %s | %s |\n", $1, $2 }' "$tsv"
} >"$md"

note ""
note "✓ wrote $tsv"
note "       $md"
