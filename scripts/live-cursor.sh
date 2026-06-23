#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real cursor harness.
# Skips (never fails) when the `cursor-agent` CLI or its auth is absent.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need cursor-agent
need_env "Cursor auth" CURSOR_API_KEY
LL_MODEL="${CURSOR_E2E_MODEL:-}"

live_run_journeys cursor
