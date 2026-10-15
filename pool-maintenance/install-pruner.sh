#!/usr/bin/env bash
# Installs cargo-prune-superseded, the pruner the justfile's `pool-maintenance`
# recipe runs, at the justfile's pin: `just bootstrap` calls it with
# prune-superseded-version and prune-superseded-rust. Nothing is installed when
# that version is already the one on PATH, so a second bootstrap reinstalls
# nothing. The pruner needs a newer Rust than rust-toolchain.toml pins, so it is
# built with its own toolchain, installed minimal.
#
#   install-pruner.sh <version> <rust-toolchain>
#
# Exit status: 0 the pinned version is on PATH; 1 the install failed, or
# succeeded but PATH still resolves another version (the message says which);
# 2 a usage error.
set -euo pipefail

usage() {
  echo "install-pruner: $*; usage: install-pruner.sh <version> <rust-toolchain> (just bootstrap passes the justfile's pins)" >&2
  exit 2
}

fail() {
  echo "install-pruner: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || usage "expected 2 arguments, got $#"
readonly VERSION="$1" RUST="$2"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage "version '$VERSION' is not MAJOR.MINOR.PATCH"
[[ "$RUST" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] || usage "rust toolchain '$RUST' is not a numbered release"

readonly WANT="cargo-prune-superseded $VERSION"
have() { cargo-prune-superseded --version 2>/dev/null || true; }
[ "$(have)" = "$WANT" ] && exit 0

for tool in rustup cargo; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool not found on PATH; run just setup."
done
rustup toolchain install --profile minimal "$RUST" \
  || fail "installing Rust $RUST failed (above); check the network, then re-run just bootstrap."
cargo "+$RUST" install --locked cargo-prune-superseded --version "$VERSION" \
  || fail "cargo install of cargo-prune-superseded $VERSION failed (above); check the network, then re-run just bootstrap."
now="$(have)"
[ "$now" = "$WANT" ] \
  || fail "installed $VERSION, but PATH resolves $(command -v cargo-prune-superseded || echo 'no cargo-prune-superseded') ('${now:-<unknown version>}'); put cargo's bin directory (~/.cargo/bin) first on PATH."
