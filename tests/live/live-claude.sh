#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real claude-code harness.
# Fails (red build) if the `claude` CLI or its auth is absent — this tier expects
# the harness configured (CI), so a missing prerequisite is a broken setup.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" \
  || { echo "live-claude: cannot resolve this script's directory" >&2; exit 1; }
# shellcheck source=tests/live/live-lib.sh
source "$DIR/live-lib.sh" || { echo "live-claude: cannot load $DIR/live-lib.sh" >&2; exit 1; }

need claude
need_env "Claude auth" CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_API_KEY
# Cheap, valid default; override with CLAUDE_E2E_MODEL.
LL_MODEL="${CLAUDE_E2E_MODEL:-haiku}"

live_run_journeys claude-code
