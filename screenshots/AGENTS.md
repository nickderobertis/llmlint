# Terminal screenshots

Deterministic SVG screenshots of llmlint's **real** colorized output, gated by
[screencomp](https://github.com/nickderobertis/screencomp). Informational, like
the benches — **never part of `just check` or the CI gate**; the `Visual docs`
workflow (`.github/workflows/visual-docs.yml`) owns the comparison on PRs.

## What it is

`screenshots/screenshots.sh` drives the **real release `llmlint` binary** against the
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

- **`freeze` is version-pinned** (`just`'s `freeze-version`, which
  `screenshots-tools` installs, and `freeze_version` in
  `screenshots/ci-install-freeze.sh`, which CI's `capture-command` runs; an e2e
  journey holds the two equal).
- **The font is vendored** (`fonts/JetBrainsMono-Regular.ttf`, OFL — see
  `fonts/JetBrainsMono-OFL.txt`) and passed via `--font.file`, so freeze never
  fetches one over the network (which also makes capture offline and fast). The
  font is embedded into each SVG as base64, so the file renders the same on
  GitHub and crates.io with nothing external to load.
- **The environment is cleared**: `screenshots.sh` unsets every ambient
  `LLMLINT_*` / `ONEHARNESS_*` variable before it captures. The scenes render the
  real binary, which reads those settings ahead of most other layers, so an
  exported one prints straight into a shot — `LLMLINT_ONEHARNESS_BIN` shows up
  verbatim in the `config` scene's effective config — and drifts the hash against
  a baseline CI captured with a clean shell.

The result: identical bytes on every machine and runner — the SVG only changes
when the report's **content or formatting** changes, which is exactly what the
gate should catch.

## Lanes: one per arch, each with its own baseline

screencomp scopes captures per CPU arch (a *lane*). `[capture].arches` declares
**`x86_64` and `arm64`**, each with its own committed
`shots/baseline/<arch>.json`; CI runs one job per lane (`arm64` on
`ubuntu-24.04-arm`).

Both are declared even though the bytes are identical, because the guard is
**local**: `.githooks/pre-push` classifies and re-blesses the lane of the host it
runs on, and refuses a host arch no lane declares. llmlint is developed on arm64
and released from CI's x86_64, so both are real hosts.

**Re-blessing, on any host:** `just screenshots-bless` rewrites **this host's lane
only** (`screenshots/host-arch.sh` names it — the single place a lane name is derived
from `uname -m`, shared with the capture and the guard). Commit it with
`docs/screenshots/`; the identical-bytes contract is what makes that safe, and CI's
job for the *other* lane is the check on it.

## Outputs

- `shots/current/<arch>/captures.json` + the SVGs — the capture screencomp reads
  (gitignored; regenerated). `$SHOTS_OUT` overrides the directory; the reusable
  workflow exports it per arch lane.
- `shots/baseline/<arch>.json` — the committed digest baseline (no images).
- `docs/screenshots/*.svg` — the committed copies embedded in the README.

## The animated demo GIF (`docs/screenshots/demo.gif`)

The SVGs are static; the README **hero** is an animated GIF of the live-progress
view (rules resolving as their judges return, then clearing to the report — see
`docs/design/interactive-progress.md`). `screenshots/demo-gif.py` drives the **real
release binary** against the same `fixture/` for its data (genuine rules/verdicts/
report), then reconstructs the frames the view draws and renders them with the same
**vendored JetBrains Mono font** — Pillow only, no `ttyd`/`ffmpeg`. Unlike the SVGs
it is **not** hash-gated (a GIF isn't byte-reproducible across Pillow versions), so
it is regenerated on demand (`just screenshots-gif`) and committed. Regenerate it
when the live view's format changes (`src/commands/progress.rs`).

## Commands

- `just screenshots-tools` — install the pinned `freeze` (needs Go). screencomp
  is installed separately (see its README); CI installs both itself — `freeze`
  via `screenshots/ci-install-freeze.sh`, which picks the prebuilt release matching
  the runner's arch (so the arm64 lane's runner gets an arm64 binary) and pins the
  same version this recipe does.
- `just screenshots` — capture (builds the release binaries, writes the shots +
  the README copies). Quiet on success.
- `just screenshots-gif` — regenerate the animated demo GIF (needs Python 3 +
  Pillow). Builds the release binaries, then writes `docs/screenshots/demo.gif`.
- `just screenshots-bless` — after an **intended** output change, recapture and
  refresh **this host's** lane, `shots/baseline/<arch>.json` (the arch from
  `screenshots/host-arch.sh`). Commit it alongside `docs/screenshots/`.

## The strict gate

CI (`fail-on-drift: true`) fails when a capture diverges from the committed
baseline. The local pre-push guard (`.githooks/pre-push`, enable with
`git config core.hooksPath .githooks`) re-captures **only** when a
`[guard].paths` file changes (`screencomp.toml`), and on drift it regenerates
**this host's lane** baseline, builds a review gallery (`shots/review/index.html`),
and blocks the push so you commit the refreshed baseline + README images
deliberately. If the host's arch is not one of the declared lanes it refuses
instead, naming what is declared and how to add the lane.

## Changing the screenshots

Editing the report format (`src/domain/report.rs`), the CLI surface, the fixture,
or the scenes in `screenshots/screenshots.sh` will change the SVGs. That is expected —
run `just screenshots-bless` and commit the new baseline + `docs/screenshots/`.
Bumping `freeze-version` or the vendored font reflows every shot; bless once and
keep the two `freeze` version pins in sync (`freeze-version` in the justfile and
`freeze_version` in `screenshots/ci-install-freeze.sh` — a journey here holds them
equal). A reflow changes every lane identically, so one host's bless covers both
baselines.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] the guard's and the freeze installer's journey lists are the reference for what their script journeys prove about two tools the gate never installs, so they are kept complete rather than terse -->
## The pre-push visual guard (`.githooks/pre-push`)

The `pre_push_guard_*` journeys (`tests/visual_guard.rs`) drive the **real hook
script** the way git does (cwd = a scratch repo, the range on
`SCREENCOMP_GUARD_RANGE`), with stubs at its
subprocess seams (`GuardRepo`): a `screencomp` that records argv and answers with
a chosen exit, a `freeze`, and a `screenshots/screenshots.sh`. The real tools are not
installed by `just setup` or CI's gate, so they are stubbed as the suite stubs
oneharness; a change to the hook's own logic gets its journey here. The hook's own
lane helper (`screenshots/host-arch.sh`) is **not** stubbed — the real one is copied
in, so the lane under test is the one this host would really guard. They are
`#[cfg(unix)]` — the hook is bash.

The invariant they pin: `[capture].arches` declares one lane per arch, each with
its own committed baseline, and the guard is **local** — it classifies and
re-blesses the lane of the **host it runs on**, refusing a host arch no lane
declares. Journeys cover the clean push, drift (which rewrites that lane's
manifest and blocks), and the undeclared-host refusal; each drives configurations
that discriminate the host's lane from the first declared one on **every** CI
arch, so the suite is not x86_64-only. A companion check holds every declared
lane's baseline present and byte-equal — the identical-bytes contract that lets
one host bless its own lane and CI's job for the other check it.

The hook's `just lint-llm-validate` step runs the real recipe from a copied-in
justfile with only `llmlint` stubbed. Keep the hook's PATH to the stubs, a lone
`just` link, and the system dirs, and HOME scratch: the recipe also looks in
`~/.local/bin`, so a host llmlint would otherwise stand in for the stub or for
its absence.

## CI's `freeze` installer (`screenshots/ci-install-freeze.sh`)

CI runs the arm64 lane on an arm64 runner, so the capture step must fetch the
`freeze` release matching the **runner's** architecture, and validate it against
digests pinned in this repository. The `ci_install_freeze_*` journeys
(`tests/visual_guard.rs`) drive the real script with real `curl`/`tar`/`install`;
only what a test cannot own is stood in — the runner's CPU (a `uname` ahead of the real one on `PATH`) and
charmbracelet's release server (a local release tree over `file://`, with its own
pin file). They cover every `uname -m` spelling installing the matching asset
(proven by running the installed binary), an architecture freeze does not publish
for, an archive failing its pinned digest, one missing the binary, and the pin
agreeing with the justfile's.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
