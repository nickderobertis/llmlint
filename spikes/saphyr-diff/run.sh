#!/usr/bin/env bash
# SPIKE (plan authoring:serde-yaml-replacement) — never lands.
#
# One command for the whole differential measurement:
#   1. runs the e2e suite once with SPIKE_E2E_DUMP set, so every YAML document a
#      journey writes is captured (tests/e2e/main.rs `spike_fs_write`);
#   2. runs the harness over this repository's configs, those captured
#      documents, the fleet's real llmlint.yml files (read only), the synthetic
#      feature/error cases, the plugin VersionProbe, the version text and the
#      workflow walks, then times the largest real documents;
#   3. checks what serde-saphyr and saphyr add against deny.toml, and that
#      neither pulls a libyaml-derived crate.
#
# Usage: bash spikes/saphyr-diff/run.sh            (from the repository root)
#        SPIKE_E2E_DUMP=<dir> bash spikes/saphyr-diff/run.sh   reuses a capture
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

dump="${SPIKE_E2E_DUMP:-}"
if [ -z "$dump" ] || [ ! -d "$dump" ]; then
  # Inside the (gitignored) target dir, so nothing is written outside the tree.
  dump="${dump:-$root/target/saphyr-diff/e2e-dump}"
  rm -rf "$dump"
  mkdir -p "$dump"
  echo "## capturing the e2e suite's YAML documents into $dump" >&2
  cargo build -q -p llmlint -p llmlint-mock-oneharness
  log="$(dirname "$dump")/e2e.log"
  if ! SPIKE_E2E_DUMP="$dump" cargo nextest run -p llmlint-e2e --no-fail-fast >"$log" 2>&1; then
    echo "e2e suite failed; see $log" >&2
    exit 1
  fi
fi

echo "## harness" >&2
cargo run -q --release --manifest-path spikes/saphyr-diff/Cargo.toml -- "$root" "$dump" --time

echo
echo "=== dependencies serde-saphyr 1.3.0 + saphyr 0.2.1 add (spikes/saphyr-deps) ==="
deps=spikes/saphyr-deps/Cargo.toml
banned='^(serde_yaml|serde_yaml_ng|serde_norway|serde_yml|unsafe-libyaml|unsafe-libyaml-norway|libyml) '
if cargo tree --manifest-path "$deps" -e normal,build --prefix none | grep -E "$banned"; then
  echo "DEPS banned YAML crate present (above)"
else
  echo "DEPS none of serde_yaml, serde_yaml_ng, serde_norway, serde_yml, unsafe-libyaml, unsafe-libyaml-norway, libyml in the graph"
fi
echo "DEPS crates in the graph:"
cargo tree --manifest-path "$deps" -e normal,build --prefix none | sed 's/ (.*//' | sort -u | grep -v '^saphyr-deps ' | sed 's/^/  /'
echo "DEPS cargo deny check (this repository's deny.toml):"
cargo deny --manifest-path "$deps" check --config deny.toml 2>&1 | grep -E '^(error|advisories|.*FAILED)' | sed 's/^/  /'
