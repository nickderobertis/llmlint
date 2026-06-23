#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real claude-code harness.
# Fails (red build) if the `claude` CLI or its auth is absent — this tier expects
# the harness configured (CI), so a missing prerequisite is a broken setup.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need claude
need_env "Claude auth" CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_API_KEY
# Cheap, valid default; override with CLAUDE_E2E_MODEL.
LL_MODEL="${CLAUDE_E2E_MODEL:-haiku}"

live_run_journeys claude-code
