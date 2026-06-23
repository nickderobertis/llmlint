#!/usr/bin/env bash
# llmlint local setup — make a fresh machine ready to run the quality gate.
#
# Idempotent and safe to re-run. It:
#   1. ensures rustup + the pinned toolchain — rust-toolchain.toml stays the
#      source of truth; rustup just realises it,
#   2. ensures `just` (the task runner), pinned to .tool-versions, via the
#      official prebuilt installer (no slow `cargo install just` compile),
#   3. ensures the cargo subcommands the gate drives — cargo-nextest and
#      cargo-llvm-cov — pinned to the justfile,
#   4. fetches dependencies + adds toolchain components via `just bootstrap`,
#   5. records a setup stamp for the fast session check (scripts/setup-check.sh).
#
# It does NOT install oneharness (a separate *runtime* prerequisite) or
# cargo-deny/cargo-machete (only `just deps-check` needs those, and that needs a
# network advisory DB) — `just doctor` / `just deps-check` report those.
#
# Fresh machine (no `just` yet):  ./scripts/setup.sh
# Once `just` is available:        just setup
set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
_load_tool_env

say()  { printf '» %s\n' "$*"; }
ok()   { printf '✓ %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

ensure_rust() {
  if ! have rustup; then
    say "installing rustup (minimal); rust-toolchain.toml drives the toolchain"
    have curl || { printf 'error: curl is required to install rustup\n' >&2; exit 1; }
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --no-modify-path
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  fi
  # Realise the pinned channel + components declared in rust-toolchain.toml.
  say "resolving the pinned toolchain (rustup show)"
  rustup show >/dev/null
  ok "rust toolchain ready ($(rustc --version 2>/dev/null || echo unknown))"
}

ensure_just() {
  if have just; then
    ok "just present ($(just --version 2>/dev/null || echo unknown))"
    return
  fi
  local ver
  ver="$(grep -E '^just[[:space:]]' .tool-versions 2>/dev/null | awk '{print $2}')"
  have curl || { printf 'error: curl is required to install just\n' >&2; exit 1; }
  mkdir -p "$LOCAL_BIN"
  if [ -n "$ver" ]; then
    say "installing just $ver into $LOCAL_BIN (pinned by .tool-versions)"
    curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
      | bash -s -- --tag "$ver" --to "$LOCAL_BIN"
  else
    say "installing latest just into $LOCAL_BIN (.tool-versions has no pin)"
    curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
      | bash -s -- --to "$LOCAL_BIN"
  fi
  _load_tool_env
  ok "just installed ($(just --version 2>/dev/null || echo unknown))"
}

# Install a pinned cargo subcommand only when its binary is missing, so an
# already-provisioned machine (and CI, where the install action pre-installs the
# latest) is a no-op. The justfile holds the pin; empty pin => install latest.
ensure_cargo_tool() {
  local bin="$1" crate="$2" pin="$3"
  if have "$bin"; then
    ok "$bin present"
    return
  fi
  if [ -n "$pin" ]; then
    say "installing $crate $pin (cargo install --locked)"
    cargo install "$crate" --locked --version "$pin"
  else
    say "installing $crate latest (cargo install --locked)"
    cargo install "$crate" --locked
  fi
  ok "$bin installed"
}

main() {
  ensure_rust
  ensure_just
  ensure_cargo_tool cargo-nextest  cargo-nextest "$(_justfile_pin nextest)"
  ensure_cargo_tool cargo-llvm-cov cargo-llvm-cov "$(_justfile_pin llvmcov)"
  say "fetching dependencies + toolchain components (just bootstrap)"
  just bootstrap
  _write_stamp
  rm -f .dev/setup.failed
  ok "setup complete — stamp written to ${STAMP}"
  if [ -n "$(_missing_bins "$OPTIONAL_BINS")" ]; then
    printf '\nnote: oneharness is not on PATH. It is a runtime prerequisite for\n'
    printf 'live runs (`just lint-live`); the gate (`just check`) drives a mock and\n'
    printf 'does not need it. See `just doctor`.\n'
  fi
}

main "$@"
