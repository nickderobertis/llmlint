#!/usr/bin/env bash
# Print this host's screencomp capture lane: the normalized CPU architecture that
# names shots/current/<arch>/ and shots/baseline/<arch>.json.
#
# One place, so the three consumers can never disagree about what a lane is
# called: the capture (scripts/screenshots.sh), the local pre-push guard
# (.githooks/pre-push), and `just screenshots-bless`. The names match
# [capture].arches in screencomp.toml, which is what screencomp fans CI out over.
# llmlint: ignore-file[new_code_lands_in_a_project] llmlint is deliberately a single binary crate with no monorepo and no Nx project graph (AGENTS.md, "Stack and composition"), so there is no project definition for a shell helper to land in; its owning surface is the justfile + the capture/guard/bless callers, and the pre-push journeys in tests/e2e/main.rs exercise it through the real hook
set -euo pipefail

arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="arm64" ;;
esac
printf '%s\n' "$arch"
