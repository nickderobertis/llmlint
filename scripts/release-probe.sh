#!/usr/bin/env bash
# llmlint: ignore-file[new_code_lands_in_a_project] a single binary crate with no Nx project graph (AGENTS.md) has no project for a shell script to belong to
# What does the public registry serve, right now, for ONE release target of this
# repository? The targets are declared in `release-targets.toml`, which names this
# script as its `probe`.
#
#   scripts/release-probe.sh crate:llmlint      -> 0.4.2   (exit 0)
#   scripts/release-probe.sh pypi:llmlint-cli   -> 0.4.2   (exit 0)
#
# Exactly three answers, and a caller must be able to tell them apart:
#
#   * exit 0, one line on stdout   — the version that registry currently serves;
#   * exit 0, empty stdout         — that registry has no release of it yet;
#   * non-zero, reason on stderr   — NOT ANSWERED, stdout empty.
#
# "Not answered" and "no release yet" are different answers all the way out. A
# caller holds indefinitely on the first and must never read it as evidence that
# a release has not happened — collapsing the two launches dependent work whose
# dependency never landed. Any identifier other than the two declared ids (no
# registry qualification, an unsupported registry, a malformed name, or a
# well-formed id for a package this repository does not publish) is therefore
# NOT ANSWERED, never empty output.
#
# It assumes nothing beyond PATH (curl, mktemp, python3) and HOME: spawned as a
# direct subprocess from the repository root, with no credential of any kind.
# Both targets are on public registries, so an unauthenticated read is all it
# needs; curl runs with `-q` so no ~/.curlrc can add a header or a netrc lookup,
# and nothing from the environment is ever put on the request. Each answer is
# bounded well inside sixty seconds.
#
# LLMLINT_RELEASE_PROBE_CRATES_URL / LLMLINT_RELEASE_PROBE_PYPI_URL replace the
# registry base URLs (defaults https://crates.io, https://pypi.org) so the tests
# can point the real script at a local stand-in registry.
set -euo pipefail

readonly UA="llmlint-release-probe (https://github.com/nickderobertis/llmlint)"
# Worst case: two attempts of at most 12s each plus a 1s backoff — ~25s.
readonly CONNECT_TIME=5
readonly MAX_TIME=12
readonly RETRIES=1

# Not answered: reason on stderr, nothing on stdout, non-zero exit.
unanswered() {
    printf 'release-probe: %s\n' "$*" >&2
    exit 1
}

if [ "$#" -ne 1 ]; then
    unanswered "usage: release-probe.sh <id> takes exactly one argument, got $#; pass crate:llmlint or pypi:llmlint-cli"
fi

id=$1
registry=${id%%:*}
name=${id#*:}
if [ "$registry" = "$id" ]; then
    unanswered "unrecognised identifier '$id': expected a registry-qualified <registry>:<name>; pass crate:llmlint or pypi:llmlint-cli"
fi
# Bash's own matching, not grep's: a name check that shelled out would report a
# PATH missing `grep` as a malformed identifier, which is a different answer.
if ! [[ $name =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    unanswered "unrecognised identifier '$id': '$name' is not a registry artifact name; pass crate:llmlint or pypi:llmlint-cli"
fi

case "$registry" in
    crate) base=${LLMLINT_RELEASE_PROBE_CRATES_URL:-https://crates.io} ;;
    pypi) base=${LLMLINT_RELEASE_PROBE_PYPI_URL:-https://pypi.org} ;;
    *) unanswered "unrecognised identifier '$id': this repository publishes to crate: and pypi: only; pass crate:llmlint or pypi:llmlint-cli" ;;
esac

# Only what release-targets.toml declares. Another package's version is not an
# answer about this repository's releases.
case "$id" in
    crate:llmlint) url="${base%/}/api/v1/crates/$name" ;;
    pypi:llmlint-cli) url="${base%/}/pypi/$name/json" ;;
    *) unanswered "unrecognised identifier '$id': not a release target of this repository; pass crate:llmlint or pypi:llmlint-cli (see release-targets.toml)" ;;
esac

for tool in curl mktemp python3; do
    command -v "$tool" >/dev/null 2>&1 || unanswered "$tool is not on PATH, so '$id' cannot be looked up; install $tool or add it to PATH, then re-run"
done

body=$(mktemp)
trap 'rm -f "$body"' EXIT

status=$(curl -q --silent --show-error --location \
    --connect-timeout "$CONNECT_TIME" --max-time "$MAX_TIME" \
    --retry "$RETRIES" --retry-delay 1 \
    --user-agent "$UA" --header 'Accept: application/json' \
    --output "$body" --write-out '%{http_code}' "$url") \
    || unanswered "could not read $url for '$id' (see curl's message above); check the network or the registry's status, then re-run"

# A registry that has never served this artifact answers 404. That is the ONLY
# way to report "no release yet" — any other unexpected status is not answered.
if [ "$status" = 404 ]; then
    exit 0
fi
if [ "$status" != 200 ]; then
    unanswered "$url answered HTTP $status for '$id'; the registry is failing or refusing the read, re-run once it answers 200 or 404"
fi

version=$(python3 -c '
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
if sys.argv[2] == "crate":
    crate = payload["crate"]
    # The stable release is what the registry serves a dependent. A crate with
    # only prereleases has none, and answering nothing there would read as "no
    # release yet" for a release that already happened.
    version = crate.get("max_stable_version") or crate["newest_version"]
else:
    version = payload["info"]["version"]
# One line, one version: anything else is not an answer this probe can give.
if not isinstance(version, str) or not version or any(c.isspace() for c in version):
    sys.exit(1)
print(version)
' "$body" "$registry") || unanswered "$url answered HTTP 200 for '$id' with no version this probe could read; the registry's response shape changed, so update the parser in this script"

printf '%s\n' "$version"
