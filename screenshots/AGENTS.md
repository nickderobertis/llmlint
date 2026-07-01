# Terminal screenshots

Deterministic SVG screenshots of llmlint's **real** colorized output, gated by
[screencomp](https://github.com/nickderobertis/screencomp). Informational, like
the benches — **never part of `just check` or the CI gate**; the `Visual docs`
workflow (`.github/workflows/visual-docs.yml`) owns the comparison on PRs.

## What it is

`scripts/screenshots.sh` drives the **real release `llmlint` binary** against the
mock-oneharness fixture in `fixture/` — exactly as the e2e suite does — so the
captured text is genuine CLI output; only the judge verdicts are scripted
(`fixture/verdicts.json`), so there is no model, network, or cost. Each scene is
rendered to an SVG by [`freeze`](https://github.com/charmbracelet/freeze).

One shot per command, so the gallery documents the whole CLI surface:

- `lint` — the report, with a `view` toggle the gallery flips between three
  levels of detail:
  - `default` — the report a user sees: failing rule + locations + summary. The
    fixture also carries a not-relevant rule, so the summary's `… not relevant`
    segment shows here.
  - `verbose` — `-v`, itemizing every rule so PASS (green), SKIP (yellow), and
    N/A (dim, not relevant) show too. (`default`/`verbose` are colorized via
    `--color always`; real ANSI.)
  - `debug` — the oneharness debug view `-v` prints to **stderr**: the exact
    `oneharness run …` command and the raw result for each judge. This is the
    only thing the verbose level adds beyond the itemized report (a literal `-vv`
    is byte-identical to `-v`), so it is its own scene. Plain text, captured from
    stderr; tall (it embeds the full system prompt per judge).
- `multi-judge` — the per-judge breakdown a `judges: N` rule prints (each judge's
  held/violated + rationale). Kept out of the headline `lint` scene so that stays
  single-judge; driven by its own nested fixture (`fixture/multijudge/`), pinned
  with `-c` so config discovery never merges it with the main scene. Colorized.
- `init` — writing a starter config (`wrote llmlint.yml`).
- `config` — the effective merged config + its sources, as JSON.
- `doctor` — the oneharness preflight check.

**Consistent text size.** Every scene is rendered at a fixed window width
(`freeze --width 835`, with `--wrap 92` folding the few over-wide lines), so the
gallery/README — which display each SVG at one fixed width — render the text at
the same size on every card. Without this, auto-width made a narrow `init` scale
up huge and a wide `config` shrink.

**Path normalization** (so the bytes/hashes are identical on every machine):
- `config`'s lone source is captured with its fixture-dir prefix stripped
  (leaving the natural `llmlint.yml`).
- `doctor` resolves the mock via `PATH` as a bare `oneharness` (no absolute
  override path). The mock reports `oneharness 0.2.529 (mock)`, so the shot shows
  that `(mock)` marker — honest about where the number comes from.
- `debug` carries three per-run paths (the mock binary, the generated `--schema`
  tempfile, and `--cwd`); the script rewrites each to a fixed placeholder
  (`oneharness`, `/tmp/llmlint-schema.json`, `.`).

## Why it is byte-reproducible (and needs no container)

screencomp gates on the **hash** of each image, so capture must be deterministic.
Unlike a rasterized PNG (whose anti-aliasing drifts across CPUs — why the web
app in `allowlister-remote` captures inside a pinned Playwright container), an
SVG is pure layout math. We pin both inputs:

- **`freeze` is version-pinned** (`just`'s `freeze-version`, the CI
  `capture-command`, and `screenshots-tools` all agree).
- **The font is vendored** (`fonts/JetBrainsMono-Regular.ttf`, OFL — see
  `fonts/JetBrainsMono-OFL.txt`) and passed via `--font.file`, so freeze never
  fetches one over the network (which also makes capture offline and fast). The
  font is embedded into each SVG as base64, so the file renders the same on
  GitHub and crates.io with nothing external to load.

The result: identical bytes on every machine and runner, so a single `x86_64`
lane and baseline cover everyone — the SVG only changes when the report's
**content or formatting** changes, which is exactly what the gate should catch.

## Outputs

- `shots/current/<arch>/captures.json` + the SVGs — the capture screencomp reads
  (gitignored; regenerated). `$SHOTS_OUT` overrides the directory; the reusable
  workflow exports it per arch lane.
- `shots/baseline/<arch>.json` — the committed digest baseline (no images).
- `docs/screenshots/*.svg` — the committed copies embedded in the README.

## The animated demo GIF (`docs/screenshots/demo.gif`)

The SVGs are static; the README **hero** is an animated GIF of the live-progress
view (rules resolving as their judges return, then clearing to the report — see
`docs/design/interactive-progress.md`). `scripts/demo-gif.py` drives the **real
release binary** against the same `fixture/` for its data (genuine rules/verdicts/
report), then reconstructs the frames the view draws and renders them with the same
**vendored JetBrains Mono font** — Pillow only, no `ttyd`/`ffmpeg`. Unlike the SVGs
it is **not** hash-gated (a GIF isn't byte-reproducible across Pillow versions), so
it is regenerated on demand (`just screenshots-gif`) and committed. Regenerate it
when the live view's format changes (`src/commands/progress.rs`).

## Commands

- `just screenshots-tools` — install the pinned `freeze` (needs Go). screencomp
  is installed separately (see its README); CI installs both itself.
- `just screenshots` — capture (builds the release binaries, writes the shots +
  the README copies). Quiet on success.
- `just screenshots-gif` — regenerate the animated demo GIF (needs Python 3 +
  Pillow). Builds the release binaries, then writes `docs/screenshots/demo.gif`.
- `just screenshots-bless` — after an **intended** output change, recapture and
  refresh `shots/baseline/<arch>.json`. Commit it alongside `docs/screenshots/`.

## The strict gate

CI (`fail-on-drift: true`) fails when a capture diverges from the committed
baseline. The local pre-push guard (`.githooks/pre-push`, enable with
`git config core.hooksPath .githooks`) re-captures **only** when a
`[guard].paths` file changes (`screencomp.toml`), and on drift it regenerates the
baseline, builds a review gallery (`shots/review/index.html`), and blocks the
push so you commit the refreshed baseline + README images deliberately.

## Changing the screenshots

Editing the report format (`src/domain/report.rs`), the CLI surface, the fixture,
or the scenes in `scripts/screenshots.sh` will change the SVGs. That is expected —
run `just screenshots-bless` and commit the new baseline + `docs/screenshots/`.
Bumping `freeze-version` or the vendored font reflows every shot; bless once and
keep the three `freeze` version references in sync.
