#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real crush harness.
# Skips (never fails) when the `crush` CLI or its auth is absent.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need crush
need_env "OpenAI/Anthropic auth" ANTHROPIC_API_KEY OPENAI_API_KEY
LL_MODEL="${CRUSH_E2E_MODEL:-}"

live_run_journeys crush
