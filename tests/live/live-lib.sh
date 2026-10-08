# shellcheck shell=bash
# Shared helpers for the llmlint LIVE end-to-end tier.
#
# These drive the REAL built `llmlint` binary against the REAL `oneharness` and a
# REAL, authenticated coding harness — the whole stack, no mocks. It is the live
# analogue of the hermetic e2e suite (`tests/e2e/`, which drives a mock
# oneharness). The CI workflow (`.github/workflows/live.yml`) runs it on Linux,
# macOS, and Windows: the cross-OS proof is the point. Harness *breadth* is
# oneharness's test surface, so the live tier drives one canonical harness.
#
# Contract: this tier runs in CI, where the harness CLI and its auth are expected
# to be configured. So a missing CLI, missing auth, or missing oneharness is a
# **hard failure** (red build), never a silent skip — a skip would let a broken
# live setup pass unnoticed. Set up the harness or don't run the recipe.
#
# Sourced by the harness entrypoint (`tests/live/live-claude.sh`); not run on its
# own. The entrypoint declares its harness id, the CLI it needs, the auth env vars
# it accepts, and an optional model override, then calls `live_run_journeys <id>`.
# `live_run_journeys` is harness-agnostic, so an ad-hoc script for another harness
# is a few lines (see `tests/live/AGENTS.md`).

# Strict mode is the library's own, not inherited from whichever script sources it.
set -euo pipefail

# llmlint: ignore-file[tool_output_is_signal] the paid live tier's per-journey narration is the only log of a run that cannot be replayed for free (live.yml's CI output)
LL_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
    || { printf 'FAIL: cannot resolve the repository root; source this library by its path from a readable checkout (tests/live/live-lib.sh)\n' >&2; exit 1; }

note() { printf '%s\n' "$*" >&2; }

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 ||
        fail "required tool not found on PATH: $1 (the live tier needs the real harness CLI installed)"
}

# Fail unless at least one of the named auth env vars is non-empty.
need_env() {
    local label="$1"
    shift
    local v
    for v in "$@"; do
        [ -n "${!v:-}" ] && return 0
    done
    fail "no $label configured (set one of: $*)"
}

# The release build the live recipes produce, an explicit `LLMLINT_BIN`, or
# whatever is on PATH. Platform-aware `.exe` handling for Windows runners.
ll_bin() {
    local b cand
    if [ -n "${LLMLINT_BIN:-}" ]; then
        for b in "$LLMLINT_BIN" "$LLMLINT_BIN.exe"; do
            [ -f "$b" ] && [ -x "$b" ] && { printf '%s' "$b"; return; }
        done
        fail "LLMLINT_BIN must name an executable llmlint binary (got '$LLMLINT_BIN'); build it with \`cargo build --release --locked -p llmlint --bin llmlint\` or unset LLMLINT_BIN"
    fi
    for cand in "$LL_REPO_ROOT"/target/release/llmlint{,.exe} \
                "$LL_REPO_ROOT"/target/debug/llmlint{,.exe}; do
        [ -x "$cand" ] && { printf '%s' "$cand"; return; }
    done
    command -v llmlint >/dev/null 2>&1 && { printf 'llmlint'; return; }
    printf ''
}

# llmlint needs oneharness on PATH (or via LLMLINT_ONEHARNESS_BIN); without it the
# whole stack can't run — a broken setup, so fail.
require_oneharness() {
    if [ -n "${LLMLINT_ONEHARNESS_BIN:-}" ]; then
        local b
        for b in "$LLMLINT_ONEHARNESS_BIN" "$LLMLINT_ONEHARNESS_BIN.exe"; do
            [ -f "$b" ] && [ -x "$b" ] && return 0
        done
        # llmlint itself reads the variable, so a bad one is never papered over
        # by a oneharness that happens to be on PATH.
        fail "LLMLINT_ONEHARNESS_BIN must name an executable oneharness binary (got '$LLMLINT_ONEHARNESS_BIN'); fix it or unset it to use the oneharness on PATH"
    fi
    command -v oneharness >/dev/null 2>&1 && return 0
    fail "oneharness not found (install it, or set LLMLINT_ONEHARNESS_BIN)"
}

LL_PROJECTS=()
_ll_cleanup() {
    local d
    for d in "${LL_PROJECTS[@]+"${LL_PROJECTS[@]}"}"; do
        [ -z "$d" ] || rm -rf "$d" || note "could not remove the scratch project $d; delete it by hand"
    done
}
trap _ll_cleanup EXIT

# LL_TIMEOUT and LL_MODEL are written into each project's llmlint.yml, so they
# are checked before anything is generated: the timeout must be a whole number of
# seconds (1 to 86400) and the model a plain model id (letters, digits, `.`, `_`,
# `-`, `:`, `/`, `@`), which can never close the YAML string or start a new key.
# A bad value fails before any paid call, naming the variable.
validate_settings() {
    local timeout="${LL_TIMEOUT:-120}" model="${LL_MODEL:-}"
    if ! [[ "$timeout" =~ ^[1-9][0-9]{0,4}$ ]] || [ "$timeout" -gt 86400 ]; then
        fail "LL_TIMEOUT must be a whole number of seconds from 1 to 86400, got '$timeout'; fix it or unset it to use 120"
    fi
    [ -z "$model" ] || [[ "$model" =~ ^[A-Za-z0-9._:/@-]+$ ]] \
        || fail "LL_MODEL must be a plain model id (letters, digits and . _ - : / @), got '$model'; fix it (CLAUDE_E2E_MODEL for live-claude.sh) or unset it to use the harness default"
}

# The harness id is written into llmlint.yml and oneharness.toml, so it must be a
# plain oneharness harness id (lowercase letters, digits and `-`), which can never
# close a TOML string or start a new YAML key.
validate_harness() {
    [[ "${1:-}" =~ ^[a-z0-9][a-z0-9-]*$ ]] \
        || fail "the harness id must be a plain oneharness id (lowercase letters, digits and -), got '${1:-}'; pass one such as claude-code"
}

# The `oneharness:` block both project constructors share, from the settings
# validate_settings admitted; the model is double-quoted so YAML reads it as a
# string whatever it looks like (`1.5`, `yes`).
oneharness_settings() {
    validate_settings
    echo "oneharness:"
    echo "  timeout: ${LL_TIMEOUT:-120}"
    if [ -n "${LL_MODEL:-}" ]; then echo "  model: \"${LL_MODEL}\""; fi
}

# Write a minimal real config that pins `harness` (and an optional model/timeout)
# and declares one crisp invariant. Echoes the project dir; the caller registers
# it for cleanup (this runs in a command substitution, whose variable changes the
# caller never sees) and fills in `src/lib.rs`. Every scaffolding step fails
# loudly: errexit does not reach inside a command substitution's caller check.
make_project() {
    local harness="${1:-}"
    local proj
    validate_harness "$harness"
    validate_settings
    proj="$(mktemp -d)" || fail "could not create a temporary project directory (check TMPDIR)"
    mkdir -p "$proj/src" || fail "could not create $proj/src; check that TMPDIR is writable and has space, then re-run"
    {
        echo "version: 1"
        echo "files:"
        echo '  include: ["src/**"]'
        oneharness_settings
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
    } >"$proj/llmlint.yml" || fail "could not write $proj/llmlint.yml; check that TMPDIR is writable and has space, then re-run"
    printf '%s' "$proj"
}

# A supported harness reliably ABSENT on the live runners (they install only the
# canonical harness + oneharness), distinct from the winner, to head a fallback
# chain so oneharness must fall through past it. `codex` is the issue-#146 case;
# fall back to `opencode` on the off chance the winner itself is codex.
_fallback_primary() {
    if [ "$1" = codex ]; then printf 'opencode'; else printf 'codex'; fi
}

# Like make_project but for oneharness FALLBACK mode (issue #146). The llmlint
# config pins NO harness (so llmlint omits `--harness` and oneharness selects from
# its own config), and a project `oneharness.toml` places an absent primary ahead
# of `$harness` in a fallback chain. oneharness skips the missing primary and runs
# `$harness`, naming it in the top-level `fallback.ran`; `results[0]` is the
# skipped primary. A correct llmlint reads the winner, not `results[0]`.
make_fallback_project() {
    local harness="${1:-}" primary proj
    validate_harness "$harness"
    validate_settings
    primary="$(_fallback_primary "$harness")"
    proj="$(mktemp -d)" || fail "could not create a temporary project directory (check TMPDIR)"
    mkdir -p "$proj/src" || fail "could not create $proj/src; check that TMPDIR is writable and has space, then re-run"
    {
        echo "version: 1"
        echo "files:"
        echo '  include: ["src/**"]'
        oneharness_settings
        echo "rules:"
        echo "  - name: no_todo_comments"
        echo "    description: >-"
        echo "      Every source file under src/ is free of TODO and FIXME comments."
        echo "      The property HOLDS when no source file contains a TODO or FIXME"
        echo "      marker, and is VIOLATED by any file that contains one."
    } >"$proj/llmlint.yml" || fail "could not write $proj/llmlint.yml; check that TMPDIR is writable and has space, then re-run"
    {
        echo 'run_mode = "fallback"'
        echo "harnesses = [\"${primary}\", \"${harness}\"]"
    } >"$proj/oneharness.toml" || fail "could not write $proj/oneharness.toml; check that TMPDIR is writable and has space, then re-run"
    printf '%s' "$proj"
}

LL_REPORT=""
LL_STDERR=""
LL_EXIT=0

ll_run() {
    local proj="$1"
    shift
    local bin
    bin="$(ll_bin)" || exit 1
    [ -n "$bin" ] || fail "llmlint binary not found (build it: \`cargo build --release --locked -p llmlint --bin llmlint\`, or set LLMLINT_BIN)"
    local errf
    errf="$(mktemp)" || fail "could not create a temporary file for llmlint's stderr (check TMPDIR)"
    note "  driving: llmlint --cwd <proj> --format json $* (timeout ${LL_TIMEOUT:-120}s${LL_MODEL:+, model $LL_MODEL})"
    set +e
    LL_REPORT="$("$bin" --cwd "$proj" --format json "$@" 2>"$errf")"
    LL_EXIT=$?
    set -e
    LL_STDERR="$(cat "$errf")" || fail "could not read llmlint's captured stderr ($errf); check that TMPDIR is readable, then re-run"
    rm -f "$errf" || note "could not remove $errf; delete it by hand"
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
# this is a genuine live-stack failure worth surfacing — never a skip.
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

# A satisfied invariant -> exit 0. Proves the model can read a clean file through
# the harness and return holds=true, and that llmlint maps that to a pass.
ll_live_pass() {
    local harness="$1" proj
    proj="$(make_project "$harness")"
    LL_PROJECTS+=("$proj")
    printf '%s\n' "pub fn add(a: i32, b: i32) -> i32 {" "    a + b" "}" >"$proj/src/lib.rs" \
        || fail "could not write $proj/src/lib.rs; check that TMPDIR is writable and has space, then re-run"
    note "  journey: a satisfied rule -> exit 0"
    ll_run "$proj"
    assert_pass
}

# A clear violation -> exit 1. Proves the model flags an obvious TODO through the
# harness and that llmlint maps holds=false to a non-zero exit.
ll_live_fail() {
    local harness="$1" proj
    proj="$(make_project "$harness")"
    LL_PROJECTS+=("$proj")
    printf '%s\n' \
        "// TODO: replace this placeholder with the real implementation" \
        "pub fn add(a: i32, b: i32) -> i32 {" \
        "    a + b" \
        "}" >"$proj/src/lib.rs" || fail "could not write $proj/src/lib.rs; check that TMPDIR is writable and has space, then re-run"
    note "  journey: a clear violation -> exit 1"
    ll_run "$proj"
    assert_fail
}

# Fallback selection (issue #146; the chain is make_fallback_project's): a clean
# file must still pass, which `assert_pass` (a hard fail on exit 2) checks.
ll_live_fallback() {
    local harness="$1" proj
    proj="$(make_fallback_project "$harness")"
    LL_PROJECTS+=("$proj")
    printf '%s\n' "pub fn add(a: i32, b: i32) -> i32 {" "    a + b" "}" >"$proj/src/lib.rs" \
        || fail "could not write $proj/src/lib.rs; check that TMPDIR is writable and has space, then re-run"
    note "  journey: fallback chain skips an absent primary and runs $harness -> exit 0"
    ll_run "$proj"
    assert_pass
}

# The full live run for one harness: a pass journey, a violation journey, and a
# fallback-selection journey.
live_run_journeys() {
    local harness="${1:-}" bin
    validate_harness "$harness"
    validate_settings
    need jq
    require_oneharness
    # ll_bin refuses a bad LLMLINT_BIN itself; the assignment carries its exit.
    bin="$(ll_bin)" || exit 1
    [ -n "$bin" ] || fail "llmlint binary not found (build it: \`cargo build --release --locked -p llmlint --bin llmlint\`, or set LLMLINT_BIN)"
    note "== llmlint live e2e: $harness =="
    ll_live_pass "$harness"
    ll_live_fail "$harness"
    ll_live_fallback "$harness"
    note "PASS: $harness llmlint live e2e"
}
