#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real opencode harness.
# Fails (red build) if the `opencode` CLI or its auth is absent — this tier expects
# the harness configured (CI), so a missing prerequisite is a broken setup.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need opencode
need_env "OpenAI/Anthropic auth" ANTHROPIC_API_KEY OPENAI_API_KEY
LL_MODEL="${OPENCODE_E2E_MODEL:-}"

live_run_journeys opencode
