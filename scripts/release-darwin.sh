#!/usr/bin/env bash
# The one Apple target, built and run natively on the Apple Silicon release runner.
# Intel macOS (x86_64-apple-darwin) is no longer released.
# llmlint: ignore[code_lands_in_the_domain_that_owns_it] Production release builders stay in scripts, their existing owning domain; release-targets owns their artifact contract and on-demand journeys.
set -euo pipefail
out="${1:?usage: bash scripts/release-darwin.sh OUT}"
bash scripts/release-packages.sh "$out" aarch64-apple-darwin || {
  echo "release-darwin: Darwin packaging into $out failed; fix the release-packages diagnostic above and rerun" >&2
  exit 1
}
