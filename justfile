# Canonical command surface for llmlint.
#
# `just bootstrap` must work from a clean clone; `just check` is the full quality
# gate and fails on any issue (no warnings-only mode). Recipes are quiet on
# success and specific on failure.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Feature that builds the mock-oneharness fixture the e2e tests drive.
FEATURES := "mock-oneharness"

# List available recipes.
default:
    @just --list

# Set up from a clean clone: toolchain components (from rust-toolchain.toml) +
# fetched dependencies.
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

# Verbose, install-free diagnostics (kept out of the gate).
doctor:
    rustc --version
    cargo --version
    oneharness --version || echo "oneharness not installed (it is a runtime prerequisite)"

# Run the CLI through cargo, e.g. `just run -- --help`.
run *ARGS:
    cargo run --quiet -- {{ARGS}}
