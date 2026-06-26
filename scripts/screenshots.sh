#!/usr/bin/env bash
# Capture the terminal screenshots that screencomp gates, galleries, and posts to
# PRs (see screencomp.toml + .github/workflows/visual-docs.yml).
#
# It drives the REAL release `llmlint` binary against the mock-oneharness fixture
# (screenshots/fixture/) — exactly as the e2e suite does — so the captured output
# is genuine CLI output; only the judge verdicts are scripted (no model, no
# network, no cost). Each scene's output is rendered to a deterministic SVG by
# `freeze` using the VENDORED, pinned font
# (screenshots/fonts/JetBrainsMono-Regular.ttf), so the bytes — and therefore the
# screencomp digests — are identical on every machine and CI runner without a
# pinned container. That byte-determinism is the whole contract: change a
# command's output (or its formatting) and that scene's SVG (and hash) changes;
# otherwise it does not.
#
# Scenes — one per command, so the gallery documents the whole CLI surface:
#   lint    the default report a user sees (failing rule + locations + summary),
#           with a `view` toggle the gallery flips between `default` and the
#           `-v` `verbose` report (which itemizes PASS/SKIP too).
#   init    writing a starter config.
#   config  the effective merged config + its sources, as JSON.
#   doctor  the oneharness preflight check.
# The `lint` scene is colorized (real ANSI through `--color always`); the other
# three are plain text — freeze renders both the same way (`--language ansi`).
#
# Output (screencomp's capture contract):
#   $SHOTS_OUT/captures.json   index: {schema, shots:[{name,toggles,hash,image}]}
#   $SHOTS_OUT/<scene>.svg     one SVG per scene (lint has one per `view` toggle)
# $SHOTS_OUT defaults to shots/current/<arch> (the reusable workflow exports it
# per lane). The SVGs are also copied to docs/screenshots/ (committed) for the
# README + gallery.
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

# Deterministic freeze flags. The vendored font (embedded into the SVG as base64)
# is what makes the output reproducible across machines; everything else is fixed
# window styling. Auto width/height follow the content, so they only move when the
# captured text does.
freeze_flags=(
  # Force terminal/ANSI mode. freeze's content-based auto-detection is flaky —
  # it intermittently misreads the colored report as a source file ("Language
  # Unknown") and then ignores --font.file and hangs fetching a default font over
  # the network. `--language ansi` is unconditional, offline, and byte-identical
  # to the auto-detected render — for the colorized `lint` scene it preserves the
  # ANSI color, and for the plain-text scenes it renders the text verbatim.
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

# captures.json identity is `name + JSON.stringify(toggles)`; entries collect one
# "name|toggles|hash|image" record per rendered scene, sorted at the end.
entries=()

# Render one captured text file to a scene SVG, hash it, and record it. A scene
# marked `require_ansi=1` must carry ANSI escapes (freeze needs them for the
# colored render; their absence means llmlint failed to produce the report, which
# `--language ansi` would otherwise paper over as a blank window). Plain scenes
# only have to be non-empty.
render_scene() {
  local name="$1" toggles="$2" image="$3" src="$4" require_ansi="$5"
  if [ "$require_ansi" = 1 ] && ! grep -q $'\033' "$src"; then
    {
      echo "screenshots: scene '$name' produced no ANSI — cannot render the colored report."
      echo "---- captured stdout ($(wc -c <"$src") bytes) ----"
      cat -v "$src"
    } >&2
    exit 1
  fi
  if [ ! -s "$src" ]; then
    echo "screenshots: scene '$name' produced no output — cannot render." >&2
    exit 1
  fi
  # `< /dev/null`: freeze reads stdin whenever it is not a character device (its
  # IsPipe check), so under CI's piped stdin it would ignore the file argument and
  # render empty input ("No input"). Pointing stdin at /dev/null (a char device)
  # forces it down the read-the-file path on every runner.
  freeze "$src" "${freeze_flags[@]}" -o "$SHOTS_OUT/$image" </dev/null >&2
  local hash
  hash="$(sha256 "$SHOTS_OUT/$image")"
  entries+=("$name|$toggles|$hash|$image")
  # The committed copies: same bytes, just outside the gitignored shots/ tree.
  cp "$SHOTS_OUT/$image" "$docs_dir/$image"
}

# --- lint: the report, default and `-v` verbose ------------------------------
# `--color always` forces ANSI through the pipe; `--max-parallel 1` keeps the
# multi-judge order stable so the per-judge lines render identically every run.
# `-c` pins the fixture config so upward config discovery never picks up a parent
# llmlint.yml from wherever the repo is checked out (CI).
for view in default verbose; do
  verbosity=()
  [ "$view" = "verbose" ] && verbosity=(-v)
  out="$tmp_state/lint-$view.ansi"
  ( cd "$fixture" \
      && LLMLINT_MOCK_VERDICTS="$fixture/verdicts.json" \
         LLMLINT_MOCK_STATE="$tmp_state/state-$view" \
         "$llmlint_bin" -c "$fixture/llmlint.yml" --oneharness-bin "$mock_bin" \
         --color always --max-parallel 1 "${verbosity[@]}" ) >"$out" 2>/dev/null || true
  render_scene "lint" "{\"view\":\"$view\"}" "lint-$view.svg" "$out" 1
done

# --- init: write a starter config (in a clean dir so the message is stable) ---
init_dir="$tmp_state/init"
mkdir -p "$init_dir"
out="$tmp_state/init.txt"
( cd "$init_dir" && "$llmlint_bin" init ) >"$out" 2>/dev/null || true
render_scene "init" "{}" "init.svg" "$out" 0

# --- config: the effective merged config + its sources, as JSON ---------------
# `--cwd`/`-c` pin the fixture; the lone source is then the fixture's absolute
# llmlint.yml path, which varies per checkout — strip the fixture prefix so the
# captured text (and its hash) is the same on every machine, leaving the natural
# `llmlint.yml`.
out="$tmp_state/config.txt"
( cd "$fixture" && "$llmlint_bin" config -c "$fixture/llmlint.yml" --cwd "$fixture" ) \
  >"$out" 2>/dev/null || true
sed -i "s|$fixture/||g" "$out"
render_scene "config" "{}" "config.svg" "$out" 0

# --- doctor: the oneharness preflight check -----------------------------------
# Put the mock on PATH as `oneharness` (no --oneharness-bin / env override) so the
# resolved binary is the bare `oneharness` a user with it installed would see,
# rather than an absolute, per-machine path.
doctor_bin="$tmp_state/bin"
mkdir -p "$doctor_bin"
cp "$mock_bin" "$doctor_bin/oneharness"
out="$tmp_state/doctor.txt"
( cd "$tmp_state" && PATH="$doctor_bin:$PATH" \
    LLMLINT_ONEHARNESS_BIN= "$llmlint_bin" doctor ) >"$out" 2>/dev/null || true
render_scene "doctor" "{}" "doctor.svg" "$out" 0

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

echo "screenshots: wrote ${#entries[@]} shots to $SHOTS_OUT and docs/screenshots/" >&2
