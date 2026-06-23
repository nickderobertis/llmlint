#!/usr/bin/env bash
# Live e2e: real llmlint -> real oneharness -> real goose harness.
# Skips (never fails) when the `goose` CLI or its auth is absent.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/live-lib.sh
source "$DIR/live-lib.sh"

need goose
need_env "OpenAI auth" OPENAI_API_KEY
# goose selects provider/model from GOOSE_PROVIDER/GOOSE_MODEL; leave llmlint's
# model unset unless GOOSE_E2E_MODEL is given.
LL_MODEL="${GOOSE_E2E_MODEL:-}"

live_run_journeys goose
