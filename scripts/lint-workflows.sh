#!/usr/bin/env bash
# Lint every GitHub Actions workflow in .github/workflows with the pinned
# actionlint (`actionlint-version` in the justfile). Part of `just check`, so the
# whole workflow set is checked on every change. Quiet on success; on a finding
# actionlint prints it (file:line:col + rule) and this exits non-zero.
#
# actionlint also runs shellcheck over `run:` scripts when shellcheck is on PATH
# (CI's ubuntu runners ship it), so install shellcheck locally to see the same
# findings CI does.
# llmlint: ignore-file[new_code_lands_in_a_project] a single binary crate with no Nx project graph (AGENTS.md) has no project for a shell script to belong to
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/setup-lib.sh
. scripts/setup-lib.sh
# `just setup` installs into ~/.local/bin, which a non-login shell may not have.
_load_tool_env

want="$(_justfile_pin actionlint)"
if ! command -v actionlint >/dev/null 2>&1; then
  echo "lint-workflows: actionlint not found on PATH" >&2
  echo "                install the pinned release ($want): just actionlint-tools" >&2
  echo "                (or, without just: bash scripts/install-actionlint.sh)" >&2
  exit 1
fi
have="$(actionlint -version 2>/dev/null | head -n1 || true)"
if [ "$have" != "$want" ]; then
  echo "lint-workflows: $(command -v actionlint) is actionlint ${have:-<unknown version>}," >&2
  echo "                but the justfile pins $want" >&2
  echo "                install the pinned release: just actionlint-tools" >&2
  echo "                (or, without just: bash scripts/install-actionlint.sh)" >&2
  exit 1
fi

actionlint
