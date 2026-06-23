#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real codex harness.
# Fails (red build) if the `codex` CLI or its auth is absent — this tier expects
# the harness configured (CI), so a missing prerequisite is a broken setup.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need codex
need_env "OpenAI auth" OPENAI_API_KEY
# Omit unless overridden; codex picks its own default model.
LL_MODEL="${CODEX_E2E_MODEL:-}"

live_run_journeys codex
