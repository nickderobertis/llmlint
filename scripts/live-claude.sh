#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real claude-code harness.
# Skips (never fails) when the `claude` CLI or its auth is absent.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need claude
need_env "Claude auth" CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_API_KEY
# Cheap, valid default; override with CLAUDE_E2E_MODEL.
LL_MODEL="${CLAUDE_E2E_MODEL:-haiku}"

live_run_journeys claude-code
