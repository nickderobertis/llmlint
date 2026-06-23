# Shared helpers for the local-setup scripts (setup.sh, setup-check.sh) and the
# session hook (session-setup.sh). Sourced, not executed: callers set their own
# `set -eu`. All functions assume the current directory is the repo root.
#
# llmlint deliberately does NOT use asdf/direnv (see AGENTS.md). The dev
# environment is: rustup + the pinned rust-toolchain.toml, `just`, and the two
# cargo subcommands the gate drives (`cargo nextest`, `cargo llvm-cov`).

# Binaries that must resolve for the dev environment to be considered ready: the
# Rust toolchain, the task runner, and the test/coverage subcommands `just test`
# (hence `just check`) drives. cargo-deny/cargo-machete are NOT here — they back
# `just deps-check`, which is separate from the gate and needs a network DB.
REQUIRED_BINS="rustc cargo just cargo-nextest cargo-llvm-cov"

# Soft requirements: their absence is an advisory, never a "not ready" verdict.
# oneharness is a *runtime* prerequisite (the harness llmlint shells out to), not
# a build/test input — the e2e suite drives a mock fixture, so the gate passes
# without it. `just doctor` reports it; setup never installs it.
OPTIONAL_BINS="oneharness"

# Where `just` is installed by setup.sh when it is missing (the official
# installer's default target we pass via --to).
LOCAL_BIN="$HOME/.local/bin"

# Machine-local setup state. Lives outside target/ so `cargo clean` does not wipe
# it (cleaning build artifacts does not un-provision the machine). Git-ignored.
STAMP=".dev/setup.stamp"

# Put the installed toolchains on PATH for this process. A non-interactive shell
# (and some hook contexts) does not source the user's rc, so cargo/just binaries
# may be installed yet unresolved; this normalises that without a fresh login.
# Idempotent and safe when nothing is installed.
_load_tool_env() {
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  local dir
  for dir in "$HOME/.cargo/bin" "$LOCAL_BIN"; do
    [ -d "$dir" ] || continue
    case ":$PATH:" in
      *":$dir:"*) : ;;
      *) PATH="$dir:$PATH"; export PATH ;;
    esac
  done
}

# The pinned version of a `<name>-version := "x.y.z"` variable in the justfile,
# or empty if absent. Single source of truth shared by setup.sh (what to install)
# and _fingerprint (what to re-trigger on).
_justfile_pin() {
  grep -E "^$1-version :=" justfile 2>/dev/null | head -n1 | cut -d'"' -f2
}

# SHA-256 of stdin using whatever tool is available; a stable sentinel if none is
# (so the stamp comparison still works, falling back to binary-presence only).
_sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 | awk '{print $NF}'
  else
    printf 'no-sha256-tool\n'
  fi
}

# Fingerprint of the inputs setup depends on: the pinned Rust toolchain, the asdf
# tool versions (`just`), and the dev-tool version pins in the justfile. A change
# to any of these invalidates the stamp so setup re-runs (e.g. after `just
# upgrade` or a toolchain bump).
_fingerprint() {
  {
    [ -f rust-toolchain.toml ] && cat rust-toolchain.toml
    [ -f .tool-versions ] && cat .tool-versions
    [ -f justfile ] && grep -E '^[a-z][a-z-]*-version :=' justfile || true
  } 2>/dev/null | _sha256_stdin
}

# Echo the subset of $1 (a space-separated list of binary names) that does not
# resolve on PATH, each prefixed with a space; empty when all resolve.
_missing_bins() {
  local b out=""
  for b in $1; do
    command -v "$b" >/dev/null 2>&1 || out="$out $b"
  done
  printf '%s' "$out"
}

# Is the dev environment ready? Returns 0 when every required binary resolves and
# the stamp matches the current fingerprint; otherwise returns 1 and sets REASON.
# Soft requirements (OPTIONAL_BINS) do not affect readiness; surface them with
# _missing_bins where an advisory is wanted.
_check_ready() {
  REASON=""
  local missing
  missing="$(_missing_bins "$REQUIRED_BINS")"
  if [ -n "$missing" ]; then
    REASON="missing tools:$missing"
    return 1
  fi
  local want have_fp
  want="$(_fingerprint)"
  have_fp="$(cat "$STAMP" 2>/dev/null || true)"
  if [ -z "$have_fp" ]; then
    REASON="no setup stamp (first run on this machine)"
    return 1
  fi
  if [ "$want" != "$have_fp" ]; then
    REASON="toolchain or tool versions changed since last setup"
    return 1
  fi
  return 0
}

# Record the current fingerprint as the stamp of a successful setup.
_write_stamp() {
  mkdir -p "$(dirname "$STAMP")"
  _fingerprint > "$STAMP"
}
