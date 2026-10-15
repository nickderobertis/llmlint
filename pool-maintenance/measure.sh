#!/usr/bin/env bash
# The measurement behind pool-maintenance/budgets.yaml's
# llmlint-pool-maintenance-seconds: how long one steady-state pass of the
# declared pool maintenance (the justfile's `pool-maintenance` recipe, which
# onevcs.toml names) takes. `onebudgetspec check` runs it with
# ONEBUDGETSPEC_RESULT set, and the figure is the pruner's own report of the
# timed pass, (d) below; this script never times or judges anything itself.
#
#   (a) one declared pass clears any backlog, after the ordinary build (the
#       gate's `cargo test --no-run`, the build nextest runs), so the backlog
#       includes whatever that build supersedes. Its report goes to a scratch
#       file and reaches the result only as evidence in the detail, never the
#       figure.
#   (b) the workspace's test targets, built that way under a per-package profile
#       override for every workspace member (`debug = 1`), which gives this
#       repository's own crates new metadata hashes. Every entry that build
#       created is backdated past the 2-day window, standing in for one run's
#       builds since superseded.
#   (c) the ordinary build, so each unit's newest build is the ordinary one.
#   (d) one declared pass, timed by the pruner, which removes (b)'s builds.
#
# It then holds the pass to that: it removed something, everything it removed
# is an entry (b) created, and every other entry is still there. The entry
# lists are left in target/pool-maintenance-measure/ for inspection.
#
# The pass builds the workspace twice, so only the pool-maintenance project's
# budgets-host target (`just budgets-host`) runs it, never `check`.
#
# Exit status: 0 measured; 1 a step failed, or the pass removed something other
# than (b)'s builds or nothing at all (the message says which); 2 not run by
# onebudgetspec.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" \
  || {
    echo "measure: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout." >&2
    exit 1
  }
readonly ROOT

fail() {
  echo "measure: $*" >&2
  exit 1
}

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "measure: ONEBUDGETSPEC_RESULT is not set; run it through onebudgetspec: just budgets-host" >&2
  exit 2
fi
readonly RESULT="$ONEBUDGETSPEC_RESULT"

for tool in just cargo jq cargo-prune-superseded; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool not found on PATH; run just bootstrap (just setup on a bare machine)."
done
cd "$ROOT" || fail "cannot enter $ROOT; run it from an intact checkout whose directories you can read and enter."
want="$(just --evaluate prune-superseded-version)" || fail "the justfile has no prune-superseded-version pin; restore it from git."
have="$(cargo-prune-superseded --version 2>/dev/null || true)"
[ "$have" = "cargo-prune-superseded $want" ] \
  || fail "$(command -v cargo-prune-superseded) is '${have:-<unknown version>}', but the justfile pins $want; run just bootstrap."
read -r -a targets <<<"$(just --evaluate pool-maintenance-targets)"
[ "${#targets[@]}" -gt 0 ] || fail "the justfile's pool-maintenance-targets names no target dir; restore it from git."

# The builds must land where the declared recipe prunes, whatever the caller's
# environment says.
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR

readonly WORK="${targets[0]}/pool-maintenance-measure"
rm -rf "$WORK"
mkdir -p "$WORK"

# Every build entry the pruner judges, one path per line, sorted: the top-level
# entries of deps/, .fingerprint/, build/ and incremental/ in every profile
# directory (one holding .fingerprint/) under each target dir.
entries() {
  local target profile kind
  for target in "${targets[@]}"; do
    [ -d "$target" ] || continue
    find "$target" -type d -name .fingerprint -prune -print | while IFS= read -r profile; do
      profile="${profile%/.fingerprint}"
      for kind in deps .fingerprint build incremental; do
        if [ -d "$profile/$kind" ]; then
          find "$profile/$kind" -mindepth 1 -maxdepth 1
        fi
      done
    done
  done | LC_ALL=C sort
}

# The pruner's `builds removed: N` from a result file.
builds_removed() {
  jq -r '.detail' "$1" | sed -n 's/.*builds removed: \([0-9][0-9]*\).*/\1/p'
}

# (a) The ordinary build, then the backlog pass, its report kept apart from the
# figure.
cargo test --workspace --locked --no-run >&2 || fail "the ordinary build failed (above); fix the build, then re-run."
ONEBUDGETSPEC_RESULT="$WORK/backlog.json" just pool-maintenance >&2
[ -s "$WORK/backlog.json" ] || fail "the backlog pass wrote no report to $WORK/backlog.json; check the pruner's output above."
backlog_seconds="$(jq -r '.value' "$WORK/backlog.json")"
backlog_removed="$(builds_removed "$WORK/backlog.json")"

# (b) The alternative build, then every entry it created backdated.
overrides=()
while IFS= read -r member; do
  overrides+=(--config "profile.dev.package.\"$member\".debug = 1")
done < <(cargo metadata --format-version 1 --no-deps --locked | jq -r '.packages[].name')
[ "${#overrides[@]}" -gt 0 ] || fail "cargo metadata listed no workspace member to rebuild."
entries >"$WORK/before-alternative.txt"
cargo test --workspace --locked --no-run "${overrides[@]}" >&2 \
  || fail "the alternative build failed (above); fix the build, then re-run."
entries >"$WORK/after-alternative.txt"
LC_ALL=C comm -13 "$WORK/before-alternative.txt" "$WORK/after-alternative.txt" >"$WORK/alternative.txt"
[ -s "$WORK/alternative.txt" ] \
  || fail "the alternative build created no entry, so it gave no crate a new metadata hash; nothing would be superseded."
tr '\n' '\0' <"$WORK/alternative.txt" | xargs -0 sh -c 'find "$@" -exec touch -h -t 200001010000 {} +' sh

# (c) The ordinary build again, now the newest of every unit.
cargo test --workspace --locked --no-run >&2 || fail "the ordinary build failed (above); fix the build, then re-run."
entries >"$WORK/before-pass.txt"
LC_ALL=C comm -23 "$WORK/before-pass.txt" "$WORK/alternative.txt" >"$WORK/ordinary.txt"

# (d) The timed pass: the pruner reports the figure to onebudgetspec itself.
just pool-maintenance >&2
[ -s "$RESULT" ] || fail "the timed pass wrote no report to $RESULT; check the pruner's output above."
entries >"$WORK/after-pass.txt"
LC_ALL=C comm -23 "$WORK/before-pass.txt" "$WORK/after-pass.txt" >"$WORK/removed.txt"
LC_ALL=C comm -23 "$WORK/ordinary.txt" "$WORK/after-pass.txt" >"$WORK/ordinary-lost.txt"
LC_ALL=C comm -23 "$WORK/removed.txt" "$WORK/alternative.txt" >"$WORK/removed-not-alternative.txt"

removed="$(builds_removed "$RESULT")"
[ "${removed:-0}" -gt 0 ] \
  || fail "the timed pass removed no build, so it measured no superseded build; see $WORK/alternative.txt."
[ ! -s "$WORK/removed-not-alternative.txt" ] \
  || fail "the timed pass removed entries the alternative build did not create: $WORK/removed-not-alternative.txt."
[ ! -s "$WORK/ordinary-lost.txt" ] \
  || fail "the timed pass removed ordinary-build entries: $WORK/ordinary-lost.txt."

count() { grep -c . "$1" || true; }
evidence="untimed backlog pass (evidence, not the figure): $backlog_seconds s, builds removed: ${backlog_removed:-0}"
evidence="$evidence; entries removed: $(count "$WORK/removed.txt"), all of the alternative build's $(count "$WORK/alternative.txt")"
evidence="$evidence; ordinary entries retained: all $(count "$WORK/ordinary.txt")"
report="$(jq -c --arg evidence "$evidence" '.detail += "; " + $evidence' "$RESULT")" \
  || fail "cannot read the timed pass's report in $RESULT as JSON (jq's error above); check the pruner's output above, then re-run just budgets-host."
printf '%s\n' "$report" >"$RESULT"
echo "measure: $(jq -r '.value' "$RESULT") s; $(jq -r '.detail' "$RESULT"); lists in $WORK" >&2
