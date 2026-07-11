#!/usr/bin/env bash
# Idempotent setup for the optional `llmlint` LLM-judge tier (oneharness + llmlint).
#
# Wired into the Claude Code SessionStart hook (.claude/settings.json) so web/cloud
# sessions can run `just lint-llm` / `just lint-llm-diff` with no manual steps; also
# safe to run by hand (`just setup-llmlint`) or from a terminal. Every step
# tolerates failure and the script always exits 0 — a flaky install must never
# break session startup.
#
# What it does, and why:
#   1. Installs the `llmlint` binary from PyPI via `uv tool`. `llmlint-cli` wraps
#      the prebuilt binary and depends on `oneharness-cli`, so one dependency
#      resolution fetches both wheels — no Rust toolchain and no github.com
#      reachability (works in restricted-egress sessions where PyPI is reachable).
#      `uv tool` links only the *requested* package's executable onto PATH, but
#      llmlint >= 0.3.17 finds `oneharness` beside its own binary in the tool venv —
#      so this one install is a complete setup; no separate oneharness install /
#      PATH entry. `--upgrade` bumps an older cached tool, honouring the floor below
#      (`just lint-llm-diff` needs the changed-file-scoped `--diff` and three-dot
#      `--diff-base` default; `just lint-llm-validate` needs the `validate` gate).
#   2. In a Claude Code session, persists PATH (so the freshly installed binary
#      resolves) into CLAUDE_ENV_FILE so later Bash calls inherit it.
#
# Harness selection: the committed `oneharness.toml` is in fallback mode (codex +
# gpt-5.5 primary, claude-code + opus-4.8 secondary), so llmlint runs the primary
# for a contributor with Codex authenticated and falls through to claude-code in a
# Claude Code session where codex is absent — no `ONEHARNESS_*` override needed
# (one would only clobber the fallback list). If your fallback order can't select
# the right harness for some environment, set ONEHARNESS_HARNESSES there.
# llmlint: ignore-file[robust_shell, tool_output_is_signal, boundary_inputs_validated] deliberate for a session-startup installer (see header): `set -e` is omitted so a flaky install can't abort the hook — the script owns its exit codes and always exits 0; success stays quiet while failures log-and-continue rather than block startup; and the toolchain is installed from PyPI (`uv tool install llmlint-cli`) whose wheels ship with Trusted Publishing + PEP 740 attestations, so no unvalidated external input is executed.
set -uo pipefail

# Version floor, as a PyPI constraint (the `llmlint-cli` package version tracks the
# wrapped binary version). `uv tool install --upgrade` installs the newest release
# satisfying it; oneharness comes along transitively at a compatible version.
# llmlint >= 0.3.17 finds `oneharness` beside its own executable (so a lone
# `uv tool install llmlint-cli` works), gives the whole-tree default the composed
# llmlint.yml relies on (it omits `files.include`), restricts `--diff` to the
# changed files (skipping empty diffs) so `just lint-llm-diff` judges only the
# branch's changes, treats a plain `--diff-base <ref>` as three-dot/merge-base
# (0.3.15), and ships the deterministic `validate` gate — config structure +
# `llmlint: ignore` directives + fragment version bumps — that `just
# lint-llm-validate` runs with no model call (0.3.17).
readonly LLMLINT_MIN="0.3.17"
readonly BIN_DIR="$HOME/.local/bin"

log() { printf 'setup-llmlint: %s\n' "$*" >&2; }

# Install llmlint from PyPI via uv (the repo's Python package manager). uv is a
# clean-clone prerequisite; if it is somehow absent, log an actionable pointer and
# leave any already-installed binary in place rather than aborting startup.
ensure_toolchain() {
  if ! command -v uv >/dev/null 2>&1; then
    log "uv not found; cannot install llmlint (install uv: https://docs.astral.sh/uv/)"
    return 0
  fi
  # llmlint-cli pulls oneharness-cli as a dependency into the same tool venv, where
  # llmlint discovers the `oneharness` binary beside its own — no separate install.
  log "installing llmlint-cli >= $LLMLINT_MIN via uv tool"
  uv tool install --upgrade "llmlint-cli>=$LLMLINT_MIN" >&2 \
    || log "llmlint-cli install failed (continuing)"
}

# Persist env for the rest of the session via CLAUDE_ENV_FILE (Claude Code sources
# it into every later Bash call). PATH so the freshly installed binaries resolve.
# No-op outside a session.
persist_session_env() {
  [ -n "${CLAUDE_ENV_FILE:-}" ] || { log "no CLAUDE_ENV_FILE (not a session); skipping env"; return 0; }
  {
    case ":${PATH}:" in *":${BIN_DIR}:"*) ;; *) printf 'export PATH=%q\n' "${BIN_DIR}:${PATH}";; esac
    # No ONEHARNESS_* override: oneharness.toml's fallback mode selects the harness
    # (codex primary, claude-code secondary), so a Claude Code session — where codex
    # is absent — falls through to claude-code on its own. Set ONEHARNESS_HARNESSES
    # here only if a specific environment's fallback order can't pick correctly.
  } >> "$CLAUDE_ENV_FILE"
  log "exported PATH"
}

export PATH="${BIN_DIR}:${PATH}"
ensure_toolchain
persist_session_env
# `llmlint doctor` confirms the sibling `oneharness` is reachable (it is not on
# PATH — llmlint resolves it beside its own binary), so report via doctor.
if command -v llmlint >/dev/null 2>&1; then
  log "ready (llmlint: $(llmlint --version 2>/dev/null || echo unknown))"
  llmlint doctor >&2 2>&1 || log "llmlint doctor reported an issue (see above)"
else
  log "llmlint not installed"
fi
exit 0
