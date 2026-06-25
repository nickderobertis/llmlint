# Terminal screenshots

Deterministic SVG screenshots of llmlint's **real** colorized output, gated by
[screencomp](https://github.com/nickderobertis/screencomp). Informational, like
the benches — **never part of `just check` or the CI gate**; the `Visual docs`
workflow (`.github/workflows/visual-docs.yml`) owns the comparison on PRs.

## What it is

`scripts/screenshots.sh` drives the **real release `llmlint` binary** against the
mock-oneharness fixture in `fixture/` — exactly as the e2e suite does — so the
captured text is genuine CLI output with real ANSI color (`--color always`); only
the judge verdicts are scripted (`fixture/verdicts.json`), so there is no model,
network, or cost. Each scene is rendered to an SVG by [`freeze`](https://github.com/charmbracelet/freeze).

One shot, `lint-report`, with a `view` toggle the gallery flips between:

- `default` — the report a user sees (failing rule + locations + summary), and
- `verbose` — `-v`, itemizing every rule so PASS (green) and SKIP (yellow) show.

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

## Commands

- `just screenshots-tools` — install the pinned `freeze` (needs Go). screencomp
  is installed separately (see its README); CI installs both itself.
- `just screenshots` — capture (builds the release binaries, writes the shots +
  the README copies). Quiet on success.
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
