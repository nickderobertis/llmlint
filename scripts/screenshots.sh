#!/usr/bin/env bash
# Capture the terminal screenshots that screencomp gates, galleries, and posts to
# PRs (see screencomp.toml + .github/workflows/visual-docs.yml).
#
# It drives the REAL release `llmlint` binary against the mock-oneharness fixture
# (screenshots/fixture/) — exactly as the e2e suite does — so the captured output
# is genuine CLI output with real ANSI color; only the judge verdicts are scripted
# (no model, no network, no cost). Each scene's colored output is rendered to a
# deterministic SVG by `freeze` using the VENDORED, pinned font
# (screenshots/fonts/JetBrainsMono-Regular.ttf), so the bytes — and therefore the
# screencomp digests — are identical on every machine and CI runner without a
# pinned container. That byte-determinism is the whole contract: change the
# report's formatting and the SVG (and its hash) changes; otherwise it does not.
#
# Output (screencomp's capture contract):
#   $SHOTS_OUT/captures.json          index: {schema, shots:[{name,toggles,hash,image}]}
#   $SHOTS_OUT/lint-report-<view>.svg one SVG per `view` toggle value
# $SHOTS_OUT defaults to shots/current/<arch> (the reusable workflow exports it
# per lane). The README hero copies land in docs/screenshots/ (committed).
#
# Requires `freeze` on PATH (install the pinned version with `just screenshots-tools`).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="arm64" ;;
esac
SHOTS_OUT="${SHOTS_OUT:-shots/current/$arch}"
font="$repo_root/screenshots/fonts/JetBrainsMono-Regular.ttf"
fixture="$repo_root/screenshots/fixture"
docs_dir="$repo_root/docs/screenshots"

if ! command -v freeze >/dev/null 2>&1; then
  echo "screenshots: 'freeze' not on PATH. Install the pinned version with:" >&2
  echo "             just screenshots-tools" >&2
  exit 1
fi

# Build the binaries the capture drives: the real CLI and the mock oneharness it
# talks to (the fixture feature). Release, like a user would run.
llmlint_bin="$repo_root/target/release/llmlint"
mock_bin="$repo_root/target/release/llmlint-mock-oneharness"
if [ -z "${SCREENSHOTS_NO_BUILD:-}" ] || [ ! -x "$llmlint_bin" ] || [ ! -x "$mock_bin" ]; then
  cargo build --release --locked --features mock-oneharness \
    --bin llmlint --bin llmlint-mock-oneharness >&2
fi

# Portable SHA-256 (Linux coreutils vs macOS/BSD).
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# One shot, `lint-report`, with a `view` toggle the gallery flips between:
#   default — the report a user sees (failing rule + locations + summary)
#   verbose — `-v`, itemizing every rule so PASS (green) and SKIP (yellow) show too
views=(default verbose)
shot_name="lint-report"

# Deterministic freeze flags. The vendored font (embedded into the SVG as base64)
# is what makes the output reproducible across machines; everything else is fixed
# window styling. Auto width/height follow the content, so they only move when the
# report does.
freeze_flags=(
  # Force terminal/ANSI mode. freeze's content-based auto-detection is flaky —
  # it intermittently misreads the colored report as a source file ("Language
  # Unknown") and then ignores --font.file and hangs fetching a default font over
  # the network. `--language ansi` is unconditional, offline, and byte-identical
  # to the auto-detected render, so it is both the determinism and the CI fix.
  --language ansi
  --font.file "$font"
  --font.family "JetBrains Mono"
  --font.size 14
  --window
  --background "#0d1117"
  --padding "20,30"
  --margin 0
  --border.radius 8
)

rm -rf "$SHOTS_OUT"
mkdir -p "$SHOTS_OUT" "$docs_dir"
tmp_state="$(mktemp -d)"
trap 'rm -rf "$tmp_state"' EXIT

entries=()
for view in "${views[@]}"; do
  verbosity=()
  [ "$view" = "verbose" ] && verbosity=(-v)
  ansi="$tmp_state/$view.ansi"
  err="$tmp_state/$view.err"
  # `--color always` forces ANSI through the pipe; `--max-parallel 1` keeps the
  # multi-judge order stable so the per-judge lines render identically every run.
  # `-c` pins the fixture config so upward config discovery never picks up a
  # parent llmlint.yml from wherever the repo happens to be checked out (CI).
  set +e
  ( cd "$fixture" \
      && LLMLINT_MOCK_VERDICTS="$fixture/verdicts.json" \
         LLMLINT_MOCK_STATE="$tmp_state/state-$view" \
         "$llmlint_bin" -c "$fixture/llmlint.yml" --oneharness-bin "$mock_bin" \
         --color always --max-parallel 1 "${verbosity[@]}" ) >"$ansi" 2>"$err"
  rc=$?
  set -e

  # A failing lint exits 1 by design (the fixture has a failing rule), so the
  # exit code is not the signal — the presence of ANSI is. freeze needs the
  # escape codes to render terminal mode; without them it dies with an opaque
  # "Language Unknown", so guard it here and surface what llmlint actually did.
  if ! grep -q $'\033' "$ansi"; then
    {
      echo "screenshots: scene '$view' produced no ANSI (llmlint exit $rc) — cannot render."
      echo "---- llmlint stdout ($(wc -c <"$ansi") bytes) ----"
      cat -v "$ansi"
      echo "---- llmlint stderr ----"
      cat "$err"
    } >&2
    exit 1
  fi

  image="$shot_name-$view.svg"
  freeze "$ansi" "${freeze_flags[@]}" -o "$SHOTS_OUT/$image" >&2
  hash="$(sha256 "$SHOTS_OUT/$image")"
  # captures.json identity is `name + JSON.stringify(toggles)`; emit toggles with
  # the same compact, key-sorted shape screencomp expects.
  entries+=("$shot_name|{\"view\":\"$view\"}|$hash|$image")

  # The committed README hero(es): same bytes, just outside the gitignored tree.
  cp "$SHOTS_OUT/$image" "$docs_dir/$image"
done

# Write captures.json, shots sorted by identity, schema 1, trailing newline — the
# exact shape screencomp's classify/manifest/gallery read. All fields are safe
# ASCII (names, toggle values, hex digests, file names), so plain printf is sound.
{
  printf '{\n  "schema": 1,\n  "shots": [\n'
  IFS=$'\n' sorted=($(printf '%s\n' "${entries[@]}" | sort)); unset IFS
  last=$((${#sorted[@]} - 1))
  for i in "${!sorted[@]}"; do
    IFS='|' read -r name toggles hash image <<<"${sorted[$i]}"
    comma=","
    [ "$i" -eq "$last" ] && comma=""
    printf '    {\n      "name": "%s",\n      "toggles": %s,\n      "hash": "%s",\n      "image": "%s"\n    }%s\n' \
      "$name" "$toggles" "$hash" "$image" "$comma"
  done
  printf '  ]\n}\n'
} >"$SHOTS_OUT/captures.json"

echo "screenshots: wrote ${#views[@]} shots to $SHOTS_OUT and docs/screenshots/" >&2
