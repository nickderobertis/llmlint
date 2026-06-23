#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real qwen harness.
# Skips (never fails) when the `qwen` CLI or its auth is absent.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need qwen
need_env "OpenAI auth" OPENAI_API_KEY
LL_MODEL="${QWEN_E2E_MODEL:-}"

live_run_journeys qwen
