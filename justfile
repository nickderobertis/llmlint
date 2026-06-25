# Canonical command surface for llmlint.
#
# `just setup` provisions a bare machine from a fresh clone; `just bootstrap` is
# its cargo-level step (also called directly by CI). `just check` is the full
# quality gate and fails on any issue (no warnings-only mode). Recipes are quiet
# on success and specific on failure.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Feature that builds the mock-oneharness fixture the e2e tests drive.
FEATURES := "mock-oneharness"

# Pinned cargo dev tools that the gate drives but the toolchain doesn't ship.
# `scripts/setup.sh` installs these (reading the pins here); CI installs the
# latest of each via taiki-e/install-action. Keep in sync with that workflow.
nextest-version := "0.9.137"
llvmcov-version := "0.8.7"

# Tools for the informational performance suite (`bench*`, `profile`). NOT part
# of the gate or `just setup`: benchmarks measure, they don't block. CI's
# Performance workflow installs the latest of each via taiki-e/install-action;
# locally, `just bench-tools` installs these pins on demand.
hyperfine-version := "1.20.0"
critcmp-version := "0.1.8"
samply-version := "0.13.1"

# Renderer for the terminal screenshots (`just screenshots`). NOT part of the
# gate or `just setup`: screenshots are informational, like the benches. CI's
# Visual-docs workflow installs the same pinned version; `just screenshots-tools`
# installs it locally on demand. screencomp (the classify/gallery/PR-comment tool)
# is installed separately — see https://github.com/nickderobertis/screencomp.
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
# Fetch deps + add toolchain components (the cargo step `setup` finishes with).
bootstrap:
    rustup show active-toolchain
    rustup component add rustfmt clippy llvm-tools
    cargo fetch --locked

# Full quality gate: format check, lint, tests (unit + integration + e2e) with
# coverage enforced, and docs. Fails on any issue.
check: fmt-check lint test doc
    @echo "check: ok"

# Verify formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Format the codebase in place.
format:
    cargo fmt --all

# Lint with clippy; any warning is an error.
lint:
    cargo clippy --all-targets --features {{FEATURES}} -- -D warnings

# Full test suite (unit + integration + the e2e binary journeys) with line
# coverage enforced. 95% is the gate; lower it only with a documented reason in
# AGENTS.md.
test:
    cargo llvm-cov nextest --features {{FEATURES}} --locked --fail-under-lines 95

# The end-to-end binary journeys in isolation (also run by `test`/`check`).
test-e2e:
    cargo nextest run --features {{FEATURES}} --test e2e --locked

# Build the docs with warnings denied (kept in the gate so doc links don't rot).
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --features {{FEATURES}}

# Advisory + license audit and unused-dependency check. Separate from `check`:
# `cargo deny` needs a network-fetched advisory DB.
deps-check:
    @command -v cargo-deny >/dev/null || { echo "cargo-deny not installed: cargo install cargo-deny --locked" >&2; exit 1; }
    @command -v cargo-machete >/dev/null || { echo "cargo-machete not installed: cargo install cargo-machete --locked" >&2; exit 1; }
    cargo deny check
    cargo machete

# Upgrade dependencies, then re-run the full gate.
upgrade:
    cargo update
    @just check

# Build under the declared MSRV (advisory; needs the 1.85 toolchain installed).
msrv:
    cargo +1.85 check --locked --all-targets --features {{FEATURES}}

# Opt-in LIVE run against the real oneharness + a real, authenticated harness.
# Makes real (paid) model calls, so it is deliberately out of `check` and CI.
# Example: `just lint-live --cwd ../some-repo`.
lint-live *ARGS:
    cargo run -- {{ARGS}}

# --- LIVE e2e: built llmlint -> real oneharness -> a real harness -------------
# The live analogue of the hermetic e2e suite: `just check` drives a mock
# oneharness, this drives the whole stack end to end (`scripts/live-claude.sh`).
# It proves the built binary + oneharness + a real harness work together; the CI
# workflow (`.github/workflows/live.yml`) runs it on Linux, macOS, and Windows.
# Harness breadth is oneharness's test surface, so one canonical harness
# (claude-code) is enough here. Expects the harness configured, so a missing
# CLI/auth/oneharness is a HARD FAILURE, not a skip. Real (paid) model calls — out
# of the `check` gate. Model via `CLAUDE_E2E_MODEL` (see `tests/AGENTS.md`).

# Build the optimized binary the live script drives (release, like a real user).
_live-build:
    cargo build --release --locked --bin llmlint

# The full stack end to end. Fails if the harness CLI + auth aren't set up.
live-claude: _live-build
    bash scripts/live-claude.sh

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

# Engine micro-benchmarks (Criterion); saves the `current` baseline for bench-compare.
bench:
    cargo bench --locked --bench engine -- --save-baseline current

# Save current engine benchmarks as the `base` baseline (run on the comparison point).
bench-base:
    cargo bench --locked --bench engine -- --save-baseline base

# Diff the latest `bench` run against `base` (run `bench-base` first; needs critcmp).
bench-compare:
    critcmp base current

# End-to-end CLI latency for every command (hyperfine); writes target/bench/results.*.
bench-cli:
    @bash scripts/bench.sh

# Fast smoke check of the CLI benchmark harness (one run, no warmup, no stable numbers).
bench-cli-smoke:
    @bash scripts/bench.sh --dry-run

# Deterministic engine allocation counts (counting allocator; exact, comparable across commits).
bench-allocs:
    cargo bench --locked --quiet --bench engine_allocs

# Deterministic end-to-end CLI instruction counts (cachegrind; Linux-only, needs valgrind).
bench-instructions:
    @bash scripts/bench-instructions.sh

# Run the portable benchmark layers (Criterion + hyperfine + allocation counts).
bench-all: bench bench-cli bench-allocs

# Record a sampling/callgrind profile to find bottlenecks; see scripts/profile.sh for modes.
profile *ARGS:
    @bash scripts/profile.sh {{ARGS}}

# --- Terminal screenshots (informational; never part of `check` or CI's gate) -
# Deterministic SVGs of the real CLI output, rendered by `freeze` from a vendored
# pinned font, gated/galleried/PR-commented by screencomp (see screenshots/AGENTS.md).
# Regenerating is out of the gate, like the benches; CI's Visual-docs workflow owns
# the comparison, and the pre-push guard regenerates the baseline locally on drift.

# Install the pinned screenshot renderer (`freeze`) on demand. Needs Go.
screenshots-tools:
    @command -v go >/dev/null || { echo "go not found: needed to install freeze; see https://go.dev/dl" >&2; exit 1; }
    go install github.com/charmbracelet/freeze@v{{freeze-version}}
    @echo "installed freeze to $(go env GOPATH)/bin (ensure it is on PATH)"

# Capture the screenshots: drive the real binary against the mock fixture, render
# each scene to shots/current/<arch>/ + docs/screenshots/. Needs `freeze` on PATH.
screenshots:
    @bash scripts/screenshots.sh

# Refresh the committed baseline manifest from a fresh capture (after an intended
# output change). Commit shots/baseline/*.json + docs/screenshots/ alongside.
screenshots-bless: screenshots
    @command -v screencomp >/dev/null || { echo "screencomp not installed: https://github.com/nickderobertis/screencomp#install" >&2; exit 1; }
    screencomp manifest --input shots/current --output shots/baseline/$(uname -m | sed 's/amd64/x86_64/;s/aarch64/arm64/').json
    @echo "baseline refreshed; commit shots/baseline/ + docs/screenshots/"
