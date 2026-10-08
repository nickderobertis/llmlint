#!/usr/bin/env bash
# The shell toolchain every project that owns shell scripts runs through:
#
#   shell.sh format [--write] FILE...   shfmt -d (check) or -w (write), the
#                                       style .editorconfig records
#   shell.sh lint FILE...               shellcheck
#   shell.sh files                      every shell script in the tree
#   shell.sh versions                   the pins and the tools found (Nx's cache
#                                       key for the lint targets)
#
# Each project's `format` and `lint` targets call this beside cargo fmt and
# clippy. The versions are the justfile's `shfmt-version` and
# `shellcheck-version` — the one pin, which tools/shell/install-shell-tools.sh
# installs into ~/.local/bin — and a tool off the pin is refused rather than
# allowed to format or lint to a different standard.
#
# LLMLINT_SHELL_TOOLS=off (`just check-portable`, the macOS/Windows `cross` jobs)
# makes format and lint a no-op with a notice: shell formatting and lint do not
# depend on the platform, and the Linux gate enforces them.
#
# A shell script is a tracked or unignored file ending in .sh or .bash, or one
# whose first line is a sh/bash/dash/ksh/zsh shebang.
#
# Exit status: 0 clean; 1 a finding, or a tool missing or off its pin; 2 a usage
# error.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
  || {
    echo "shell: cannot resolve the repository root from ${BASH_SOURCE[0]}; run it from an intact checkout." >&2
    exit 1
  }
readonly ROOT
readonly LOCAL_BIN="$HOME/.local/bin"

usage() {
  echo "shell: $1" >&2
  echo "usage: tools/shell/shell.sh format [--write] FILE... | lint FILE... | files | versions" >&2
  exit 2
}

# The `<name>-version := "x.y.z"` pin in the justfile, or empty.
pin() {
  { grep -E "^$1-version :=" "$ROOT/justfile" 2>/dev/null || true; } | head -n1 | cut -d'"' -f2
}

# The version a tool reports, or empty when it does not run.
have_version() {
  case "$1" in
    shfmt) { "$1" --version 2>/dev/null || true; } | head -n1 | sed 's/^v//' ;;
    shellcheck) { "$1" --version 2>/dev/null || true; } | sed -n 's/^version: //p' ;;
  esac
}

# Refuse a tool that is missing or off its pin, naming the fix.
require() {
  local tool="$1" want have
  want="$(pin "$tool")"
  if [ -z "$want" ]; then
    echo "shell: no $tool-version pin in $ROOT/justfile; restore the line: $tool-version := \"<version>\"" >&2
    exit 1
  fi
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "shell: $tool not found on PATH; install the pinned $tool $want: just shell-tools" >&2
    exit 1
  fi
  have="$(have_version "$tool")"
  if [ "$have" != "$want" ]; then
    echo "shell: $(command -v "$tool") is $tool ${have:-<unknown version>}, but the justfile pins $want." >&2
    echo "       Run just shell-tools (it installs the pin into $LOCAL_BIN, which this script puts first on PATH)." >&2
    exit 1
  fi
}

# Whether format and lint run (on) or are left to the Linux gate (off).
enabled() {
  case "${LLMLINT_SHELL_TOOLS:-on}" in
    on) return 0 ;;
    off) return 1 ;;
    *) usage "LLMLINT_SHELL_TOOLS must be 'on' (the default) or 'off' (got '${LLMLINT_SHELL_TOOLS}')" ;;
  esac
}

[ "$#" -ge 1 ] || usage "no step given"
readonly STEP="$1"
shift
PATH="$LOCAL_BIN:$PATH"

case "$STEP" in
  files)
    [ "$#" -eq 0 ] || usage "'files' takes no arguments"
    cd "$ROOT"
    git ls-files -z --cached --others --exclude-standard | while IFS= read -r -d '' f; do
      [ -f "$f" ] || continue
      case "$f" in
        *.sh | *.bash) printf '%s\n' "$f" ;;
        *)
          first=""
          IFS= read -r first <"$f" 2>/dev/null || true
          if [[ $first =~ ^#!.*[/\ ](ba|da|k|z)?sh(\ |$) ]]; then
            printf '%s\n' "$f"
          fi
          ;;
      esac
    done | LC_ALL=C sort
    ;;

  versions)
    [ "$#" -eq 0 ] || usage "'versions' takes no arguments"
    for tool in shfmt shellcheck; do
      printf '%s pin=%s have=%s\n' "$tool" "$(pin "$tool")" "$(have_version "$tool")"
    done
    printf 'LLMLINT_SHELL_TOOLS=%s\n' "${LLMLINT_SHELL_TOOLS:-on}"
    ;;

  format)
    write=false
    if [ "${1:-}" = "--write" ]; then
      write=true
      shift
    fi
    [ "$#" -ge 1 ] || usage "'format' needs at least one file"
    if ! enabled; then
      echo "shell: LLMLINT_SHELL_TOOLS=off — shfmt skipped; the Linux gate enforces it." >&2
      exit 0
    fi
    require shfmt
    if "$write"; then
      shfmt -w "$@"
    elif ! shfmt -d "$@" >&2; then
      echo "shell: the files above are not formatted to the .editorconfig style; write it with: just format" >&2
      exit 1
    fi
    ;;

  lint)
    [ "$#" -ge 1 ] || usage "'lint' needs at least one file"
    if ! enabled; then
      echo "shell: LLMLINT_SHELL_TOOLS=off — shellcheck skipped; the Linux gate enforces it." >&2
      exit 0
    fi
    require shellcheck
    if ! shellcheck "$@" >&2; then
      echo "shell: fix each finding above at its file:line, or disable it at that site with its reason (# shellcheck disable=SCxxxx  # why)." >&2
      exit 1
    fi
    ;;

  *) usage "unknown step '$STEP'" ;;
esac
