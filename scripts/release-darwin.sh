#!/usr/bin/env bash
# Both Apple targets share one Apple Silicon runner; x86_64 products run through Rosetta.
set -euo pipefail
bash scripts/release-packages.sh "${1:?usage: bash scripts/release-darwin.sh OUT}" x86_64-apple-darwin aarch64-apple-darwin
