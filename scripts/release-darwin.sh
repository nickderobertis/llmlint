#!/usr/bin/env bash
# Both Apple targets share one Apple Silicon runner; x86_64 products run through Rosetta.
# llmlint: ignore[code_lands_in_the_domain_that_owns_it] Production release builders stay in scripts, their existing owning domain; release-targets owns their artifact contract and on-demand journeys.
set -euo pipefail
out="${1:?usage: bash scripts/release-darwin.sh OUT}"
bash scripts/release-packages.sh "$out" x86_64-apple-darwin aarch64-apple-darwin || {
  echo "release-darwin: Darwin packaging into $out failed; fix the release-packages diagnostic above and rerun" >&2
  exit 1
}
