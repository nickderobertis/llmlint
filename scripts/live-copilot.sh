#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real copilot harness.
# Fails (red build) if the `copilot` CLI or its auth is absent — this tier expects
# the harness configured (CI), so a missing prerequisite is a broken setup.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need copilot
need_env "Copilot auth" COPILOT_GITHUB_TOKEN
LL_MODEL="${COPILOT_E2E_MODEL:-}"

live_run_journeys copilot
