#!/usr/bin/env bash
# Pass-through oneharness wrapper for the accuracy/timing eval (`evals/run.py`).
#
# llmlint reads only the verdict from oneharness's JSON report and discards the
# per-result `usage` (tokens, cost_usd) and `duration_ms` it also carries. The
# eval needs those to compare cost across judge backends, so the runner points
# `--oneharness-bin` here: it execs the real oneharness unchanged and copies each
# `run` report into `$LLMLINT_EVAL_TAP_DIR` for the runner to sum. Everything
# llmlint sees (stdout, stderr, exit code) is exactly what oneharness produced.
set -euo pipefail

real="${LLMLINT_EVAL_REAL_ONEHARNESS:?LLMLINT_EVAL_REAL_ONEHARNESS must name the real oneharness binary}"

if [ "${1:-}" != run ] || [ -z "${LLMLINT_EVAL_TAP_DIR:-}" ]; then
    exec "$real" "$@"
fi

out="$(mktemp "${LLMLINT_EVAL_TAP_DIR}/run-XXXXXX")"
set +e
"$real" "$@" >"$out"
code=$?
set -e
cat "$out"
mv "$out" "$out.json"
exit "$code"
