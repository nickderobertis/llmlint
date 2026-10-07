# Canonical command surface for llmlint.
#
# `just setup` provisions a bare machine from a fresh clone; `just bootstrap` is
# its cargo-level step (also called directly by CI). `just check` is the full
# quality gate and fails on any issue (no warnings-only mode). Recipes are quiet
# on success and specific on failure.
#
# The gate recipes (check, test, lint, lint-sh, lint-workflows, fmt-check,
# format, doc) DELEGATE to Nx (scripts/nx runs it on the pinned bun): each project
# declares what its targets do (cargo fmt, clippy, nextest under cargo-llvm-cov,
# shellcheck, actionlint), and the root only chooses which projects run them.
# scripts/nx-tier.sh picks the tier: with no flag the AFFECTED tier — `nx
# affected` from the explicit base scripts/nx-base.sh prints (NX_BASE, validated,
# else the merge base with origin/main); `--all` the FULL SWEEP (`nx run-many
# --all`). See AGENTS.md "Commits, releases, and merging" for which CI run uses
# which tier.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Pinned cargo dev tools that the gate drives but the toolchain doesn't ship.
# `scripts/setup.sh` installs these (reading the pins here); CI installs the
# latest of each via taiki-e/install-action. Keep in sync with that workflow.
nextest-version := "0.9.137"
llvmcov-version := "0.8.7"

# The GitHub Actions workflow linter `lint-workflows` (hence `check`) runs.
# `scripts/install-actionlint.sh` installs this pin from the prebuilt release,
# verified against the digests in `scripts/actionlint.sha256` (refresh those when
# this moves); `just setup`, `just actionlint-tools`, and CI's gate job all use it.
actionlint-version := "1.7.12"

# Tools for the informational performance suite (`bench*`, `profile`). NOT part
# of the gate or `just setup`: benchmarks measure, they don't block. CI's
# Performance workflow installs the latest of each via taiki-e/install-action;
# locally, `just bench-tools` installs these pins on demand.
hyperfine-version := "1.20.0"
critcmp-version := "0.1.8"
samply-version := "0.13.1"

# Renderer for the terminal screenshots (`just screenshots`). NOT part of the
# gate or `just setup`: screenshots are informational, like the benches. CI's
# Visual-docs workflow installs the same pinned version from the prebuilt release
# matching its runner's arch (`screenshots/ci-install-freeze.sh`, whose pin a
# journey holds equal to this one); `just screenshots-tools` installs it locally
# on demand. screencomp (the classify/gallery/PR-comment tool) is installed
# separately — see https://github.com/nickderobertis/screencomp.
freeze-version := "0.2.2"

# List available recipes.
default:
    @just --list

# Idempotent. With no `just` yet, run `./scripts/setup.sh` directly instead.
# One-command machine setup: rustup + pinned toolchain, just, cargo dev tools.
setup:
    @bash scripts/setup.sh

# Exit 0 when ready, else exit 1 with the reason and the fix. No installs.
# Fast, install-free dev-environment readiness check (also run by the hook).
setup-check:
    @bash scripts/setup-check.sh

# CI calls this directly after installing the toolchain + tools its own way.
# Fetch deps, add toolchain components, install the pinned bun + the locked Nx.
bootstrap:
    rustup show active-toolchain
    rustup component add rustfmt clippy llvm-tools
    cargo fetch --locked
    bash scripts/bun.sh ensure
    bash scripts/nx --version >/dev/null

# Full quality gate: format check, clippy and the project-boundary check,
# shellcheck, actionlint, build, every project's tests (unit, e2e, the offline
# release-targets and script journeys) with the coverage-measured ones under
# cargo-llvm-cov, docs, and the 95% line floor over the union. The affected tier
# by default; `just check --all` is the full sweep. Fails on any issue.
[positional-arguments]
check *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t format lint lint-sh lint-workflows build test doc coverage; \
    else \
      bash scripts/nx affected --base="$tier" -t format lint lint-sh lint-workflows build test doc coverage; \
    fi
    @echo "check: ok"

# The portable part of the gate, which CI's macOS/Windows `cross` jobs run:
# format, clippy and the boundary check, and every test uninstrumented
# (LLMLINT_COVERAGE=off). Coverage, shellcheck and actionlint are platform-
# independent and run in `check` on Linux. Same tier flag as `check`.
[positional-arguments]
check-portable *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    export LLMLINT_COVERAGE=off; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t format lint test; \
    else \
      bash scripts/nx affected --base="$tier" -t format lint test; \
    fi
    @echo "check-portable: ok"

# The test targets (each coverage-measured one writes its profiles; no floor).
[positional-arguments]
test *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t test; \
    else \
      bash scripts/nx affected --base="$tier" -t test; \
    fi

# Lint: clippy per crate (-D warnings) and the project-boundary check. Shell and
# workflow lint are `lint-sh` and `lint-workflows` (separate so the macOS/Windows
# cross jobs can run this one without shellcheck or actionlint); `check` runs all.
[positional-arguments]
lint *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t lint; \
    else \
      bash scripts/nx affected --base="$tier" -t lint; \
    fi

# Format the affected crates in place (`--all` for every crate).
[positional-arguments]
format *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t format --configuration=write; \
    else \
      bash scripts/nx affected --base="$tier" -t format --configuration=write; \
    fi

# Verify formatting without modifying files (the gate's `format` target).
[positional-arguments]
fmt-check *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t format; \
    else \
      bash scripts/nx affected --base="$tier" -t format; \
    fi

# Build the docs with warnings denied (kept in the gate so doc links don't rot).
[positional-arguments]
doc *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t doc; \
    else \
      bash scripts/nx affected --base="$tier" -t doc; \
    fi

# Every coverage-measured project's tests under cargo-llvm-cov, then the 95% line
# floor over their union (lower it only with a documented reason in AGENTS.md).
coverage:
    @bash scripts/nx run coverage:coverage

# shellcheck over each project's scripts (and .githooks/pre-push); part of
# `check`. Fix a finding, or disable it at its site with a reason.
[positional-arguments]
lint-sh *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t lint-sh; \
    else \
      bash scripts/nx affected --base="$tier" -t lint-sh; \
    fi

# actionlint over every workflow (the ci-workflows project); part of `check`.
# Fix a workflow finding at its site rather than suppress it.
[positional-arguments]
lint-workflows *flags:
    @tier="$(bash scripts/nx-tier.sh "$@")"; \
    if [ "$tier" = all ]; then \
      bash scripts/nx run-many --all -t lint-workflows; \
    else \
      bash scripts/nx affected --base="$tier" -t lint-workflows; \
    fi

# Install the pinned actionlint into ~/.local/bin; a no-op when it is already there.
actionlint-tools:
    @bash scripts/install-actionlint.sh

# The end-to-end binary journeys alone (also in `check`); builds the binary and
# the fixture first.
test-e2e:
    @bash scripts/nx run llmlint-e2e:test

# The release declaration's network tier: the `#[ignore]`-d tests reconciling
# the restated release-target schema against onevcs's canonical one and probing
# live crates.io/PyPI (the offline tests are the release-targets project's
# `test`, in the gate tiers). The `.github/workflows/release-targets.yml`
# workflow runs this.
test-release-targets:
    @bash scripts/nx run release-targets:network

# The real-oneharness tier: `tests/real-oneharness/`'s `#[ignore]`-d tests, which
# feed the `--config` list llmlint forwards to the released oneharness's own
# `oneharness config --format json` and assert which file's settings win. Free and
# model-free, but installs `oneharness-cli` from PyPI (network), so it is outside
# the gate tiers. The pin is the multi-file floor (a test holds them equal).
oneharness-cli-version := "0.18.0"

test-oneharness:
    @bash scripts/nx run real-oneharness:network

# Dogfood the version-bump check on llmlint's own versioned plugin
# (`assets/config_lint.yml`, which no standard config glob matches, so it is named
# explicitly). Fails if it changed vs the base without a `version:` bump. Out of
# `check`: it needs a base ref (default `origin/main`, the PR base) and network to
# resolve it, so CI runs it against the PR base. The base is a positional arg —
# override it with `just check-version-bump <ref>`.
check-version-bump base="origin/main":
    @bash scripts/nx run config-lint-plugin:check-version-bump --base={{quote(base)}}

# Advisory + license audit and unused-dependency check (the workspace project's
# `supply-chain` target). Separate from `check`: `cargo deny` needs a
# network-fetched advisory DB.
deps-check:
    @command -v cargo-deny >/dev/null || { echo "cargo-deny not installed: cargo install cargo-deny --locked" >&2; exit 1; }
    @command -v cargo-machete >/dev/null || { echo "cargo-machete not installed: cargo install cargo-machete --locked" >&2; exit 1; }
    @bash scripts/nx run workspace:supply-chain

# Upgrade dependencies, then re-run the gate as a full sweep (an upgrade can reach anything).
upgrade:
    cargo update
    @just check --all

# Build under the declared MSRV (advisory; needs the 1.85 toolchain installed).
msrv:
    cargo +1.85 check --locked --workspace --all-targets

# Opt-in LIVE run against the real oneharness + a real, authenticated harness.
# Makes real (paid) model calls, so it is deliberately out of `check` and CI.
# Example: `just lint-live --cwd ../some-repo`.
lint-live *ARGS:
    cargo run -- {{ARGS}}

# --- LIVE e2e: built llmlint -> real oneharness -> a real harness -------------
# The live analogue of the hermetic e2e suite: `just check` drives a mock
# oneharness, this drives the whole stack end to end (`tests/live/live-claude.sh`).
# It proves the built binary + oneharness + a real harness work together; the CI
# workflow (`.github/workflows/live.yml`) runs it on Linux, macOS, and Windows.
# Harness breadth is oneharness's test surface, so one canonical harness
# (claude-code) is enough here. Expects the harness configured, so a missing
# CLI/auth/oneharness is a HARD FAILURE, not a skip. Real (paid) model calls — out
# of the `check` gate. Model via `CLAUDE_E2E_MODEL` (see `tests/live/AGENTS.md`).

# The full stack end to end (builds the release binary first, like a real user).
# Fails if the harness CLI + auth aren't set up.
live-claude:
    @bash scripts/nx run live:live

# Windows-only: prove the colorized report actually RENDERS on a real Windows
# console (cell attributes are red/green), not just that ANSI bytes are emitted.
# Drives the release binary against the mock-oneharness fixture (no model, no
# cost), so it is deterministic and free; the CI workflow
# (`.github/workflows/win-color.yml`) runs it on windows-latest. The hermetic e2e
# + screenshots only assert ANSI is *emitted* (platform-independent); this asserts
# a Windows console *interprets* it. A rendering regression is a HARD FAILURE.
win-color:
    @bash scripts/nx run win-color:win-color

# Verbose, install-free diagnostics (kept out of the gate).
doctor:
    rustc --version
    cargo --version
    oneharness --version || echo "oneharness not installed (it is a runtime prerequisite)"

# Run the CLI through cargo, e.g. `just run -- --help`.
run *ARGS:
    cargo run --quiet -- {{ARGS}}

# --- Performance suite (informational; never part of `check` or CI's gate) ----
# Benchmarks are non-deterministic on shared hardware, so they measure rather
# than gate — like the live `lint-live` check. `just check`/clippy already
# type-check `benches/`, so the bench can't rot without a phase of its own.

# Install the benchmark + profiling tools (hyperfine, critcmp, samply), pinned.
# On-demand only: not run by `just setup` (the gate doesn't need these).
bench-tools:
    @command -v cargo-binstall >/dev/null || { echo "cargo-binstall not found: see https://github.com/cargo-bins/cargo-binstall, or 'cargo install' each tool" >&2; exit 1; }
    cargo binstall --no-confirm --disable-telemetry hyperfine@{{hyperfine-version}} critcmp@{{critcmp-version}} samply@{{samply-version}}

# Engine micro-benchmarks (Criterion), saved as BASELINE (default `current`, what
# bench-compare reads); extra arguments go to Criterion (e.g. --measurement-time 3).
[positional-arguments]
bench baseline="current" *criterion_args:
    @for a in "$@"; do printf '%s' "$a" | grep -Eq '^[A-Za-z0-9_./=:-]+$' || { printf "bench: argument '%s' is not a plain baseline name or Criterion option\n" "$a" >&2; exit 2; }; done
    @bash scripts/nx run bench:bench -- --save-baseline "$@"

# Save current engine benchmarks as the `base` baseline (run on the comparison point).
bench-base:
    @just bench base

# Diff the latest `bench` run against `base` (run `bench-base` first; needs critcmp).
bench-compare:
    critcmp base current

# End-to-end CLI latency for every command (hyperfine); writes target/bench/results.*.
bench-cli:
    @bash scripts/nx run bench:bench-cli

# Fast smoke check of the CLI benchmark harness (one run, no warmup, no stable numbers).
bench-cli-smoke:
    @bash scripts/nx run bench:bench-cli-smoke

# Deterministic engine allocation counts (counting allocator; exact, comparable across
# commits); also written to target/bench/allocs.md for the Performance report.
bench-allocs:
    @bash scripts/nx run bench:bench-allocs

# Deterministic end-to-end CLI instruction counts (cachegrind; Linux-only, needs valgrind).
bench-instructions:
    @bash scripts/nx run bench:bench-instructions

# Run the portable benchmark layers (Criterion + hyperfine + allocation counts).
bench-all: bench bench-cli bench-allocs

# Record a sampling/callgrind profile to find bottlenecks; see benches/profile.sh for modes.
[positional-arguments]
profile *ARGS:
    @bash scripts/nx run bench:profile -- "$@"

# --- Terminal screenshots (informational; never part of `check` or CI's gate) -
# Deterministic SVGs of the real CLI output, rendered by `freeze` from a vendored
# pinned font, gated/galleried/PR-commented by screencomp (see screenshots/AGENTS.md).
# Regenerating is out of the gate, like the benches; CI's Visual-docs workflow owns
# the comparison — one job per lane in [capture].arches (x86_64 and arm64) — and the
# pre-push guard regenerates THIS host's lane baseline locally on drift.

# Install the pinned screenshot renderer (`freeze`) on demand. Needs Go.
screenshots-tools:
    @command -v go >/dev/null || { echo "go not found: needed to install freeze; see https://go.dev/dl" >&2; exit 1; }
    go install github.com/charmbracelet/freeze@v{{freeze-version}}
    @echo "installed freeze to $(go env GOPATH)/bin (ensure it is on PATH)"

# Capture the screenshots: drive the real binary against the mock fixture, render
# each scene to shots/current/<arch>/ + docs/screenshots/. Needs `freeze` on PATH.
screenshots:
    @bash scripts/nx run screenshots:capture

# Regenerate the animated demo GIF (docs/screenshots/demo.gif — the README hero
# showing the live-progress view). Like the screenshots it drives the REAL release
# binary against the mock fixture, then renders faithful frames of the live view
# with the vendored JetBrains Mono font (Pillow only — no ttyd/ffmpeg). It is
# informational, NOT hash-gated (a GIF isn't byte-reproducible), so regenerate on
# demand and commit the result. Needs Python 3 + Pillow (`pip install Pillow`).
screenshots-gif:
    @command -v python3 >/dev/null || { echo "python3 not found: needed to render the demo GIF" >&2; exit 1; }
    @python3 -c "import PIL" 2>/dev/null || { echo "Pillow not installed: pip install Pillow" >&2; exit 1; }
    @bash scripts/nx run screenshots:gif

# Refresh the committed baseline manifest from a fresh capture (after an intended
# output change). Commit shots/baseline/*.json + docs/screenshots/ alongside.
#
# There is one lane per arch in [capture].arches (screencomp.toml), and this
# refreshes THIS host's lane only — an arm64 host rewrites shots/baseline/arm64.json,
# an x86_64 host shots/baseline/x86_64.json, each via screenshots/host-arch.sh so the
# name matches what the pre-push guard classifies. The shots are byte-identical
# across arches, so the other lane needs no local rewrite; CI's job for it is what
# checks the two agree.
screenshots-bless:
    @bash scripts/nx run screenshots:bless
    @echo "baseline refreshed for the $(bash screenshots/host-arch.sh) lane; commit shots/baseline/ + docs/screenshots/"

# Install/refresh the optional llmlint toolchain. Idempotent.
setup-llmlint:
    ./scripts/setup-llmlint.sh

# Optional LLM-as-judge lint; non-deterministic and out of `check`.
lint-llm *paths:
    llmlint {{paths}}

# Files whose only inline ignore-directive occurrences are *examples* — placeholder
# rule names in the docs, prompt template, ignore parser, and its tests — not real
# suppressions. llmlint *defines* the ignore-directive syntax, so check-ignores
# can't tell an example from the real thing and flags them all. Keep this list
# current as those examples move between files.
ignore-scan-exclude := "README.md:AGENTS.md:tests/e2e/AGENTS.md:assets/default_template.md:scripts/setup-llmlint.sh:src/domain/ignore.rs:src/io/files.rs:src/commands/check_ignores.rs:src/errors.rs:src/domain/plan.rs:tests/e2e/main.rs"

# Deterministic llmlint config/ignore/version-bump validation. The exclude above is
# applied via the LLMLINT_FILES_EXCLUDE env layer (issue #152: CLI > env > config;
# env adds to the config's `files.exclude` denylist) and scoped to this recipe
# only, so the LLM lint (`lint-llm-diff`) keeps full file coverage.
lint-llm-validate *args:
    PATH="$HOME/.local/bin:$PATH" LLMLINT_FILES_EXCLUDE="{{ignore-scan-exclude}}" llmlint validate {{args}}

# llmlint scoped to changed files since the merge-base with main. `--no-ignore-check`
# skips the deterministic ignore-directive *structure* gate here (the LLM judge still
# runs on every changed file, example-directive files included): `lint-llm-validate`
# above already runs that structural check with the `ignore-scan-exclude` denylist, so
# re-checking here would only re-trip on the excluded example-directive files.
lint-llm-diff base="origin/main" *args:
    llmlint --diff --diff-base "{{base}}" --no-ignore-check {{args}}
