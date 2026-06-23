# Shared helpers for the llmlint LIVE end-to-end tier.
#
# These drive the REAL `llmlint` binary against the REAL `oneharness` and a REAL,
# authenticated coding harness — the whole stack, no mocks. They are the live
# analogue of the hermetic e2e suite (`tests/e2e/`, which drives a mock
# oneharness), and mirror oneharness's own `scripts/e2e-*.sh` tier.
#
# Contract (same spirit as oneharness): a missing CLI or missing auth is a
# **skip**, never a failure, so the suite is safe to run anywhere. A clean run is
# *required* once the prerequisites are present — that is the whole point.
#
# Sourced by the per-harness scripts (`scripts/live-<harness>.sh`); not run on its
# own. Each per-harness script declares its harness id, the CLI it needs, the auth
# env vars it accepts, and an optional `<HARNESS>_E2E_MODEL` override, then calls
# `live_run_journeys <harness-id>`.

# Repo root = the parent of this script's directory.
LL_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- logging -----------------------------------------------------------------

note() { printf '%s\n' "$*" >&2; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# A missing prerequisite exits cleanly (status 0): an environment without the
# harness installed/authed has nothing to verify, it is not a regression.
skip() { printf 'SKIP: %s\n' "$*" >&2; exit 0; }

need() { command -v "$1" >/dev/null 2>&1 || skip "required tool not found: $1"; }

# Skip unless at least one of the named env vars is non-empty.
need_env() {
    local label="$1"
    shift
    local v
    for v in "$@"; do
        [ -n "${!v:-}" ] && return 0
    done
    skip "no $label configured (set one of: $*)"
}

# --- binary resolution -------------------------------------------------------

# The release build the live recipes produce, an explicit `LLMLINT_BIN`, or
# whatever is on PATH. Platform-aware `.exe` handling for Windows runners.
ll_bin() {
    local b cand
    if [ -n "${LLMLINT_BIN:-}" ]; then
        for b in "$LLMLINT_BIN" "$LLMLINT_BIN.exe"; do
            [ -x "$b" ] && { printf '%s' "$b"; return; }
        done
        printf '%s' "$LLMLINT_BIN"
        return
    fi
    for cand in "$LL_REPO_ROOT"/target/release/llmlint{,.exe} \
                "$LL_REPO_ROOT"/target/debug/llmlint{,.exe}; do
        [ -x "$cand" ] && { printf '%s' "$cand"; return; }
    done
    command -v llmlint >/dev/null 2>&1 && { printf 'llmlint'; return; }
    printf ''
}

# llmlint needs oneharness on PATH (or via LLMLINT_ONEHARNESS_BIN); without it the
# whole stack can't run, so skip rather than fail.
require_oneharness() {
    if [ -n "${LLMLINT_ONEHARNESS_BIN:-}" ]; then
        local b
        for b in "$LLMLINT_ONEHARNESS_BIN" "$LLMLINT_ONEHARNESS_BIN.exe"; do
            [ -x "$b" ] && return 0
        done
    fi
    command -v oneharness >/dev/null 2>&1 && return 0
    skip "oneharness not found (install it, or set LLMLINT_ONEHARNESS_BIN)"
}

# --- throwaway project scaffolding ------------------------------------------

LL_PROJECTS=()
_ll_cleanup() {
    local d
    for d in "${LL_PROJECTS[@]+"${LL_PROJECTS[@]}"}"; do
        [ -n "$d" ] && rm -rf "$d"
    done
}
trap _ll_cleanup EXIT

# Write a minimal real config that pins `harness` (and an optional model/timeout)
# and declares one crisp invariant. Echoes the project dir; the caller fills in
# `src/lib.rs`.
make_project() {
    local harness="$1"
    local proj
    proj="$(mktemp -d)"
    LL_PROJECTS+=("$proj")
    mkdir -p "$proj/src"
    {
        echo "version: 1"
        echo "files:"
        echo '  include: ["src/**"]'
        echo "oneharness:"
        echo "  timeout: ${LL_TIMEOUT:-120}"
        [ -n "${LL_MODEL:-}" ] && echo "  model: ${LL_MODEL}"
        echo "agents:"
        echo "  judge:"
        echo "    harness: ${harness}"
        echo "rules:"
        echo "  - name: no_todo_comments"
        echo "    description: >-"
        echo "      Every source file under src/ is free of TODO and FIXME comments."
        echo "      The property HOLDS when no source file contains a TODO or FIXME"
        echo "      marker, and is VIOLATED by any file that contains one."
        echo "    agent: judge"
    } >"$proj/llmlint.yml"
    printf '%s' "$proj"
}

# --- run + assert ------------------------------------------------------------

LL_REPORT=""
LL_STDERR=""
LL_EXIT=0

ll_run() {
    local proj="$1"
    shift
    local bin
    bin="$(ll_bin)"
    [ -n "$bin" ] || skip "llmlint binary not found (build it: \`just _live-build\`, or set LLMLINT_BIN)"
    local errf
    errf="$(mktemp)"
    note "  driving: llmlint --cwd <proj> --format json $* (timeout ${LL_TIMEOUT:-120}s${LL_MODEL:+, model $LL_MODEL})"
    set +e
    LL_REPORT="$("$bin" --cwd "$proj" --format json "$@" 2>"$errf")"
    LL_EXIT=$?
    set -e
    LL_STDERR="$(cat "$errf")"
    rm -f "$errf"
}

_ll_dump() {
    note "  --- llmlint exit: $LL_EXIT ---"
    note "  --- llmlint stdout (report) ---"
    printf '%s\n' "$LL_REPORT" | sed 's/^/    /' >&2
    if [ -n "$LL_STDERR" ]; then
        note "  --- llmlint stderr ---"
        printf '%s\n' "$LL_STDERR" | sed 's/^/    /' >&2
    fi
}

# Exit 2 means llmlint could not complete the run (a oneharness/harness/schema
# error). We only get here after `need`/`need_env` confirmed the CLI + auth, so
# this is a genuine live-stack failure worth surfacing, not a skip.
_ll_guard_completed() {
    if [ "$LL_EXIT" = 2 ]; then
        _ll_dump
        fail "llmlint could not complete the run (exit 2) — the live stack errored despite CLI + auth being present"
    fi
}

_ll_rule_outcome_is() {
    printf '%s' "$LL_REPORT" |
        jq -e --arg want "$1" \
            '.rules[] | select(.name=="no_todo_comments") | .outcome==$want' >/dev/null
}

assert_pass() {
    _ll_guard_completed
    if [ "$LL_EXIT" != 0 ]; then
        _ll_dump
        fail "expected every rule to hold (exit 0) on a clean file, but llmlint exited $LL_EXIT"
    fi
    _ll_rule_outcome_is pass || { _ll_dump; fail "rule no_todo_comments did not pass on a clean file"; }
    note "  ok: clean file judged clean (exit 0, rule passed)"
}

assert_fail() {
    _ll_guard_completed
    if [ "$LL_EXIT" != 1 ]; then
        _ll_dump
        fail "expected a violation (exit 1) on a file with a TODO, but llmlint exited $LL_EXIT"
    fi
    _ll_rule_outcome_is fail || { _ll_dump; fail "rule no_todo_comments did not flag the planted TODO"; }
    note "  ok: planted TODO flagged (exit 1, rule failed)"
}

# --- journeys ----------------------------------------------------------------

# A satisfied invariant -> exit 0. Proves the model can read a clean file through
# the harness and return holds=true, and that llmlint maps that to a pass.
ll_live_pass() {
    local harness="$1" proj
    proj="$(make_project "$harness")"
    printf '%s\n' "pub fn add(a: i32, b: i32) -> i32 {" "    a + b" "}" >"$proj/src/lib.rs"
    note "  journey: a satisfied rule -> exit 0"
    ll_run "$proj"
    assert_pass
}

# A clear violation -> exit 1. Proves the model flags an obvious TODO through the
# harness and that llmlint maps holds=false to a non-zero exit.
ll_live_fail() {
    local harness="$1" proj
    proj="$(make_project "$harness")"
    printf '%s\n' \
        "// TODO: replace this placeholder with the real implementation" \
        "pub fn add(a: i32, b: i32) -> i32 {" \
        "    a + b" \
        "}" >"$proj/src/lib.rs"
    note "  journey: a clear violation -> exit 1"
    ll_run "$proj"
    assert_fail
}

# The full per-harness live run: a pass journey and a violation journey.
live_run_journeys() {
    local harness="$1"
    need jq
    require_oneharness
    note "== llmlint live e2e: $harness =="
    ll_live_pass "$harness"
    ll_live_fail "$harness"
    note "PASS: $harness llmlint live e2e"
}
