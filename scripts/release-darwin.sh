#!/usr/bin/env bash
# Both Apple targets share one Apple Silicon runner; x86_64 products run through Rosetta.
# llmlint: ignore[code_lands_in_the_domain_that_owns_it] Production release builders stay in scripts, their existing owning domain; release-targets owns their artifact contract and on-demand journeys.
set -euo pipefail
bash scripts/release-packages.sh "${1:?usage: bash scripts/release-darwin.sh OUT}" x86_64-apple-darwin aarch64-apple-darwin
