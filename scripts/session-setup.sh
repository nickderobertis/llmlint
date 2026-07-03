#!/usr/bin/env bash
# Claude Code SessionStart hook: a fast, NON-BLOCKING dev-environment check that
# auto-provisions when the environment is not ready.
#
# It must never run the install in the foreground. Provisioning takes minutes (a
# rustup toolchain download plus cargo-tool compiles), and doing that
# synchronously inside a SessionStart hook freezes the session until it finishes
# — the session waits on the hook. So when the environment is not ready this
# launches `just setup` DETACHED in the background and returns immediately; the
# session is never blocked and tools appear within a few minutes. Stdout is
# injected as session context, so a ready environment stays silent.
#
# Escape hatches (for CI or anyone who wants the old behavior):
#   LLMLINT_SKIP_SETUP=1     — do nothing (no provisioning, no advice).
#   LLMLINT_NO_AUTO_SETUP=1  — advise running `just setup` instead of launching
#                              it; the agent provisions as a visible first step.
set -eu

# Skip in CI (the runner provisions tools its own way) and offer an escape hatch
# for any other automated context.
[ -n "${GITHUB_ACTIONS:-}" ] && exit 0
[ -n "${LLMLINT_SKIP_SETUP:-}" ] && exit 0

ROOT="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
_load_tool_env

launcher="nohup"
command -v setsid >/dev/null 2>&1 && launcher="setsid"

# Self-usage: make the *released* llmlint binary available for dogfooding
# (`llmlint lint-config`, linting this repo's own configs) without waiting the
# minutes a source build takes. `pip install llmlint-cli` ships the prebuilt
# binary as a wheel from PyPI — reachable even where github.com is blocked — in
# seconds. Installed into a dedicated venv (immune to PEP 668 externally-managed
# system Pythons) and symlinked onto PATH, in the BACKGROUND like the main
# setup, with its own lock + log. Quiet no-op when llmlint is already present.
ensure_llmlint() {
  command -v llmlint >/dev/null 2>&1 && return 0
  command -v python3 >/dev/null 2>&1 || return 0
  mkdir -p .dev
  "$launcher" bash -c '
    exec 9>.dev/llmlint-pip.lock; flock -n 9 || exit 0
    python3 -m venv .dev/llmlint-venv
    .dev/llmlint-venv/bin/pip install --quiet llmlint-cli
    for d in /usr/local/bin "$HOME/.local/bin"; do
      mkdir -p "$d" 2>/dev/null || continue
      [ -w "$d" ] || continue
      ln -sf "$PWD/.dev/llmlint-venv/bin/llmlint" "$d/llmlint" && exit 0
    done
    echo "no writable PATH dir for the llmlint symlink" >&2; exit 1
  ' >.dev/llmlint-pip.log 2>&1 </dev/null &
  printf '%s\n' \
    "[llmlint] Installing the released llmlint binary in the background (pip: llmlint-cli," \
    "log: .dev/llmlint-pip.log) so 'llmlint' is usable without a source build."
}

# Ready -> ensure the released binary, stay otherwise silent and cheap.
if _check_ready; then
  ensure_llmlint
  exit 0
fi

# Opt-out: advise instead of provisioning. Do NOT block on a multi-minute install.
if [ -n "${LLMLINT_NO_AUTO_SETUP:-}" ]; then
  printf '%s\n' \
    "[llmlint] Dev environment not set up yet (${REASON})." \
    "ACTION: run 'just setup' (or './scripts/setup.sh' if just is missing) as your FIRST step," \
    "before building or testing. It installs the pinned Rust toolchain, just, and the cargo dev" \
    "tools (cargo-nextest, cargo-llvm-cov), then fetches deps via 'just bootstrap' — a few minutes" \
    "on a fresh machine. Verify anytime with 'just setup-check'." \
    "Also: 'pip install llmlint-cli' provides the released llmlint binary for self-linting."
  exit 0
fi

# Default: provision hands-off, but DETACHED so the session is never blocked. A
# flock keeps two concurrent sessions from launching setup twice; the lock is
# held by the background job for its whole run, not by this returning hook.
mkdir -p .dev
"$launcher" bash -c 'exec 9>.dev/setup.lock; flock -n 9 || exit 0; exec bash scripts/setup.sh' \
  >.dev/setup.log 2>&1 </dev/null &
ensure_llmlint
printf '%s\n' \
  "[llmlint] Dev environment not ready (${REASON}); provisioning in the BACKGROUND" \
  "(log: .dev/setup.log). It does not block this session. Tools appear within a few minutes:" \
  "verify with 'just setup-check'. Set LLMLINT_NO_AUTO_SETUP=1 to advise instead."
exit 0
