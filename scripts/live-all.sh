#!/usr/bin/env bash
# Run every per-harness live e2e script in turn. Each one SKIPs (exit 0) when its
# CLI/auth is absent and FAILs (exit non-zero) only on a real regression, so this
# aggregator surfaces just the genuine failures. Mirrors oneharness's `live-all`.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

harnesses=(claude codex opencode goose qwen crush copilot cursor)

ran=0
failed=0
failed_names=()
for h in "${harnesses[@]}"; do
    printf '\n=== live-%s ===\n' "$h" >&2
    ran=$((ran + 1))
    if ! bash "$DIR/live-$h.sh"; then
        failed=$((failed + 1))
        failed_names+=("$h")
    fi
done

printf '\nlive-all: ran %d harness scripts, %d failed' "$ran" "$failed" >&2
if [ "$failed" -ne 0 ]; then
    printf ' (%s)\n' "${failed_names[*]}" >&2
    exit 1
fi
printf '\n' >&2
